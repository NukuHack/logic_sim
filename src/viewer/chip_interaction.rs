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
	let mut carry = vec![(Vec2::ZERO, PendingComponent { name: chip_name.to_string(), linked_bus_partner: None })];

	if let Some(terminus_type) = chip_type.and_then(|t| t.corresponding_bus_terminus()) {
		if let Some(desc) = v.library.iter().find(|d| d.chip_type == terminus_type) {
			let terminus_name = desc.name.clone();
			carry[0].0 = Vec2::new(-BUS_PAIR_SPACING / 2.0, 0.0);
			carry.push((Vec2::new(BUS_PAIR_SPACING / 2.0, 0.0), PendingComponent { name: terminus_name, linked_bus_partner: Some(0) }));
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
	let CanvasInteraction::MovingSelection { anchor, originals } = &v.canvas_interaction else { return };
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
		CanvasInteraction::None => {}
	}
	v.canvas_interaction = CanvasInteraction::None;
}

/// Reverts an in-flight move (if any) and clears the selection/rubber-band
/// state entirely -- the shared "Escape / right-click / new pickup" reset.
pub(crate) fn cancel_all(v: &mut ViewerState) {
	revert_move(v);
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
	v.sim.key_modifiers() & key_mods_bits::SHIFT != 0
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
