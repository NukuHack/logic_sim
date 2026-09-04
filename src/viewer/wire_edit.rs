//! Wire edit mode (`ChipInteractionController`'s `wireToEdit` family): right-click a wire →
//! "Edit", then its bend points become draggable -- clicking the wire's line inserts a new
//! bend there, dragging moves it (grid-snapped / straightened like placement), Delete removes
//! the selected bend, and Enter/Escape/empty-click leaves.

use crate::description::{ChipDescription, WireConnectionType};
use crate::render::layout;
use crate::render::scene::placed::{place_sub_chips, PlacedSubChip};
use crate::render::scene::wire_endpoints::{WireCtx, WirePointCache};
use crate::structs::Vec2;
use crate::viewer::state::{ViewerState, WireEditState};
use std::collections::HashMap;

/// Screen-pixel distance for grabbing an existing bend handle or hitting a
/// wire's line while editing -- same feel as `wire_click_tolerance`.
fn grab_tolerance(camera: &crate::render::camera::Camera) -> f32 {
	8.0 / camera.zoom.max(0.0001)
}

/// The edited wire's world-space vertices: `[source endpoint, ...bends...,
/// target endpoint]`, resolved exactly the way drawing resolves them.
pub(crate) fn edited_wire_vertices(v: &ViewerState) -> Option<(usize, Vec<Vec2>)> {
	let state = v.wire_edit?;
	let root_desc = v.library.get(&v.root_chip_name);
	if state.wire_index >= root_desc.wires.len() {
		return None;
	}
	let placed = place_sub_chips(root_desc, &v.library);
	Some((state.wire_index, wire_vertices(root_desc, &v.library, &placed, state.wire_index)))
}

/// World-space vertex list of wire `index` within `chip` (see above).
pub(crate) fn wire_vertices(chip: &ChipDescription, _library: &crate::ChipLibrary, placed: &[PlacedSubChip], index: usize) -> Vec<Vec2> {
	let owner_to_placed: HashMap<i32, usize> = placed.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
	let mut cache: WirePointCache = HashMap::new();
	let ctx = WireCtx { chip, placed, owner_to_placed: &owner_to_placed, wires: &chip.wires };
	let src = ctx.endpoint(index, false, &mut cache, 0).unwrap_or(chip.wires[index].cached_source_point);
	let dst = ctx.endpoint(index, true, &mut cache, 0).unwrap_or(chip.wires[index].cached_target_point);
	let mut verts = Vec::with_capacity(chip.wires[index].points.len() + 2);
	verts.push(src);
	verts.extend_from_slice(&chip.wires[index].points);
	verts.push(dst);
	verts
}

/// Enters edit mode on `wire_index` -- or leaves when it's already being
/// edited (`EnterWireEditMode`'s toggle).
pub(crate) fn enter(v: &mut ViewerState, wire_index: usize) {
	let already = matches!(v.wire_edit, Some(state) if state.wire_index == wire_index);
	v.wire_edit = if already { None } else { Some(WireEditState { wire_index, selected_bend: None }) };
}

// this is for one TODO
/// TODO make this used
#[allow(unused)]
pub(crate) fn find_wire_network(chip: &ChipDescription, root_wire: usize) -> Vec<usize> {
	let mut network = std::collections::HashSet::new();
	let mut queue = vec![root_wire];

	while let Some(current) = queue.pop() {
		if !network.insert(current) {
			continue;
		}
		let target_wire = &chip.wires[current];
		let src_addr = target_wire.source_pin_address;

		for (i, w) in chip.wires.iter().enumerate() {
			if network.contains(&i) {
				continue;
			}
			// Shared source pin net
			if w.source_pin_address == src_addr {
				queue.push(i);
			}
			// Taps into current wire
			if w.connection_type != WireConnectionType::ToPins && w.connected_wire_index as usize == current {
				queue.push(i);
			}
			// Wire tapped by current wire
			if target_wire.connection_type != WireConnectionType::ToPins && target_wire.connected_wire_index as usize == i {
				queue.push(i);
			}
		}
	}

	let mut result: Vec<usize> = network.into_iter().collect();
	result.sort_unstable();
	result
}

pub(crate) fn exit(v: &mut ViewerState) {
	v.wire_edit = None;
}

/// Whether a click at `world_pos` grabs one of the edited wire's existing
/// bends (nearest within tolerance).
pub(crate) fn bend_hit(v: &ViewerState, world_pos: Vec2) -> Option<usize> {
	let (_, verts) = edited_wire_vertices(v)?;
	// Bend i sits at vertex i+1; endpoints aren't grabbable.
	let tol_sq = grab_tolerance(&v.camera).powi(2);
	let mut best: Option<(f32, usize)> = None;
	for (vi, p) in verts.iter().enumerate().skip(1).take(verts.len().saturating_sub(2)) {
		let d = *p - world_pos;
		let dist_sq = d.magnitude_sq();
		if dist_sq <= tol_sq && best.is_none_or(|(b, _)| dist_sq < b) {
			best = Some((dist_sq, vi));
		}
	}
	best.map(|(_, vi)| vi - 1)
}

/// Click landing on the edited wire's line but not on a handle: inserts a
/// new bend at the closest point and selects it (`InsertPoint`). Returns
/// the new bend's index.
pub(crate) fn insert_point_at_click(v: &mut ViewerState, world_pos: Vec2) -> Option<usize> {
	let state = v.wire_edit?;
	let root_chip_name = v.root_chip_name.clone();
	let root_desc = v.library.get(&root_chip_name);
	if state.wire_index >= root_desc.wires.len() {
		return None;
	}
	let placed = place_sub_chips(root_desc, &v.library);
	let verts = wire_vertices(root_desc, &v.library, &placed, state.wire_index);

	// Closest segment + projection point (`GetClosestPointOnWire`).
	let mut best = (f32::MAX, 0usize, Vec2::ZERO);
	for seg in 0..verts.len().saturating_sub(1) {
		let (a, b) = (verts[seg], verts[seg + 1]);
		let point = crate::render::scene::wire_endpoints::closest_point_on_segment(world_pos, a, b);
		let dist_sq = (point - world_pos).magnitude_sq();
		if dist_sq < best.0 {
			best = (dist_sq, seg, point);
		}
	}
	if best.0 > grab_tolerance(&v.camera).powi(2) {
		return None;
	}

	// Bend index inside `points`: vertex `seg+1` is bend `seg`, so a point
	// projected onto segment `seg` inserts after that bend.
	let bend_index = best.1;
	crate::viewer::undo::record_wire_list_snapshot_pair_before(v, |v| {
		let chip = v.library.get_mut(&v.root_chip_name.clone());
		chip.wires[state.wire_index].points.insert(bend_index, best.2);
		for dep in chip.wires.iter_mut() {
			if dep.connection_type == WireConnectionType::ToPins || dep.connected_wire_index != state.wire_index as i32 {
				continue;
			}
			if dep.connected_wire_segment_index > bend_index as i32 {
				dep.connected_wire_segment_index += 1;
			}
		}
	});
	v.wire_edit = Some(WireEditState { wire_index: state.wire_index, selected_bend: Some(bend_index) });
	Some(bend_index)
}

/// Begins carrying the selected bend (click on a handle starts its drag).
pub(crate) fn begin_drag(v: &mut ViewerState, bend_index: usize) {
	if let Some(state) = v.wire_edit.as_mut() {
		state.selected_bend = Some(bend_index);
	}
}

/// Live drag update for the selected bend: snapped/straightened exactly
/// like a pending wire's preview (`SetWirePointWithSnapping`).
pub(crate) fn update_drag(v: &mut ViewerState, world_pos: Vec2) {
	let Some(state) = v.wire_edit.as_ref() else { return };
	let Some(bend) = state.selected_bend else { return };
	let root_chip_name = v.root_chip_name.clone();
	let snap = v.should_snap_to_grid();
	let straighten = v.force_straight_wires();

	let chip = v.library.get(&root_chip_name);
	let Some(wire) = chip.wires.get(state.wire_index) else { return };
	let mut pos = world_pos;
	if snap {
		pos = layout::snap_to_grid_centred(pos);
	}
	if straighten {
		// Straighten against the neighbouring vertex (previous bend, or the
		// resolved source end for bend 0) -- `straightLineRefPoint`.
		let neighbours = wire_end_neighbours(v, state.wire_index);
		let prev = if bend == 0 { neighbours.0 } else { wire.points[bend - 1] };
		pos = layout::force_straight_line(prev, pos);
	}
	let chip = v.library.get_mut(&root_chip_name);
	chip.wires[state.wire_index].points[bend] = pos;
}

/// `(source-end vertex, target-end vertex)` of wire `index`, for
/// straightening against.
fn wire_end_neighbours(v: &ViewerState, wire_index: usize) -> (Vec2, Vec2) {
	let root_desc = v.library.get(&v.root_chip_name);
	let placed = place_sub_chips(root_desc, &v.library);
	let verts = wire_vertices(root_desc, &v.library, &placed, wire_index);
	let last = verts.len().saturating_sub(1);
	(verts[0], verts[last])
}

/// Commits an in-flight bend drag as one undoable geometry edit: the
/// whole wire list as-is becomes the "after" half; the "before" half is
/// the same list with this bend back at `original` (a drag shifts nothing
/// but that one point, so no other reconstruction is needed). Equal
/// halves -- a plain click on a handle -- record nothing.
pub(crate) fn commit_drag(v: &mut ViewerState, wire_index: usize, bend_index: usize, original: Vec2) {
	let after = crate::viewer::undo::capture_wire_list(v);
	let mut before_wires = after.wires.clone();
	if let Some((wire, _)) = before_wires.get_mut(wire_index) {
		if let Some(point) = wire.points.get_mut(bend_index) {
			*point = original;
		}
	}
	crate::viewer::undo::record_wire_list_edit(v, crate::viewer::undo::FullWireState { wires: before_wires });
}

/// Deletes the currently selected bend, if it can be deleted
/// (endpoints-are-bends don't exist here; any real bend goes, with
/// dependents' attachment indices shifted like
/// `NotifyParentWirePointWillBeDeleted`). Returns whether anything happened.
pub(crate) fn delete_selected_bend(v: &mut ViewerState) -> bool {
	let (wire_index, bend) = match v.wire_edit {
		Some(WireEditState { wire_index, selected_bend: Some(bend) }) => (wire_index, bend),
		_ => return false,
	};
	{
		let chip = v.library.get(&v.root_chip_name);
		let Some(wire) = chip.wires.get(wire_index) else { return false };
		if bend >= wire.points.len() {
			return false;
		}
	}

	crate::viewer::undo::record_wire_list_snapshot_pair_before(v, |v| {
		let root_chip_name = v.root_chip_name.clone();
		let chip = v.library.get_mut(&root_chip_name);
		chip.wires[wire_index].points.remove(bend);
		// Dependent taps attached at/after the removed vertex shift back.
		for dep in chip.wires.iter_mut() {
			if dep.connection_type == WireConnectionType::ToPins || dep.connected_wire_index != wire_index as i32 {
				continue;
			}
			if dep.connected_wire_segment_index > bend as i32 {
				dep.connected_wire_segment_index -= 1;
			}
		}
	});
	v.wire_edit = Some(WireEditState { wire_index, selected_bend: None });
	true
}

#[cfg(test)]
mod tests {
	//! White-box: the edit-mode flow drives `pub(crate)` viewer state
	//! directly (no GPU), mirroring how `canvas`/`chip_interaction` tests
	//! exercise their own flows.

	use super::*;
	use crate::description::{ChipLibrary, ChipType, PinAddress, WireDescription};
	use crate::viewer::state::WireEditState;

	fn viewer_with_wire() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		let mut v = ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		let chip = v.library.get_mut("ROOT");
		let mut wire = WireDescription::new(PinAddress::new(1, 2), PinAddress::new(2, 1));
		wire.points = vec![Vec2::new(-4.0, 4.0)];
		chip.wires.push(wire);
		v
	}

	#[test]
	fn enter_toggles_and_exit_clears() {
		let mut v = viewer_with_wire();
		crate::viewer::wire_edit::enter(&mut v, 0);
		assert_eq!(v.wire_edit, Some(WireEditState { wire_index: 0, selected_bend: None }));
		// Entering on the same wire again leaves (`EnterWireEditMode`'s toggle).
		crate::viewer::wire_edit::enter(&mut v, 0);
		assert_eq!(v.wire_edit, None);
		crate::viewer::wire_edit::enter(&mut v, 0);
		crate::viewer::wire_edit::exit(&mut v);
		assert_eq!(v.wire_edit, None);
	}

	/// Clicking the edited wire's line inserts a bend exactly at the
	/// closest point and selects it; the inserted geometry is undoable.
	#[test]
	fn clicking_the_line_inserts_a_selected_bend() {
		let mut v = viewer_with_wire();

		crate::viewer::wire_edit::enter(&mut v, 0);
		// The wire runs (-6,-2) -> (-4,4) -> (6,2) roughly through y≈0 mid;
		// click right on its middle.
		let inserted = crate::viewer::wire_edit::insert_point_at_click(&mut v, Vec2::new(0.0, 0.5));
		assert!(inserted.is_some(), "a click near the line inserts");
		assert_eq!(v.wire_edit.and_then(|e| e.selected_bend), inserted);

		let chip = v.library.get("ROOT");
		assert_eq!(chip.wires[0].points.len(), 2, "one bend existed, one was inserted");

		crate::viewer::undo::try_undo(&mut v);
		assert_eq!(v.library.get("ROOT").wires[0].points.len(), 1, "the insert undoes");

		// Far from the line: nothing inserts.
		crate::viewer::wire_edit::enter(&mut v, 0);
		assert_eq!(crate::viewer::wire_edit::insert_point_at_click(&mut v, Vec2::new(0.0, 500.0)), None);
	}

	/// Delete removes the selected bend and shifts dependents' attachment
	/// indices that pointed past it; undo restores both.
	#[test]
	fn deleting_a_bend_shifts_dependent_indices_and_undoes() {
		let mut v = viewer_with_wire();
		{
			let chip = v.library.get_mut("ROOT");
			chip.wires[0].points = vec![Vec2::new(-4.0, 4.0), Vec2::new(0.0, 8.0)];
			// A tap attached to segment 1 (after the bend being deleted).
			let mut tap = WireDescription::new_tapped_source(PinAddress::new(1, 2), PinAddress::new(3, 1), 0, 1, Vec2::ZERO);
			tap.points.clear();
			chip.wires.push(tap);
		}

		crate::viewer::wire_edit::enter(&mut v, 0);
		v.wire_edit = Some(WireEditState { wire_index: 0, selected_bend: Some(0) });
		assert!(crate::viewer::wire_edit::delete_selected_bend(&mut v));

		let chip = v.library.get("ROOT");
		assert_eq!(chip.wires[0].points.len(), 1);
		assert_eq!(chip.wires[1].connected_wire_segment_index, 0, "the dependent's attachment shifted back past the removed vertex");

		crate::viewer::undo::try_undo(&mut v);
		let chip = v.library.get("ROOT");
		assert_eq!(chip.wires[0].points.len(), 2, "the bend comes back");
		assert_eq!(chip.wires[1].connected_wire_segment_index, 1, "and so does the dependent's attachment index");
	}

	/// A committed drag records one undo entry restoring the grabbed
	/// position; a click-without-move records nothing.
	#[test]
	fn committed_drags_undo_but_plain_grabs_do_not() {
		let mut v = viewer_with_wire();

		crate::viewer::wire_edit::enter(&mut v, 0);
		crate::viewer::wire_edit::begin_drag(&mut v, 0);
		crate::viewer::wire_edit::update_drag(&mut v, Vec2::new(-8.0, 12.0));
		crate::viewer::wire_edit::commit_drag(&mut v, 0, 0, Vec2::new(-4.0, 4.0));
		assert_eq!(v.undo.history_len(), 1, "a moved drag records one entry");
		assert_eq!(v.library.get("ROOT").wires[0].points[0], Vec2::new(-8.0, 12.0));

		let grab_point = v.library.get("ROOT").wires[0].points[0];
		crate::viewer::undo::try_undo(&mut v);
		assert_eq!(v.library.get("ROOT").wires[0].points[0], Vec2::new(-4.0, 4.0), "undo restores the grab-time point");

		// Grab without moving: no history entry.
		crate::viewer::wire_edit::commit_drag(&mut v, 0, 0, grab_point);
		assert_eq!(v.undo.history_len(), 1, "an unmoved commit is a no-op");
	}
}
