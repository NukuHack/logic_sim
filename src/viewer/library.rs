//! Chip-library bookkeeping: syncing collections with the loaded library,
//! the delete/rename confirmations' message building, collection
//! deletion, and row reordering -- the data half of the chip-library UI
//! (its frames are built by `render::editor_ui`, its clicks applied by
//! `viewer::actions`).

use crate::json::{ChipCollection, ProjectDescription};
use crate::render::editor_ui::LibrarySelection;
use crate::viewer::state::ViewerState;
use crate::{ChipLibrary, ChipType, SavePaths, Saver};

/// Mandatory catch-all collection every project's library falls back to
/// -- mirrors `ChipLibraryMenu`'s `defaultOtherChipsCollectionName`.
pub(crate) const DEFAULT_LIBRARY_COLLECTION_NAME: &str = "OTHER";

/// Whether `name` is a chip the player actually authored (as opposed to a built-in primitive
/// like `AND`/`NAND`/`Pulse`) -- i.e. whether "Open" makes any sense for it.
pub(crate) fn is_custom_chip(library: &ChipLibrary, name: &str) -> bool {
	library.try_get(name).map(|d| d.chip_type == ChipType::Custom).unwrap_or(false)
}

/// True if placing `chip_to_place` as a new subchip inside `root_chip_name` would create a
/// recursive cycle -- either because it *is* `root_chip_name` itself, or because its own
/// definition, directly or transitively through its own subchips, already contains
/// `root_chip_name` somewhere inside it.
pub(crate) fn would_create_cycle(library: &ChipLibrary, root_chip_name: &str, chip_to_place: &str) -> bool {
	if chip_to_place.eq_ignore_ascii_case(root_chip_name) {
		return true;
	}
	let mut visited = std::collections::HashSet::new();
	chip_contains(library, chip_to_place, root_chip_name, &mut visited)
}

/// True if `chip_name`'s own definition includes `target` anywhere inside it, directly or via any
/// of its subchips recursively. `visited` (chip names already expanded, lower-cased) guards
/// against looping forever if `library` somehow already describes a cycle (e.g. a hand-edited
/// save) -- same defensive purpose as the wire-endpoint resolver's own recursion guard.
fn chip_contains(library: &ChipLibrary, chip_name: &str, target: &str, visited: &mut std::collections::HashSet<String>) -> bool {
	if !visited.insert(chip_name.to_ascii_lowercase()) {
		return false;
	}
	let Some(desc) = library.try_get(chip_name) else { return false };
	desc.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(target) || chip_contains(library, &s.name, target, visited))
}

/// Whether dev-only builtins (`ChipType::is_dev_only`) are hidden from
/// player-facing lists in this build -- release builds hide them; debug
/// builds list them like any other chip.
pub(crate) fn dev_chips_hidden() -> bool {
	cfg!(not(debug_assertions))
}

/// Whether `name` belongs in a player-facing chip list (library panel,
/// bottom bar, search) in a build where dev-only chips are
/// `include_dev`-visible. Resolves the name through `library`, so a custom
/// chip shadowing a dev-only builtin stays listed (its type is `Custom`);
/// names with no library entry are left alone.
pub(crate) fn is_listed_in_build(library: &ChipLibrary, name: &str, include_dev: bool) -> bool {
	include_dev || !library.try_get(name).is_some_and(|d| d.chip_type.is_dev_only())
}

/// [`is_listed_in_build`] against the build actually running.
pub(crate) fn is_listed_in_current_build(library: &ChipLibrary, name: &str) -> bool {
	is_listed_in_build(library, name, !dev_chips_hidden())
}

/// Ensures `prefs.chip_collections` has an `OTHER` collection and that every chip in
/// `library` belongs to *some* collection, adding any stragglers to `OTHER` -- mirrors the
/// collection-syncing half of `ChipLibraryMenu.OnMenuOpened`.
pub(crate) fn sync_library_collections(prefs: &mut ProjectDescription, library: &ChipLibrary, unsaved_drafts: &std::collections::HashSet<String>) {
	sync_library_collections_gated(prefs, library, unsaved_drafts, !dev_chips_hidden())
}

fn sync_library_collections_gated(
	prefs: &mut ProjectDescription,
	library: &ChipLibrary,
	unsaved_drafts: &std::collections::HashSet<String>,
	include_dev: bool,
) {
	if !prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
		prefs.chip_collections.push(ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
	}
	let already_collected: std::collections::HashSet<String> =
		prefs.chip_collections.iter().flat_map(|c| c.chips.iter().map(|n| n.to_ascii_lowercase())).collect();
	let default_index =
		prefs.chip_collections.iter().position(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)).expect("just ensured above");

	let mut stray_names: Vec<String> = library
		.iter()
		.map(|d| d.name.clone())
		.filter(|n| {
			!already_collected.contains(&n.to_ascii_lowercase())
				&& !unsaved_drafts.contains(&n.to_ascii_lowercase())
				&& is_listed_in_build(library, n, include_dev)
		})
		.collect();
	stray_names.sort();
	prefs.chip_collections[default_index].chips.extend(stray_names);
}

/// Drops dev-only builtins (`ChipType::is_dev_only`) from a project's in-memory palette
/// bookkeeping -- every collection row plus any starred entries -- when running in a build
/// that hides them (release; see `dev_chips_hidden`). Projects saved from a debug build may
/// still carry those names on disk; pruning here (rather than at render time) keeps every row
/// index the UI and its click handlers agree on.
pub(crate) fn prune_hidden_chips_from_palette(prefs: &mut ProjectDescription, library: &ChipLibrary) {
	prune_hidden_chips_from_palette_gated(prefs, library, !dev_chips_hidden())
}

fn prune_hidden_chips_from_palette_gated(prefs: &mut ProjectDescription, library: &ChipLibrary, include_dev: bool) {
	if include_dev {
		return;
	}
	for collection in &mut prefs.chip_collections {
		collection.chips.retain(|n| is_listed_in_build(library, n, include_dev));
	}
	prefs.starred_list.retain(|item| item.is_collection || is_listed_in_build(library, &item.name, include_dev));
}

/// Resets whichever inline popup (new/rename collection, delete
/// confirmation) is open in the library panel, without leaving the
/// library itself -- mirrors `ChipLibraryMenu.ResetPopupState`.
pub(crate) fn reset_library_popup_state(v: &mut ViewerState) {
	v.library_creating_collection = false;
	v.library_renaming_collection = false;
	v.library_confirming_chip_delete = false;
	v.library_confirming_collection_delete = false;
	v.library_delete_message.clear();
	v.overlay_text_input.clear();
}

/// Names of every custom chip in `library` that directly contains
/// `chip_name` as one of its own sub-chips -- a name-only simplification
/// of `ChipLibrary.GetDirectParentChips`, enough to build a delete
/// warning without needing the full chip-dependency graph this port
/// doesn't otherwise build.
fn direct_parent_chip_names(library: &ChipLibrary, chip_name: &str) -> Vec<String> {
	library.iter().filter(|d| d.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(chip_name))).map(|d| d.name.clone()).collect()
}

/// Builds the chip-library DELETE confirmation message -- mirrors
/// `ChipLibraryMenu.CreateDeleteConfirmationMessage`, simplified to a
/// single wrapped paragraph (no coloured-by-severity variant, since
/// `editor_ui`'s confirmation panel doesn't distinguish one).
pub(crate) fn chip_delete_confirm_message(v: &ViewerState, chip_name: &str) -> String {
	let mut parents = direct_parent_chip_names(&v.library, chip_name);
	let used_in_current = v.library.get(&v.root_chip_name).sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(chip_name));
	if used_in_current {
		parents.retain(|p| !p.eq_ignore_ascii_case(&v.root_chip_name));
	}

	let mut message = if used_in_current {
		"Are you sure you want to delete the chip you are CURRENTLY EDITING? ".to_string()
	} else {
		"Are you sure you want to delete this chip? ".to_string()
	};

	let mut uses: Vec<String> = Vec::new();
	if used_in_current {
		uses.push("the current chip".to_string());
	}
	uses.extend(parents.iter().map(|p| format!("\"{p}\"")));

	match uses.len() {
		0 => message.push_str("It is not used anywhere."),
		1 => message.push_str(&format!("It is used by {}.", uses[0])),
		2 => message.push_str(&format!("It is used by {} and {}.", uses[0], uses[1])),
		n => message.push_str(&format!("It is used by {} and {} others.", uses[0], n - 1)),
	}

	message
}

/// Actually deletes chip `name` -- from disk (via `Saver::delete_chip`,
/// backed up into the project's `Deleted Chips/` folder), from every
/// collection that lists it, and from the starred list -- then drops it
/// from `v.library` and clears the library selection. Mirrors the
/// `isConfirmingChipDeletion` branch of `ChipLibraryMenu`'s DELETE
/// button.
pub(crate) fn delete_chip_from_library(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	// Resave every parent chip with this chip's instances (and their
	// wiring) stripped, before the file goes away -- `Project`'s
	// UpdateAndSaveAffectedChips willDelete pass.
	let parent_names: Vec<String> =
		v.library.iter().filter(|d| d.sub_chips.iter().any(|s| s.name.eq_ignore_ascii_case(name))).map(|d| d.name.clone()).collect();
	for parent_name in parent_names {
		let Ok(mut pristine) = crate::viewer::save_flow::load_single_chip_from_disk(paths, &v.project_name, &parent_name) else { continue };
		pristine.sub_chips.retain(|s| !s.name.eq_ignore_ascii_case(name));
		pristine.wires.retain(|w| {
			let owner_gone = |addr: crate::PinAddress| !pristine.sub_chips.iter().any(|s| s.id == addr.pin_owner_id);
			!owner_gone(w.source_pin_address) && !owner_gone(w.target_pin_address)
		});
		if let Err(e) = Saver::save_chip(paths, &v.project_name, &v.library, &pristine) {
			eprintln!("warning: failed to resave affected chip '{parent_name}': {e}");
			continue;
		}
		*v.library.get_mut(&parent_name) = pristine;
	}

	if let Err(e) = Saver::delete_chip(paths, &v.project_name, name, true) {
		*status = Some(format!("Failed to delete chip '{name}': {e}"));
		return;
	}
	for collection in &mut v.prefs.chip_collections {
		collection.chips.retain(|c| !c.eq_ignore_ascii_case(name));
	}
	v.prefs.set_starred(name, false, false);
	v.prefs.all_custom_chip_names.retain(|c| !c.eq_ignore_ascii_case(name));
	v.library.remove(name);
	v.mark_saved(name);
	v.library_selection = LibrarySelection::None;
}

/// Deletes the collection at `index` -- moves its chips into `OTHER`
/// (creating it first if somehow missing), drops its starred entry (if
/// any), then removes it from `prefs.chip_collections`. Mirrors
/// `ChipLibraryMenu.DeleteSelectedCollection`.
pub(crate) fn delete_collection(prefs: &mut ProjectDescription, index: usize) {
	if !prefs.chip_collections.iter().any(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME)) {
		prefs.chip_collections.push(ChipCollection::new(DEFAULT_LIBRARY_COLLECTION_NAME, Vec::<String>::new()));
	}
	let Some(collection) = prefs.chip_collections.get(index) else { return };
	let name = collection.name.clone();
	let chips = collection.chips.clone();

	if let Some(default_collection) = prefs
		.chip_collections
		.iter_mut()
		.find(|c| c.name.eq_ignore_ascii_case(DEFAULT_LIBRARY_COLLECTION_NAME) && !c.name.eq_ignore_ascii_case(&name))
	{
		default_collection.chips.extend(chips);
	}

	prefs.set_starred(&name, false, true);
	prefs.chip_collections.remove(index);
}

/// Moves whatever's selected in the library panel one step within its own list (`force_jump =
/// false`, mirrors the original's combined UP/DOWN buttons -- steps if it can, otherwise
/// falls back to a jump), or straight into the previous/next collection outright (`force_jump
/// = true`, mirrors the separate JUMP UP/DOWN buttons).
pub(crate) fn move_selected_library_row(v: &mut ViewerState, down: bool, force_jump: bool) {
	match v.library_selection {
		LibrarySelection::Chip(ci, chi) => {
			let len = v.prefs.chip_collections.get(ci).map(|c| c.chips.len()).unwrap_or(0);
			let can_step = if down { chi + 1 < len } else { chi > 0 };
			if can_step && !force_jump {
				let new_idx = if down { chi + 1 } else { chi - 1 };
				if let Some(c) = v.prefs.chip_collections.get_mut(ci) {
					c.chips.swap(chi, new_idx);
				}
				v.library_selection = LibrarySelection::Chip(ci, new_idx);
				return;
			}
			let target_ci = if down { Some(ci + 1) } else { ci.checked_sub(1) };
			let Some(target_ci) = target_ci else { return };
			if target_ci >= v.prefs.chip_collections.len() {
				return;
			}
			let Some(name) = v.prefs.chip_collections.get_mut(ci).map(|c| c.chips.remove(chi)) else { return };
			let target = &mut v.prefs.chip_collections[target_ci];
			target.is_toggled_open = true;
			let new_idx = if down { 0 } else { target.chips.len() };
			target.chips.insert(new_idx, name);
			v.library_selection = LibrarySelection::Chip(target_ci, new_idx);
		}
		LibrarySelection::Collection(ci) => {
			let len = v.prefs.chip_collections.len();
			let can_step = if down { ci + 1 < len } else { ci > 0 };
			if can_step {
				let new_idx = if down { ci + 1 } else { ci - 1 };
				v.prefs.chip_collections.swap(ci, new_idx);
				v.library_selection = LibrarySelection::Collection(new_idx);
			}
		}
		LibrarySelection::Starred(i) => {
			let len = v.prefs.starred_list.len();
			let can_step = if down { i + 1 < len } else { i > 0 };
			if can_step {
				let new_idx = if down { i + 1 } else { i - 1 };
				v.prefs.starred_list.swap(i, new_idx);
				v.library_selection = LibrarySelection::Starred(new_idx);
			}
		}
		LibrarySelection::None => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::SubChipDescription;
	use crate::json::StarredItem;
	use crate::structs::Vec2;
	use crate::{ChipDescription, ProjectDescription};

	fn lib_with_chips(names: &[&str]) -> ChipLibrary {
		let mut lib = ChipLibrary::new();
		for &name in names {
			lib.add(ChipDescription::new(name, ChipType::Custom));
		}
		lib
	}

	#[test]
	fn sync_library_collections_files_strays_under_other() {
		let mut prefs = ProjectDescription::default();
		let library = lib_with_chips(&["Alpha", "Beta"]);

		sync_library_collections(&mut prefs, &library, &std::collections::HashSet::new());

		assert!(prefs.chip_collections.iter().any(|c| c.name == "OTHER"));
		let other = prefs.chip_collections.iter().find(|c| c.name == "OTHER").unwrap();
		assert_eq!(other.chips.len(), 2);
	}

	/// Never-saved Ctrl+N drafts must stay out of the collections -- they
	/// only join the (persisted) library once actually saved with Ctrl+S.
	#[test]
	fn sync_library_collections_skips_unsaved_drafts() {
		let mut prefs = ProjectDescription::default();
		let library = lib_with_chips(&["Saved", "New Chip"]);
		let mut drafts = std::collections::HashSet::new();
		drafts.insert("new chip".to_string());

		sync_library_collections(&mut prefs, &library, &drafts);

		let other = prefs.chip_collections.iter().find(|c| c.name == "OTHER").unwrap();
		assert_eq!(other.chips, vec!["Saved".to_string()], "the draft stays out, the saved stray is filed");
	}

	fn dev_builtin(chip_type: ChipType) -> ChipDescription {
		ChipDescription::new(crate::builtins::name_for(chip_type), chip_type)
	}

	/// Release builds keep the dev-only builtins (`dev.RAM-8`, the bus
	/// termini -- see `ChipType::is_dev_only`) out of the stray-filing sync;
	/// debug builds list them like anything else.
	#[test]
	fn sync_library_collections_hides_dev_only_chips_when_hidden() {
		let mut library = lib_with_chips(&["Alpha"]);
		library.add(dev_builtin(ChipType::BusTerminus4Bit));
		let no_drafts = std::collections::HashSet::new();

		let mut release_prefs = ProjectDescription::default();
		sync_library_collections_gated(&mut release_prefs, &library, &no_drafts, false);
		let other = release_prefs.chip_collections.iter().find(|c| c.name == "OTHER").unwrap();
		assert_eq!(other.chips, vec!["Alpha".to_string()], "the dev-only chip isn't filed");

		let mut debug_prefs = ProjectDescription::default();
		sync_library_collections_gated(&mut debug_prefs, &library, &no_drafts, true);
		let other = debug_prefs.chip_collections.iter().find(|c| c.name == "OTHER").unwrap();
		assert_eq!(other.chips.len(), 2, "with dev chips visible, both strays are filed");
	}

	/// Opening a project saved from a debug build in a build that hides the
	/// dev-only builtins strips their rows from every collection and the
	/// starred list, leaving everything else exactly as it was.
	#[test]
	fn pruning_hidden_chips_cleans_collections_and_stars_without_touching_the_rest() {
		let (bus, terminus, dev_ram, custom) = (
			crate::builtins::name_for(ChipType::Bus1Bit),
			crate::builtins::name_for(ChipType::BusTerminus8Bit),
			crate::builtins::name_for(ChipType::DevRam8Bit),
			"MINE".to_string(),
		);
		let mut library = lib_with_chips(&[&custom]);
		library.add(dev_builtin(ChipType::Bus1Bit));
		library.add(dev_builtin(ChipType::BusTerminus8Bit));
		library.add(dev_builtin(ChipType::DevRam8Bit));

		let mut prefs = ProjectDescription::default();
		prefs.chip_collections.push(ChipCollection::new("BUS", vec![bus.clone(), terminus.clone()]));
		prefs.chip_collections.push(ChipCollection::new("MEMORY", vec![dev_ram.clone()]));
		prefs.set_starred(&dev_ram, true, false);
		prefs.set_starred("MY COLLECTION", true, true);
		let before = prefs.clone();

		prune_hidden_chips_from_palette_gated(&mut prefs, &library, false);

		let bus_collection = prefs.chip_collections.iter().find(|c| c.name == "BUS").unwrap();
		assert_eq!(bus_collection.chips, vec![bus], "the plain bus row survives, its terminus partner goes");
		let memory = prefs.chip_collections.iter().find(|c| c.name == "MEMORY").unwrap();
		assert!(memory.chips.is_empty(), "the dev RAM row goes");
		assert!(!prefs.is_starred(&dev_ram, false), "a hidden chip can't stay starred");
		assert!(prefs.is_starred("MY COLLECTION", true), "collection stars are untouched");

		// A build that lists dev chips prunes nothing at all.
		let mut debug_prefs = before.clone();
		prune_hidden_chips_from_palette_gated(&mut debug_prefs, &library, true);
		assert_eq!(debug_prefs, before, "with dev chips visible the palette is untouched");
	}

	#[test]
	fn sync_library_collections_is_idempotent() {
		let mut prefs = ProjectDescription::default();
		let library = lib_with_chips(&["Alpha"]);
		sync_library_collections(&mut prefs, &library, &std::collections::HashSet::new());
		let snapshot = prefs.chip_collections.clone();
		sync_library_collections(&mut prefs, &library, &std::collections::HashSet::new());
		assert_eq!(prefs.chip_collections.len(), snapshot.len());
	}

	#[test]
	fn would_create_cycle_detects_direct_and_transitive_self_containment() {
		let mut library = ChipLibrary::new();

		// A -> B -> C chain.
		let mut a = ChipDescription::new("A", ChipType::Custom);
		a.sub_chips.push(SubChipDescription {
			name: "B".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		library.add(a);
		let mut b = ChipDescription::new("B", ChipType::Custom);
		b.sub_chips.push(SubChipDescription {
			name: "C".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		library.add(b);
		library.add(ChipDescription::new("C", ChipType::Custom));

		// Placing A inside C closes C -> A -> B -> C.
		assert!(would_create_cycle(&library, "C", "A"));
		// Placing A inside B closes B -> A -> B too.
		assert!(would_create_cycle(&library, "B", "A"));
		// Placing B inside A is fine (A is above B in the chain).
		assert!(!would_create_cycle(&library, "A", "B"));
		// A chip is trivially a cycle with itself.
		assert!(would_create_cycle(&library, "a", "A"));
	}

	#[test]
	fn delete_collection_moves_chips_into_other_and_unstars() {
		let mut prefs = ProjectDescription::default();
		prefs.chip_collections.push(ChipCollection::new("MINE", vec!["X".to_string()]));
		prefs.set_starred("MINE", true, true);

		delete_collection(&mut prefs, 0);

		assert!(prefs.chip_collections.iter().all(|c| c.name != "MINE"));
		let other = prefs.chip_collections.iter().find(|c| c.name == "OTHER").expect("chips moved into OTHER");
		assert_eq!(other.chips, vec!["X".to_string()]);
		assert!(!prefs.is_starred("MINE", true));
	}

	#[test]
	fn chip_delete_confirm_message_lists_parents_and_current_use() {
		let mut library = ChipLibrary::new();
		let mut parent = ChipDescription::new("PARENT", ChipType::Custom);
		parent.sub_chips.push(SubChipDescription {
			name: "TARGET".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		library.add(parent);
		library.add(ChipDescription::new("TARGET", ChipType::Custom));

		let mut v = viewer_state_for_tests(library);
		let message = chip_delete_confirm_message(&v, "TARGET");
		assert!(message.contains("\"PARENT\""), "{message}");
		assert!(!message.contains("not used"));

		// Using TARGET in the currently-open chip adds the louder warning.
		let root = v.library.get_mut(&v.root_chip_name);
		root.sub_chips.push(SubChipDescription {
			name: "TARGET".into(),
			id: 1,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: Vec::new(),
		});
		let message = chip_delete_confirm_message(&v, "TARGET");
		assert!(message.contains("CURRENTLY EDITING"), "{message}");
	}

	/// Minimal stand-in `ViewerState` for the pure-message helpers:
	/// everything except the fields they read stays default.
	fn viewer_state_for_tests(library: ChipLibrary) -> ViewerState {
		let root_chip_name = "New Chip".to_string();
		let mut library = library;
		library.add(ChipDescription::new(&root_chip_name, ChipType::Custom));
		ViewerState::new("", library, root_chip_name, Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	#[test]
	fn starred_items_round_trip_through_set_and_is() {
		let mut prefs = ProjectDescription::default();
		prefs.set_starred("CHIP", true, false);
		prefs.starred_list.push(StarredItem { name: "COLL".to_string(), is_collection: true });
		assert!(prefs.is_starred("chip", false));
		assert!(prefs.is_starred("COLL", true));
		prefs.set_starred("CHIP", false, false);
		assert!(!prefs.is_starred("CHIP", false));
	}
}
