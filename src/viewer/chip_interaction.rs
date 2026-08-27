//! Component selection & movement, multi-component placement carries, and
//! rubber-band box selection -- the viewer-facing half of the original's
//! `DLS.Game.ChipInteractionController` (its selection/move/placement
//! state machine), adapted to this port's description-driven data model:
//!
//! - a *pickup* (`ViewerState::pending_place`) carries
//!   `Vec<(Vec2, PendingComponent)>` -- what to instantiate plus each
//!   component's position relative to the cursor -- so placing a bus
//!   origin automatically carries its linked terminus partner along;
//! - a *drag* moves the real placed subchip positions live as the cursor
//!   moves (attached wires stretch with them), rendering the carried
//!   components translucently like a placement ghost, and reverts
//!   everything if released overlapping something it may not cover;
//! - a *rubber band* drawn on empty canvas selects every component lying
//!   even partially inside it once released.

use crate::render::foundation::SceneGeometry;
use crate::render::layout::{self, snap_to_grid_centred};
use crate::render::scene;
use crate::render::theme;
use crate::sim::key_mods_bits;
use crate::structs::Vec2;
use crate::viewer::state::ViewerState;
use crate::{PinAddress, SubChipDescription, WireConnectionType, WireDescription};
use std::collections::HashMap;

/// One component picked up for placement: which library chip to
/// instantiate. The `Vec2` half of its `pending_place` entry carries its
/// position relative to the cursor. A pickup normally holds one component;
/// picking up a bus origin additionally carries its linked terminus
/// partner as a second entry (see [`start_placing`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingComponent {
	pub(crate) name: String,
	/// Index within the same carry vec of this bus origin/terminus' linked
	/// partner entry. Both halves of a pair get one; on drop they're written
	/// to each side's `SubChipDescription::internal_data[0]` (see
	/// `viewer::bus_wiring` for what that link guarantees downstream).
	pub(crate) linked_bus_partner: Option<usize>,
	/// Full description override for *duplicated* entries (`DuplicateElements`
	/// copies the original's internal data, label, and per-instance pin
	/// colours verbatim rather than starting from library defaults);
	/// `None` for ordinary library pickups.
	pub(crate) duplicate_of: Option<SubChipDescription>,
	/// Wires internal to the duplicated group (`DuplicateElements`'s
	/// duplicated-wire pass), carried once -- on the first entry -- and
	/// pushed onto the chip when the carry drops. Bend points are stored
	/// relative to the group's centroid so they land where the group lands.
	pub(crate) attached_wires: Vec<WireDescription>,
}

/// What a press-drag over the canvas is currently doing. `None` whenever
/// the last left press was swallowed by some UI layer or completed another
/// action (wire placement, pin toggle, ...).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum CanvasInteraction {
	#[default]
	None,
	/// Carrying the current selection around: where the grab started (world
	/// space) and each selected subchip's `(id, position at grab time)`.
	/// Positions update live in the library as the cursor moves -- wires
	/// stretch automatically, since their endpoints resolve from those
	/// positions every frame -- and revert if dropped somewhere illegal.
	MovingSelection { anchor: Vec2, originals: Vec<(i32, Vec2)> },
	/// Rubber-band selecting from a fixed world-space corner to the cursor.
	SelectionBox { start: Vec2 },
	/// Dragging one of the edited wire's bend points (wire edit mode's
	/// `isMovingWireEditPoint`): the bend follows the cursor live; commit
	/// records a wire-list undo entry, cancel restores `original`.
	WireBendDrag { wire_index: usize, bend_index: usize, original: Vec2 },
}

/// How much bigger than its component the faint selection rectangle is
/// drawn, on every side -- so the highlight always reads as sitting
/// *around* the component rather than exactly on it (mirrors
/// `DrawSettings.SelectionBoundsPadding`).
pub(crate) const SELECTION_BOUNDS_PAD: f32 = 0.08;

/// Total world-space distance between a bus origin and the terminus
/// partner carried with it (`ChipInteractionController.StartPlacing`'s
/// `busPairSpacing = GridSize * 8`; each half sits spacing/2 either side
/// of the cursor).
pub(crate) const BUS_PAIR_SPACING: f32 = layout::GRID_SIZE * 8.0;

/// Picks up `chip_name` for placement, replacing whatever carry/drag/
/// selection was in flight (mirrors `StartPlacing`'s `CancelEverything`).
/// A bus-*origin* pickup automatically carries its terminus partner as a
/// second entry -- mutually linked, offset half a pair-spacing to either
/// side of the cursor -- mirroring `StartPlacing`'s auto-place branch
/// rather than the placement-time special case this replaces.
pub(crate) fn start_placing(v: &mut ViewerState, chip_name: &str) {
	cancel_all(v);
	v.pending_wire = None;

	let chip_type = v.library.try_get(chip_name).map(|d| d.chip_type);
	let mut carry = vec![(
		Vec2::ZERO,
		PendingComponent { name: chip_name.to_string(), linked_bus_partner: None, duplicate_of: None, attached_wires: Vec::new() },
	)];

	if let Some(terminus_type) = chip_type.and_then(|t| t.corresponding_bus_terminus()) {
		if let Some(desc) = v.library.iter().find(|d| d.chip_type == terminus_type) {
			let terminus_name = desc.name.clone();
			carry[0].0 = Vec2::new(-BUS_PAIR_SPACING / 2.0, 0.0);
			carry.push((
				Vec2::new(BUS_PAIR_SPACING / 2.0, 0.0),
				PendingComponent { name: terminus_name, linked_bus_partner: Some(0), duplicate_of: None, attached_wires: Vec::new() },
			));
			carry[0].1.linked_bus_partner = Some(1);
		}
	}

	v.pending_place = carry;
}

/// A press landing on placed subchip `id`'s body: toggles it into/out of
/// the selection (shift held -- "multi-mode"), or selects it alone, then
/// starts carrying whatever ended up selected. Mirrors `Select` +
/// `StartMovingSelectedItems`.
///
/// Only subchips take part in selection/dragging: boundary dev-pins' click
/// surfaces stay owned by their other interactions (a dev-pin stub starts a
/// wire, an input's bit cells toggle it), matching the original's
/// pin-first priority.
pub(crate) fn begin_drag_on_component(v: &mut ViewerState, id: i32, anchor: Vec2) {
	if multi_mode_held(v) {
		match v.selected_ids.iter().position(|&sel| sel == id) {
			Some(pos) => {
				v.selected_ids.remove(pos);
			}
			None => v.selected_ids.push(id),
		}
	} else if !v.selected_ids.contains(&id) {
		v.selected_ids.clear();
		v.selected_ids.push(id);
	}

	let root_chip_name = v.root_chip_name.clone();
	let chip = v.library.get(&root_chip_name);
	let originals: Vec<(i32, Vec2)> =
		v.selected_ids.iter().filter_map(|&sel| chip.sub_chips.iter().find(|s| s.id == sel).map(|s| (sel, s.position))).collect();
	v.canvas_interaction = if originals.is_empty() { CanvasInteraction::None } else { CanvasInteraction::MovingSelection { anchor, originals } };
}

/// Starts a rubber-band selection at `press_world_pos`. Without shift the
/// current selection clears up front (shift keeps it, so several bands can
/// add up), mirroring `HandleLeftMouseDown`'s empty-canvas branch.
pub(crate) fn begin_selection_box(v: &mut ViewerState, press_world_pos: Vec2) {
	if !multi_mode_held(v) {
		v.selected_ids.clear();
	}
	v.canvas_interaction = CanvasInteraction::SelectionBox { start: press_world_pos };
}

/// Moves every carried component to follow the cursor, preserving their
/// grabbed relative arrangement. With snapping enabled, the first carried
/// component snaps onto the grid and the rest follow by the same delta --
/// snapping each independently would make them jiggle against one another
/// (same reasoning as the original's relative-snap branch).
pub(crate) fn update_move_to_cursor(v: &mut ViewerState, cursor_world: Vec2) {
	match &v.canvas_interaction {
		CanvasInteraction::MovingSelection { anchor, originals } => {
			let anchor = *anchor;
			let originals = originals.clone();

			let mut delta = cursor_world - anchor;
			if let Some((_, first_original)) = originals.first() {
				if v.should_snap_to_grid() {
					delta = snap_to_grid_centred(*first_original + delta) - *first_original;
				}
			}

			let root_chip_name = v.root_chip_name.clone();
			let chip = v.library.get_mut(&root_chip_name);
			for (id, original) in &originals {
				if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == *id) {
					sub.position = *original + delta;
				}
			}
		}
		CanvasInteraction::WireBendDrag { .. } => {
			crate::viewer::wire_edit::update_drag(v, cursor_world);
		}
		_ => {}
	}
}

/// Finishes whatever the current left-press drag became: commits (or
/// reverts) a moving selection, or turns the rubber band into selection
/// membership for every component even partially inside it. No-op when
/// nothing is in flight (e.g. the press landed on a UI layer).
pub(crate) fn handle_canvas_release(v: &mut ViewerState, cursor_world: Vec2) {
	match v.canvas_interaction.clone() {
		CanvasInteraction::MovingSelection { originals, .. } => {
			if move_is_illegal(v) {
				revert_move(v);
			} else {
				// A committed drag is one recorded move action
				// (`FinishMovingElements` -> `RecordMoveElements`); a
				// reverted one records nothing, and neither does a plain
				// click that never actually moved anything.
				let root_chip_name = v.root_chip_name.clone();
				let chip = v.library.get(&root_chip_name);
				let entries: Vec<(i32, Vec2, Vec2)> = originals
					.iter()
					.filter_map(|&(id, original)| chip.sub_chips.iter().find(|s| s.id == id).map(|s| (id, original, s.position)))
					.filter(|&(_, original, new)| original != new)
					.collect();
				crate::viewer::undo::record_move(v, entries);
			}
			// The selection itself survives the drag (only Escape/right-click
			// clears it), mirroring `FinishMovingElements`.
		}
		CanvasInteraction::SelectionBox { start } => finish_selection_box(v, start, cursor_world),
		CanvasInteraction::WireBendDrag { wire_index, bend_index, original } => {
			// A real drag commits as one wire-list undo entry; a plain
			// click on a handle snapshots equal lists and records nothing
			// (see `record_wire_list_edit_with`).
			crate::viewer::wire_edit::commit_drag(v, wire_index, bend_index, original);
		}
		CanvasInteraction::None => {}
	}
	v.canvas_interaction = CanvasInteraction::None;
}

/// Reverts an in-flight move (if any) and clears the selection/rubber-band
/// state entirely -- the shared "Escape / right-click / new pickup" reset.
pub(crate) fn cancel_all(v: &mut ViewerState) {
	revert_move(v);
	// A carried wire-edit bend goes back to where it was grabbed; edit
	// mode itself stays on (`CancelEverything` doesn't exit it either).
	if let CanvasInteraction::WireBendDrag { wire_index, bend_index, original } = v.canvas_interaction {
		let root_chip_name = v.root_chip_name.clone();
		if let Some(wire) = v.library.get_mut(&root_chip_name).wires.get_mut(wire_index) {
			if let Some(point) = wire.points.get_mut(bend_index) {
				*point = original;
			}
		}
	}
	v.canvas_interaction = CanvasInteraction::None;
	v.selected_ids.clear();
}

/// Appends the canvas's interaction overlays to `geo`: a faint rectangle
/// (the component's own footprint grown by [`SELECTION_BOUNDS_PAD`] on
/// every side) over every selected component -- tinted red while an
/// illegal drag is in flight -- plus the rubber-band rectangle while one
/// is being drawn. Called after the scene itself, so highlights sit on
/// top of everything drawn beneath them.
pub(crate) fn append_selection_geometry(geo: &mut SceneGeometry, v: &ViewerState, cursor_world: Vec2) {
	let highlight_col = match &v.canvas_interaction {
		CanvasInteraction::MovingSelection { .. } if move_is_illegal(v) => theme::SELECTION_BOX_INVALID_COL,
		CanvasInteraction::MovingSelection { .. } => theme::SELECTION_BOX_MOVING_COL,
		_ => theme::SELECTION_BOX_COL,
	};

	let root_desc = v.library.get(&v.root_chip_name);
	let placed = scene::place_sub_chips(root_desc, &v.library);
	for &id in &v.selected_ids {
		if let Some(sub) = placed.iter().find(|p| p.id == id) {
			geo.add_rect(sub.centre, selection_bounds_size(sub.size), highlight_col);
		}
	}

	if let CanvasInteraction::SelectionBox { start } = &v.canvas_interaction {
		let centre = (*start + cursor_world) / 2.0;
		let size = Vec2::new((start.x - cursor_world.x).abs(), (start.y - cursor_world.y).abs());
		geo.add_rect(centre, size, theme::SELECTION_BOX_COL);
	}
}

/// Footprint of a component's selection rectangle: its own body size grown
/// by [`SELECTION_BOUNDS_PAD`] on every side.
fn selection_bounds_size(body_size: Vec2) -> Vec2 {
	body_size + Vec2::splat(SELECTION_BOUNDS_PAD * 2.0)
}

/// Whether two axis-aligned rects (centre + size) overlap at all -- even
/// partially. Touching edges don't count, so flush-adjacent placements
/// stay legal.
fn boxes_overlap(a_centre: Vec2, a_size: Vec2, b_centre: Vec2, b_size: Vec2) -> bool {
	(a_centre.x - b_centre.x).abs() * 2.0 < a_size.x + b_size.x && (a_centre.y - b_centre.y).abs() * 2.0 < a_size.y + b_size.y
}

fn multi_mode_held(v: &ViewerState) -> bool {
	// `KeyboardShortcuts.MultiModeHeld`: Alt *or* Shift.
	let mods = v.sim.key_modifiers();
	mods & (key_mods_bits::SHIFT | key_mods_bits::ALT) != 0
}

/// Whether the carried selection currently overlaps anything it may not
/// land on -- any non-carried placed component (the obstacle rule
/// `FinishMovingElements` applies before committing a move). Wires aren't
/// obstacles: components may sit across wires freely, same as placement.
fn move_is_illegal(v: &ViewerState) -> bool {
	let CanvasInteraction::MovingSelection { originals, .. } = &v.canvas_interaction else { return false };

	let root_desc = v.library.get(&v.root_chip_name);
	let placed = scene::place_sub_chips(root_desc, &v.library);
	let carried: Vec<(Vec2, Vec2)> = originals.iter().filter_map(|(id, _)| placed.iter().find(|p| p.id == *id).map(|p| (p.centre, p.size))).collect();

	placed
		.iter()
		.filter(|p| !originals.iter().any(|(id, _)| *id == p.id))
		.any(|obstacle| carried.iter().any(|&(centre, size)| boxes_overlap(centre, size, obstacle.centre, obstacle.size)))
}

/// Restores every carried component to its grab-time position.
fn revert_move(v: &mut ViewerState) {
	let CanvasInteraction::MovingSelection { originals, .. } = &v.canvas_interaction else { return };

	let root_chip_name = v.root_chip_name.clone();
	let chip = v.library.get_mut(&root_chip_name);
	for (id, original) in originals {
		if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == *id) {
			sub.position = *original;
		}
	}
}

/// Selects every placed component whose (padded) footprint lies even
/// partially inside the rubber band, adding to whatever selection the
/// press-time clear left in place. A degenerate (plain-click-sized) band
/// adds nothing -- its selection effect already happened at press time.
fn finish_selection_box(v: &mut ViewerState, start: Vec2, end: Vec2) {
	let size = Vec2::new((start.x - end.x).abs(), (start.y - end.y).abs());
	if size.x * size.y <= 1e-6 {
		return;
	}
	let centre = (start + end) / 2.0;

	let hits: Vec<i32> = {
		let root_desc = v.library.get(&v.root_chip_name);
		scene::place_sub_chips(root_desc, &v.library)
			.iter()
			.filter(|sub| boxes_overlap(centre, size, sub.centre, selection_bounds_size(sub.size)))
			.map(|sub| sub.id)
			.collect()
	};
	for id in hits {
		if !v.selected_ids.contains(&id) {
			v.selected_ids.push(id);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ChipLibrary;

	fn viewer_with_builtins() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(crate::ChipDescription::new("ROOT", crate::ChipType::Custom));
		ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	fn place_nand(v: &mut ViewerState, pos: Vec2) -> i32 {
		start_placing(v, "NAND");
		let mut status = None;
		crate::viewer::canvas::try_place_pending_components(v, pos, &mut status);
		assert!(status.is_none(), "drop on free canvas space must succeed");
		v.library.get("ROOT").sub_chips.last().expect("placement succeeded").id
	}

	fn position_of(v: &ViewerState, id: i32) -> Vec2 {
		v.library.get("ROOT").sub_chips.iter().find(|s| s.id == id).expect("component exists").position
	}

	#[test]
	fn start_placing_a_bus_carries_its_linked_terminus_as_a_second_entry() {
		let mut v = viewer_with_builtins();

		start_placing(&mut v, "BUS-4");

		assert_eq!(v.pending_place.len(), 2);
		assert_eq!(v.pending_place[0].1.name, "BUS-4");
		assert_eq!(v.pending_place[1].1.name, "BUS-TERMINUS-4");
		assert_eq!(v.pending_place[0].0, Vec2::new(-BUS_PAIR_SPACING / 2.0, 0.0));
		assert_eq!(v.pending_place[1].0, Vec2::new(BUS_PAIR_SPACING / 2.0, 0.0));
		assert_eq!(v.pending_place[0].1.linked_bus_partner, Some(1));
		assert_eq!(v.pending_place[1].1.linked_bus_partner, Some(0));

		// Anything else is a single-component carry at the cursor itself.
		start_placing(&mut v, "NAND");
		assert_eq!(v.pending_place.len(), 1);
		assert_eq!(v.pending_place[0].0, Vec2::ZERO);
		assert_eq!(v.pending_place[0].1.linked_bus_partner, None);
	}

	#[test]
	fn dropping_the_carry_places_the_mutually_linked_bus_pair() {
		let mut v = viewer_with_builtins();
		start_placing(&mut v, "BUS-4");

		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);

		assert!(v.pending_place.is_empty(), "the whole carry drops together");
		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 2);
		let (origin_id, terminus_id) = (chip.sub_chips[0].id, chip.sub_chips[1].id);
		assert_eq!(position_of(&v, origin_id), Vec2::new(-BUS_PAIR_SPACING / 2.0, 0.0));
		assert_eq!(position_of(&v, terminus_id), Vec2::new(BUS_PAIR_SPACING / 2.0, 0.0));
		assert!(crate::viewer::bus_wiring::bus_pair_linked(chip, &v.library, origin_id, terminus_id), "dropped pair satisfies the linked-pair check");
	}

	#[test]
	fn blocked_drop_leaves_the_whole_carry_pending() {
		let mut v = viewer_with_builtins();
		place_nand(&mut v, Vec2::ZERO);

		start_placing(&mut v, "NAND");
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);

		assert_eq!(v.pending_place.len(), 1, "landing on an existing component keeps the carry alive");
		assert_eq!(v.library.get("ROOT").sub_chips.len(), 1, "nothing extra got placed");
	}

	#[test]
	fn box_select_picks_up_partially_covered_components_only() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));

		// The band reaches only partway into `a`'s body; `b` stays outside entirely.
		begin_selection_box(&mut v, Vec2::new(-1.0, -1.0));
		handle_canvas_release(&mut v, Vec2::new(0.05, 1.0));

		assert_eq!(v.selected_ids, vec![a]);
		assert_ne!(v.selected_ids, vec![b]);

		// A degenerate (click-sized) band adds nothing further -- and with
		// shift held (so the press didn't clear first) the existing
		// selection survives it untouched.
		v.sim.set_key_modifiers(key_mods_bits::SHIFT);
		begin_selection_box(&mut v, Vec2::new(-1.0, -1.0));
		handle_canvas_release(&mut v, Vec2::new(-1.0000001, -1.0));
		v.sim.set_key_modifiers(0);
		assert_eq!(v.selected_ids, vec![a]);
	}

	#[test]
	fn drag_moves_live_and_reverts_when_released_on_an_obstacle() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));

		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		update_move_to_cursor(&mut v, Vec2::new(4.0, 0.0));
		assert_eq!(position_of(&v, a), Vec2::new(4.0, 0.0), "carried component follows the cursor live");

		handle_canvas_release(&mut v, Vec2::new(4.0, 0.0));
		assert_eq!(position_of(&v, a), Vec2::ZERO, "overlapping `b` is illegal, so the move reverts");
		assert_eq!(v.canvas_interaction, CanvasInteraction::None);
		assert_eq!(v.selected_ids, vec![a], "the selection itself survives a cancelled move");

		// A free-space drop sticks.
		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		update_move_to_cursor(&mut v, Vec2::new(-4.0, 0.0));
		handle_canvas_release(&mut v, Vec2::new(-4.0, 0.0));
		assert_eq!(position_of(&v, a), Vec2::new(-4.0, 0.0));
		assert_eq!(position_of(&v, b), Vec2::new(4.0, 0.0), "non-carried components never move");
	}

	#[test]
	fn shift_press_toggles_multi_selection_membership() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));
		v.sim.set_key_modifiers(key_mods_bits::SHIFT);

		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		assert_eq!(v.selected_ids, vec![a]);
		begin_drag_on_component(&mut v, b, Vec2::new(4.0, 0.0));
		assert_eq!(v.selected_ids, vec![a, b]);
		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		assert_eq!(v.selected_ids, vec![b], "shift-pressing a selected component removes it again");

		v.sim.set_key_modifiers(0);
	}

	/// The real click router: an empty-canvas press must open the rubber
	/// band, and a body press must select + start the drag -- while a pin
	/// under the cursor still claims the click for wire placement first
	/// (covered end-to-end by the bus-wiring tests in `canvas`).
	#[test]
	fn canvas_click_routes_empty_space_to_box_select_and_bodies_to_dragging() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);

		crate::viewer::canvas::handle_canvas_click(&mut v, Vec2::new(-9.0, -9.0), &mut None);
		assert_eq!(v.canvas_interaction, CanvasInteraction::SelectionBox { start: Vec2::new(-9.0, -9.0) });
		handle_canvas_release(&mut v, Vec2::new(-9.0, -9.0));
		assert!(v.selected_ids.is_empty(), "a plain empty-canvas click cleared the selection");

		crate::viewer::canvas::handle_canvas_click(&mut v, Vec2::ZERO, &mut None);
		assert_eq!(v.selected_ids, vec![a], "pressing the body selects it");
		assert!(matches!(v.canvas_interaction, CanvasInteraction::MovingSelection { .. }), "and starts carrying it");
	}

	#[test]
	fn cancel_all_reverts_an_inflight_move_and_clears_everything() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		place_nand(&mut v, Vec2::new(4.0, 0.0));

		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		update_move_to_cursor(&mut v, Vec2::new(4.0, 0.0));
		cancel_all(&mut v);

		assert_eq!(position_of(&v, a), Vec2::ZERO, "an in-flight move is undone");
		assert_eq!(v.canvas_interaction, CanvasInteraction::None);
		assert!(v.selected_ids.is_empty());
	}

	// ---- Rendering-side checks (white-box: drives the overlay builder and
	// the full frame stack on a real ViewerState) ----

	fn placed_body(v: &ViewerState, id: i32) -> (Vec2, Vec2) {
		let root_desc = v.library.get("ROOT");
		let sub = crate::render::scene::place_sub_chips(root_desc, &v.library).into_iter().find(|p| p.id == id).expect("placed");
		(sub.centre, sub.size)
	}

	fn bounds(geo: &SceneGeometry) -> (Vec2, Vec2) {
		let mut iter = geo.triangles.iter().map(|t| t.pos);
		let first = iter.next().expect("non-empty geometry");
		let mut min = first;
		let mut max = first;
		for p in iter {
			min.x = min.x.min(p.x);
			min.y = min.y.min(p.y);
			max.x = max.x.max(p.x);
			max.y = max.y.max(p.y);
		}
		(min, max)
	}

	fn has_alpha_near(geo: &SceneGeometry, target: f32) -> bool {
		geo.triangles.iter().any(|v| (v.colour[3] - target).abs() < 1e-4)
	}

	fn has_colour(geo: &SceneGeometry, colour: theme::Rgba) -> bool {
		geo.triangles.iter().any(|v| v.colour == colour)
	}

	#[test]
	fn selection_highlight_is_the_body_grown_by_a_constant_pad() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		v.selected_ids.push(a);

		let mut geo = SceneGeometry::default();
		append_selection_geometry(&mut geo, &v, Vec2::ZERO);

		let (centre, size) = placed_body(&v, a);
		let (min, max) = bounds(&geo);
		assert_eq!(min, centre - Vec2::new(size.x / 2.0 + SELECTION_BOUNDS_PAD, size.y / 2.0 + SELECTION_BOUNDS_PAD));
		assert_eq!(max, centre + Vec2::new(size.x / 2.0 + SELECTION_BOUNDS_PAD, size.y / 2.0 + SELECTION_BOUNDS_PAD));
		assert!(geo.triangles.iter().all(|vtx| vtx.colour == theme::SELECTION_BOX_COL), "an idle selection highlights in the plain faint white");
	}

	#[test]
	fn highlight_colour_tracks_drag_validity_and_rubber_band_tracks_the_cursor() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		place_nand(&mut v, Vec2::new(4.0, 0.0));

		// A legal drag highlights in the slightly brighter moving tint...
		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		update_move_to_cursor(&mut v, Vec2::new(-4.0, 0.0));
		let mut geo = SceneGeometry::default();
		append_selection_geometry(&mut geo, &v, Vec2::ZERO);
		assert!(has_colour(&geo, theme::SELECTION_BOX_MOVING_COL), "legal move uses the moving tint");
		assert!(!has_colour(&geo, theme::SELECTION_BOX_INVALID_COL));

		// ...an illegal one flips to red.
		update_move_to_cursor(&mut v, Vec2::new(4.0, 0.0));
		let mut geo = SceneGeometry::default();
		append_selection_geometry(&mut geo, &v, Vec2::ZERO);
		assert!(has_colour(&geo, theme::SELECTION_BOX_INVALID_COL), "landing on an obstacle tints the carry red");

		// A rubber band draws one quad from its fixed start to the live cursor.
		cancel_all(&mut v);
		begin_selection_box(&mut v, Vec2::new(-2.0, -1.0));
		let mut geo = SceneGeometry::default();
		append_selection_geometry(&mut geo, &v, Vec2::new(3.0, 2.0));
		assert!(has_colour(&geo, theme::SELECTION_BOX_COL), "the band itself is the faint white");
		let (min, max) = bounds(&geo);
		assert_eq!(min, Vec2::new(-2.0, -1.0));
		assert_eq!(max, Vec2::new(3.0, 2.0));
	}

	#[test]
	fn multi_drag_moves_every_selected_component_preserving_arrangement_under_snapping() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(1.0, 0.0));
		v.prefs.prefs_snapping = 2; // "always snap"

		v.sim.set_key_modifiers(key_mods_bits::SHIFT);
		begin_drag_on_component(&mut v, a, Vec2::new(0.0, 0.0));
		begin_drag_on_component(&mut v, b, Vec2::new(1.0, 0.0));
		v.sim.set_key_modifiers(0);

		update_move_to_cursor(&mut v, Vec2::new(1.53, 0.47)); // anchor was on b (the last press)

		assert_eq!(position_of(&v, a), Vec2::new(0.5, 0.5), "the first-carried component snaps onto the grid");
		assert_eq!(position_of(&v, b), Vec2::new(1.5, 0.5), "the rest follow by the same delta, no jiggle");

		handle_canvas_release(&mut v, Vec2::new(1.53, 0.47));
		assert_eq!(position_of(&v, a), Vec2::new(0.5, 0.5), "a legal group drop commits for everyone");
	}

	/// The end-to-end render contract: while a drag is live, exactly the
	/// carried components' own triangles fade to the placement-ghost alpha
	/// (their pins/wires stay solid), and drawing a rubber band shows the
	/// faint band instead.
	#[test]
	fn frame_renders_carried_components_translucent_and_shows_the_rubber_band() {
		use crate::viewer::canvas::PENDING_PLACEMENT_ALPHA;
		use crate::viewer::frame::build_viewer_stack;

		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		v.camera.position = Vec2::ZERO;
		v.camera.zoom = 100.0;
		v.camera_fitted = true; // keep our camera; skip auto-fit
		let cam = v.camera; // Camera is Copy; avoids borrowing `v` across the builds below
		let mouse_at = move |world: Vec2| cam.world_to_screen(world);

		// Idle: nothing translucent anywhere on the canvas layer.
		let stack = build_viewer_stack(&mut v, None, 1280.0, 800.0, mouse_at(Vec2::new(-50.0, -50.0)));
		let canvas_geo = &stack.layers()[0].geometry;
		assert!(!has_alpha_near(canvas_geo, PENDING_PLACEMENT_ALPHA), "idle scene has no ghost-faded vertices");

		// Dragging: the carried component fades, everything else stays solid,
		// and the moving-tint highlight quad rides on top.
		begin_drag_on_component(&mut v, a, Vec2::ZERO);
		update_move_to_cursor(&mut v, Vec2::new(4.0, 0.0));
		let stack = build_viewer_stack(&mut v, None, 1280.0, 800.0, mouse_at(Vec2::new(4.0, 0.0)));
		let canvas_geo = &stack.layers()[0].geometry;
		assert!(has_alpha_near(canvas_geo, PENDING_PLACEMENT_ALPHA), "carried body renders at ghost alpha");
		assert!(has_alpha_near(canvas_geo, theme::SELECTION_BOX_MOVING_COL[3]), "moving highlight drawn over it");
		assert_eq!(position_of(&v, a), Vec2::new(4.0, 0.0), "frame building doesn't disturb the live drag");

		// Rubber band: band quad visible instead, nothing faded.
		cancel_all(&mut v);
		begin_selection_box(&mut v, Vec2::new(-2.0, -1.0));
		let stack = build_viewer_stack(&mut v, None, 1280.0, 800.0, mouse_at(Vec2::new(3.0, 2.0)));
		let canvas_geo = &stack.layers()[0].geometry;
		assert!(has_alpha_near(canvas_geo, theme::SELECTION_BOX_COL[3]), "rubber band visible while dragging the mouse");
		assert!(!has_alpha_near(canvas_geo, PENDING_PLACEMENT_ALPHA), "no ghost alpha during box select");
	}
}

/// Picks up a duplicate of the current selection (`DuplicateSelectedElements`,
/// on the MultiMode+D shortcut): every selected subchip -- plus any bus
/// partner hanging outside the selection -- is copied with fresh ids,
/// links pointing inside the group are re-mapped to their duplicates
/// (`LinkDuplicatedBuses`; links to anything outside are cleared), wires
/// internal to the group come along (bends re-anchored to the group's
/// centroid), and the whole group lands in the placement carry for
/// dropping like any pickup. Returns whether anything was picked up.
pub(crate) fn duplicate_selection(v: &mut ViewerState) -> bool {
	// Only over the bare editor (`!IsPlacingOrMovingElementOrCreatingWire`).
	if !v.pending_place.is_empty() || v.pending_wire.is_some() || !matches!(v.canvas_interaction, CanvasInteraction::None) || v.wire_edit.is_some() {
		return false;
	}
	if v.selected_ids.is_empty() {
		return false;
	}

	let root_chip_name = v.root_chip_name.clone();

	// Expand through bus partners so a carried half always brings its pair.
	let mut ids: Vec<i32> = Vec::new();
	for &id in &v.selected_ids {
		for expanded in crate::viewer::canvas::compute_component_delete_set(v, id) {
			if !ids.contains(&expanded) {
				ids.push(expanded);
			}
		}
	}

	let originals: Vec<SubChipDescription> = { v.library.get(&root_chip_name).sub_chips.iter().filter(|s| ids.contains(&s.id)).cloned().collect() };
	if originals.is_empty() {
		return false;
	}

	let centroid = {
		let mut acc = Vec2::ZERO;
		for s in &originals {
			acc += s.position;
		}
		acc / originals.len() as f32
	};

	// Fresh ids across all three id spaces (subchips + dev-pins share one).
	let mut next_id = crate::viewer::canvas::next_component_id(v.library.get(&root_chip_name));
	let mut id_map: HashMap<i32, i32> = HashMap::new();
	for source in &originals {
		next_id += 1;
		id_map.insert(source.id, next_id - 1);
	}

	// Copies with re-mapped links: a partner duplicated alongside points
	// at its fresh id; one left behind clears to "no partner" so no
	// dangling link survives into delete cascades.
	let linked_partner = |s: &SubChipDescription| s.internal_data.as_ref().and_then(|d| d.first()).map(|&v| v as i32).unwrap_or(0);
	let mut carry: Vec<(Vec2, PendingComponent)> = Vec::with_capacity(originals.len());
	for source in &originals {
		let mut copy = source.clone();
		copy.id = id_map[&source.id];
		// Only Bus components stash a linked-partner id in `internal_data[0]`
		// (see `bus_wiring`) -- remapping it to the fresh duplicate's id is
		// what makes a carried bus half still find its pair. Every other
		// component's `internal_data` is its own custom payload (e.g. the
		// ROM editor's 256-word contents) that must come along untouched;
		// previously this unconditionally truncated it down to 2 slots,
		// which silently dropped everything past index 1.
		let is_bus = v.library.try_get(&source.name).is_some_and(|d| d.chip_type.is_bus_type());
		if is_bus {
			let new_link = id_map.get(&linked_partner(source)).copied().unwrap_or(0);
			let mut data = copy.internal_data.clone().unwrap_or_default();
			data.resize(2, 0);
			data[0] = new_link as u32;
			copy.internal_data = Some(data);
		}

		carry.push((
			source.position - centroid,
			PendingComponent { name: copy.name.clone(), linked_bus_partner: None, duplicate_of: Some(copy), attached_wires: Vec::new() },
		));
	}

	// Wires whose BOTH ends sit inside the group duplicate too; taps onto
	// wires outside the group degrade to plain pin connections. Bend
	// points re-anchor relative to the centroid.
	let old_wires = v.library.get(&root_chip_name).wires.clone();
	let existing_len = old_wires.len();
	let mut attached_wires: Vec<WireDescription> = Vec::new();
	let mut wire_index_map: HashMap<usize, usize> = HashMap::new(); // old wire idx -> new wire idx
	for (old_idx, wire) in old_wires.iter().enumerate() {
		let (Some(src_new), Some(dst_new)) = (id_map.get(&wire.source_pin_address.pin_owner_id), id_map.get(&wire.target_pin_address.pin_owner_id))
		else {
			continue;
		};
		let mut copy = wire.clone();
		copy.source_pin_address = PinAddress::new(*src_new, wire.source_pin_address.pin_id);
		copy.target_pin_address = PinAddress::new(*dst_new, wire.target_pin_address.pin_id);
		copy.points = wire.points.iter().map(|p| *p - centroid).collect();
		copy.cached_source_point = copy.cached_source_point - centroid;
		copy.cached_target_point = copy.cached_target_point - centroid;

		if copy.connection_type != WireConnectionType::ToPins {
			match wire_index_map.get(&(copy.connected_wire_index.max(0) as usize)) {
				Some(&new_idx) => copy.connected_wire_index = new_idx as i32,
				None => {
					copy.connection_type = WireConnectionType::ToPins;
					copy.connected_wire_index = 0;
					copy.connected_wire_segment_index = 0;
				}
			}
		}
		wire_index_map.insert(old_idx, existing_len + attached_wires.len());
		attached_wires.push(copy);
	}

	if !attached_wires.is_empty() {
		carry[0].1.attached_wires = attached_wires;
	}

	v.pending_place = carry;
	true
}

// ---- Duplicate selection (MultiMode+D) ----
#[cfg(test)]
mod duplicate_tests {
	use super::*;
	use crate::ChipLibrary;

	fn viewer_with_builtins() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(crate::ChipDescription::new("ROOT", crate::ChipType::Custom));
		ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	fn place_nand(v: &mut ViewerState, pos: Vec2) -> i32 {
		start_placing(v, "NAND");
		let mut status = None;
		crate::viewer::canvas::try_place_pending_components(v, pos, &mut status);
		assert!(status.is_none(), "drop on free canvas space must succeed");
		v.library.get("ROOT").sub_chips.last().expect("placement succeeded").id
	}

	#[test]
	fn duplicating_two_wired_components_carries_the_wire_between_them() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(8.0, 0.0));
		{
			let chip = v.library.get_mut("ROOT");
			chip.wires.push(WireDescription::new(PinAddress::new(a, 2), PinAddress::new(b, 1)));
		}

		v.selected_ids = vec![a, b];
		assert!(duplicate_selection(&mut v));

		assert_eq!(v.pending_place.len(), 2, "both selected components are carried");
		// The internal wire rides on the first entry.
		let wires = &v.pending_place[0].1.attached_wires;
		assert_eq!(wires.len(), 1, "the wire between the two duplicates comes along");
		assert_ne!(wires[0].source_pin_address.pin_owner_id, a, "endpoints re-point at fresh ids");
		assert_ne!(wires[0].target_pin_address.pin_owner_id, b);

		// Drop: fresh ids across the shared id space, wire translated to the
		// drop site and pushed onto the chip.
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::new(40.0, 40.0), &mut None);
		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 4);
		assert_eq!(chip.wires.len(), 2, "original plus duplicated wire");
		let new_wire = &chip.wires[1];
		assert_eq!(new_wire.source_pin_address.pin_owner_id, chip.sub_chips[2].id);
		assert_eq!(new_wire.target_pin_address.pin_owner_id, chip.sub_chips[3].id);
	}

	#[test]
	fn duplicating_one_half_of_a_bus_pair_brings_and_relinks_the_partner() {
		let mut v = viewer_with_builtins();
		start_placing(&mut v, "BUS-4");
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		let ids: Vec<i32> = v.library.get("ROOT").sub_chips.iter().map(|s| s.id).collect();
		let (origin, terminus) = (ids[0], ids[1]);

		v.selected_ids = vec![origin];
		assert!(duplicate_selection(&mut v));

		// The terminus partner was pulled into the carry...
		assert_eq!(v.pending_place.len(), 2, "origin + its outside partner");
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::new(20.0, 20.0), &mut None);

		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 4, "pair duplicated as a pair");
		let data = |id: i32| chip.sub_chips.iter().find(|s| s.id == id).expect("exists").internal_data.clone().unwrap_or_default()[0] as i32;
		// The duplicated pair links to each other, not to the originals.
		assert_ne!(data(chip.sub_chips[2].id), terminus, "duplicate origin doesn't point at the original terminus");
		assert_eq!(data(chip.sub_chips[2].id), chip.sub_chips[3].id, "duplicate origin points at the duplicate terminus");
		assert_eq!(data(chip.sub_chips[3].id), chip.sub_chips[2].id, "and vice versa");
	}

	#[test]
	fn duplicate_is_refused_while_something_else_is_in_flight() {
		let mut v = viewer_with_builtins();
		place_nand(&mut v, Vec2::ZERO);
		v.selected_ids.push(v.library.get("ROOT").sub_chips[0].id);

		start_placing(&mut v, "NAND");
		assert!(!duplicate_selection(&mut v), "a placement carry blocks duplicating");
		v.pending_place.clear();

		v.pending_wire = Some(crate::viewer::wire_draft::PendingWire {
			start: crate::viewer::wire_draft::PendingWireEnd::Pin { owner_id: 1, pin_id: 2, is_source: true, position: Vec2::ZERO },
			bend_points: Vec::new(),
			bit_count: crate::PinBitCount::Bit1,
		});
		assert!(!duplicate_selection(&mut v), "a pending wire blocks duplicating");
	}

	/// Regression test for the bug this fix addresses: duplicating a
	/// component whose `internal_data` is a large custom payload (a ROM's
	/// 256 words, not a bus's 2-slot linked-partner data) used to get
	/// silently truncated down to 2 elements by the bus-remapping logic,
	/// which ran unconditionally on every component's `internal_data`.
	#[test]
	fn duplicating_a_rom_carries_its_entire_internal_data_not_just_the_first_two_words() {
		let mut v = viewer_with_builtins();
		start_placing(&mut v, "ROM 256\u{d7}16");
		let mut status = None;
		crate::viewer::canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut status);
		assert!(status.is_none(), "drop on free canvas space must succeed");

		let rom_id = v.library.get("ROOT").sub_chips.last().expect("placement succeeded").id;
		let full_data: Vec<u32> = (0..256).collect();
		{
			let sub = v.library.get_mut("ROOT").sub_chips.iter_mut().find(|s| s.id == rom_id).unwrap();
			sub.internal_data = Some(full_data.clone());
		}

		v.selected_ids = vec![rom_id];
		assert!(duplicate_selection(&mut v));

		let carried = &v.pending_place[0].1.duplicate_of.as_ref().expect("ROM copy is carried as a full override").internal_data;
		assert_eq!(carried.as_ref(), Some(&full_data), "every word rides along, not just the first two");
	}
}
