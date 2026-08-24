//! Chip save/open flows: the Ctrl+S save popup's modes (save / save-as /
//! rename / replace), new-chip creation, and switching the viewer to a
//! different chip -- everything that moves whole `ChipDescription`s
/// between the in-memory library and the project's on-disk chip files.
use crate::render::editor_ui::SaveChipMode;
use crate::viewer::library::{is_custom_chip, reset_all_driven_inputs, DEFAULT_LIBRARY_COLLECTION_NAME};
use crate::viewer::state::{close_top_overlay, ViewerState};
use crate::{ChipDescription, ChipLibrary, ChipType, SavePaths, Saver};

/// Determines which buttons `Overlay::SaveChip` should show for the
/// currently-typed name, by comparing it against `v.root_chip_name` (the
/// chip's current identity) and the rest of `v.library` -- see
/// `editor_ui::SaveChipMode`'s docs for what each variant means and
/// `build_save_chip_popup`'s docs for why this is re-derived identically
/// on both the render side and the click-handling side. Case-insensitive,
/// matching `ChipLibrary`'s own lookup rules.
pub(crate) fn save_chip_mode(v: &ViewerState, typed: &str) -> SaveChipMode {
	let typed = typed.trim();
	if typed.eq_ignore_ascii_case(&v.root_chip_name) {
		SaveChipMode::Save
	} else if v.library.try_get(typed).is_some() {
		SaveChipMode::Replace
	} else {
		SaveChipMode::SaveAsOrRename
	}
}

/// Adds `add_name` to the project's `all_custom_chip_names`/`chip_collections`
/// bookkeeping if it isn't already there (and removes `remove_name` from
/// both, if given), then persists the updated `ProjectDescription`.
/// Mirrors what the sidebar/search actually list -- without this, a
/// freshly Saved-As/Renamed chip would only be reachable if you already
/// remembered its exact name to type into search.
fn register_chip_name_in_project(v: &mut ViewerState, paths: &SavePaths, remove_name: Option<&str>, add_name: &str) {
	if let Some(old) = remove_name {
		v.prefs.all_custom_chip_names.retain(|n| n != old);
		for c in v.prefs.chip_collections.iter_mut() {
			c.chips.retain(|n| n != old);
		}
	}
	if !v.prefs.all_custom_chip_names.iter().any(|n| n == add_name) {
		v.prefs.all_custom_chip_names.push(add_name.to_string());
	}
	if !v.prefs.chip_collections.iter().any(|c| c.chips.iter().any(|n| n == add_name)) {
		if !v.prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
			v.prefs.chip_collections.push(crate::json::ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
		}
		let other =
			v.prefs.chip_collections.iter_mut().find(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)).expect("just ensured above");
		other.chips.push(add_name.to_string());
	}

	let mut desc = v.prefs.clone();
	match Saver::save_project_description(paths, &mut desc) {
		Ok(()) => v.prefs = desc,
		Err(e) => eprintln!("warning: failed to update project description: {e}"),
	}
}

/// Plain overwrite/create (`SaveChipMode::Save`): writes the current
/// in-memory chip back to its own file under its own (unchanged) name.
/// No other chip or file is touched.
fn save_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let name = v.root_chip_name.clone();
	let desc = v.library.get(&name).clone();
	match Saver::save_chip(paths, &v.project_name, &desc) {
		Ok(()) => {
			v.mark_saved(&name);
			*status = Some(format!("Saved '{name}'"));
		}
		Err(e) => *status = Some(format!("Failed to save '{name}': {e}")),
	}
}

/// Saves a *copy* of the currently-open chip under `new_name`
/// (`SaveChipMode::SaveAsOrRename`, "Save As" button), leaving its
/// existing on-disk file (under its current name, if it has one)
/// completely untouched. Since that current identity's `v.library` entry
/// has been edited in place all session, once we fork away from it its
/// in-memory copy no longer matches what's actually on disk under that
/// name -- so it's reloaded fresh from its own file right after (see
/// `load_single_chip_from_disk`), discarding whatever of this session's
/// edits hadn't already been saved under *that* identity. The viewer
/// then switches over to the new name.
fn save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();

	match Saver::save_chip(paths, &v.project_name, &new_desc) {
		Ok(()) => {
			v.library.add(new_desc);
			v.mark_saved(new_name);
			register_chip_name_in_project(v, paths, None, new_name);

			if !old_name.eq_ignore_ascii_case(new_name) {
				match load_single_chip_from_disk(paths, &v.project_name, &old_name) {
					Ok(pristine) => {
						v.library.add(pristine);
					}
					Err(_) => {
						// No on-disk file for the old identity (it was never actually saved under that
						// name to begin with) -- nothing to revert to, so leave the in-memory draft as is.
					}
				}
			}

			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Saved as '{new_name}'"));
			v.rebuild_sim();
		}
		Err(e) => *status = Some(format!("Failed to save '{new_name}': {e}")),
	}
}

/// Backs up (moves to the project's "Deleted Chips" folder -- see
/// `Saver::delete_chip`'s `backup_in_deleted_folder`) whatever chip is
/// currently saved under `new_name`, then does exactly what
/// `save_chip_as` does. The chip's own existing file, if any under its
/// *current* name, is left untouched either way -- only the chip being
/// overwritten at the destination name is backed up.
fn replace_chip_with_current(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	if let Err(e) = Saver::delete_chip(paths, &v.project_name, new_name, true) {
		*status = Some(format!("Failed to back up existing '{new_name}': {e}"));
		return;
	}
	v.library.remove(new_name);
	save_chip_as(v, paths, status, new_name);
}

/// Actually renames the chip (`SaveChipMode::SaveAsOrRename`, "Rename"
/// button): moves its on-disk file to `new_name` -- no copy left under
/// the old name, the old file is deleted outright (no backup, since this
/// is a rename rather than a delete) -- and updates the project's
/// chip-name bookkeeping to match.
fn rename_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();

	match Saver::save_chip(paths, &v.project_name, &new_desc) {
		Ok(()) => {
			if let Err(e) = Saver::delete_chip(paths, &v.project_name, &old_name, false) {
				eprintln!("warning: renamed '{old_name}' to '{new_name}' but failed to remove the old file: {e}");
			}
			v.library.remove(&old_name);
			v.library.add(new_desc);
			v.mark_saved(new_name);
			v.mark_saved(&old_name);
			register_chip_name_in_project(v, paths, Some(&old_name), new_name);
			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Renamed '{old_name}' to '{new_name}'"));
			v.rebuild_sim();
		}
		Err(e) => *status = Some(format!("Failed to rename to '{new_name}': {e}")),
	}
}

/// Applies the `Overlay::SaveChip` popup's Confirm action -- shared by
/// its "Save"/"Replace" button (`EditorAction::SaveChipConfirm`) and
/// pressing Enter directly for those same two (unambiguous) modes; see
/// the key-handler's own guard for why `SaveAsOrRename` never reaches
/// here via Enter (that mode's own two buttons call
/// `confirm_save_chip_as`/`confirm_save_chip_rename` directly instead,
/// since which of "keep both" or "actually rename" is meant can't be
/// inferred, only chosen).
pub(crate) fn confirm_save_chip_popup(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	match save_chip_mode(v, &typed) {
		SaveChipMode::Save => save_current_chip(v, paths, status),
		SaveChipMode::Replace => replace_chip_with_current(v, paths, status, &typed),
		SaveChipMode::SaveAsOrRename => return,
	}
	close_top_overlay(v);
}

pub(crate) fn confirm_save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	save_chip_as(v, paths, status, &typed);
	close_top_overlay(v);
}

pub(crate) fn confirm_save_chip_rename(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if typed.is_empty() {
		return;
	}
	rename_current_chip(v, paths, status, &typed);
	close_top_overlay(v);
}

/// Re-reads a single chip's own save file from disk, without touching
/// anything else in `v.library` -- used to revert one specific chip's
/// in-memory entry back to "whatever's actually saved" (e.g. the chip
/// left behind, untouched, by a Save-As/Replace under a new name; see
/// `save_chip_as`), as opposed to blindly reloading the whole project.
fn load_single_chip_from_disk(paths: &SavePaths, project_name: &str, chip_name: &str) -> std::io::Result<ChipDescription> {
	let path = paths.chips_path(project_name).join(format!("{chip_name}.json"));
	let json = std::fs::read_to_string(path)?;
	crate::json::parse_chip_description(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Drops every canvas interaction draft that references the *previous*
/// root chip's contents (pending wire endpoints and carry entries key off
/// subchip ids/positions; a selection or in-flight drag would silently
/// refer to whatever now sits at those ids in the new chip) -- the same
/// trigger set `pending_wire`'s docs describe, shared by both chip-switch
/// flows above.
fn reset_canvas_interaction(v: &mut ViewerState) {
	v.pending_wire = None;
	v.pending_place.clear();
	crate::viewer::chip_interaction::cancel_all(v);
}

/// Discards whatever unsaved edits this session made to whichever chip
/// is currently open (`v.root_chip_name`), by reloading its pristine
/// on-disk copy back over its `v.library` entry (same "reload from disk"
/// move `save_chip_as` already does for the identity it forks away
/// from -- see `load_single_chip_from_disk`). Called by `open_chip_by_name`
/// right before it actually switches away to a different chip. If the
/// chip has no file on disk yet (a brand new, never-saved chip), there's
/// nothing to revert to, so its in-memory draft is left exactly as it
/// was -- it simply isn't reachable again once you navigate away, same
/// as it already wasn't reachable after an app restart.
fn discard_unsaved_changes(v: &mut ViewerState, paths: &SavePaths) {
	let leaving = v.root_chip_name.clone();
	if !is_custom_chip(&v.library, &leaving) {
		return;
	}
	if let Ok(pristine) = load_single_chip_from_disk(paths, &v.project_name, &leaving) {
		v.library.add(pristine);
	}
}

/// Picks a fresh, not-yet-used (case-insensitively) name for a
/// brand-new chip, starting from "New Chip" and falling back to
/// "New Chip 2", "New Chip 3", ... the first suffix that isn't already
/// taken in `library` -- so hitting Ctrl+N repeatedly never collides
/// with an earlier still-unsaved draft (or a saved chip that happens to
/// already be named "New Chip").
pub(crate) fn unique_new_chip_name(library: &ChipLibrary) -> String {
	if library.try_get("New Chip").is_none() {
		return "New Chip".to_string();
	}
	let mut n = 2;
	loop {
		let candidate = format!("New Chip {n}");
		if library.try_get(&candidate).is_none() {
			return candidate;
		}
		n += 1;
	}
}

/// Ctrl+N: starts a brand-new, blank custom chip (no pins, no subchips,
/// no wires -- see `ChipDescription::new`) and switches the viewer over
/// to it, exactly as if it were an existing chip being opened. First
/// discards any unsaved edits on whichever chip is currently open, the
/// same as any other switch (see `discard_unsaved_changes`), so Ctrl+N
/// can't be used to accidentally lose track of that.
///
/// Nothing is persisted: the new chip lives only in `v.library`, marked
/// as an unsaved draft (`ViewerState::unsaved_drafts`) -- so it isn't
/// added to the project's `all_custom_chip_names`/library sidebar (that's
/// `register_chip_name_in_project`'s job, run from the save flow) and
/// `sync_library_collections` skips it too, meaning no prefs write can
/// ever sneak its name into the sidebar or onto disk. Only an actual
/// Ctrl+S save promotes the draft to a real, listed chip.
pub(crate) fn start_new_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	discard_unsaved_changes(v, paths);

	let name = unique_new_chip_name(&v.library);
	v.library.add(ChipDescription::new(&name, ChipType::Custom));
	v.mark_unsaved_draft(&name);

	v.root_chip_name = name.clone();
	reset_all_driven_inputs(&mut v.library);
	v.rebuild_sim();
	v.camera_fitted = false;
	reset_canvas_interaction(v);
	*status = Some(format!("New chip '{name}'"));
}

/// Actually switches the viewer over to `name`'s own definition -- i.e.
/// "open this chip" -- if it's a custom chip in `v.library`. This used to
/// be exactly what left-clicking a chip in the library sidebar did (via
/// `EditorAction::SelectChip`); it's now reached only through that row's
/// right-click "Open" popup, the search popup's `UseChip`, and
/// `viewer::actions`' own `EditorAction::UseChip`, so a left click alone
/// no longer jumps the viewer away from whatever chip is currently open.
/// Builtins are refused (see `is_custom_chip`) -- their "Open" row is
/// greyed out in the popup, so reaching this arm for one at all would
/// mean the disabled-row guard in `context_menu::build_context_menu` was
/// bypassed somehow.
///
/// On an actual switch (`name` differs from the chip currently open),
/// first discards any unsaved edits to the chip being left via
/// `discard_unsaved_changes` -- so `v.library`'s in-memory copy of it
/// reverts to whatever's actually on disk, and navigating back to it
/// later shows that saved state rather than the draft you were mid-edit
/// on. Persisting those edits instead is `Ctrl+S`'s job (see
/// `confirm_save_chip_popup`) and must happen *before* switching away.
/// Also only re-fits the camera on an actual switch, never on an
/// in-place edit of the chip already on screen (that's `rebuild_sim`'s
/// job to *not* do -- see its own doc comment).
pub(crate) fn open_chip_by_name(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	let switching = name != v.root_chip_name;

	if is_custom_chip(&v.library, name) {
		if switching {
			discard_unsaved_changes(v, paths);
		}
		v.root_chip_name = name.to_string();
		reset_all_driven_inputs(&mut v.library);
		v.rebuild_sim();
		if switching {
			v.camera_fitted = false;
			reset_canvas_interaction(v);
		}
	} else if v.library.try_get(name).is_some() {
		*status = Some(format!("Chip '{}' is a builtin component", name));
	} else {
		*status = Some(format!("Chip '{}' not found in library", name));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::viewer::actions::open_library_panel;
	use crate::viewer::state::open_save_chip;

	#[test]
	fn unique_new_chip_name_never_collides_with_existing_drafts() {
		let mut library = ChipLibrary::new();
		assert_eq!(unique_new_chip_name(&library), "New Chip");

		library.add(ChipDescription::new("New Chip", ChipType::Custom));
		assert_eq!(unique_new_chip_name(&library), "New Chip 2");

		library.add(ChipDescription::new("new chip 2", ChipType::Custom));
		assert_eq!(unique_new_chip_name(&library), "New Chip 3");
	}

	/// Switching chips must drop every canvas draft that references the
	/// *previous* chip's ids -- pendings, selection, and any drag in flight.
	#[test]
	fn switching_chips_clears_pendings_selection_and_drag_state() {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		library.add(ChipDescription::new("OTHER", ChipType::Custom));

		let mut v = ViewerState::new("", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		let root = v.root_chip_name.clone();

		// Fill every kind of canvas draft state on ROOT.
		crate::viewer::chip_interaction::start_placing(&mut v, "NAND");
		try_place_pending_components_via_public_path(&mut v);
		let id = v.library.get(&root).sub_chips[0].id;
		v.selected_ids.push(id);
		crate::viewer::chip_interaction::begin_drag_on_component(&mut v, id, crate::structs::Vec2::ZERO);
		v.pending_wire = None;
		assert!(has_draft_state(&v), "precondition: drafts exist");

		reset_canvas_interaction(&mut v);

		assert!(v.pending_place.is_empty());
		assert!(v.pending_wire.is_none());
		assert!(v.selected_ids.is_empty());
		assert_eq!(v.canvas_interaction, crate::viewer::chip_interaction::CanvasInteraction::None);
	}

	fn try_place_pending_components_via_public_path(v: &mut ViewerState) {
		crate::viewer::canvas::try_place_pending_components(v, crate::structs::Vec2::ZERO, &mut None);
	}

	fn has_draft_state(v: &ViewerState) -> bool {
		!v.pending_place.is_empty()
			|| v.pending_wire.is_some()
			|| !v.selected_ids.is_empty()
			|| !matches!(v.canvas_interaction, crate::viewer::chip_interaction::CanvasInteraction::None)
	}

	/// The Ctrl+N contract end-to-end: a brand-new chip lives only in
	/// memory until an explicit Ctrl+S -- opening the library panel (whose
	/// sync files strays into collections) and any subsequent prefs write
	/// must keep it out of both the sidebar state and the on-disk project
	/// description. Only the real save flow promotes it.
	#[test]
	fn new_chip_stays_out_of_library_and_disk_until_saved() {
		let root = crate::save_system::test_util::temp_dir("new_chip_unsaved");
		let paths = SavePaths::new(&root);
		let project = crate::create_project(&paths, "P").expect("project created");

		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		let mut v =
			ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		v.prefs = project.description;

		let mut status = None;
		start_new_chip(&mut v, &paths, &mut status);
		let name = v.root_chip_name.clone();
		assert_eq!(name, "New Chip");
		assert!(v.unsaved_drafts.contains("new chip"), "the fresh chip starts life as an unsaved draft");

		// Opening the library panel must not file the draft into any collection...
		open_library_panel(&mut v);
		assert!(
			v.prefs.chip_collections.iter().flat_map(|c| c.chips.iter()).all(|n| !n.eq_ignore_ascii_case(&name)),
			"draft leaked into the sidebar collections"
		);
		// ...and a prefs write happening afterwards must not put it on disk either.
		let mut desc = v.prefs.clone();
		Saver::save_project_description(&paths, &mut desc).expect("description saved");
		let text = std::fs::read_to_string(paths.project_description_path("P")).expect("description readable");
		assert!(!text.to_ascii_lowercase().contains("new chip"), "draft leaked into the saved description: {text}");

		// An actual Ctrl+S (same-name confirm) promotes the draft: file written, marker cleared...
		open_save_chip(&mut v);
		confirm_save_chip_popup(&mut v, &paths, &mut status);
		assert!(paths.chips_path("P").join("New Chip.json").is_file(), "chip file written by the save");
		assert!(!v.unsaved_drafts.contains("new chip"), "saving clears the draft marker");

		// ...and from then on the library panel does list it.
		open_library_panel(&mut v);
		let other = v.prefs.chip_collections.iter().find(|c| c.name == "OTHER").expect("OTHER collection exists");
		assert!(other.chips.iter().any(|n| n.eq_ignore_ascii_case(&name)), "saved chip joins the library");

		let _ = std::fs::remove_dir_all(&root);
	}
}
