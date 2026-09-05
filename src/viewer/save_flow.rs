//! Chip save/open flows: the Ctrl+S save popup's modes (save / save-as / rename / replace),
//! new-chip creation, and switching the viewer to a different chip -- everything that moves
//! whole `ChipDescription`s between the in-memory library and the project's on-disk chip
//! files.
use crate::gate_op::{calculate_num_input_bits, is_combinational, MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING};
use crate::render::editor_ui::{LibrarySelection, SaveChipMode};
use crate::viewer::library::{is_custom_chip, DEFAULT_LIBRARY_COLLECTION_NAME};
use crate::viewer::state::{Overlay, PendingUnsavedAction, ViewerState};
use crate::{ChipDescription, ChipLibrary, ChipType, SavePaths, Saver, Simulator};

/// Resolves `desc.should_be_cached`/`desc.cache_kind` right before they're written to disk --
/// the one place the "was the caching checkbox actually touched?" rule from
/// `ViewerState::cache_toggle_touched` gets applied.
pub(crate) fn resolve_should_cache(v: &ViewerState, desc: &mut ChipDescription, touched_as: &str) {
	let sim = Simulator::build(desc, &v.library);
	let root = sim.root();

	if !is_combinational(&sim, root) {
		desc.cache_kind = crate::description::CacheKind::Off;
		return;
	}

	if v.cache_toggle_touched.contains(&touched_as.to_ascii_lowercase()) {
		// User explicitly set this via the customize checkbox -- keep it,
		// only forcing it off if it no longer fits even the wider
		// user-caching budget (e.g. a live edit widened an input pin).
		if !desc.cache_kind.is_off() && calculate_num_input_bits(&sim, root) > MAX_NUM_INPUT_BITS_WHEN_USER_CACHING {
			desc.cache_kind = crate::description::CacheKind::Off;
		}
		return;
	}

	// Untouched: re-derive from the auto-cache rule every time, so caching
	// never silently turns itself on for a chip nobody asked to cache.
	desc.cache_kind = if calculate_num_input_bits(&sim, root) <= MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING {
		crate::description::CacheKind::None
	} else {
		crate::description::CacheKind::Off
	};
}

/// Determines which buttons `Overlay::SaveChip` should show for the currently-typed name, by
/// comparing it against `v.root_chip_name` (the chip's current identity) and the rest of
/// `v.library` -- see `editor_ui::SaveChipMode`'s docs for what each variant means and
/// `build_save_chip_popup`'s docs for why this is re-derived identically on both the render
/// side and the click-handling side.
pub(crate) fn save_chip_mode(v: &ViewerState, typed: &str) -> SaveChipMode {
	let typed = typed.trim();
	if typed.eq_ignore_ascii_case(&v.root_chip_name) {
		SaveChipMode::Save
	} else if v.library.try_get(typed).is_some() {
		SaveChipMode::Replace
	} else if v.unsaved_drafts.contains(&v.root_chip_name.to_ascii_lowercase()) {
		// Unsaved draft saved under a new name: it's a first-time Save, NOT Rename/SaveAs
		SaveChipMode::Save
	} else {
		SaveChipMode::SaveAsOrRename
	}
}

/// Updates the project's `chip_collections` bookkeeping for a chip that was just created,
/// renamed, or saved-as (`add_name`, already present in `v.library` by this point), then
/// persists the updated `ProjectDescription`. `all_custom_chip_names` is no longer hand-maintained
/// here -- it's rederived from `v.library` (the actual source of truth) right before saving, so
/// there's nothing to push/retain for it.
///
/// `chip_collections` membership and ordering, on the other hand, *is* real user-authored state
/// (which folder a chip lives in, and where) that `v.library` has no way to reconstruct, so it's
/// still handled explicitly: on rename (`remove_name` given), the old entry is renamed in place
/// -- preserving which collection holds it and where, mirroring `EnsureChipRenamedInCollections`
/// -- rather than moving it to OTHER; on first creation, it's simply filed under OTHER since it
/// isn't in any collection yet.
fn register_chip_name_in_project(v: &mut ViewerState, paths: &SavePaths, remove_name: Option<&str>, add_name: &str) {
	if let Some(old) = remove_name {
		// Rename within whichever collection already holds the chip,
		// keeping its position (and its membership of any other list) intact.
		let renamed = v
			.prefs
			.chip_collections
			.iter_mut()
			.find_map(|c| c.chips.iter_mut().find(|n| n.eq_ignore_ascii_case(old)).map(|slot| *slot = add_name.to_string()))
			.is_some();
		if !renamed {
			for c in v.prefs.chip_collections.iter_mut() {
				c.chips.retain(|n| !n.eq_ignore_ascii_case(old));
			}
		}
	}
	if !v.prefs.chip_collections.iter().any(|c| c.chips.iter().any(|n| n == add_name)) {
		if !v.prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
			v.prefs.chip_collections.push(crate::json::ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
		}
		let other =
			v.prefs.chip_collections.iter_mut().find(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)).expect("just ensured above");
		other.chips.push(add_name.to_string());
	}
	v.prefs.recompute_all_custom_chip_names(&v.library);

	let mut desc = v.prefs.clone();
	match Saver::save_project_description(paths, &mut desc) {
		Ok(()) => v.prefs = desc,
		Err(e) => log::warn!("failed to update project description: {e}"),
	}
}

/// Renames a starred entry pointing at `old_name` (`Project.RenameStarred`):
/// a starred shortcut must survive the chip it points at being renamed.
fn rename_starred_chip(v: &mut ViewerState, paths: &SavePaths, old_name: &str, new_name: &str) {
	let mut modified = false;
	for item in &mut v.prefs.starred_list {
		if !item.is_collection && item.name.eq_ignore_ascii_case(old_name) {
			item.name = new_name.to_string();
			modified = true;
			break;
		}
	}
	if modified {
		let mut desc = v.prefs.clone();
		match Saver::save_project_description(paths, &mut desc) {
			Ok(()) => v.prefs = desc,
			Err(e) => log::warn!("failed to update project description: {e}"),
		}
	}
}

/// Stamps a never-saved chip's on-disk identity defaults
/// (`DescriptionCreator.CreateChipDescription`'s `hasSavedDesc == false`
/// branch): body size at least the pin/name-derived minimum, and a random
/// HSV body colour. Chips that have been saved before keep whatever they had.
fn stamp_first_save_defaults(v: &mut ViewerState, name: &str) {
	if !v.unsaved_drafts.contains(&name.to_ascii_lowercase()) {
		return;
	}
	use crate::render::{layout, theme};
	let input_bits: Vec<crate::PinBitCount> = v.library.get(name).input_pins.iter().map(|p| p.bit_count).collect();
	let output_bits: Vec<crate::PinBitCount> = v.library.get(name).output_pins.iter().map(|p| p.bit_count).collect();
	let chip = v.library.get_mut(name);
	chip.size = layout::calculate_min_chip_size(&input_bits, &output_bits, chip, theme::FONT_SIZE_CHIP_NAME).max(chip.size);
	if chip.colour[3] == 0.0 {
		chip.colour = random_initial_chip_colour();
	}
}

/// `DescriptionCreator.RandomInitialChipColour`: random hue, saturation and
/// value each lerped 0.2..1 (value/saturation only), so fresh chips are
/// varied but never near-black/near-grey.
fn random_initial_chip_colour() -> [f32; 4] {
	fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 4] {
		let i = (h * 6.0).floor() as i32;
		let f = h * 6.0 - i as f32;
		let p = v * (1.0 - s);
		let q = v * (1.0 - f * s);
		let t = v * (1.0 - (1.0 - f) * s);
		let (r, g, b) = match (i % 6 + 6) % 6 {
			0 => (v, t, p),
			1 => (q, v, p),
			2 => (p, v, t),
			3 => (p, q, v),
			4 => (t, p, v),
			_ => (v, p, q),
		};
		[r, g, b, 1.0]
	}
	let mut rng = rand::thread_rng();
	use rand::Rng;
	hsv_to_rgb(rng.gen::<f32>(), rng.gen_range(0.2..=1.0), rng.gen_range(0.2..=1.0))
}

/// The save-time parent cascade (`Project.UpdateAndSaveAffectedChips`): every chip whose
/// subchips include `target_name` is re-derived from its saved description with this chip's
/// change folded in, and resaved. - `removed_pin_ids`: dev-pin ids present in the *previous*
/// saved version but gone from the new one -- wires attached to those pins of the affected
/// chips' instances are deleted; - `renamed_to`: the chip was (also) renamed -- matching
/// subchip instances are re-pointed at the new name.
fn resave_affected_parent_chips(v: &mut ViewerState, paths: &SavePaths, target_name: &str, removed_pin_ids: &[i32], renamed_to: Option<&str>) {
	let parent_names: Vec<String> =
		v.library.iter().filter(|d| d.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(target_name))).map(|d| d.name.clone()).collect();

	for parent_name in parent_names {
		let Ok(mut pristine) = load_single_chip_from_disk(paths, &v.prefs.project_name, &parent_name) else { continue };
		pristine.wires.retain(|w| {
			let attaches_to_removed = |addr: crate::PinAddress| {
				pristine
					.sub_chips
					.iter()
					.find(|s| s.id == addr.pin_owner_id)
					.is_some_and(|s| s.name.eq_ignore_ascii_case(target_name) && removed_pin_ids.contains(&addr.pin_id))
			};
			!attaches_to_removed(w.source_pin_address) && !attaches_to_removed(w.target_pin_address)
		});
		if let Some(new_name) = renamed_to {
			for sub in &mut pristine.sub_chips {
				if sub.name.eq_ignore_ascii_case(target_name) {
					sub.name = new_name.to_string();
				}
			}
		}
		if let Err(e) = Saver::save_chip(paths, &v.prefs.project_name, &v.library, &pristine) {
			log::warn!("failed to resave affected chip '{parent_name}': {e}");
			continue;
		}
		*v.library.get_mut(&parent_name) = pristine;
	}
}

/// Ids of boundary dev-pins the previously-saved version had that `desc`
/// no longer does -- what `UpdateAndSaveAffectedChips` diffs before
/// scrubbing dangling wires out of parent chips.
fn removed_boundary_pin_ids(v: &ViewerState, paths: &SavePaths, name: &str, desc: &ChipDescription) -> Vec<i32> {
	let Ok(previous) = load_single_chip_from_disk(paths, &v.prefs.project_name, name) else { return Vec::new() };
	let still_present = |id: i32| desc.input_pins.iter().chain(desc.output_pins.iter()).any(|p| p.id == id);
	previous.input_pins.iter().chain(previous.output_pins.iter()).filter(|p| !still_present(p.id)).map(|p| p.id).collect()
}

/// Plain overwrite/create (`SaveChipMode::Save`): writes the current
/// in-memory chip back to its own file under its own (unchanged) name.
/// A never-saved draft gets its identity stamped first and is auto-starred
/// (`Project.SaveFromDescription`'s new-chip path); a previously-saved chip
/// whose boundary pins shrank resaves its parent chips without the orphaned
/// wires.
fn save_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let name = v.root_chip_name.clone();
	let was_draft = v.unsaved_drafts.contains(&name.to_ascii_lowercase());
	let removed_pins = if was_draft { Vec::new() } else { removed_boundary_pin_ids(v, paths, &name, v.library.get(&name)) };
	if was_draft {
		stamp_first_save_defaults(v, &name);
	}
	let mut desc = v.library.get(&name).clone();
	resolve_should_cache(v, &mut desc, &name);
	v.library.get_mut(&name).cache_kind = desc.cache_kind;
	match Saver::save_chip(paths, &v.prefs.project_name, &v.library, &desc) {
		Ok(()) => {
			v.mark_saved(&name);
			// This chip's definition just changed on disk -- any LUT built
			// from its old behaviour (its own entry, or any parent chip's
			// that embeds it) is now stale. Simply viewing/editing without
			// saving never reaches this call, so the cache otherwise
			// survives untouched (see `ViewerState::rebuild_sim`).
			v.sim.clear_caching(&name);
			if !removed_pins.is_empty() {
				resave_affected_parent_chips(v, paths, &name, &removed_pins, None);
			}
			if was_draft {
				// New chips are automatically starred (`Project.SaveFromDescription`).
				v.prefs.set_starred(&name, true, false);
				register_chip_name_in_project(v, paths, None, &name);
			}
			*status = Some(format!("Saved '{name}'"));
		}
		Err(e) => *status = Some(format!("Failed to save '{name}': {e}")),
	}
}

/// Saves a *copy* of the currently-open chip under `new_name`
/// (`SaveChipMode::SaveAsOrRename`, "Save As" button), leaving its existing on-disk file
/// (under its current name, if it has one) completely untouched. Since that current
/// identity's `v.library` entry has been edited in place all session, once we fork away from
/// it its in-memory copy no longer matches what's actually on disk under that name -- so it's
/// reloaded fresh from its own file right after (see `load_single_chip_from_disk`),
/// discarding whatever of this session's edits hadn't already been saved under *that*
/// identity.
fn save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();
	resolve_should_cache(v, &mut new_desc, &old_name);

	match Saver::save_chip(paths, &v.prefs.project_name, &v.library, &new_desc) {
		Ok(()) => {
			v.library.add(new_desc);
			v.mark_saved(new_name);
			// New identity written to disk -- see the matching comment in
			// `save_current_chip` for why this must invalidate the cache.
			v.sim.clear_caching(&old_name);
			// A Save-As is a new chip as far as starring goes
			// (`isNewChip = ... || saveMode is SaveMode.SaveAs`); the single
			// project-description write below persists both bookkeepings.
			v.prefs.set_starred(new_name, true, false);
			register_chip_name_in_project(v, paths, None, new_name);

			if !old_name.eq_ignore_ascii_case(new_name) {
				match load_single_chip_from_disk(paths, &v.prefs.project_name, &old_name) {
					Ok(pristine) => {
						v.library.add(pristine);
					}
					Err(_) => {
						// No on-disk file for the old identity (it was never actually saved under that
						// name to begin with) -- nothing to revert to, so leave the in-memory draft as is.
					}
				}
			}

			v.undo.clear();
			v.exit_view_mode();
			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Saved as '{new_name}'"));
			v.restart_sim_fresh();
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
	if let Err(e) = Saver::delete_chip(paths, &v.prefs.project_name, new_name, true) {
		*status = Some(format!("Failed to back up existing '{new_name}': {e}"));
		return;
	}
	v.library.remove(new_name);
	save_chip_as(v, paths, status, new_name);
}

/// Actually renames the chip (`SaveChipMode::SaveAsOrRename`, "Rename" button): moves its on-
/// disk file to `new_name` -- no copy left under the old name, the old file is deleted
/// outright (no backup, since this is a rename rather than a delete) -- updates the project's
/// chip-name bookkeeping to match, re-points parents' subchip instances
/// (`UpdateAndSaveAffectedChips`' willRename pass), and carries any starred shortcut over to
/// the new name.
fn rename_current_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, new_name: &str) {
	let old_name = v.root_chip_name.clone();
	let removed_pins = removed_boundary_pin_ids(v, paths, &old_name, v.library.get(&old_name));
	let mut new_desc = v.library.get(&old_name).clone();
	new_desc.name = new_name.to_string();
	resolve_should_cache(v, &mut new_desc, &old_name);

	match Saver::save_chip(paths, &v.prefs.project_name, &v.library, &new_desc) {
		Ok(()) => {
			if let Err(e) = Saver::delete_chip(paths, &v.prefs.project_name, &old_name, false) {
				log::warn!("renamed '{old_name}' to '{new_name}' but failed to remove the old file: {e}");
			}
			v.library.remove(&old_name);
			v.library.add(new_desc);
			v.mark_saved(new_name);
			v.mark_saved(&old_name);
			// The old name's cache entry is now orphaned (nothing looks it
			// up again) and the new name has none yet -- see the matching
			// comment in `save_current_chip`.
			v.sim.clear_caching(&old_name);
			resave_affected_parent_chips(v, paths, &old_name, &removed_pins, Some(new_name));
			register_chip_name_in_project(v, paths, Some(&old_name), new_name);
			rename_starred_chip(v, paths, &old_name, new_name);
			v.undo.clear();
			v.exit_view_mode();
			v.root_chip_name = new_name.to_string();
			*status = Some(format!("Renamed '{old_name}' to '{new_name}'"));
			v.restart_sim_fresh();
		}
		Err(e) => *status = Some(format!("Failed to rename to '{new_name}': {e}")),
	}
}

/// The authoritative save/rename name gate (`ChipSaveMenu.IsValidSaveName`):
/// non-blank, a legal file name on every OS, and -- when it names a
/// *different* existing chip -- only acceptable via the explicit Replace
/// flow (the popup's mode logic routes those; everything else must refuse).
/// The UI's Confirm-button greying mirrors the non-library halves of this;
/// this function is the one that can't be bypassed by keyboard paths.
fn valid_save_name(v: &ViewerState, typed: &str) -> bool {
	let typed = typed.trim();
	if typed.is_empty() || !crate::save_system::valid_file_name(typed) || typed.len() > crate::render::editor_ui::MAX_CHIP_NAME_LENGTH {
		return false;
	}
	let already_used = v.library.try_get(typed).is_some();
	!already_used || typed.eq_ignore_ascii_case(&v.root_chip_name)
}

/// Applies the `Overlay::SaveChip` popup's Confirm action -- shared by its "Save"/"Replace"
/// button (`EditorAction::SaveChipConfirm`) and pressing Enter directly for those same two
/// (unambiguous) modes; see the key-handler's own guard for why `SaveAsOrRename` never
/// reaches here via Enter (that mode's own two buttons call
/// `confirm_save_chip_as`/`confirm_save_chip_rename` directly instead, since which of "keep
/// both" or "actually rename" is meant can't be inferred, only chosen).
pub(crate) fn confirm_save_chip_popup(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if !valid_save_name(v, &typed) {
		*status = Some("Invalid chip name".to_string());
		return;
	}
	match save_chip_mode(v, &typed) {
		SaveChipMode::Save => {
			if v.unsaved_drafts.contains(&v.root_chip_name.to_ascii_lowercase()) && !typed.eq_ignore_ascii_case(&v.root_chip_name) {
				let old_name = v.root_chip_name.clone();
				v.unsaved_drafts.remove(&old_name.to_ascii_lowercase());
				if let Some(mut desc) = v.library.remove(&old_name) {
					desc.name = typed.clone();
					v.library.add(desc);
				}
				v.root_chip_name = typed.clone();
				v.mark_unsaved_draft(&typed);
			}
			save_current_chip(v, paths, status);
		}
		SaveChipMode::Replace => replace_chip_with_current(v, paths, status, &typed),
		SaveChipMode::SaveAsOrRename => return,
	}
	v.close_top_overlay();
}

pub(crate) fn confirm_save_chip_as(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if !valid_save_name(v, &typed) {
		*status = Some("Invalid chip name".to_string());
		return;
	}
	save_chip_as(v, paths, status, &typed);
	v.close_top_overlay();
}

pub(crate) fn confirm_save_chip_rename(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	let typed = v.overlay_text_input.trim().to_string();
	if !valid_save_name(v, &typed) {
		*status = Some("Invalid chip name".to_string());
		return;
	}
	rename_current_chip(v, paths, status, &typed);
	v.close_top_overlay();
}

/// Re-reads a single chip's own save file from disk, without touching
/// anything else in `v.library` -- used to revert one specific chip's
/// in-memory entry back to "whatever's actually saved" (e.g. the chip
/// left behind, untouched, by a Save-As/Replace under a new name; see
/// `save_chip_as`), as opposed to blindly reloading the whole project.
pub(crate) fn load_single_chip_from_disk(paths: &SavePaths, project_name: &str, chip_name: &str) -> std::io::Result<ChipDescription> {
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
	crate::viewer::wire_edit::exit(v);
}

/// Discards whatever unsaved edits this session made to whichever chip is currently open
/// (`v.root_chip_name`), by reloading its pristine on-disk copy back over its `v.library`
/// entry (same "reload from disk" move `save_chip_as` already does for the identity it forks
/// away from -- see `load_single_chip_from_disk`).
fn discard_unsaved_changes(v: &mut ViewerState, paths: &SavePaths) {
	let leaving = v.root_chip_name.clone();
	if !is_custom_chip(&v.library, &leaving) {
		return;
	}
	if v.unsaved_drafts.contains(&leaving.to_ascii_lowercase()) {
		// Draft was never saved to disk: purge from library and drafts set
		v.library.remove(&leaving);
		v.unsaved_drafts.remove(&leaving.to_ascii_lowercase());
		return;
	}
	if let Ok(pristine) = load_single_chip_from_disk(paths, &v.prefs.project_name, &leaving) {
		v.library.add(pristine);
	}
}

// ---- Unsaved-changes gate (`ActiveChipHasUnsavedChanges` + `UnsavedChangesPopup`) ----

/// Whether the currently-open chip has edits that exist only in memory -- port of
/// `Project.ActiveProject.ActiveChipHasUnsavedChanges`. The comparison goes through
/// `json::is_equivalent_json` (structural, float-tolerant) rather than raw string inequality,
/// mirroring `Saver.HasUnsavedChanges`'s token-level comparison -- so e.g. dragging a
/// component back to where it started isn't an edit.
pub(crate) fn active_chip_has_unsaved_changes(v: &ViewerState, paths: &SavePaths) -> bool {
	let name = v.root_chip_name.clone();
	if !is_custom_chip(&v.library, &name) {
		return false;
	}
	// Both sides serialize through the project-aware writer -- the exact
	// shape `Saver::save_chip` puts on disk -- so library-resolved
	// `OutputPinColourInfo` (and bus `null`s) cancel out instead of
	// reading as edits.
	let saved_json = load_single_chip_from_disk(paths, &v.prefs.project_name, &name)
		.ok()
		.and_then(|pristine| crate::json::serialize_chip_description_for_save(&pristine, &v.library).ok());
	let Some(saved_json) = saved_json else {
		return !v.library.get(&name).sub_chips.is_empty();
	};
	let Ok(current_json) = crate::json::serialize_chip_description_for_save(v.library.get(&name), &v.library) else { return true };
	!crate::json::is_equivalent_json(&saved_json, &current_json)
}

/// Opens the confirmation popup remembering `pending` as the action to
/// resume on Continue (`UnsavedChangesPopup.OpenPopup(callback)`).
fn open_unsaved_changes_prompt(v: &mut ViewerState, pending: PendingUnsavedAction) {
	v.pending_unsaved_action = Some(pending);
	v.open_overlay(Overlay::UnsavedChanges);
}

/// The shared post-open cleanup of the library-panel/search "open this
/// chip" call sites (`ExitLibrary`-style): leave every overlay and drop
/// the selection/open flyout so nothing stale points at the panel that's
/// now behind us.
fn finish_open_from_library(v: &mut ViewerState) {
	v.close_all_overlays();
	v.library_selection = LibrarySelection::None;
	v.bottom_bar_open_collection = None;
}

/// Gates "switch the viewer to editing `name`" behind the unsaved-changes prompt -- the shape
/// every OPEN call site of the original shares (`if ActiveChipHasUnsavedChanges()
/// OpenPopup(OpenChipIfConfirmed) else OpenChipIfConfirmed(true)`): with nothing to confirm
/// (or re-opening the chip already on screen) it acts straight away; otherwise the popup
/// opens and [`PendingUnsavedAction::OpenChip`] remembers what to resume.
pub(crate) fn request_open_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str, close_overlays: bool) {
	if name != v.root_chip_name && active_chip_has_unsaved_changes(v, paths) {
		open_unsaved_changes_prompt(v, PendingUnsavedAction::OpenChip { name: name.to_string(), close_overlays });
		return;
	}
	open_chip_by_name(v, paths, status, name);
	if close_overlays {
		finish_open_from_library(v);
	}
}

/// Ctrl+N's gated twin (see `request_open_chip`): prompts before throwing
/// away the current chip's unsaved edits, else creates straight away.
pub(crate) fn request_start_new_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	if active_chip_has_unsaved_changes(v, paths) {
		open_unsaved_changes_prompt(v, PendingUnsavedAction::StartNewChip);
		return;
	}
	start_new_chip(v, paths, status);
}

/// Escape-to-menu's gated twin (see `request_open_chip`): prompts before
/// abandoning the chip; when clean, asks the app shell to leave via
/// [`ViewerState::exit_requested`] (the viewer can't swap screens
/// itself).
pub(crate) fn request_exit_to_menu(v: &mut ViewerState, paths: &SavePaths) {
	if active_chip_has_unsaved_changes(v, paths) {
		open_unsaved_changes_prompt(v, PendingUnsavedAction::ReturnToMenu);
		return;
	}
	v.exit_requested = true;
}

/// Runs whatever originally opened the unsaved-changes prompt -- the
/// confirmed half of the original's stored callback (`callback(true)`),
/// shared by the popup's Continue button and pressing Enter. Cancel
/// instead just closes the popup (dropping the pending action with it --
/// see `state::close_top_overlay`). Either way the popup itself closes.
pub(crate) fn confirm_unsaved_changes_popup(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	match v.pending_unsaved_action.take() {
		Some(PendingUnsavedAction::OpenChip { name, close_overlays }) => {
			open_chip_by_name(v, paths, status, &name);
			if close_overlays {
				finish_open_from_library(v);
			}
		}
		Some(PendingUnsavedAction::StartNewChip) => start_new_chip(v, paths, status),
		Some(PendingUnsavedAction::ReturnToMenu) => v.exit_requested = true,
		None => {}
	}
	v.close_top_overlay();
}

/// Picks a fresh, not-yet-used (case-insensitively) name for a
/// brand-new chip, starting from "New_Chip" and falling back to
/// "New_Chip_2", "New_Chip_3", ... the first suffix that isn't already
/// taken in `library` -- so hitting Ctrl+N repeatedly never collides
/// with an earlier still-unsaved draft (or a saved chip that happens to
/// already be named "New_Chip").
const NEW_NAME: &str = "New_Chip";
pub(crate) fn unique_new_chip_name(library: &ChipLibrary) -> String {
	if library.try_get(NEW_NAME).is_none() {
		return NEW_NAME.to_string();
	}
	let mut n = 2;
	loop {
		let candidate = format!("{NEW_NAME}_{n}");
		if library.try_get(&candidate).is_none() {
			return candidate;
		}
		n += 1;
	}
}

/// Ctrl+N: starts a brand-new, blank custom chip (no pins, no subchips, no wires -- see
/// `ChipDescription::new`) and switches the viewer over to it, exactly as if it were an
/// existing chip being opened.
pub(crate) fn start_new_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>) {
	discard_unsaved_changes(v, paths);

	let name = unique_new_chip_name(&v.library);
	v.library.add(ChipDescription::new(&name, ChipType::Custom));
	v.mark_unsaved_draft(&name);

	v.undo.clear();
	v.exit_view_mode();
	v.root_chip_name = name.clone();
	v.sim.reset_driven_inputs();
	v.restart_sim_fresh();
	v.camera_fitted = false;
	reset_canvas_interaction(v);
	*status = Some(format!("New chip '{name}'"));
}

/// Actually switches the viewer over to `name`'s own definition -- i.e.
pub(crate) fn open_chip_by_name(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	let switching = name != v.root_chip_name;

	if is_custom_chip(&v.library, name) {
		if switching {
			discard_unsaved_changes(v, paths);
			v.undo.clear();
			v.exit_view_mode();
		}
		v.root_chip_name = name.to_string();
		v.sim.reset_driven_inputs();
		v.restart_sim_fresh();
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
		assert_eq!(unique_new_chip_name(&library), "New_Chip");

		library.add(ChipDescription::new("New_Chip", ChipType::Custom));
		assert_eq!(unique_new_chip_name(&library), "New_Chip_2");

		library.add(ChipDescription::new("new_chip_2", ChipType::Custom));
		assert_eq!(unique_new_chip_name(&library), "New_Chip_3");
	}

	/// The save-name gate (`IsValidSaveName`): blanks and filename-illegal
	/// names are refused outright; a *different* existing chip's name is
	/// refused (Replace is a separate flow); the active chip's own name is
	/// always acceptable.
	#[test]
	fn valid_save_name_mirrors_the_originals_rules() {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		library.add(ChipDescription::new("OTHER", ChipType::Custom));
		let v = ViewerState::new("", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());

		assert!(!valid_save_name(&v, ""), "blank");
		assert!(!valid_save_name(&v, "   "), "whitespace");
		assert!(!valid_save_name(&v, "bad:name"), "forbidden character");
		assert!(!valid_save_name(&v, "CON"), "reserved device name");
		assert!(!valid_save_name(&v, "MY VERY LONG CHIP NAME X"), "over the length cap");
		assert!(!valid_save_name(&v, "OTHER"), "a different existing chip's name");
		assert!(!valid_save_name(&v, "NAND"), "a builtin's name");
		assert!(valid_save_name(&v, "Brand New"), "fresh name");
		assert!(valid_save_name(&v, "root"), "the active chip's own name (case-insensitive)");
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

	/// Switching which chip is being edited (`open_chip_by_name` ->
	/// `restart_sim_fresh`) must not throw away the LUT cache built for
	/// combinational chips: a cache entry has nothing to do with which
	/// chip happens to be the *edited root* right now, so losing it on
	/// every switch would force an expensive truth-table sweep to be
	/// redone each time the user simply opens a different chip.
	#[test]
	fn caching_state_survives_switching_the_edited_root_chip() {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		library.add(ChipDescription::new("OTHER", ChipType::Custom));

		let root = crate::save_system::test_util::temp_dir("caching_survives_root_switch");
		let paths = SavePaths::new(&root);
		crate::create_project(&paths, "P").expect("project created");

		let mut v =
			ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());

		// Seed the cache as if ROOT's subtree had already been simulated
		// and some chip's truth table built.
		{
			let mut sim = v.sim.lock();
			sim.caching.combinational_chip_cache.insert("SEEDED".into(), Box::new(crate::gate_op::Lut::new(vec![vec![0]])));
			sim.caching.not_combinational_chip_cache.insert("SEEDED_NC".into());
		}

		open_chip_by_name(&mut v, &paths, &mut None, "OTHER");
		assert_eq!(v.root_chip_name, "OTHER");

		let sim = v.sim.lock();
		assert!(
			sim.caching.combinational_chip_cache.contains_key("SEEDED"),
			"combinational LUT cache must carry across a root-chip switch, not be rebuilt from scratch"
		);
		assert!(
			sim.caching.not_combinational_chip_cache.contains("SEEDED_NC"),
			"the not-combinational memo set must also carry across a root-chip switch"
		);
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
		assert_eq!(name, "New_Chip");
		assert!(v.unsaved_drafts.contains("new_chip"), "the fresh chip starts life as an unsaved draft");

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
		assert!(paths.chips_path("P").join("New_Chip.json").is_file(), "chip file written by the save");
		assert!(!v.unsaved_drafts.contains("new chip"), "saving clears the draft marker");

		// ...and from then on the library panel does list it.
		open_library_panel(&mut v);
		let other = v.prefs.chip_collections.iter().find(|c| c.name == "OTHER").expect("OTHER collection exists");
		assert!(other.chips.iter().any(|n| n.eq_ignore_ascii_case(&name)), "saved chip joins the library");

		let _ = std::fs::remove_dir_all(&root);
	}

	fn viewer_on_saved_project(paths: &SavePaths, root: &str, other: &str) -> ViewerState {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(ChipDescription::new(root, ChipType::Custom));
		library.add(ChipDescription::new(other, ChipType::Custom));
		let v = ViewerState::new("P", library, root.to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		for name in [root, other] {
			Saver::save_chip(paths, "P", &v.library, &v.library.get(name).clone()).expect("chip written");
		}
		v
	}

	/// First save of a draft stamps its identity (`RandomInitialChipColour`
	/// + minimum size), stars it, and persists both bookkeepings.
	#[test]
	fn first_save_stamps_identity_and_stars_the_chip() {
		let root = crate::save_system::test_util::temp_dir("first_save_stamp");
		let paths = SavePaths::new(&root);
		let mut v = {
			let mut library = ChipLibrary::new();
			crate::register_all_builtins(&mut library);
			library.add(ChipDescription::new("ROOT", ChipType::Custom));
			let mut v =
				ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
			v.prefs = crate::create_project(&paths, "P").expect("project").description;
			v
		};

		start_new_chip(&mut v, &paths, &mut None);
		let name = v.root_chip_name.clone();
		// Give it pins so the min-size stamp has something to compute from.
		let mut pin = crate::PinDescription::new("IN", 1, crate::PinBitCount::Bit1);
		pin.position = crate::structs::Vec2::ZERO;
		v.library.get_mut(&name).input_pins.push(pin);

		open_save_chip(&mut v);
		confirm_save_chip_popup(&mut v, &paths, &mut None);

		let saved = v.library.get(&name);
		assert!(saved.size.x > 0.0 && saved.size.y > 0.0, "min size stamped: {:?}", saved.size);
		assert_eq!(saved.colour[3], 1.0, "opaque random body colour");
		assert!(saved.colour.iter().take(3).any(|&c| c > 0.0), "not black: {:?}", saved.colour);
		assert!(v.prefs.is_starred(&name, false), "freshly saved chips are auto-starred");

		let text = std::fs::read_to_string(paths.project_description_path("P")).unwrap();
		assert!(text.contains(&name), "starred entry persisted to disk");

		// Saving again must NOT re-roll the colour.
		let colour_before = saved.colour;
		open_save_chip(&mut v);
		confirm_save_chip_popup(&mut v, &paths, &mut None);
		assert_eq!(v.library.get(&name).colour, colour_before, "already-saved identity is kept");

		let _ = std::fs::remove_dir_all(&root);
	}

	/// Renaming keeps the chip in its collection (in place, position kept)
	/// and carries any starred shortcut over -- plus re-points parents'
	/// subchip instances on disk.
	#[test]
	fn rename_preserves_collections_starred_and_parents() {
		let root = crate::save_system::test_util::temp_dir("rename_bookkeeping");
		let paths = SavePaths::new(&root);
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		for name in ["ROOT", "OLD NAME"] {
			library.add(ChipDescription::new(name, ChipType::Custom));
		}
		let mut v =
			ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		v.prefs = crate::create_project(&paths, "P").expect("project").description;
		Saver::save_chip(&paths, "P", &v.library, &v.library.get("OLD NAME").clone()).expect("saved");

		// File it into a named collection and star it.
		v.prefs.chip_collections.push(crate::json::ChipCollection::new("MYCOLL", vec!["OLD NAME".to_string()]));
		v.prefs.set_starred("OLD NAME", false, false);
		v.prefs.set_starred("OLD NAME", true, false);

		open_chip_by_name(&mut v, &paths, &mut None, "OLD NAME");
		v.overlay_text_input = "NEW NAME".to_string();
		rename_current_chip(&mut v, &paths, &mut None, "NEW NAME");

		let coll = v.prefs.chip_collections.iter().find(|c| c.name == "MYCOLL").expect("collection survives");
		assert_eq!(coll.chips, vec!["NEW NAME".to_string()], "renamed in place within its collection");
		assert!(v.prefs.is_starred("NEW NAME", false), "the starred shortcut follows the rename");
		assert!(!v.prefs.is_starred("OLD NAME", false));

		let _ = std::fs::remove_dir_all(&root);
	}

	/// Saving a chip whose boundary pins shrank scrubs dangling wires out
	/// of parent chips (`UpdateAndSaveAffectedChips`).
	#[test]
	fn saving_removed_pins_cascades_to_parent_chips() {
		use crate::{PinAddress, PinDescription, SubChipDescription, WireDescription};
		let root = crate::save_system::test_util::temp_dir("pin_removal_cascade");
		let paths = SavePaths::new(&root);

		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		let mut child = ChipDescription::new("CHILD", ChipType::Custom);
		let mut pin = PinDescription::new("OUT", 1, crate::PinBitCount::Bit1);
		pin.position = crate::structs::Vec2::ZERO;
		child.output_pins.push(pin);
		library.add(child.clone());
		library.add(ChipDescription::new("ROOT", ChipType::Custom));
		let mut v =
			ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		v.prefs = crate::create_project(&paths, "P").expect("project").description;
		Saver::save_chip(&paths, "P", &v.library, &v.library.get("CHILD").clone()).expect("child saved");

		// Parent uses CHILD and wires its OUT pin somewhere.
		let parent = v.library.get_mut("ROOT");
		parent.sub_chips.push(SubChipDescription {
			name: "CHILD".into(),
			id: 9,
			internal_data: None,
			position: crate::structs::Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		parent.wires.push(WireDescription::new(PinAddress::new(9, 1), PinAddress::new(0, 5)));
		Saver::save_chip(&paths, "P", &v.library, &v.library.get("ROOT").clone()).expect("parent saved");

		// Remove the pin from CHILD and save.
		v.library.get_mut("CHILD").output_pins.clear();
		open_chip_by_name(&mut v, &paths, &mut None, "CHILD");
		open_save_chip(&mut v);
		confirm_save_chip_popup(&mut v, &paths, &mut None);

		let parent_json = std::fs::read_to_string(paths.chips_path("P").join("ROOT.json")).unwrap();
		let parent_desc = crate::json::parse_chip_description(&parent_json).unwrap();
		assert!(parent_desc.wires.is_empty(), "dangling wire scrubbed from parent: {parent_json}");
		assert_eq!(parent_desc.sub_chips.len(), 1, "the CHILD instance itself stays");

		let _ = std::fs::remove_dir_all(&root);
	}

	fn place_a_nand(v: &mut ViewerState) {
		crate::viewer::chip_interaction::start_placing(v, "NAND");
		crate::viewer::canvas::try_place_pending_components(v, crate::structs::Vec2::ZERO, &mut None);
	}

	/// The dirty-detection contract: a saved-but-unedited chip is clean,
	/// any in-memory edit dirties it, saving cleans it again -- and a
	/// never-saved draft is dirty exactly once something is placed in it
	/// (`LastSavedDescription == null -> Elements.Count > 0`).
	#[test]
	fn active_chip_has_unsaved_changes_tracks_disk_truth() {
		let root = crate::save_system::test_util::temp_dir("unsaved_detect");
		let paths = SavePaths::new(&root);

		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");
		assert!(!active_chip_has_unsaved_changes(&v, &paths), "unedited saved chip is clean");

		place_a_nand(&mut v);
		assert!(active_chip_has_unsaved_changes(&v, &paths), "an in-memory-only placement is an unsaved change");

		open_save_chip(&mut v);
		confirm_save_chip_popup(&mut v, &paths, &mut None);
		assert!(!active_chip_has_unsaved_changes(&v, &paths), "saving cleans it");

		// A never-saved draft: blank = clean, anything placed = dirty.
		start_new_chip(&mut v, &paths, &mut None);
		assert!(!active_chip_has_unsaved_changes(&v, &paths), "a blank never-saved chip isn't dirty");
		place_a_nand(&mut v);
		assert!(active_chip_has_unsaved_changes(&v, &paths), "a placed component makes the draft dirty");

		let _ = std::fs::remove_dir_all(&root);
	}

	/// Switching away while dirty must prompt first and only switch on
	/// Continue; Cancel keeps you on the chip with nothing lost.
	#[test]
	fn request_open_chip_prompts_while_dirty_and_confirms_through() {
		let root = crate::save_system::test_util::temp_dir("unsaved_gate_open");
		let paths = SavePaths::new(&root);
		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");
		place_a_nand(&mut v);

		request_open_chip(&mut v, &paths, &mut None, "OTHER", true);
		assert_eq!(v.root_chip_name, "ROOT", "the switch waits behind the prompt");
		assert_eq!(v.overlays.last(), Some(&Overlay::UnsavedChanges));
		assert_eq!(v.pending_unsaved_action, Some(PendingUnsavedAction::OpenChip { name: "OTHER".to_string(), close_overlays: true }));

		v.close_top_overlay();
		assert_eq!(v.root_chip_name, "ROOT", "cancel leaves everything as it was");
		assert!(v.pending_unsaved_action.is_none());

		request_open_chip(&mut v, &paths, &mut None, "OTHER", true);
		confirm_unsaved_changes_popup(&mut v, &paths, &mut None); // Continue
		assert_eq!(v.root_chip_name, "OTHER", "continue performs the deferred open");
		assert!(v.overlays.is_empty() && v.pending_unsaved_action.is_none());
		assert!(!v.exit_requested);

		let _ = std::fs::remove_dir_all(&root);
	}

	/// A clean chip switches immediately -- no prompt -- and the library
	/// variant still does its leave-the-library cleanup on the way.
	#[test]
	fn clean_chip_switches_without_prompting() {
		let root = crate::save_system::test_util::temp_dir("unsaved_gate_clean");
		let paths = SavePaths::new(&root);
		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");

		open_library_panel(&mut v);
		request_open_chip(&mut v, &paths, &mut None, "OTHER", true);
		assert_eq!(v.root_chip_name, "OTHER", "switched straight away");
		assert!(v.overlays.is_empty() && v.pending_unsaved_action.is_none(), "no prompt; library left as usual");

		let _ = std::fs::remove_dir_all(&root);
	}

	/// The bar/context-menu open path (`close_overlays == false`) must
	/// switch on Continue without tearing down whatever panels were open
	/// underneath the prompt.
	#[test]
	fn confirmed_open_without_cleanup_leaves_other_overlays_open() {
		let root = crate::save_system::test_util::temp_dir("unsaved_gate_nocleanup");
		let paths = SavePaths::new(&root);
		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");
		place_a_nand(&mut v);

		crate::viewer::state::open_search(&mut v); // some panel under the prompt
		request_open_chip(&mut v, &paths, &mut None, "OTHER", false);
		assert_eq!(v.overlays.last(), Some(&Overlay::UnsavedChanges));

		confirm_unsaved_changes_popup(&mut v, &paths, &mut None);
		assert_eq!(v.root_chip_name, "OTHER", "the switch happened");
		assert_eq!(v.overlays, vec![Overlay::Search], "only the prompt closed; Search stays");

		let _ = std::fs::remove_dir_all(&root);
	}

	/// A clean chip never prompts: Escape-to-menu requests the exit
	/// straight away, Ctrl+N switches immediately, and a *dirty* chip
	/// routes both through the popup whose Continue finishes them.
	#[test]
	fn escape_and_new_chip_gates() {
		let root = crate::save_system::test_util::temp_dir("unsaved_gate_exit");
		let paths = SavePaths::new(&root);
		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");

		request_exit_to_menu(&mut v, &paths);
		assert!(v.exit_requested, "clean chip: leave without asking");

		// Reset the (never-consumed here) request, then make the chip dirty.
		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");
		place_a_nand(&mut v);
		request_exit_to_menu(&mut v, &paths);
		assert!(!v.exit_requested, "dirty chip: prompt instead of leaving");

		// Cancel keeps you in the editor with nothing pending.
		v.close_top_overlay();
		assert!(!v.exit_requested && v.pending_unsaved_action.is_none());

		// Re-request, then Continue resumes the exit.
		request_exit_to_menu(&mut v, &paths);
		confirm_unsaved_changes_popup(&mut v, &paths, &mut None);
		assert!(v.exit_requested, "continue resumes the exit");

		let mut v = viewer_on_saved_project(&paths, "ROOT", "OTHER");
		place_a_nand(&mut v);
		request_start_new_chip(&mut v, &paths, &mut None);
		assert_eq!(v.root_chip_name, "ROOT", "dirty chip: new-chip waits behind the prompt");
		confirm_unsaved_changes_popup(&mut v, &paths, &mut None);
		assert_eq!(v.root_chip_name, "New_Chip", "continue starts the fresh chip");
	}
}
