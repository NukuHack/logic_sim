//! Editor-action application: the single funnel every editor overlay
//! button's `EditorAction` goes through, mutating live viewer/project
//! state (and persisting prefs via the save system where required).

use crate::json::ChipCollection;
use crate::render::editor_ui::{EditorAction, LibrarySelection};
use crate::viewer::chip_interaction;
use crate::viewer::customize as customize_flow;
use crate::viewer::library::{
	chip_delete_confirm_message, delete_chip_from_library, delete_collection, move_selected_library_row, reset_library_popup_state,
	sync_library_collections,
};
use crate::viewer::popups::{apply_prefs_field_text, apply_rom_editor, confirm_key_select_popup, confirm_naming_popup, confirm_rom_cell, cycle_pref};
use crate::viewer::save_flow::{confirm_save_chip_as, confirm_save_chip_popup, confirm_save_chip_rename, open_chip_by_name};
use crate::viewer::state::{close_all_overlays, close_top_overlay, open_overlay, reset_preferences_draft, Overlay, ViewerState};
use crate::{SavePaths, Saver};

/// Applies a click on one of the editor overlays.
pub(crate) fn apply_editor_action(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, action: EditorAction) {
	match action {
		EditorAction::ClosePopup => close_top_overlay(v),
		EditorAction::CyclePref(i) => cycle_pref(&mut v.prefs, i),
		EditorAction::SelectPrefsField(field) => v.prefs_field_focus = Some(field),
		EditorAction::ApplyPreferences => {
			apply_prefs_field_text(v);
			v.sync_sim_clock_pref();
			let mut desc = v.prefs.clone();
			match Saver::save_project_description(paths, &mut desc) {
				Ok(()) => v.prefs = desc,
				Err(e) => *status = Some(format!("Failed to save preferences: {e}")),
			}
			v.overlays.retain(|o| *o != Overlay::Preferences);
			reset_preferences_draft(v);
		}
		EditorAction::SelectCollection(i) => {
			v.library_selection = LibrarySelection::Collection(i);
			if let Some(c) = v.prefs.chip_collections.get_mut(i) {
				c.is_toggled_open = !c.is_toggled_open;
			}
		}
		EditorAction::SelectChipRow { collection, chip } => {
			v.library_selection = LibrarySelection::Chip(collection, chip);
		}
		EditorAction::SelectStarredRow(i) => {
			v.library_selection = LibrarySelection::Starred(i);
		}
		EditorAction::ToggleStarred { name, is_collection } => {
			let now_starred = !v.prefs.is_starred(&name, is_collection);
			v.prefs.set_starred(&name, now_starred, is_collection);
		}
		EditorAction::MoveSelectedStep(down) => move_selected_library_row(v, down, false),
		EditorAction::MoveSelectedJump(down) => move_selected_library_row(v, down, true),
		EditorAction::OpenSelectedChip(name) => {
			open_chip_by_name(v, paths, status, &name);
			close_all_overlays(v);
			v.library_selection = LibrarySelection::None;
			v.bottom_bar_open_collection = None;
		}
		EditorAction::RequestDeleteChip(name) => {
			v.library_delete_message = chip_delete_confirm_message(v, &name);
			v.library_confirming_chip_delete = true;
		}
		EditorAction::BeginNewCollection => {
			v.library_creating_collection = true;
			v.library_renaming_collection = false;
			v.overlay_text_input.clear();
		}
		EditorAction::BeginRenameCollection => {
			if let LibrarySelection::Collection(i) = v.library_selection {
				if let Some(c) = v.prefs.chip_collections.get(i) {
					v.overlay_text_input = c.name.clone();
					v.library_renaming_collection = true;
					v.library_creating_collection = false;
				}
			}
		}
		EditorAction::RequestDeleteCollection => {
			if let LibrarySelection::Collection(i) = v.library_selection {
				if v.prefs.chip_collections.get(i).is_some_and(|c| c.chips.is_empty()) {
					delete_collection(&mut v.prefs, i);
					v.library_selection = LibrarySelection::None;
				} else {
					v.library_delete_message = "Are you sure you want to delete this collection? Its chips will be moved to \"OTHER\".".to_string();
					v.library_confirming_collection_delete = true;
				}
			}
		}
		EditorAction::ConfirmCollectionName => {
			let new_name = v.overlay_text_input.trim().to_string();
			if !new_name.is_empty() {
				if v.library_creating_collection {
					v.prefs.chip_collections.push(ChipCollection::new(&new_name, Vec::<String>::new()));
					v.library_selection = LibrarySelection::Collection(v.prefs.chip_collections.len() - 1);
				} else if v.library_renaming_collection {
					if let LibrarySelection::Collection(i) = v.library_selection {
						if let Some(c) = v.prefs.chip_collections.get_mut(i) {
							let old_name = c.name.clone();
							c.name = new_name.clone();
							for item in &mut v.prefs.starred_list {
								if item.is_collection && item.name.eq_ignore_ascii_case(&old_name) {
									item.name = new_name.clone();
								}
							}
						}
					}
				}
			}
			reset_library_popup_state(v);
		}
		EditorAction::CancelLibraryPopup => reset_library_popup_state(v),
		EditorAction::ConfirmDelete => {
			if v.library_confirming_chip_delete {
				let name = match v.library_selection {
					LibrarySelection::Chip(ci, chi) => v.prefs.chip_collections.get(ci).and_then(|c| c.chips.get(chi)).cloned(),
					LibrarySelection::Starred(i) => v.prefs.starred_list.get(i).filter(|it| !it.is_collection).map(|it| it.name.clone()),
					_ => None,
				};
				if let Some(name) = name {
					delete_chip_from_library(v, paths, status, &name);
				}
			} else if v.library_confirming_collection_delete {
				if let LibrarySelection::Collection(i) = v.library_selection {
					delete_collection(&mut v.prefs, i);
				}
				v.library_selection = LibrarySelection::None;
			}
			reset_library_popup_state(v);
		}
		EditorAction::PlaceChip(name) => {
			let mut desc = v.prefs.clone();
			if let Err(e) = Saver::save_project_description(paths, &mut desc) {
				*status = Some(format!("Failed to save chip library: {e}"));
			} else {
				v.prefs = desc;
			}
			close_all_overlays(v);
			v.library_selection = LibrarySelection::None;
			v.pending_wire = None;
			// Fills the carry (a bus origin brings its linked terminus
			// partner along) and cancels any selection drag in flight --
			// see `chip_interaction::start_placing`.
			chip_interaction::start_placing(v, &name);
		}
		EditorAction::ExitLibrary => {
			let mut desc = v.prefs.clone();
			if let Err(e) = Saver::save_project_description(paths, &mut desc) {
				*status = Some(format!("Failed to save chip library: {e}"));
			} else {
				v.prefs = desc;
			}
			close_all_overlays(v);
			v.library_selection = LibrarySelection::None;
		}
		EditorAction::ToggleStarredCollectionPopup(name) => {
			v.bottom_bar_open_collection = if v.bottom_bar_open_collection.as_deref() == Some(name.as_str()) { None } else { Some(name) };
		}
		EditorAction::CloseStarredCollectionPopup => v.bottom_bar_open_collection = None,
		EditorAction::UseChip(name) => {
			open_chip_by_name(v, paths, status, &name);
			close_all_overlays(v);
		}
		EditorAction::ConfirmName => confirm_naming_popup(v, status),
		EditorAction::ChooseKey(c) => v.overlay_key_choice = Some(c),
		EditorAction::ConfirmKey => confirm_key_select_popup(v, status),
		EditorAction::RomSelectCell(idx) => {
			if let Some(editor) = v.rom_editor.as_mut() {
				editor.selected = idx.min(crate::render::editor_ui::ROM_WORD_COUNT - 1);
				v.overlay_text_input = editor.data[editor.selected].to_string();
			}
		}
		EditorAction::RomConfirmCell => confirm_rom_cell(v, status),
		EditorAction::RomApply => apply_rom_editor(v, status),
		EditorAction::SaveChipConfirm => confirm_save_chip_popup(v, paths, status),
		EditorAction::SaveChipSaveAs => confirm_save_chip_as(v, paths, status),
		EditorAction::SaveChipRename => confirm_save_chip_rename(v, paths, status),
		EditorAction::OpenChipCustomize => customize_flow::open_customize(v),
		EditorAction::CustomizeCancel => customize_flow::cancel_customize(v),
		EditorAction::CustomizeConfirm => customize_flow::confirm_customize(v, status),
		EditorAction::CustomizeCycleNameLocation => customize_flow::cycle_name_location(v),
		EditorAction::CustomizePickColour(i) => customize_flow::pick_colour(v, i),
		EditorAction::CustomizeGrabDisplayMove(i) => customize_flow::start_move_display(v, i),
		EditorAction::CustomizeGrabDisplayScale(i) => customize_flow::start_scale_display(v, i),
		EditorAction::CustomizeResizeStart(corner) => customize_flow::start_resize(v, corner),
		EditorAction::CustomizePlaceEntry(entry) => customize_flow::place_list_entry(v, entry),
	}
}

/// Tab into the library: sync collections first so chips that exist but
/// were never explicitly filed still show up -- while never-saved
/// Ctrl+N-style drafts stay out (see `sync_library_collections`), so the
/// panel and the project description can't pick one up before it's been
/// saved with Ctrl+S. Shared by its keyboard shortcut and any future
/// mouse affordance.
pub(crate) fn open_library_panel(v: &mut ViewerState) {
	sync_library_collections(&mut v.prefs, &v.library, &v.unsaved_drafts);
	open_overlay(v, Overlay::Library);
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ProjectDescription;

	/// An empty collection deletes immediately (no confirmation), while a
	/// non-empty one only opens the confirmation panel -- mirrors the
	/// original's DELETE-button behaviour for collections.
	#[test]
	fn request_delete_collection_branches_on_emptiness() {
		let mut prefs = ProjectDescription::default();
		prefs.chip_collections.push(ChipCollection::new("EMPTY", Vec::<String>::new()));
		prefs.chip_collections.push(ChipCollection::new("FULL", vec!["X".to_string()]));

		// The empty-collection branch is exactly `delete_collection`.
		delete_collection(&mut prefs, 0);
		assert!(prefs.chip_collections.iter().all(|c| c.name != "EMPTY"));
		assert!(prefs.chip_collections.iter().any(|c| c.name == "FULL" && c.chips.len() == 1));
	}
}
