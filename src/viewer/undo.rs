//! Undo/redo for the chip editor, ported from `DLS.Game.UndoController`
//! (and its `MoveUndoAction` / `WireExistenceAction` /
//! `ElementExistenceAction` trio), adapted to this port's
//! description-driven data model:
//!
//! - a *move* stores `(id, position at grab, position at drop)` per
//!   carried component; wires need no counterpart because wire geometry
//!   here resolves live from component positions;
//! - *wire add/delete* stores the `WireDescription` plus its index in the
//!   root chip's wire list; deleting additionally snapshots the FULL wire
//!   list (flagging the doomed one), so cascade losses -- a tap wire
//!   dying with the wire it tapped onto -- come back on undo exactly as
//!   the original's `FullWireState.Restore` does;
//! - *element add/delete* stores the placed subchip + boundary dev-pin
//!   descriptions involved (bus-pair partners included via
//!   `compute_component_delete_set`); deleting again snapshots the full
//!   wire list first.
//!
//! History is strictly linear: recording while undone truncates the redo
//! tail (`RecordUndoAction`'s `RemoveRange`). Replaying an action applies
//! it to live state then rebuilds the simulation; anything that would no
//! longer resolve is skipped silently rather than half-applied, mirroring
//! the original swallowing its own trigger exceptions.

use crate::description::{ChipDescription, PinAddress, PinDescription, SubChipDescription, WireDescription};
use crate::render::scene;
use crate::viewer::canvas::{self, compute_component_delete_set};
use crate::viewer::state::ViewerState;
use crate::ChipLibrary;

/// The editor's whole undo history for the current chip. Cleared wherever
/// the edited root changes (a new chip means a new controller in the
/// original).
#[derive(Default)]
pub(crate) struct UndoController {
	history: Vec<UndoAction>,
	/// How many entries of `history` are currently applied (`0..=len`);
	/// undoing decrements, redoing increments. Equal to `history.len()`
	/// when nothing is undone.
	index: usize,
}

impl UndoController {
	pub(crate) fn clear(&mut self) {
		self.history.clear();
		self.index = 0;
	}

	/// How many actions have ever been recorded (minus truncated redo
	/// tails) -- test introspection for "did this gesture record anything".
	#[cfg(test)]
	pub(crate) fn history_len(&self) -> usize {
		self.history.len()
	}
}

/// One recorded edit. `NoOp` is never stored by [`record`]; it exists only
/// as the transient slot an action is swapped into while being applied,
/// so the rest of `ViewerState` can be mutated through the same `&mut`.
enum UndoAction {
	NoOp,
	Move(MoveAction),
	ElementExistence(ElementExistenceAction),
	WireExistence(WireExistenceAction),
	/// A wire-geometry edit (wire-edit-mode bend insert/move/delete):
	/// whole-list before/after snapshots, so dependent wires' shifted
	/// attachment indices restore exactly.
	WireListEdit(Box<WireListEditAction>),
}

struct WireListEditAction {
	before: FullWireState,
	after: FullWireState,
}

struct MoveAction {
	entries: Vec<(i32, crate::structs::Vec2, crate::structs::Vec2)>,
}

/// The full wire list of the edited chip at capture time, each entry
/// flagged with whether it must be re-created on restore (the ones whose
/// death the action is about to cause). Restoring rebuilds the list as
/// exactly the un-flagged subset, in order -- which also resurrects any
/// wires that were only lost as side effects (e.g. taps cascading away
/// with their anchor).
pub(crate) struct FullWireState {
	pub(crate) wires: Vec<(WireDescription, bool)>,
}

struct ElementExistenceAction {
	subchips: Vec<SubChipDescription>,
	pins: Vec<(PinDescription, bool)>,
	wire_state: Option<FullWireState>,
	is_delete: bool,
}

struct WireExistenceAction {
	wire: WireDescription,
	wire_index: usize,
	is_delete: bool,
	full_state: Option<FullWireState>,
}

// ---- Recording (called by the editing flows at the same points the
// original's controller gets its Record* calls) ----

fn record(v: &mut ViewerState, action: UndoAction) {
	if matches!(action, UndoAction::NoOp) {
		return;
	}
	let undo = &mut v.undo;
	if undo.index != undo.history.len() {
		undo.history.truncate(undo.index);
	}
	undo.history.push(action);
	undo.index = undo.history.len();
}

/// Records a committed selection move: `entries` are
/// `(id, grab-time position, dropped position)` per carried component.
pub(crate) fn record_move(v: &mut ViewerState, entries: Vec<(i32, crate::structs::Vec2, crate::structs::Vec2)>) {
	if entries.is_empty() {
		return;
	}
	record(v, UndoAction::Move(MoveAction { entries }));
}

/// Records a freshly added wire (already pushed at `wire_index`).
pub(crate) fn record_add_wire(v: &mut ViewerState, wire: WireDescription, wire_index: usize) {
	record(v, UndoAction::WireExistence(WireExistenceAction { wire, wire_index, is_delete: false, full_state: None }));
}

/// Records a wire-geometry edit that has *just finished*: snapshots the
/// (already mutated) wire list as the "after" half, with the "before"
/// half supplied by the caller's earlier capture. Equal snapshots (a
/// click that grabbed a handle but never moved it) record nothing.
pub(crate) fn record_wire_list_edit(v: &mut ViewerState, before: FullWireState) {
	let after = capture_full_wire_state(v, &[], &[]);
	if before.wires.len() == after.wires.len() && before.wires.iter().zip(&after.wires).all(|(a, b)| wire_entries_equal(a, b)) {
		return;
	}
	record(v, UndoAction::WireListEdit(Box::new(WireListEditAction { before, after })));
}

/// Captures a "before" snapshot for a wire-geometry edit. Pair with
/// [`record_wire_list_edit`] after mutating.
pub(crate) fn capture_wire_list(v: &mut ViewerState) -> FullWireState {
	capture_full_wire_state(v, &[], &[])
}

/// Runs `edit` between two automatic snapshots -- the one-shot form for
/// edits that don't need to interleave other work between capture and
/// mutation.
pub(crate) fn record_wire_list_snapshot_pair_before(v: &mut ViewerState, edit: impl FnOnce(&mut ViewerState)) {
	let before = capture_full_wire_state(v, &[], &[]);
	edit(v);
	let after = capture_full_wire_state(v, &[], &[]);
	if before.wires.len() != after.wires.len() || before.wires.iter().zip(&after.wires).any(|(a, b)| !wire_entries_equal(a, b)) {
		record(v, UndoAction::WireListEdit(Box::new(WireListEditAction { before, after })));
	}
}

fn wire_entries_equal(a: &(WireDescription, bool), b: &(WireDescription, bool)) -> bool {
	a.1 == b.1 && a.0.source_pin_address == b.0.source_pin_address && a.0.target_pin_address == b.0.target_pin_address && a.0.points == b.0.points
}

/// Records the deletion of the wire at `wire_index`, snapshotting the
/// whole wire list first so undo can bring back everything that died with
/// it (`RecordDeleteWire`'s full-state backup).
pub(crate) fn delete_wire_with_undo(v: &mut ViewerState, root_chip_name: &str, wire_index: usize) {
	let Some(wire) = v.library.get(root_chip_name).wires.get(wire_index).cloned() else { return };
	let full_state = capture_full_wire_state(v, &[wire_index], &[]);
	record(v, UndoAction::WireExistence(WireExistenceAction { wire, wire_index, is_delete: true, full_state: Some(full_state) }));
	let library = v.library.clone();
	scene::delete_wire(v.library.get_mut(root_chip_name), wire_index, &library);
	v.rebuild_sim();
}

/// Records freshly placed elements (subchips and/or boundary dev-pins,
/// already pushed onto the chip) -- nothing is wired yet, so no wire
/// backup is needed (`RecordAddElements` with `hasWires: false`).
pub(crate) fn record_add_elements(v: &mut ViewerState, subchips: Vec<SubChipDescription>, pins: Vec<(PinDescription, bool)>) {
	if subchips.is_empty() && pins.is_empty() {
		return;
	}
	record(v, UndoAction::ElementExistence(ElementExistenceAction { subchips, pins, wire_state: None, is_delete: false }));
}

/// Deletes every id produced by `ids` from the current root chip,
/// expanding bus-partner closures and recording ONE element-deletion
/// action covering the lot (with a pre-delete wire snapshot) --
/// `RecordDeleteElements`. A single entry point for both the Delete-key
/// batch and single-component context-menu deletes.
pub(crate) fn delete_components_with_undo(v: &mut ViewerState, ids: impl Iterator<Item = i32>) {
	let root_chip_name = v.root_chip_name.clone();
	let mut all_ids: Vec<i32> = Vec::new();
	for id in ids {
		// Stale ids (e.g. left over in a selection something else already
		// deleted) must not produce empty no-op history entries.
		let exists = {
			let chip = v.library.get(&root_chip_name);
			chip.sub_chips.iter().any(|s| s.id == id) || chip.input_pins.iter().any(|p| p.id == id) || chip.output_pins.iter().any(|p| p.id == id)
		};
		if !exists {
			continue;
		}
		for expanded in compute_component_delete_set(v, id) {
			if !all_ids.contains(&expanded) {
				all_ids.push(expanded);
			}
		}
	}
	if all_ids.is_empty() {
		return;
	}

	let (subchips, pins) = capture_elements(v, &all_ids);
	let wire_state = capture_full_wire_state(v, &[], &all_ids);
	record(v, UndoAction::ElementExistence(ElementExistenceAction { subchips, pins, wire_state: Some(wire_state), is_delete: true }));
	canvas::apply_component_deletion(v, &all_ids);
	v.rebuild_sim();
}

/// Clones the placed subchip + boundary dev-pin descriptions for `ids`.
fn capture_elements(v: &ViewerState, ids: &[i32]) -> (Vec<SubChipDescription>, Vec<(PinDescription, bool)>) {
	let chip = v.library.get(&v.root_chip_name);
	let subchips = chip.sub_chips.iter().filter(|s| ids.contains(&s.id)).cloned().collect();
	let pins = chip
		.input_pins
		.iter()
		.filter(|p| ids.contains(&p.id))
		.map(|p| (p.clone(), true))
		.chain(chip.output_pins.iter().filter(|p| ids.contains(&p.id)).map(|p| (p.clone(), false)))
		.collect();
	(subchips, pins)
}

/// Snapshots the current wire list; wires touching an owner in
/// `flagged_owners` or sitting at `flagged_index` are marked recreate-on-
/// restore.
fn capture_full_wire_state(v: &ViewerState, flagged_indices: &[usize], flagged_owners: &[i32]) -> FullWireState {
	let chip = v.library.get(&v.root_chip_name);
	FullWireState {
		wires: chip
			.wires
			.iter()
			.enumerate()
			.map(|(i, w)| {
				let flagged = flagged_indices.contains(&i)
					|| flagged_owners.contains(&w.source_pin_address.pin_owner_id)
					|| flagged_owners.contains(&w.target_pin_address.pin_owner_id);
				(w.clone(), flagged)
			})
			.collect(),
	}
}

// ---- Replay ----

/// Undoes the most recent still-applied action, if any
/// (`UndoController.TryUndo`).
pub(crate) fn try_undo(v: &mut ViewerState) {
	if v.undo.index == 0 {
		return;
	}
	cancel_in_flight(v);
	v.undo.index -= 1;
	let mut action = std::mem::replace(&mut v.undo.history[v.undo.index], UndoAction::NoOp);
	apply_action(v, &mut action, true);
	v.undo.history[v.undo.index] = action;
}

/// Re-applies the most recently undone action, if any
/// (`UndoController.TryRedo`).
pub(crate) fn try_redo(v: &mut ViewerState) {
	if v.undo.index >= v.undo.history.len() {
		return;
	}
	cancel_in_flight(v);
	let mut action = std::mem::replace(&mut v.undo.history[v.undo.index], UndoAction::NoOp);
	apply_action(v, &mut action, false);
	v.undo.history[v.undo.index] = action;
	v.undo.index += 1;
}

/// `Project.ActiveProject.controller.CancelEverything()` before any
/// replay: nothing in flight may reference state an action is about to
/// change.
fn cancel_in_flight(v: &mut ViewerState) {
	v.pending_wire = None;
	v.pending_place.clear();
	crate::viewer::chip_interaction::cancel_all(v);
}

fn apply_action(v: &mut ViewerState, action: &mut UndoAction, undo: bool) {
	match action {
		UndoAction::NoOp => {}
		UndoAction::Move(move_action) => apply_move(v, move_action, undo),
		UndoAction::ElementExistence(element_action) => apply_element_existence(v, element_action, undo),
		UndoAction::WireExistence(wire_action) => apply_wire_existence(v, wire_action, undo),
		UndoAction::WireListEdit(edit_action) => {
			let state = if undo { &edit_action.before } else { &edit_action.after };
			restore_full_wire_state(v, state);
			v.exit_view_mode();
			v.rebuild_sim();
		}
	}
}

fn apply_move(v: &mut ViewerState, action: &MoveAction, undo: bool) {
	let root_chip_name = v.root_chip_name.clone();
	let chip = v.library.get_mut(&root_chip_name);
	let mut present = Vec::new();
	for &(id, original, new) in &action.entries {
		if let Some(sub) = chip.sub_chips.iter_mut().find(|s| s.id == id) {
			sub.position = if undo { original } else { new };
			present.push(id);
		}
	}
	// The moved elements end up selected, like `MoveUndoAction.Trigger`'s
	// `Select(element, additive: true)` calls.
	v.selected_ids = present;
}

fn apply_element_existence(v: &mut ViewerState, action: &ElementExistenceAction, undo: bool) {
	let add_element = if action.is_delete { undo } else { !undo };
	let root_chip_name = v.root_chip_name.clone();

	if add_element {
		let chip = v.library.get_mut(&root_chip_name);
		for sub in &action.subchips {
			if !chip.sub_chips.iter().any(|existing| existing.id == sub.id) {
				chip.sub_chips.push(sub.clone());
			}
		}
		for (pin, is_input) in &action.pins {
			let list = if *is_input { &mut chip.input_pins } else { &mut chip.output_pins };
			if !list.iter().any(|existing| existing.id == pin.id) {
				list.push(pin.clone());
			}
		}
		let restored_ids: Vec<i32> = action.subchips.iter().map(|s| s.id).chain(action.pins.iter().map(|(p, _)| p.id)).collect();
		if let Some(state) = &action.wire_state {
			restore_full_wire_state(v, state);
		}
		v.selected_ids = restored_ids;
	} else {
		// Redoing an add-undo / re-doing a delete: expand through bus
		// partners so paired halves always go together, exactly like a
		// fresh delete would.
		let base_ids: Vec<i32> = action.subchips.iter().map(|s| s.id).chain(action.pins.iter().map(|(p, _)| p.id)).collect();
		let mut all_ids: Vec<i32> = Vec::new();
		for id in base_ids {
			for expanded in compute_component_delete_set(v, id) {
				if !all_ids.contains(&expanded) {
					all_ids.push(expanded);
				}
			}
		}
		if !all_ids.is_empty() {
			canvas::apply_component_deletion(v, &all_ids);
		}
	}

	// Structural change under the viewer: a view stack hanging off the old
	// topology can't be trusted anymore.
	v.exit_view_mode();
	v.rebuild_sim();
}

fn apply_wire_existence(v: &mut ViewerState, action: &WireExistenceAction, undo: bool) {
	let add_wire = if action.is_delete { undo } else { !undo };
	let root_chip_name = v.root_chip_name.clone();

	if add_wire {
		match &action.full_state {
			Some(state) => {
				if wire_state_resolvable(v, state) {
					restore_full_wire_state(v, state);
				}
			}
			None => {
				let chip = v.library.get_mut(&root_chip_name);
				let index = action.wire_index.min(chip.wires.len());
				chip.wires.insert(index, action.wire.clone());
			}
		}
	} else {
		// The replay is strictly linear so `wire_index` should line up;
		// verify defensively and fall back to identity-by-addresses
		// before removing anything.
		let index = {
			let chip = v.library.get(&root_chip_name);
			match chip.wires.get(action.wire_index) {
				Some(existing) if same_endpoints(existing, &action.wire) => Some(action.wire_index),
				_ => chip.wires.iter().position(|existing| same_endpoints(existing, &action.wire)),
			}
		};
		if let Some(index) = index {
			let library = v.library.clone();
			let chip = v.library.get_mut(&root_chip_name);
			scene::delete_wire(chip, index, &library);
		}
	}

	// See `apply_element_existence`: topology changed under the viewer.
	v.exit_view_mode();
	v.rebuild_sim();
}

fn same_endpoints(a: &WireDescription, b: &WireDescription) -> bool {
	a.source_pin_address == b.source_pin_address && a.target_pin_address == b.target_pin_address
}

/// Rebuilds the wire list exactly as it was at capture time
/// (`FullWireState.Restore`): every snapshot entry comes back in order --
/// survivors are replaced in place, and flagged (deleted-by-the-action)
/// ones are re-created. The flags only distinguish those two insertion
/// paths in the original; the resulting list is the full snapshot either
/// way, so restoring is a wholesale clone of it.
fn restore_full_wire_state(v: &mut ViewerState, state: &FullWireState) {
	let root_chip_name = v.root_chip_name.clone();
	let chip = v.library.get_mut(&root_chip_name);
	chip.wires = state.wires.iter().map(|(wire, _)| wire.clone()).collect();
}

/// Whether every snapshotted wire still has both endpoints to attach to --
/// checked BEFORE mutating anything, so a failed restore leaves the chip
/// untouched instead of half-applied.
fn wire_state_resolvable(v: &ViewerState, state: &FullWireState) -> bool {
	let chip = v.library.get(&v.root_chip_name);
	state.wires.iter().all(|(wire, _)| {
		address_resolvable(chip, &v.library, &wire.source_pin_address) && address_resolvable(chip, &v.library, &wire.target_pin_address)
	})
}

/// Whether `address` names a real pin of `chip`: either a pin of the named
/// subchip's definition, or (dev-pin addressing) the boundary dev-pin
/// itself -- the same resolution rules `sim::connect` uses.
fn address_resolvable(chip: &ChipDescription, library: &ChipLibrary, address: &PinAddress) -> bool {
	if let Some(sub) = chip.sub_chips.iter().find(|s| s.id == address.pin_owner_id) {
		let Some(def) = library.try_get(&sub.name) else { return false };
		def.input_pins.iter().chain(def.output_pins.iter()).any(|p| p.id == address.pin_id)
	} else {
		chip.input_pins.iter().chain(chip.output_pins.iter()).any(|p| p.id == address.pin_owner_id)
	}
}

#[cfg(test)]
mod tests {
	//! White-box: the actions replay straight against a live
	//! `ViewerState`'s description graph, which only exists inside the
	//! viewer -- so the history contracts are pinned here with the same
	//! placement helpers the other viewer tests use.

	use super::*;
	use crate::description::{ChipType, PinBitCount};
	use crate::structs::Vec2;
	use crate::{ChipLibrary, PinAddress, WireDescription};

	fn viewer_with_builtins() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	fn place_nand(v: &mut ViewerState, pos: Vec2) -> i32 {
		crate::viewer::chip_interaction::start_placing(v, "NAND");
		canvas::try_place_pending_components(v, pos, &mut None);
		let root = v.root_chip_name.clone();
		v.library.get(&root).sub_chips.last().expect("placed").id
	}

	fn wire_two_outputs(v: &mut ViewerState, a: i32, b: i32) -> usize {
		let chip = v.library.get_mut("ROOT");
		chip.wires.push(WireDescription::new(PinAddress::new(a, 2), PinAddress::new(b, 0)));
		chip.wires.len() - 1
	}

	fn subchip_count(v: &ViewerState) -> usize {
		v.library.get("ROOT").sub_chips.len()
	}

	fn wire_count(v: &ViewerState) -> usize {
		v.library.get("ROOT").wires.len()
	}

	fn position_of(v: &ViewerState, id: i32) -> Vec2 {
		v.library.get("ROOT").sub_chips.iter().find(|s| s.id == id).expect("exists").position
	}

	fn commit_move(v: &mut ViewerState, id: i32, grab: Vec2, drop: Vec2) {
		crate::viewer::chip_interaction::begin_drag_on_component(v, id, grab);
		crate::viewer::chip_interaction::update_move_to_cursor(v, drop);
		crate::viewer::chip_interaction::handle_canvas_release(v, drop);
	}

	#[test]
	fn placing_then_undoing_removes_and_redo_restores() {
		let mut v = viewer_with_builtins();

		let a = place_nand(&mut v, Vec2::ZERO);
		assert_eq!(subchip_count(&v), 1);

		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 0, "undo removed the placed component");

		try_redo(&mut v);
		assert_eq!(subchip_count(&v), 1);
		assert_eq!(v.library.get("ROOT").sub_chips[0].id, a, "redo restores the same instance");
	}

	#[test]
	fn wire_add_delete_round_trips_through_history() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));
		let index = wire_two_outputs(&mut v, a, b);
		assert_eq!(wire_count(&v), 1);

		delete_wire_with_undo(&mut v, "ROOT", index);
		assert_eq!(wire_count(&v), 0, "deleted");

		try_undo(&mut v);
		assert_eq!(wire_count(&v), 1, "undo brings the wire back");
		assert_eq!(v.library.get("ROOT").wires[0].source_pin_address, PinAddress::new(a, 2));

		try_redo(&mut v);
		assert_eq!(wire_count(&v), 0, "redo deletes it again");

		// Now unwind everything, newest first: the delete, then each
		// placement (which are recorded actions too).
		try_undo(&mut v);
		assert_eq!(wire_count(&v), 1, "the wire delete undid away");
		assert_eq!(subchip_count(&v), 2);

		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 1, "the second placement undid away");
		assert_eq!(wire_count(&v), 0, "its wiring went with it");

		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 0, "the first placement undid away");
	}

	#[test]
	fn deleting_a_wired_component_restores_its_wires_on_undo() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));
		wire_two_outputs(&mut v, a, b);

		delete_components_with_undo(&mut v, std::iter::once(a));
		assert_eq!(subchip_count(&v), 1, "A went away");
		assert_eq!(wire_count(&v), 0, "its wiring went with it");

		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 2, "the component came back");
		assert_eq!(wire_count(&v), 1, "so did the wire attached to it");
		assert_eq!(position_of(&v, b), Vec2::new(4.0, 0.0), "untouched components stay put");
	}

	#[test]
	fn committed_moves_undo_and_redo_positions_and_select() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);

		commit_move(&mut v, a, Vec2::ZERO, Vec2::new(6.0, 2.0));
		assert_eq!(position_of(&v, a), Vec2::new(6.0, 2.0));

		try_undo(&mut v);
		assert_eq!(position_of(&v, a), Vec2::ZERO, "undo restores the grabbed position");
		assert_eq!(v.selected_ids, vec![a], "the moved element stays selected, like MoveUndoAction.Trigger");

		try_redo(&mut v);
		assert_eq!(position_of(&v, a), Vec2::new(6.0, 2.0));

		// A reverted (illegal) drop records nothing: there is no move in
		// history for it. The placement of B itself *is* recorded, so the
		// next undo removes B entirely (A untouched), and only the undo
		// after that would rewind A's committed move.
		let b = place_nand(&mut v, Vec2::new(-4.0, 0.0));
		commit_move(&mut v, b, Vec2::new(-4.0, 0.0), Vec2::new(6.0, 2.0)); // overlaps A's current spot -> reverts
		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 1, "B's placement was the last recorded action -- the reverted drag added none");
		assert_eq!(position_of(&v, a), Vec2::new(6.0, 2.0), "A's redo stands");
	}

	#[test]
	fn recording_while_undone_truncates_the_redo_tail() {
		let mut v = viewer_with_builtins();
		place_nand(&mut v, Vec2::ZERO);
		place_nand(&mut v, Vec2::new(4.0, 0.0));

		try_undo(&mut v);
		assert_eq!(subchip_count(&v), 1);

		place_nand(&mut v, Vec2::new(8.0, 0.0));
		assert_eq!(subchip_count(&v), 2, "the fresh placement landed");

		try_redo(&mut v);
		assert_eq!(subchip_count(&v), 2, "nothing left to redo -- the branch was cut off");
	}

	/// Deleting a linked bus pair records BOTH halves, so a single undo
	/// resurrects the complete pair (the partner cascades on delete).
	#[test]
	fn bus_pair_deletes_and_restores_together() {
		let mut v = viewer_with_builtins();
		crate::viewer::chip_interaction::start_placing(&mut v, "BUS-4");
		canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		let origin_id = v.library.get("ROOT").sub_chips[0].id;

		delete_components_with_undo(&mut v, std::iter::once(origin_id));
		assert_eq!(subchip_count(&v), 0, "both halves went together");

		try_undo(&mut v);
		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 2, "origin AND terminus came back");
		assert!(crate::viewer::bus_wiring::bus_pair_linked(chip, &v.library, chip.sub_chips[0].id, chip.sub_chips[1].id), "their link came back too");
	}

	/// Boundary dev-pins participate too: adding one via the IN/OUT
	/// palette flow is undoable.
	#[test]
	fn dev_pin_placement_is_undoable() {
		let mut v = viewer_with_builtins();
		crate::viewer::chip_interaction::start_placing(&mut v, "IN-8");
		canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		assert_eq!(v.library.get("ROOT").input_pins.len(), 1);

		try_undo(&mut v);
		assert_eq!(v.library.get("ROOT").input_pins.len(), 0, "the dev-pin undid away");

		try_redo(&mut v);
		assert_eq!(v.library.get("ROOT").input_pins.len(), 1);
		let pin = &v.library.get("ROOT").input_pins[0];
		assert_eq!(pin.bit_count, PinBitCount::Bit8, "the restored pin keeps its description");
	}

	/// Undo/redo with empty history are silent no-ops.
	#[test]
	fn empty_history_is_a_no_op() {
		let mut v = viewer_with_builtins();
		try_undo(&mut v);
		try_redo(&mut v);
		assert_eq!(subchip_count(&v), 0);
	}
}
// ---- Hardening: edge cases ----

#[cfg(test)]
mod edge_case_tests {
	//! Extra pins around the tricky corners: cascade restorations,
	//! identity-based redo, batch composition, and interactions with
	//! chip switching / view mode. Same white-box justification as the
	//! core history tests above.

	use super::*;
	use crate::description::ChipType;
	use crate::structs::Vec2;
	use crate::{ChipLibrary, PinAddress, WireDescription};

	fn viewer_with_builtins() -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		ViewerState::new("", library, "ROOT".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	fn place_nand(v: &mut ViewerState, pos: Vec2) -> i32 {
		crate::viewer::chip_interaction::start_placing(v, "NAND");
		canvas::try_place_pending_components(v, pos, &mut None);
		let root = v.root_chip_name.clone();
		v.library.get(&root).sub_chips.last().expect("placed").id
	}

	fn wire_count(v: &ViewerState) -> usize {
		v.library.get("ROOT").wires.len()
	}

	fn endpoints(v: &ViewerState, index: usize) -> (PinAddress, PinAddress) {
		let w = &v.library.get("ROOT").wires[index];
		(w.source_pin_address, w.target_pin_address)
	}

	/// Deleting an anchor wire detaches wires tapping onto it
	/// (`RemoveConnectionDependency`) rather than killing them; the
	/// recorded full-state restore must bring the *anchor* back exactly,
	/// and redo must detach again (`FullWireState` snapshots the whole
	/// list, so both sides of that swap are exact).
	#[test]
	fn tapped_wires_survive_their_anchor_and_undo_restores_both() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));
		let c = place_nand(&mut v, Vec2::new(8.0, 0.0));

		let chip = v.library.get_mut("ROOT");
		chip.wires.push(WireDescription::new(PinAddress::new(a, 2), PinAddress::new(b, 1)));
		chip.wires.push(WireDescription::new_tapped_target(PinAddress::new(a, 2), PinAddress::new(c, 0), 0, 0, Vec2::ZERO));
		assert_eq!(wire_count(&v), 2);

		delete_wire_with_undo(&mut v, "ROOT", 0);
		assert_eq!(wire_count(&v), 1, "the tap is detached, not killed");
		assert_eq!(endpoints(&v, 0).1, PinAddress::new(c, 0), "the survivor keeps its own electrical target");

		try_undo(&mut v);
		assert_eq!(wire_count(&v), 2, "anchor AND tap came back");
		assert_eq!(endpoints(&v, 1).0, PinAddress::new(a, 2), "the tap kept its source address");

		try_redo(&mut v);
		assert_eq!(wire_count(&v), 1, "redo re-detaches");
	}

	/// Redoing a wire delete must remove the SAME wire even if the stored
	/// index would now point elsewhere -- verified by endpoints, not
	/// position alone.
	#[test]
	fn wire_delete_redo_targets_the_right_wire_by_identity() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));
		let c = place_nand(&mut v, Vec2::new(8.0, 0.0));

		{
			let chip = v.library.get_mut("ROOT");
			chip.wires.push(WireDescription::new(PinAddress::new(a, 2), PinAddress::new(b, 0)));
			chip.wires.push(WireDescription::new(PinAddress::new(b, 2), PinAddress::new(c, 0)));
		}
		delete_wire_with_undo(&mut v, "ROOT", 0); // A -> B
		assert_eq!(endpoints(&v, 0).0, PinAddress::new(b, 2), "the surviving wire shifted into slot 0");

		try_undo(&mut v);
		try_redo(&mut v);
		assert_eq!(wire_count(&v), 1);
		assert_eq!(endpoints(&v, 0).0, PinAddress::new(b, 2), "redo removed A->B again, never B->C");

		try_undo(&mut v);
		assert_eq!(endpoints(&v, 0).0, PinAddress::new(a, 2), "undo brings back exactly what that action deleted");
	}

	#[test]
	fn multi_selection_move_restores_every_carried_position() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let b = place_nand(&mut v, Vec2::new(4.0, 0.0));

		v.sim.set_key_modifiers(crate::sim::key_mods_bits::SHIFT);
		crate::viewer::chip_interaction::begin_drag_on_component(&mut v, a, Vec2::ZERO);
		crate::viewer::chip_interaction::begin_drag_on_component(&mut v, b, Vec2::new(4.0, 0.0));
		v.sim.set_key_modifiers(0);
		for id in [a, b] {
			if let Some(sub) = v.library.get_mut("ROOT").sub_chips.iter_mut().find(|s| s.id == id) {
				sub.position += Vec2::new(10.0, 5.0);
			}
		}
		crate::viewer::chip_interaction::handle_canvas_release(&mut v, Vec2::new(10.0, 5.0));

		try_undo(&mut v);
		let pos = |v: &ViewerState, id: i32| v.library.get("ROOT").sub_chips.iter().find(|s| s.id == id).expect("exists").position;
		assert_eq!(pos(&v, a), Vec2::ZERO, "every carried component rewinds together");
		assert_eq!(pos(&v, b), Vec2::new(4.0, 0.0));
		assert_eq!(v.selected_ids, vec![a, b], "the whole moved group stays selected");
	}

	#[test]
	fn mixed_dev_pin_and_subchip_batch_delete_round_trips_as_one_action() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		crate::viewer::chip_interaction::start_placing(&mut v, "IN-4");
		canvas::try_place_pending_components(&mut v, Vec2::new(-6.0, 0.0), &mut None);

		let pin_id = v.library.get("ROOT").input_pins[0].id;
		delete_components_with_undo(&mut v, [a, pin_id].into_iter());

		let chip = v.library.get("ROOT");
		assert!(chip.sub_chips.is_empty() && chip.input_pins.is_empty(), "the whole batch went in one go");

		try_undo(&mut v);
		let chip = v.library.get("ROOT");
		assert_eq!(chip.sub_chips.len(), 1, "subchip and dev-pin come back together");
		assert_eq!(chip.input_pins.len(), 1);
		assert_eq!(chip.input_pins[0].bit_count, crate::description::PinBitCount::Bit4, "the pin keeps its description");
	}

	/// A configured Pulse keeps its custom duration across a
	/// delete/undo/redo cycle -- descriptions clone wholesale.
	#[test]
	fn configured_pulse_keeps_its_duration_through_a_cycle() {
		let mut v = viewer_with_builtins();
		crate::viewer::chip_interaction::start_placing(&mut v, "PULSE");
		canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		v.library.get_mut("ROOT").sub_chips[0].internal_data = Some(vec![77, 0, 0]);

		let id = v.library.get("ROOT").sub_chips[0].id;
		delete_components_with_undo(&mut v, std::iter::once(id));
		try_undo(&mut v);

		let sub = &v.library.get("ROOT").sub_chips[0];
		assert_eq!(sub.id, id, "same instance id restored");
		assert_eq!(sub.internal_data, Some(vec![77, 0, 0]), "its configuration came back with it");
	}

	/// Switching the edited root clears history (`SetNewActiveDevChip`
	/// builds a fresh controller): undo after a switch must be a no-op on
	/// the NEW chip, not replay actions belonging to the old one.
	#[test]
	fn switching_chips_clears_history_and_undo_is_a_no_op_afterwards() {
		let root = crate::save_system::test_util::temp_dir("undo_chip_switch");
		let paths = crate::SavePaths::new(&root);
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("FIRST", ChipType::Custom));
		library.add(ChipDescription::new("SECOND", ChipType::Custom));
		for name in ["FIRST", "SECOND"] {
			crate::Saver::save_chip(&paths, "P", &library, &library.get(name).clone()).expect("saved");
		}
		let mut v = ViewerState::new("P", library, "FIRST".to_string(), Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());

		place_nand(&mut v, Vec2::ZERO);
		assert_eq!(v.undo.history_len(), 1);

		crate::viewer::save_flow::open_chip_by_name(&mut v, &paths, &mut None, "SECOND");
		assert_eq!(v.undo.history_len(), 0, "the switch started a fresh history");

		try_undo(&mut v);
		assert!(v.library.get("SECOND").sub_chips.is_empty(), "undo does not reach across chips");

		crate::viewer::save_flow::open_chip_by_name(&mut v, &paths, &mut None, "FIRST");
		assert_eq!(
			v.library.get("FIRST").sub_chips.len(),
			0,
			"reopening shows the on-disk chip -- the unsaved NAND was discarded by the switch, as ever"
		);

		let _ = std::fs::remove_dir_all(&root);
	}

	/// Replaying a structural action while a view-only chip is on screen
	/// drops the view instead of leaving it pointing through changed
	/// topology.
	#[test]
	fn structural_undo_drops_any_open_view() {
		let mut v = viewer_with_builtins();
		let mut library_entry = ChipDescription::new("SUB", ChipType::Custom);
		v.library.add(std::mem::take(&mut library_entry));
		crate::viewer::chip_interaction::start_placing(&mut v, "SUB");
		canvas::try_place_pending_components(&mut v, Vec2::ZERO, &mut None);
		let id = v.library.get("ROOT").sub_chips[0].id;

		v.enter_view_mode(id);
		assert!(!v.can_edit_viewed_chip());

		try_undo(&mut v);
		assert!(v.can_edit_viewed_chip(), "the view was dropped with the topology it pointed at");
		assert!(v.library.get("ROOT").sub_chips.is_empty());
	}

	/// Grabbing and releasing without moving (a plain click-select)
	/// records nothing -- history stays clean for real edits.
	#[test]
	fn zero_delta_click_select_records_nothing() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let history_after_place = v.undo.history_len();

		crate::viewer::chip_interaction::begin_drag_on_component(&mut v, a, Vec2::ZERO);
		crate::viewer::chip_interaction::handle_canvas_release(&mut v, Vec2::ZERO);

		assert_eq!(v.undo.history_len(), history_after_place, "a click is not an edit");
	}

	/// Deleting an id that no longer exists on the chip (a stale
	/// selection) records nothing and changes nothing.
	#[test]
	fn stale_delete_ids_are_silent_no_ops() {
		let mut v = viewer_with_builtins();
		let a = place_nand(&mut v, Vec2::ZERO);
		let history_before = v.undo.history_len();

		delete_components_with_undo(&mut v, [a + 500].into_iter());
		assert_eq!(v.undo.history_len(), history_before, "no history entry for a missing id");
		assert!(v.library.get("ROOT").sub_chips.iter().any(|s| s.id == a), "the real component is untouched");
	}
}
