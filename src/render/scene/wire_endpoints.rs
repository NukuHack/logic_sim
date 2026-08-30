//! Wire-endpoint resolution: how each end of a saved `WireDescription`
//! becomes a world-space point -- either a pin's live position, or (for
//! wires tapped onto other wires) the cached attachment point re-projected
//! onto its anchor segment -- plus the shared "closest point on what's
//! actually drawn" search and wire deletion.

use crate::description::{ChipDescription, ChipLibrary, PinBitCount, WireConnectionType, WireDescription};
use crate::structs::Vec2;
use std::collections::HashMap;

use crate::render::scene::pin_resolve::{resolve_pin_bit_count, resolve_pin_position};
use crate::render::scene::placed::{place_sub_chips, PlacedSubChip};

/// Memoizes resolved wire-endpoint world positions within one `build_scene`
/// call, keyed by `(wire index into chip.wires, is_target)`. Needed because
/// resolving one wire-tap endpoint can require resolving another wire's
/// endpoints in turn (see `resolve_wire_endpoint`), and the same wire can be
/// revisited many times (e.g. a bus fanning out to several taps).
pub(crate) type WirePointCache = HashMap<(usize, bool), Option<Vec2>>;

/// How many wire-to-wire attachment hops to follow before giving up. Real projects only ever
/// nest a couple of levels deep (`WireInstance`'s own `ConnectedWireRecursionDepth` tracks
/// this for draw-ordering, and stays small in practice), so this is purely a guard against a
/// hand-edited or corrupted save file describing a connection cycle -- without it, a cycle
/// would recurse forever instead of just drawing that wire wrong.
const MAX_WIRE_CONNECTION_DEPTH: u32 = 64;

/// The closest point to `p` on line segment `a`-`b`. Mirrors
/// `WireInstance.ClosestPointOnLineSegment`; used to re-project a
/// wire-tap's cached attachment point onto its target wire's segment.
pub(crate) fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
	let ab = Vec2::new(b.x - a.x, b.y - a.y);
	let sqr_len = ab.x * ab.x + ab.y * ab.y;
	if sqr_len <= 1e-12 {
		return a;
	}
	let ap = Vec2::new(p.x - a.x, p.y - a.y);
	let t = ((ap.x * ab.x + ap.y * ab.y) / sqr_len).clamp(0.0, 1.0);
	Vec2::new(a.x + ab.x * t, a.y + ab.y * t)
}

/// Bundles the chip-definition context the wire-endpoint resolvers need
/// (the chip itself, its sub-chips laid out into world positions, the
/// sub-chip-id -> layout-index map, and the wire list) so call sites pass
/// one borrow instead of four parallel parameters -- these always travel
/// together and every caller has all four on hand.
pub(crate) struct WireCtx<'a> {
	pub chip: &'a ChipDescription,
	pub placed: &'a [PlacedSubChip<'a>],
	pub owner_to_placed: &'a HashMap<i32, usize>,
	pub wires: &'a [WireDescription],
}

impl<'a> WireCtx<'a> {
	/// Resolves world-space point index `point_index` along wire `wire_idx`'s
	/// own polyline, i.e. `[source-endpoint, ...bends..., target-endpoint]`.
	/// Interior indices are just that wire's saved bend points (already in
	/// world space, no resolution needed); the two endpoint indices recurse
	/// into [`WireCtx::endpoint`], since either one might itself be a tap on
	/// yet another wire. Mirrors `WireInstance.GetWirePoint`.
	fn point(&self, wire_idx: usize, point_index: usize, cache: &mut WirePointCache, depth: u32) -> Option<Vec2> {
		let wire = self.wires.get(wire_idx)?;
		let last_index = wire.points.len() + 1; // bends.len() interior points + 2 endpoints
		if point_index == 0 {
			self.endpoint(wire_idx, false, cache, depth)
		} else if point_index == last_index {
			self.endpoint(wire_idx, true, cache, depth)
		} else {
			wire.points.get(point_index - 1).copied()
		}
	}

	/// Resolves one end of wire `wire_idx` (`is_target`: false = source, true = target) to a
	/// world-space position.
	pub fn endpoint(&self, wire_idx: usize, is_target: bool, cache: &mut WirePointCache, depth: u32) -> Option<Vec2> {
		if let Some(&cached) = cache.get(&(wire_idx, is_target)) {
			return cached;
		}
		if depth > MAX_WIRE_CONNECTION_DEPTH {
			return None;
		}
		let wire = self.wires.get(wire_idx)?;

		let attaches_to_wire =
			matches!((is_target, wire.connection_type), (false, WireConnectionType::ToWireSource) | (true, WireConnectionType::ToWireTarget));

		let result = if attaches_to_wire {
			if wire.connected_wire_index < 0 {
				None
			} else {
				let target_wire_idx = wire.connected_wire_index as usize;
				let seg = wire.connected_wire_segment_index.max(0) as usize;
				let a = self.point(target_wire_idx, seg, cache, depth + 1);
				let b = self.point(target_wire_idx, seg + 1, cache, depth + 1);
				match (a, b) {
					(Some(a), Some(b)) => {
						let cached_point = if is_target { wire.cached_target_point } else { wire.cached_source_point };
						Some(closest_point_on_segment(cached_point, a, b))
					}
					_ => None,
				}
			}
		} else {
			let addr = if is_target { &wire.target_pin_address } else { &wire.source_pin_address };
			resolve_pin_position(self.chip, self.placed, self.owner_to_placed, addr.pin_owner_id, addr.pin_id, is_target)
		};

		cache.insert((wire_idx, is_target), result);
		result
	}
}

/// One point along an existing wire's drawn centreline, close enough to a click to tap a new
/// wire onto -- returned by `hit_test_wire_tap`.
#[derive(Debug, Clone, Copy)]
pub struct WireTapHit {
	pub wire_index: usize,
	pub segment_index: i32,
	pub point: Vec2,
	/// The tapped wire's real signal width, traced back to its
	/// originating `source_pin_address` the same way `draw_wires` does
	/// (not e.g. `Bit1` regardless of `connection_type` -- a wire tapped
	/// off another wire still carries that wire's bit count).
	pub bit_count: PinBitCount,
}

/// Finds whichever wire's drawn centreline `world_pos` is closest to (within `max_dist` world
/// units of any of its segments), returning that wire's index into `chip.wires` -- used to
/// resolve a right-click "delete wire" to *one specific* `WireDescription`, not e.g. every
/// wire fanning out of the same source pin.
pub fn hit_test_wire(chip: &ChipDescription, library: &ChipLibrary, world_pos: Vec2, max_dist: f32) -> Option<usize> {
	closest_wire_hit(chip, library, world_pos, max_dist).map(|hit| hit.wire_index)
}

/// Shared search behind `hit_test_wire`/`hit_test_wire_tap`: finds
/// whichever wire's drawn centreline `world_pos` is closest to (within
/// `max_dist` world units of any of its segments). Resolves each wire's
/// endpoints (including tap-on-another-wire ones) the same way
/// `draw_wires` does, so "closest to what's actually drawn" matches what
/// the player sees, not just the saved bend points.
pub fn closest_wire_hit(chip: &ChipDescription, library: &ChipLibrary, world_pos: Vec2, max_dist: f32) -> Option<WireTapHit> {
	let placed = place_sub_chips(chip, library);
	let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

	let wire_ctx = WireCtx { chip, placed: &placed, owner_to_placed: &owner_to_placed, wires: &chip.wires };
	let mut cache: WirePointCache = HashMap::new();
	let mut best: Option<(WireTapHit, f32)> = None;
	for wire_idx in 0..chip.wires.len() {
		let src = wire_ctx.endpoint(wire_idx, false, &mut cache, 0);
		let dst = wire_ctx.endpoint(wire_idx, true, &mut cache, 0);
		let (Some(src), Some(dst)) = (src, dst) else { continue };

		let mut centreline = Vec::with_capacity(chip.wires[wire_idx].points.len() + 2);
		centreline.push(src);
		centreline.extend_from_slice(&chip.wires[wire_idx].points);
		centreline.push(dst);

		for (segment_index, seg) in centreline.windows(2).enumerate() {
			let closest = closest_point_on_segment(world_pos, seg[0], seg[1]);
			let dist = ((closest.x - world_pos.x).powi(2) + (closest.y - world_pos.y).powi(2)).sqrt();
			if dist <= max_dist && best.as_ref().map(|(_, best_dist)| dist < *best_dist).unwrap_or(true) {
				// Bit count always traces back to the wire's real originating pin
				// (`source_pin_address`), regardless of `connection_type` -- a wire
				// tapped off another wire still carries that wire's signal. Mirrors
				// `draw_wires`'s own `resolve_pin_bit_count` call.
				let wire = &chip.wires[wire_idx];
				let bit_count =
					resolve_pin_bit_count(chip, &placed, &owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);
				best = Some((WireTapHit { wire_index: wire_idx, segment_index: segment_index as i32, point: closest, bit_count }, dist));
			}
		}
	}
	best.map(|(hit, _)| hit)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{ChipLibrary, ChipType, PinAddress, SubChipDescription, WireConnectionType};
	use crate::render::scene::test_support::nand_desc;
	#[test]
	fn closest_point_on_segment_projects_and_clamps() {
		let a = Vec2::new(0.0, 0.0);
		let b = Vec2::new(10.0, 0.0);
		// A point off the line projects straight down onto it...
		assert_eq!(closest_point_on_segment(Vec2::new(5.0, 3.0), a, b), Vec2::new(5.0, 0.0));
		// ...and projection clamps to the segment's ends rather than
		// extrapolating past them.
		assert_eq!(closest_point_on_segment(Vec2::new(-5.0, 0.0), a, b), a);
		assert_eq!(closest_point_on_segment(Vec2::new(15.0, 0.0), a, b), b);
	}

	#[test]
	fn closest_point_on_segment_handles_a_zero_length_segment() {
		let a = Vec2::new(3.0, 4.0);
		assert_eq!(closest_point_on_segment(Vec2::new(0.0, 0.0), a, a), a);
	}

	/// This is the regression test for the wire-bend bug: a wire tapped onto another wire's
	/// segment (`WireConnectionType::ToWireSource`) must resolve its endpoint by projecting the
	/// cached attachment point onto that other wire's segment, *not* by jumping straight to the
	/// underlying pin's position (the old, buggy behaviour) -- doing the latter desyncs the
	/// tap's resolved position from its player-authored bend points, which were drawn assuming
	/// the wire starts at the tap point.
	#[test]
	fn wire_tap_endpoint_resolves_onto_referenced_wire_segment_not_the_underlying_pin() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut chip = ChipDescription::new("TAP_TEST", ChipType::Custom);
		for id in [1, 2, 3] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				label: None,
				position: Vec2::new(id as f32 * 4.0, 0.0),
				pin_colour_info: Vec::new(),
			});
		}

		// wire 0: NAND1's output (pin 0) -> NAND2's input A (pin 0), bent
		// through one authored point so there's a real interior segment
		// (source -> bend) to tap onto.
		let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
		wire0.points = vec![Vec2::new(2.0, 5.0)];
		chip.wires.push(wire0);

		// wire 1: taps onto wire 0's first segment (its source -> its bend), attaching at a cached
		// point that's deliberately off that segment's line -- it should snap onto the segment, not
		// just be used verbatim. Its target is NAND3's input B.
		let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
		wire1.connection_type = WireConnectionType::ToWireSource;
		wire1.connected_wire_index = 0;
		wire1.connected_wire_segment_index = 0;
		wire1.cached_source_point = Vec2::new(1.0, 10.0);
		chip.wires.push(wire1);

		let placed = place_sub_chips(&chip, &lib);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let wire_ctx = WireCtx { chip: &chip, placed: &placed, owner_to_placed: &owner_to_placed, wires: &chip.wires };
		let mut cache: WirePointCache = HashMap::new();

		let wire0_src = wire_ctx.endpoint(0, false, &mut cache, 0).expect("wire 0's source should resolve via NAND1's output pin");
		let wire0_bend = chip.wires[0].points[0];

		let wire1_src = wire_ctx.endpoint(1, false, &mut cache, 0).expect("wire 1's tapped source should resolve via wire 0's segment");

		let expected = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);
		assert_eq!(wire1_src, expected);

		// Critically, the tap point must NOT be NAND1's actual output pin
		// position -- resolving straight to the pin (ignoring the tap) was
		// the bug.
		let nand1_output_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, false).unwrap();
		assert_ne!(wire1_src, nand1_output_pos);
	}
}
