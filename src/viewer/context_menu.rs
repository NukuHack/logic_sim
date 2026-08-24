//! Right-click popup handling: parsing the popup's opaque target string
//! back into a concrete thing, building the per-type row lists, and
//! applying whichever row the player clicked onto live state.

use crate::render::context_menu::{ContextMenuAction, ContextMenuItem};
use crate::render::editor_ui::LibrarySelection;
use crate::viewer::canvas::delete_component;
use crate::viewer::library::{chip_delete_confirm_message, is_custom_chip};
use crate::viewer::save_flow::request_open_chip;
use crate::viewer::state::{open_overlay, KeySelectPurpose, NamingPurpose, Overlay, PinEditState, RomEditorState, ViewerState};
use crate::{ChipLibrary, ChipType, SavePaths, Saver};

/// One right-clickable "thing" a context menu can be attached to, parsed
/// back out of `ContextMenuState::target` (kept as a plain string by that
/// module so it stays generic -- see its docs). `id`s below are always
/// scoped to the *current root chip* (`v.root_chip_name`): a subchip's
/// own `SubChipDescription::id`, or a boundary dev-pin's `PinDescription::id`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ContextTarget {
	/// A placed subchip instance on the canvas.
	Component(i32),
	/// One of the *current root chip's own* boundary dev-pins -- never a
	/// subchip's pin (the brief is explicit about that distinction).
	DevPin { is_input: bool, id: i32 },
	/// A row in the chip library sidebar, by chip name.
	LibChip(String),
	/// A plain chip's own button directly in the starred bottom bar (not
	/// one listed inside a collection's flyout) -- by chip name. Distinct
	/// from `FlyoutChip` only in which right-click rows it's offered
	/// (this one also gets "Un-star"; see the right-click handler).
	BarChip(String),
	/// A chip row inside an *open collection's* flyout
	/// (`build_starred_collection_popup`), by chip name.
	FlyoutChip(String),
}

/// Constructor for one of the context targets that just wraps a plain
/// name string (see [`ContextTarget::parse`]).
type PlainTargetCtor = fn(String) -> ContextTarget;

impl ContextTarget {
	/// Inverse of however the right-click handler built the `target`
	/// string in the first place -- kept next to that so the two stay in
	/// sync.
	pub(crate) fn parse(target: &str) -> Option<Self> {
		if let Some(rest) = target.strip_prefix("component:") {
			rest.parse().ok().map(ContextTarget::Component)
		} else if let Some(rest) = target.strip_prefix("devpin:in:") {
			rest.parse().ok().map(|id| ContextTarget::DevPin { is_input: true, id })
		} else if let Some(rest) = target.strip_prefix("devpin:out:") {
			rest.parse().ok().map(|id| ContextTarget::DevPin { is_input: false, id })
		} else {
			const PLAIN_TARGETS: [(&str, PlainTargetCtor); 3] =
				[("libchip:", ContextTarget::LibChip), ("barchip:", ContextTarget::BarChip), ("flyoutchip:", ContextTarget::FlyoutChip)];
			PLAIN_TARGETS.iter().find_map(|(prefix, wrap)| target.strip_prefix(prefix).map(|rest| wrap(rest.to_string())))
		}
	}
}

/// Builds the row list for a right-click popup opened on a placed
/// subchip of type `chip_name` -- shared by the canvas-component and (for
/// "Open"'s enabled state) library-row cases so the two stay consistent.
/// Every component gets "Label"; "Configure" is only offered for the
/// handful of chip types that actually have configurable
/// `internal_data` (see `NamingPurpose`/`KeySelectPurpose`'s docs for
/// what each one edits); "Open"/"Delete" are canvas-only (a library row
/// has no wires to cascade-delete and *is* the definition, not an
/// instance of it, so there's nothing to "open" beyond switching to it).
pub(crate) fn context_menu_items_for_component(library: &ChipLibrary, chip_name: &str) -> Vec<ContextMenuItem> {
	let mut items = vec![ContextMenuItem::new_enabled("Open", ContextMenuAction::Open, is_custom_chip(library, chip_name))];
	items.push(ContextMenuItem::new("Label", ContextMenuAction::Label));
	let chip_type = library.try_get(chip_name).map(|d| d.chip_type);
	if matches!(chip_type, Some(ChipType::Pulse) | Some(ChipType::Key) | Some(ChipType::Rom256x16)) {
		items.push(ContextMenuItem::new("Configure", ContextMenuAction::Configure));
	}
	if chip_type.unwrap_or_default().is_bus_type() {
		items.push(ContextMenuItem::new("Flip", ContextMenuAction::Flip));
	}
	items.push(ContextMenuItem::new("Delete", ContextMenuAction::Delete));
	items
}

/// Un-stars `name` (a plain chip, never a collection -- see
/// [`ContextTarget::BarChip`]'s docs) from the right-click popup on its own
/// bottom-bar button, and immediately persists the change. Unlike
/// `EditorAction::ToggleStarred` (which only mutates `v.prefs` in memory,
/// relying on the library overlay's own exit/Tab handling to save when
/// the player eventually leaves it), this has no such exit event to
/// piggyback on -- the bottom bar is usable with the library closed --
/// so it saves right away, the same way `EditorAction::PlaceChip` does
/// for the same reason.
fn unstar_bottom_bar_chip(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, name: &str) {
	v.prefs.set_starred(name, false, false);
	let mut desc = v.prefs.clone();
	if let Err(e) = Saver::save_project_description(paths, &mut desc) {
		*status = Some(format!("Failed to save chip library: {e}"));
	} else {
		v.prefs = desc;
	}
}

/// Applies a click on the currently-open right-click popup (see
/// `render::context_menu`) -- `target` is whatever `state.target` was set
/// to when the popup was opened (parsed back via [`ContextTarget::parse`]),
/// `action_id` is the clicked row's `ContextMenuItem::id`.
pub(crate) fn apply_context_menu_action(
	v: &mut ViewerState,
	paths: &SavePaths,
	status: &mut Option<String>,
	target: &str,
	action_id: ContextMenuAction,
) {
	let Some(parsed) = ContextTarget::parse(target) else { return };
	let root_chip_name = v.root_chip_name.clone();

	match (action_id, parsed) {
		(ContextMenuAction::Open, ContextTarget::Component(id)) => {
			let name = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).map(|s| s.name.clone());
			if let Some(name) = name {
				request_open_chip(v, paths, status, &name, false);
			}
		}
		(ContextMenuAction::Open, ContextTarget::LibChip(name)) => {
			request_open_chip(v, paths, status, &name, true);
		}
		(ContextMenuAction::Open, ContextTarget::BarChip(name)) | (ContextMenuAction::Open, ContextTarget::FlyoutChip(name)) => {
			request_open_chip(v, paths, status, &name, false);
		}
		(ContextMenuAction::Unstar, ContextTarget::BarChip(name)) => unstar_bottom_bar_chip(v, paths, status, &name),
		(ContextMenuAction::Delete, ContextTarget::LibChip(name)) => {
			v.library_delete_message = chip_delete_confirm_message(v, &name);
			v.library_confirming_chip_delete = true;
			// Right-click delete has no row selected yet (only a name), so
			// stash it as a `Chip` selection the confirmation can read back
			// from -- find where it actually lives in the collections list.
			for (ci, c) in v.prefs.chip_collections.iter().enumerate() {
				if let Some(chi) = c.chips.iter().position(|n| n.eq_ignore_ascii_case(&name)) {
					v.library_selection = LibrarySelection::Chip(ci, chi);
					break;
				}
			}
		}

		(ContextMenuAction::Label, ContextTarget::Component(id)) => {
			let current = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).and_then(|s| s.label.clone()).unwrap_or_default();
			open_overlay(v, Overlay::Naming);
			v.overlay_text_input = current;
			v.naming_purpose = NamingPurpose::LabelComponent(id);
		}
		(ContextMenuAction::Flip, ContextTarget::Component(id)) => {
			if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
				let mut data = sub.internal_data.clone().unwrap_or_default();
				if data.len() < 2 {
					data.resize(2, 0);
				}
				data[1] ^= 1;
				sub.internal_data = Some(data);
			}
			v.rebuild_sim();
		}
		// The dev-pin "Edit" row (`PinEditMenu`): rename +, for multi-bit
		// pins, the Decimal Display wheel. Supersedes this port's old
		// label-only naming popup for pins.
		(ContextMenuAction::Configure, ContextTarget::DevPin { is_input, id }) => {
			let chip = v.library.get(&root_chip_name);
			let pins = if is_input { &chip.input_pins } else { &chip.output_pins };
			// Copy the draft's seeds out of the library first so the
			// immutable borrow ends before the overlay opens.
			let draft = pins.iter().find(|p| p.id == id).map(|p| (p.name.clone(), p.value_display_mode.to_int().max(0) as usize));
			if let Some((current_name, display_mode_index)) = draft {
				open_overlay(v, Overlay::PinEdit);
				v.overlay_text_input = current_name;
				v.pin_edit = Some(PinEditState { is_input, pin_id: id, display_mode_index });
			}
		}
		(ContextMenuAction::Delete, ContextTarget::DevPin { id, .. }) => delete_component(v, id),

		(ContextMenuAction::Configure, ContextTarget::Component(id)) => {
			let sub_chip_name = v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).map(|s| s.name.clone());
			let chip_type = sub_chip_name.as_deref().and_then(|n| v.library.try_get(n)).map(|d| d.chip_type);
			let internal_data =
				v.library.get(&root_chip_name).sub_chips.iter().find(|s| s.id == id).and_then(|s| s.internal_data.clone()).unwrap_or_default();
			match chip_type {
				Some(ChipType::Pulse) => {
					open_overlay(v, Overlay::Naming);
					v.overlay_text_input = internal_data.first().copied().unwrap_or(0).to_string();
					v.naming_purpose = NamingPurpose::ConfigurePulseDuration(id);
				}
				Some(ChipType::Key) => {
					open_overlay(v, Overlay::KeySelect);
					v.overlay_key_choice = internal_data.first().map(|&code| code as u8 as char);
					v.key_select_purpose = KeySelectPurpose::ConfigureKeyChar(id);
				}
				Some(ChipType::Rom256x16) => {
					let mut data = internal_data;
					data.resize(crate::render::editor_ui::ROM_WORD_COUNT, 0);
					open_overlay(v, Overlay::RomEditor);
					v.overlay_text_input = data[0].to_string();
					v.rom_editor = Some(RomEditorState { component_id: id, data, selected: 0 });
				}
				_ => {}
			}
		}

		(ContextMenuAction::Delete, ContextTarget::Component(id)) => delete_component(v, id),

		_ => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn context_target_parse_round_trips_every_prefix() {
		assert_eq!(ContextTarget::parse("component:42"), Some(ContextTarget::Component(42)));
		assert_eq!(ContextTarget::parse("devpin:in:7"), Some(ContextTarget::DevPin { is_input: true, id: 7 }));
		assert_eq!(ContextTarget::parse("devpin:out:9"), Some(ContextTarget::DevPin { is_input: false, id: 9 }));
		assert_eq!(ContextTarget::parse("libchip:XOR"), Some(ContextTarget::LibChip("XOR".into())));
		assert_eq!(ContextTarget::parse("barchip:AND"), Some(ContextTarget::BarChip("AND".into())));
		assert_eq!(ContextTarget::parse("flyoutchip:OR"), Some(ContextTarget::FlyoutChip("OR".into())));
		assert_eq!(ContextTarget::parse("unknown:1"), None);
		assert_eq!(ContextTarget::parse("component:notanumber"), None);
	}

	#[test]
	fn component_items_offer_configure_only_for_configurable_types() {
		let mut library = ChipLibrary::new();
		crate::builtins::register_all(&mut library);

		let nand_items = context_menu_items_for_component(&library, "NAND");
		assert!(!nand_items.iter().any(|i| matches!(i.id, ContextMenuAction::Configure)));
		assert!(!nand_items.first().expect("Open row").enabled, "builtins can't be opened");

		let pulse_items = context_menu_items_for_component(&library, "Pulse");
		assert!(pulse_items.iter().any(|i| matches!(i.id, ContextMenuAction::Configure)));

		let bus_items = context_menu_items_for_component(&library, "BUS-4");
		assert!(bus_items.iter().any(|i| matches!(i.id, ContextMenuAction::Flip)));
	}

	fn viewer_with_output_pin(bit_count: crate::PinBitCount, mode: crate::ValueDisplayMode) -> ViewerState {
		use crate::ChipDescription;
		let mut library = ChipLibrary::new();
		let mut chip = ChipDescription::new("ROOT", ChipType::Custom);
		let mut pin = crate::PinDescription::new("DATA", 4, bit_count);
		pin.value_display_mode = mode;
		chip.output_pins.push(pin);
		library.add(chip);
		ViewerState::new("", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state())
	}

	/// Right-click "Edit" on a boundary dev-pin opens the pin-edit popup
	/// pre-seeded from the pin: the name draft in the shared buffer and
	/// the Decimal Display wheel on the pin's current mode.
	#[test]
	fn devpin_configure_opens_pin_edit_seeded_from_the_pin() {
		let root = std::env::temp_dir().join(format!("devpin_cfg_{}", std::process::id()));
		let paths = SavePaths::new(&root);
		let mut v = viewer_with_output_pin(crate::PinBitCount::Bit8, crate::ValueDisplayMode::Hex);

		apply_context_menu_action(&mut v, &paths, &mut None, "devpin:out:4", ContextMenuAction::Configure);

		assert_eq!(v.overlays, vec![Overlay::PinEdit], "the popup opened on top");
		assert_eq!(v.overlay_text_input, "DATA", "the name field starts from the pin's name");
		assert_eq!(
			v.pin_edit,
			Some(PinEditState { is_input: false, pin_id: 4, display_mode_index: crate::ValueDisplayMode::Hex as usize }),
			"the wheel starts from the pin's current mode"
		);

		let _ = std::fs::remove_dir_all(&root);
	}

	/// An id that doesn't resolve (stale popup target) opens nothing.
	#[test]
	fn devpin_configure_ignores_unknown_pin_ids() {
		let root = std::env::temp_dir().join(format!("devpin_cfg_miss_{}", std::process::id()));
		let paths = SavePaths::new(&root);
		let mut v = viewer_with_output_pin(crate::PinBitCount::Bit1, crate::ValueDisplayMode::None);

		apply_context_menu_action(&mut v, &paths, &mut None, "devpin:out:999", ContextMenuAction::Configure);

		assert!(v.overlays.is_empty() && v.pin_edit.is_none());

		let _ = std::fs::remove_dir_all(&root);
	}

	/// The bar-chip popup's rows must work even for a *greyed-out*
	/// button: the grey only blocks left-click placement, while Open
	/// switches to the chip and Un-star removes and persists it.
	#[test]
	fn barchip_popup_rows_act_on_cycle_blocked_chips() {
		use crate::viewer::chip_interaction;

		let root = std::env::temp_dir().join(format!("barchip_greyed_{}", std::process::id()));
		let paths = SavePaths::new(&root);
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		library.add(crate::ChipDescription::new("ROOT", ChipType::Custom));
		// SELFIE contains an instance of ROOT -> placing it into ROOT
		// would recurse, which is what greys its bar button out.
		let mut selfie = crate::ChipDescription::new("SELFIE", ChipType::Custom);
		selfie_subchip_root(&mut selfie);
		library.add(selfie);
		let mut v =
			ViewerState::new("P", library, "ROOT".to_string(), crate::structs::Vec2::new(1280.0, 800.0), crate::audio::default_shared_state());
		v.prefs.starred_list.push(crate::StarredItem::new("SELFIE", false));
		Saver::save_chip(&paths, "P", &v.library.get("SELFIE")).expect("chip saved");
		register_name_in_project_for_test(&mut v, &paths, "SELFIE");

		// Placing SELFIE into ROOT would recurse -- that's why it's grey.
		assert!(crate::viewer::library::would_create_cycle(&v.library, "ROOT", "SELFIE"));

		// Un-star via the popup: gone from the starred list and persisted.
		apply_context_menu_action(&mut v, &paths, &mut None, "barchip:SELFIE", ContextMenuAction::Unstar);
		assert!(!v.prefs.is_starred("SELFIE", false), "un-starring works on a greyed chip");

		// Re-star, then Open via the popup: the viewer switches despite
		// the cycle block (navigating away is always legal).
		v.prefs.set_starred("SELFIE", true, false);
		apply_context_menu_action(&mut v, &paths, &mut None, "barchip:SELFIE", ContextMenuAction::Open);
		assert_eq!(v.root_chip_name, "SELFIE", "open works on a greyed chip");
		assert!(chip_interaction::CanvasInteraction::None == v.canvas_interaction);

		let _ = std::fs::remove_dir_all(&root);
	}

	/// Persists `name` into the project's collections so the un-star save
	/// round-trips like a real session's description.
	fn register_name_in_project_for_test(v: &mut ViewerState, paths: &SavePaths, name: &str) {
		v.prefs.all_custom_chip_names.push(name.to_string());
		let mut desc = v.prefs.clone();
		Saver::save_project_description(paths, &mut desc).expect("description saved");
		v.prefs = desc;
	}

	/// Gives `selfie` a subchip instance of ROOT (making it
	/// cycle-blocked when ROOT is the open chip).
	fn selfie_subchip_root(selfie: &mut crate::ChipDescription) {
		selfie.sub_chips.push(crate::SubChipDescription {
			name: "ROOT".into(),
			id: 1,
			internal_data: None,
			position: crate::Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
	}
}
