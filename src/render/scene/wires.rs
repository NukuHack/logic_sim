//! Layer-1 wire drawing (multi-strand bus rendering), plus the
//! click-facing half of the wire model: hit-testing a click against the
//! drawn centrelines (for tapping new wires on) and deleting one wire
//! without orphaning anything that taps onto it.

use crate::description::{ChipDescription, ChipLibrary, Color, WireConnectionType, WireDescription};
use crate::render::foundation::{offset_polyline, SceneGeometry};
use crate::render::layout;
use crate::render::scene::lookup::PinStateLookup;
use crate::render::scene::pin_resolve::{resolve_pin_bit_count, resolve_pin_colour};
use crate::render::scene::placed::{place_sub_chips, PlacedSubChip};
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
		let logic = pin_state.bit_logic_state(pin_owner_id, pin_id, bit_index).unwrap_or_default();
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

/// Removes wire `index` from `chip.wires`, mirroring
/// `DevChipInstance.DeleteWire`'s two very different cases:
///
/// - **bus wire**: everything hanging off the bus origin's pins dies with
///   it -- the wire itself, every wire tapping onto it (transitively),
///   and every wire wired into either of the origin's pins;
/// - **normal wire**: wires tapping onto it are *detached, not deleted*
///   (`WireInstance.RemoveConnectionDependency`) -- each inherits the
///   removed wire's route up to their attachment point plus its electrical
///   connection, so fan-outs survive the loss of their anchor.
///
/// Every remaining wire's `connected_wire_index` is shifted so the rest of
/// the tap graph stays intact. Returns the number of wires removed.
pub fn delete_wire(chip: &mut ChipDescription, index: usize, library: &ChipLibrary) -> usize {
	if index >= chip.wires.len() {
		return 0;
	}

	let is_bus = crate::viewer::bus_wiring::is_bus_wire(chip, library, &chip.wires[index]);

	// World-space polyline of the doomed wire (endpoints resolved exactly
	// the way drawing resolves them), needed to re-route detached tappers.
	let anchor_world = if is_bus {
		Vec::new()
	} else {
		let chip_snapshot = chip.clone();
		let placed = place_sub_chips(&chip_snapshot, library);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let mut cache: WirePointCache = HashMap::new();
		let ctx = WireCtx { chip: &chip_snapshot, placed: &placed, owner_to_placed: &owner_to_placed, wires: &chip_snapshot.wires };
		let src = ctx.endpoint(index, false, &mut cache, 0);
		let dst = ctx.endpoint(index, true, &mut cache, 0);
		let mut world = Vec::with_capacity(chip.wires[index].points.len() + 2);
		world.push(src.or(Some(chip.wires[index].cached_source_point)).expect("endpoint fallback"));
		world.extend_from_slice(&chip.wires[index].points);
		world.push(dst.unwrap_or(chip.wires[index].cached_target_point));
		world
	};

	if !is_bus {
		let anchor = chip.wires[index].clone();
		let dependents: Vec<usize> = chip
			.wires
			.iter()
			.enumerate()
			.filter(|(i, w)| *i != index && w.connection_type != WireConnectionType::ToPins && w.connected_wire_index as usize == index)
			.map(|(i, _)| i)
			.collect();
		for d in dependents {
			detach_dependent(chip, d, &anchor, &anchor_world);
		}

		chip.wires.remove(index);
		shift_connected_indices_after_removal(chip, &[index]);
		return 1;
	}

	// Bus-wire cascade: seed with `index` and everything attached to the
	// origin's pins, then grow through transitive taps.
	let origin_owner = chip.wires[index].source_pin_address.pin_owner_id;
	let origin_pin_ids: Vec<i32> = chip
		.sub_chips
		.iter()
		.find(|s| s.id == origin_owner)
		.and_then(|s| library.try_get(&s.name))
		.map(|d| d.input_pins.iter().chain(d.output_pins.iter()).map(|p| p.id).collect())
		.unwrap_or_default();

	let mut to_remove = vec![index];
	loop {
		let mut added = false;
		for (i, w) in chip.wires.iter().enumerate() {
			if to_remove.contains(&i) {
				continue;
			}
			let touches_origin_pins =
				[w.source_pin_address, w.target_pin_address].iter().any(|a| a.pin_owner_id == origin_owner && origin_pin_ids.contains(&a.pin_id));
			let taps_removed = w.connection_type != WireConnectionType::ToPins && to_remove.contains(&(w.connected_wire_index as usize));
			if touches_origin_pins || taps_removed {
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
	for &i in to_remove.iter().rev() {
		chip.wires.remove(i);
	}
	shift_connected_indices_after_removal(chip, &to_remove);

	removed_count
}

/// Rewires wire `d` to no longer depend on its (about-to-be-removed)
/// anchor `anchor` whose world polyline was `anchor_world`
/// (`RemoveConnectionDependency`, generalized to both attachment sides):
/// the anchor's route up to/past the attachment point is folded into the
/// dependent's own bend list, and its attached-side connection info is
/// inherited from the anchor's corresponding side.
fn detach_dependent(chip: &mut ChipDescription, d: usize, anchor: &WireDescription, anchor_world: &[Vec2]) {
	let seg = chip.wires[d].connected_wire_segment_index;

	match chip.wires[d].connection_type {
		WireConnectionType::ToWireSource => {
			// Dependent's *source* sits ON the anchor -- its drawn start
			// was the projection onto the anchor's segment while its
			// electrical source already is the anchor's source pin. Fold
			// the anchor's route up to the attachment into the bends
			// (vertices 1..=seg; vertex 0 is the pin itself) so the drawn
			// shape survives, then fall back to a plain pin connection --
			// or, when the anchor was itself a tap, inherit its whole
			// source-side attachment (`SourceConnectionInfo` hand-over).
			let mut points: Vec<Vec2> = anchor_world.iter().take((seg as usize + 1).min(anchor_world.len())).skip(1).copied().collect();
			points.push(chip.wires[d].cached_source_point);
			points.extend_from_slice(&chip.wires[d].points);
			let dep = &mut chip.wires[d];
			dep.points = points;
			if anchor.connection_type == WireConnectionType::ToWireSource {
				dep.connected_wire_index = anchor.connected_wire_index;
				dep.connected_wire_segment_index = anchor.connected_wire_segment_index;
				dep.cached_source_point = anchor.cached_source_point;
			} else {
				dep.connection_type = WireConnectionType::ToPins;
				dep.connected_wire_index = 0;
				dep.connected_wire_segment_index = 0;
				dep.cached_source_point = Vec2::ZERO;
			}
		}
		WireConnectionType::ToWireTarget => {
			// Dependent's *target* sits ON the anchor -- purely a drawing
			// attachment (its electrical endpoints are fully its own).
			// Freeze the attachment point as an ordinary bend and draw
			// straight to the real target pin from now on.
			let mut points = chip.wires[d].points.clone();
			points.push(chip.wires[d].cached_target_point);
			let dep = &mut chip.wires[d];
			dep.points = points;
			dep.connection_type = WireConnectionType::ToPins;
			dep.connected_wire_index = 0;
			dep.connected_wire_segment_index = 0;
			dep.cached_target_point = Vec2::ZERO;
		}
		WireConnectionType::ToPins => {}
	}
}

/// Adjusts every surviving wire's `connected_wire_index` down by however
/// many `removed` indices sat below it.
fn shift_connected_indices_after_removal(chip: &mut ChipDescription, removed: &[usize]) {
	for w in chip.wires.iter_mut() {
		if w.connection_type == WireConnectionType::ToPins {
			continue;
		}
		let shift = removed.iter().filter(|&&r| (r as i32) < w.connected_wire_index).count() as i32;
		w.connected_wire_index -= shift;
	}
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

	/// Mirror of the source-tap test above for *target*-side taps
	/// (`ToWireTarget` -- "wiring into a wire"): the drawn wire must END on
	/// the tapped wire's segment rather than at its target pin's position.
	#[test]
	fn build_scene_draws_a_target_tapped_wire_ending_on_its_tap_point() {
		let mut lib = ChipLibrary::new();
		lib.add(nand_desc());

		let mut chip = ChipDescription::new("TAP_TARGET_TEST", ChipType::Custom);
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

		let mut anchor = WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0));
		anchor.points = vec![Vec2::new(2.0, 5.0)];
		chip.wires.push(anchor);

		// Runs from NAND3's output and lands ON anchor's first segment.
		let mut tap = WireDescription::new_tapped_target(PinAddress::new(3, 0), PinAddress::new(2, 1), 0, 0, Vec2::new(1.0, 10.0));
		tap.points = vec![Vec2::new(-2.0, -3.0)];
		chip.wires.push(tap);

		let placed = place_sub_chips(&chip, &lib);
		let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
		let mut cache: WirePointCache = HashMap::new();
		let wire_ctx = WireCtx { chip: &chip, placed: &placed, owner_to_placed: &owner_to_placed, wires: &chip.wires };
		let anchor_src = wire_ctx.endpoint(0, false, &mut cache, 0).unwrap();
		let anchor_bend = chip.wires[0].points[0];
		let expected_tap_point = closest_point_on_segment(chip.wires[1].cached_target_point, anchor_src, anchor_bend);

		let scene = super::super::build_scene(&chip, &lib, &AllLow, None);

		// Both wires have one bend each (2 segments => 12 verts via
		// `add_polyline`'s two quads), so the tap wire occupies [12..24].
		// Within each quad the corners are [a+n, b+n, b-n, a+n, b-n, a-n]:
		// verts 1 and 2 straddle `b`. The second quad's `b` is the end of
		// the last segment -- the attachment point.
		let last_quad = &scene.triangles[18..24];
		let end_mid = Vec2::new((last_quad[1].pos.x + last_quad[2].pos.x) / 2.0, (last_quad[1].pos.y + last_quad[2].pos.y) / 2.0);
		assert_eq!(end_mid, expected_tap_point);
	}

	/// Deleting a wire takes every wire tapping onto it with it --
	/// regardless of which side they tap from -- and renumbers surviving
	/// taps so they still point at the right anchors.
	#[test]
	fn delete_wire_cascades_to_target_side_taps_and_renumbers_survivors() {
		let mut chip = ChipDescription::new("CASCADE", ChipType::Custom);
		for id in [1, 2, 3] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				position: Vec2::ZERO,
				label: None,
				pin_colour_info: vec![],
			});
		}

		// wire 0: NAND1 -> NAND2 (the deletion target)
		chip.wires.push(WireDescription::new(PinAddress::new(1, 0), PinAddress::new(2, 0)));
		// wire 1: taps onto wire 0 from its SOURCE side
		let mut source_tap = WireDescription::new_tapped_source(PinAddress::new(1, 0), PinAddress::new(3, 1), 0, 0, Vec2::ZERO);
		source_tap.points = vec![Vec2::new(1.0, 1.0)];
		chip.wires.push(source_tap);
		// wire 2: taps onto wire 0 from its TARGET side
		let mut target_tap = WireDescription::new_tapped_target(PinAddress::new(3, 0), PinAddress::new(2, 1), 0, 0, Vec2::ZERO);
		target_tap.points = vec![Vec2::new(-1.0, -1.0)];
		chip.wires.push(target_tap);
		// wire 3: independent pin-to-pin wire
		chip.wires.push(WireDescription::new(PinAddress::new(3, 0), PinAddress::new(3, 1)));

		let mut library = ChipLibrary::new();
		library.add(nand_desc());

		delete_wire(&mut chip, 0, &library);

		// Normal-wire deletion detaches rather than cascades: both tappers
		// survive, re-anchored onto the deleted wire's own endpoints.
		assert_eq!(chip.wires.len(), 3, "anchor gone, both re-anchored survivors + the independent wire remain");

		let survivor_source_tap = chip.wires.iter().find(|w| w.target_pin_address == PinAddress::new(3, 1)).expect("source tap survives");
		assert_eq!(survivor_source_tap.source_pin_address, PinAddress::new(1, 0), "inherits the anchor's source pin");
		assert_eq!(survivor_source_tap.connection_type, WireConnectionType::ToPins, "plain pin connection after detach");
		assert!(!survivor_source_tap.points.is_empty(), "the anchor's route is folded into the bends");

		let survivor_target_tap = chip.wires.iter().find(|w| w.source_pin_address == PinAddress::new(3, 0)).expect("target tap survives");
		assert_eq!(survivor_target_tap.target_pin_address, PinAddress::new(2, 1), "inherits the anchor's target pin");
		assert_eq!(survivor_target_tap.connection_type, WireConnectionType::ToPins);
	}

	/// Deleting one half of a linked bus pair takes the whole origin net
	/// with it: wires touching the origin's pins die together.
	#[test]
	fn delete_bus_wire_cascades_across_the_origin_net() {
		use crate::viewer::bus_wiring;

		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);

		let mut chip = ChipDescription::new("BUS_NET", ChipType::Custom);
		for id in [10, 11] {
			chip.sub_chips.push(SubChipDescription {
				name: "NAND".into(),
				id,
				internal_data: None,
				position: Vec2::ZERO,
				label: None,
				pin_colour_info: vec![],
			});
		}
		// Linked bus pair at ids 20 (origin) / 21 (terminus).
		chip.sub_chips.push(SubChipDescription {
			name: "BUS-4".into(),
			id: 20,
			internal_data: Some(vec![21]),
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		chip.sub_chips.push(SubChipDescription {
			name: "BUS-TERMINUS-4".into(),
			id: 21,
			internal_data: Some(vec![20]),
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});

		// The bus wire itself (origin OUT pin 1 -> terminus IN pin 0)...
		let bus_wire = WireDescription::new(PinAddress::new(20, 1), PinAddress::new(21, 0));
		chip.wires.push(bus_wire.clone());
		// ...a NAND output wired INTO the origin (merge-at-origin input)...
		chip.wires.push(WireDescription::new(PinAddress::new(10, 2), PinAddress::new(20, 0)));
		// ...and an unrelated wire.
		chip.wires.push(WireDescription::new(PinAddress::new(11, 2), PinAddress::new(10, 0)));

		assert!(bus_wiring::is_bus_wire(&chip, &library, &bus_wire));
		delete_wire(&mut chip, 0, &library);

		assert_eq!(chip.wires.len(), 1, "everything on the origin net goes; only the unrelated wire stays");
		assert_eq!(chip.wires[0].source_pin_address, PinAddress::new(11, 2));
	}
}
