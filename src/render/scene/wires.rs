//! Layer-1 wire drawing (multi-strand bus rendering), plus the
//! click-facing half of the wire model: hit-testing a click against the
//! drawn centrelines (for tapping new wires on) and deleting one wire
//! without orphaning anything that taps onto it.

use crate::description::{ChipDescription, Color, WireConnectionType};
use crate::pin_state::LogicState;
use crate::render::foundation::{offset_polyline, SceneGeometry};
use crate::render::layout;
use crate::render::scene::lookup::PinStateLookup;
use crate::render::scene::pin_resolve::{resolve_pin_bit_count, resolve_pin_colour};
use crate::render::scene::placed::PlacedSubChip;
use crate::render::scene::wire_endpoints::{WireCtx, WirePointCache};
use crate::render::theme;
use crate::structs::Vec2;
use std::collections::HashMap;

/// Draws one wire's full `bit_count` as that many individually-coloured,
/// 1-bit-wide parallel strands rather than a single `bit_count`-scaled-
/// thick line -- so e.g. a mixed-signal 4-bit bus shows its actual 4
/// separate colours side by side, instead of a single line whose colour
/// only reflects bit 0's state (the old behaviour), or some blended
/// "average" that isn't any bit's real state.
///
/// Each strand's centreline is `centreline` (the wire's actual path,
/// including any player-authored bend points) offset sideways by that
/// strand's own constant distance via `offset_polyline` -- so every
/// strand bends together with the wire and stays a clean parallel line
/// through every corner, not just on straight runs.
///
/// Strand layout, `layout::WIRE_THICKNESS` apart: for `n` bits, strand `i`
/// sits at offset `WIRE_THICKNESS * (i - (n - 1) / 2)`. This single
/// formula handles both parities the way a real ribbon cable does: for an
/// *odd* bit count the middle strand's offset comes out to exactly `0`
/// (a real centred "middle wire"); for an *even* bit count there's no
/// strand at `0` at all -- the two middle strands straddle the centreline
/// at `+/- WIRE_THICKNESS / 2` instead, same spacing as every other
/// adjacent pair.
fn draw_wire_strands(
	geo: &mut SceneGeometry,
	centreline: &[Vec2],
	bit_count: u32,
	colour: Color,
	pin_owner_id: i32,
	pin_id: i32,
	pin_state: &dyn PinStateLookup,
) {
	let bit_count = bit_count.max(1);
	for bit_index in 0..bit_count {
		let offset = layout::WIRE_THICKNESS * (bit_index as f32 - (bit_count - 1) as f32 / 2.0);
		let strand_points = if offset == 0.0 { centreline.to_vec() } else { offset_polyline(centreline, offset) };
		let logic = pin_state.bit_logic_state(pin_owner_id, pin_id, bit_index).unwrap_or(LogicState::Low);
		let strand_colour = theme::state_colour(logic, colour);
		geo.add_polyline(&strand_points, layout::WIRE_THICKNESS, strand_colour);
	}
}

/// Layer 1 (bottom): every wire in `chip.wires`, resolved to world-space
/// polylines and drawn as thick lines. See the inline comments below for
/// how an individual wire's two endpoints are resolved.
pub(crate) fn draw_wires(
	geo: &mut SceneGeometry,
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	pin_state: &dyn PinStateLookup,
) {
	// Resolve each wire's two endpoints to world positions and draw a polyline through any
	// player-authored bend points. `ToPins` resolves straight from the pin's world position;
	// `ToWireSource`/`ToWireTarget` re-project onto the other wire's segment instead, to stay in sync with authored bends.
	let wire_ctx = WireCtx { chip, placed, owner_to_placed, wires: &chip.wires };
	let mut wire_point_cache: WirePointCache = HashMap::new();
	for (wire_idx, wire) in chip.wires.iter().enumerate() {
		let src = wire_ctx.endpoint(wire_idx, false, &mut wire_point_cache, 0);
		let dst = wire_ctx.endpoint(wire_idx, true, &mut wire_point_cache, 0);

		if let (Some(src), Some(dst)) = (src, dst) {
			// Colour/bit-count always trace back to the wire's real originating pin, regardless of
			// `connection_type` -- a wire tapped off another wire still carries that wire's signal.
			let colour = resolve_pin_colour(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);
			let bit_count =
				resolve_pin_bit_count(chip, placed, owner_to_placed, wire.source_pin_address.pin_owner_id, wire.source_pin_address.pin_id);

			let mut centreline = Vec::with_capacity(wire.points.len() + 2);
			centreline.push(src);
			centreline.extend_from_slice(&wire.points);
			centreline.push(dst);

			draw_wire_strands(
				geo,
				&centreline,
				bit_count as u32,
				colour,
				wire.source_pin_address.pin_owner_id,
				wire.source_pin_address.pin_id,
				pin_state,
			);
		}
	}
}

/// Removes wire `index` from `chip.wires` -- deliberately just that one
/// entry (the "shortest possible section": a single `WireDescription`,
/// which may be only one tap/branch of a larger fan-out or tap-chain),
/// not e.g. every wire sharing its source pin. Any *other* wire that was
/// tapping onto the removed one (`connection_type != ToPins` and
/// `connected_wire_index == index`) would otherwise be left pointing at
/// a dangling/wrong index, so those are removed too (recursively, since
/// a wire can itself be tapped by further wires) -- there's no sensible
/// position left to resolve them to once their anchor is gone. Every
/// remaining wire's `connected_wire_index` is then shifted down to
/// account for removed indices, so the rest of the tap graph stays
/// intact. Returns the number of wires actually removed (1 + however
/// many dependent taps cascaded).
pub fn delete_wire(chip: &mut ChipDescription, index: usize) -> usize {
	if index >= chip.wires.len() {
		return 0;
	}

	// Collect every index that must go: `index` itself, plus (transitively)
	// any wire tapping onto one already marked for removal.
	let mut to_remove = vec![index];
	loop {
		let mut added = false;
		for (i, w) in chip.wires.iter().enumerate() {
			if to_remove.contains(&i) {
				continue;
			}
			if w.connection_type != WireConnectionType::ToPins && to_remove.contains(&(w.connected_wire_index as usize)) {
				to_remove.push(i);
				added = true;
			}
		}
		if !added {
			break;
		}
	}
	to_remove.sort_unstable();

	let removed_count = to_remove.len();
	// Remove back-to-front so earlier indices in `to_remove` stay valid.
	for &i in to_remove.iter().rev() {
		chip.wires.remove(i);
	}

	// Shift every surviving wire's `connected_wire_index` down by however
	// many removed indices sat before it, so taps still point at the
	// right (now-renumbered) wire.
	for w in chip.wires.iter_mut() {
		if w.connection_type == WireConnectionType::ToPins {
			continue;
		}
		let shift = to_remove.iter().filter(|&&removed| (removed as i32) < w.connected_wire_index).count() as i32;
		w.connected_wire_index -= shift;
	}

	removed_count
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{ChipLibrary, ChipType, PinAddress, SubChipDescription, WireDescription};

	use crate::render::scene::lookup::AllLow;
	use crate::render::scene::placed::place_sub_chips;
	use crate::render::scene::test_support::nand_desc;
	use crate::render::scene::wire_endpoints::closest_point_on_segment;

	/// Same idea as above, but exercised end-to-end through `build_scene`
	/// (rather than calling `resolve_wire_endpoint` directly), confirming
	/// the tapped wire is actually drawn starting from the tap point.
	#[test]
	fn build_scene_draws_a_tapped_wire_starting_from_its_tap_point_not_its_pin() {
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

		let mut wire0 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
		wire0.points = vec![Vec2::new(2.0, 5.0)];
		chip.wires.push(wire0);

		let mut wire1 = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(3, 1));
		wire1.connection_type = WireConnectionType::ToWireSource;
		wire1.connected_wire_index = 0;
		wire1.connected_wire_segment_index = 0;
		wire1.cached_source_point = Vec2::new(1.0, 10.0);
		chip.wires.push(wire1);

		let placed = place_sub_chips(&chip, &lib);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let mut cache: WirePointCache = HashMap::new();
		let wire_ctx = WireCtx { chip: &chip, placed: &placed, owner_to_placed: &owner_to_placed, wires: &chip.wires };
		let wire0_src = wire_ctx.endpoint(0, false, &mut cache, 0).unwrap();
		let wire0_bend = chip.wires[0].points[0];
		let expected_tap_point = closest_point_on_segment(chip.wires[1].cached_source_point, wire0_src, wire0_bend);

		let scene = super::super::build_scene(&chip, &lib, &AllLow, None);

		// wire 1 is unbent (one quad, 6 verts), drawn right after wire 0 (bent through one point, so
		// 2 quads = 12 verts), so wire 1's quad sits at indices [12..18]. Within that quad, `add_line`
		// builds two triangles sharing edge (a+n)-(b-n): source-end corners are vertex 0 and vertex 5.
		let wire1_verts = &scene.triangles[12..18];
		let start_mid = Vec2::new((wire1_verts[0].pos.x + wire1_verts[5].pos.x) / 2.0, (wire1_verts[0].pos.y + wire1_verts[5].pos.y) / 2.0);
		assert_eq!(start_mid, expected_tap_point);
	}
}
