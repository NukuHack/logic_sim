//! Keyboard routing: converting winit modifier state into the
//! simulator's bitmask, and deciding -- per `ViewerState::stack`'s
//! keyboard target -- whether a key *press* is UI data (text fields,
//! key-select capture), an overlay/panel shortcut, or a plain-viewer
//! gesture.

use crate::render::editor_ui::{self, EditorAction, LibrarySelection, PrefValueField};
use crate::render::ui_stack::LayerId;
use crate::viewer::actions::{apply_editor_action, open_library_panel};
use crate::viewer::customize as customize_flow;
use crate::viewer::library::reset_library_popup_state;
use crate::viewer::popups::{apply_prefs_field_text, confirm_key_select_popup, confirm_naming_popup, confirm_rom_cell};
use crate::viewer::save_flow::{confirm_save_chip_popup, save_chip_mode, start_new_chip};
use crate::viewer::state::{close_all_overlays, close_top_overlay, open_preferences, open_save_chip, open_search, Overlay, ViewerState};
use crate::{sim, SavePaths, Saver};

/// Convert winit's modifier state into the `Simulator::key_modifiers`
/// bitmask (see `key_mods_bits`), using winit's own boolean accessors
/// rather than its raw `bits()` value -- see the doc comment on
/// `key_mods_bits` for why.
pub(crate) fn encode_modifiers(mods: winit::keyboard::ModifiersState) -> u32 {
	let mut bits = 0u32;
	if mods.shift_key() {
		bits |= sim::key_mods_bits::SHIFT;
	}
	if mods.control_key() {
		bits |= sim::key_mods_bits::CONTROL;
	}
	if mods.alt_key() {
		bits |= sim::key_mods_bits::ALT;
	}
	if mods.super_key() {
		bits |= sim::key_mods_bits::SUPER;
	}
	bits
}

/// What [`handle_viewer_key`] did with a key press routed to the viewer.
pub(crate) enum KeyOutcome {
	/// The press belonged to some overlay/panel/popup and was consumed.
	Consumed,
	/// Nothing wanted it anywhere in the stack -- plain-viewer gesture
	/// space fell through to Escape's "leave the chip editor" cascade.
	ReturnToMenu,
}

/// Routes a key *press* to whichever surface currently owns the keyboard,
/// per `ViewerState::stack.keyboard_target()` -- mirroring, guard-for-guard, the old
/// single match over `v.overlay` states this replaces. Typed characters are only UI data when a
/// text-field overlay owns focus; the app's key handler separately gates feeding them to Key chips on
/// `UiStack::keyboard_stop()`.
pub(crate) fn handle_viewer_key(
	v: &mut ViewerState,
	paths: &SavePaths,
	status: &mut Option<String>,
	event: &winit::event::KeyEvent,
	modifiers: winit::keyboard::ModifiersState,
) -> KeyOutcome {
	use winit::keyboard::{Key, NamedKey};
	match &event.logical_key {
		// ---- Text entry for whichever text-field overlay owns focus ----
		// The search popup deliberately types into its own `search_query`
		// buffer rather than the shared `overlay_text_input`, so a
		// collection-name field open underneath it (Library + Ctrl+F) keeps
		// its draft while the query comes and goes.
		Key::Named(NamedKey::Backspace) if v.stack.keyboard_target() == Some(LayerId::Search) => {
			v.search_query.pop();
		}
		Key::Named(NamedKey::Backspace)
			if matches!(v.stack.keyboard_target(), Some(LayerId::Naming | LayerId::RomEditor | LayerId::SaveChip))
				|| (matches!(v.stack.keyboard_target(), Some(LayerId::Library))
					&& (v.library_creating_collection || v.library_renaming_collection)) =>
		{
			v.overlay_text_input.pop();
		}
		Key::Named(NamedKey::Enter) if v.stack.keyboard_target() == Some(LayerId::Naming) => {
			confirm_naming_popup(v, status);
		}
		Key::Named(NamedKey::Enter)
			if v.stack.keyboard_target() == Some(LayerId::Library) && (v.library_creating_collection || v.library_renaming_collection) =>
		{
			apply_editor_action(v, paths, status, EditorAction::ConfirmCollectionName);
		}
		Key::Named(NamedKey::Enter) if v.stack.keyboard_target() == Some(LayerId::RomEditor) => {
			confirm_rom_cell(v, status);
		}
		Key::Named(NamedKey::Enter) if v.stack.keyboard_target() == Some(LayerId::KeySelect) && v.overlay_key_choice.is_some() => {
			confirm_key_select_popup(v, status);
		}
		// Enter only auto-confirms the unambiguous save-chip modes (a single "Save"/"Replace"
		// action) -- when both "Save As" and "Rename" are on offer, that choice needs a click.
		Key::Named(NamedKey::Enter)
			if v.stack.keyboard_target() == Some(LayerId::SaveChip)
				&& save_chip_mode(v, &v.overlay_text_input) != editor_ui::SaveChipMode::SaveAsOrRename =>
		{
			confirm_save_chip_popup(v, paths, status);
		}
		Key::Character(s) if v.stack.keyboard_target() == Some(LayerId::Search) => {
			if v.search_query.chars().count() < 64 {
				v.search_query.push_str(s);
			}
		}
		Key::Character(s)
			if matches!(v.stack.keyboard_target(), Some(LayerId::Naming | LayerId::SaveChip))
				|| (matches!(v.stack.keyboard_target(), Some(LayerId::Library))
					&& (v.library_creating_collection || v.library_renaming_collection)) =>
		{
			if v.overlay_text_input.chars().count() < 64 {
				v.overlay_text_input.push_str(s);
			}
		}
		// ROM cell values are short numbers -- a lower cap keeps a
		// stray paste from overflowing the little text field.
		Key::Character(s) if v.stack.keyboard_target() == Some(LayerId::RomEditor) => {
			if v.overlay_text_input.chars().count() < 10 {
				v.overlay_text_input.push_str(s);
			}
		}
		// The customizer's hex colour field: only hex digits and a leading
		// '#' get through, capped at "#RRGGBB" -- each accepted keystroke
		// re-parses into the draft colour immediately.
		Key::Character(s) if v.stack.keyboard_target() == Some(LayerId::CustomizePanel) => {
			let is_hexish = |c: char| c.is_ascii_hexdigit() || c == '#';
			if s.chars().all(is_hexish) && v.overlay_text_input.chars().count() < 7 {
				v.overlay_text_input.push_str(s);
				customize_flow::apply_hex_input(v);
			}
		}
		Key::Named(NamedKey::Backspace) if v.stack.keyboard_target() == Some(LayerId::CustomizePanel) => {
			v.overlay_text_input.pop();
			customize_flow::apply_hex_input(v);
		}
		Key::Named(NamedKey::Enter) if v.stack.keyboard_target() == Some(LayerId::CustomizePanel) => {
			customize_flow::confirm_customize(v, status);
		}
		Key::Named(NamedKey::Delete)
			if v.stack.keyboard_target() == Some(LayerId::CustomizePanel) && v.customize.as_ref().is_some_and(|c| c.interaction.is_active()) =>
		{
			customize_flow::delete_held_display(v);
		}
		// ---- Key-select overlay: capture the next alphanumeric key ----
		Key::Character(s) if v.stack.keyboard_target() == Some(LayerId::KeySelect) => {
			if let Some(c) = s.chars().next() {
				let upper = c.to_ascii_uppercase();
				if editor_ui::KEY_SELECT_ALLOWED_CHARS.contains(upper) {
					v.overlay_key_choice = Some(upper);
				}
			}
		}
		// ---- Preferences panel's numeric fields: digits only (mirrors
		// `PreferencesMenu.ValidateIntegerInput`), each edit re-parsed
		// straight onto the prefs so changes act live ----
		Key::Named(NamedKey::Backspace) if v.stack.keyboard_target() == Some(LayerId::Preferences) && v.prefs_field_focus.is_some() => {
			match v.prefs_field_focus {
				Some(PrefValueField::ClockSpeed) => {
					v.prefs_clock_text.pop();
				}
				Some(PrefValueField::TargetRate) => {
					v.prefs_rate_text.pop();
				}
				None => unreachable!("arm is gated on a focused field"),
			}
			apply_prefs_field_text(v);
		}
		Key::Character(s)
			if v.stack.keyboard_target() == Some(LayerId::Preferences) && v.prefs_field_focus.is_some() && prefs_field_accepts(v, s) =>
		{
			match v.prefs_field_focus {
				Some(PrefValueField::ClockSpeed) => v.prefs_clock_text.push_str(s),
				Some(PrefValueField::TargetRate) => v.prefs_rate_text.push_str(s),
				None => unreachable!("arm is gated on a focused field"),
			}
			apply_prefs_field_text(v);
		}
		// ---- Library panel keys (work while it has focus, even under another popup) ----
		Key::Named(NamedKey::Tab) if v.stack.keyboard_target() == Some(LayerId::Library) => {
			let mut desc = v.prefs.clone();
			if Saver::save_project_description(paths, &mut desc).is_ok() {
				v.prefs = desc;
			}
			close_all_overlays(v);
			v.library_selection = LibrarySelection::None;
		}
		Key::Named(NamedKey::Escape)
			if v.stack.keyboard_target() == Some(LayerId::Library)
				&& (v.library_creating_collection
					|| v.library_renaming_collection
					|| v.library_confirming_chip_delete
					|| v.library_confirming_collection_delete) =>
		{
			reset_library_popup_state(v);
		}
		// ---- Customize workspace: Escape cancels a grab/resize first,
		// only closing the workspace itself on the next press ----
		Key::Named(NamedKey::Escape)
			if v.stack.keyboard_target() == Some(LayerId::CustomizePanel) && v.customize.as_ref().is_some_and(|c| c.interaction.is_active()) =>
		{
			customize_flow::cancel_interaction(v);
		}
		Key::Named(NamedKey::Escape) if v.stack.keyboard_target().is_some_and(LayerId::is_overlay_panel) => {
			if v.stack.keyboard_target() == Some(LayerId::Library) {
				v.library_selection = LibrarySelection::None;
			}
			close_top_overlay(v);
		}
		// ---- Right-click popup: Escape dismisses it ----
		Key::Named(NamedKey::Escape) if v.context_menu.is_some() => v.context_menu = None,
		// ---- Normal viewer shortcuts (only while nothing owns the keyboard) ----
		Key::Character(s) if v.stack.keyboard_target().is_none() && s.eq_ignore_ascii_case("r") => v.rebuild_sim(),
		Key::Character(s) if v.stack.keyboard_target().is_none() && s.eq_ignore_ascii_case("f") => v.camera_fitted = !v.camera_fitted,
		// Toggle grid: the Ctrl+G form mirrors `KeyboardShortcuts.ToggleGridShortcutTriggered`
		// (works over open panels, like `PreferencesMenu.HandleKeyboardShortcuts`); plain 'g'
		// keeps working as this port's extra convenience. Both persist immediately when the
		// preferences panel isn't open, exactly like the original's save-on-toggle.
		Key::Character(s) if modifiers.control_key() && !typing_into_free_text_field(v) && s.eq_ignore_ascii_case("g") => {
			toggle_grid(v, paths);
		}
		Key::Character(s) if v.stack.keyboard_target().is_none() && s.eq_ignore_ascii_case("g") => toggle_grid(v, paths),
		Key::Character(s) if v.stack.keyboard_target().is_none() && s.eq_ignore_ascii_case("p") => open_preferences(v),
		Key::Character(s)
			if (v.stack.keyboard_target().is_none() || v.stack.keyboard_target() == Some(LayerId::Library))
				&& modifiers.control_key()
				&& s.eq_ignore_ascii_case("p") =>
		{
			open_preferences(v);
		}
		// Ctrl+Space toggles pause (`SimPauseToggleShortcutTriggered`); Space alone, while
		// paused and nothing owns the keyboard, advances a single step (`SimNextStepShortcutTriggered`,
		// which the original also only handles over the bare editor).
		Key::Named(NamedKey::Space) if modifiers.control_key() && !typing_into_free_text_field(v) => {
			v.toggle_sim_paused();
			persist_prefs_shortcut_change(v, paths);
		}
		Key::Named(NamedKey::Space) if v.stack.keyboard_target().is_none() && !modifiers.control_key() && v.prefs.prefs_sim_paused => {
			v.request_single_sim_step();
		}
		Key::Character(s)
			if (v.stack.keyboard_target().is_none() || v.stack.keyboard_target() == Some(LayerId::Library))
				&& modifiers.control_key()
				&& s.eq_ignore_ascii_case("f") =>
		{
			open_search(v);
		}
		Key::Character(s)
			if (v.stack.keyboard_target().is_none() || v.stack.keyboard_target() == Some(LayerId::Library))
				&& modifiers.control_key()
				&& s.eq_ignore_ascii_case("s") =>
		{
			open_save_chip(v);
		}
		Key::Character(s)
			if (v.stack.keyboard_target().is_none() || v.stack.keyboard_target() == Some(LayerId::Library))
				&& modifiers.control_key()
				&& s.eq_ignore_ascii_case("n") =>
		{
			start_new_chip(v, paths, status);
		}
		Key::Named(NamedKey::Tab) if v.stack.keyboard_target().is_none() => {
			open_library_panel(v);
		}
		// ---- Escape cascade, top-most thing first: popup state > whole
		// overlay > pending wire/chip > bottom-bar flyout > leave the editor ----
		Key::Named(NamedKey::Escape) => {
			if v.pending_wire.is_some() || v.pending_place.is_some() {
				v.pending_wire = None;
				v.pending_place = None;
			} else if v.bottom_bar_open_collection.is_some() {
				v.bottom_bar_open_collection = None;
			} else {
				return KeyOutcome::ReturnToMenu;
			}
		}
		_ => {}
	}
	KeyOutcome::Consumed
}

/// Applies a grid toggle and persists it straight to disk when the
/// preferences panel isn't open (mirroring `HandleKeyboardShortcuts`:
/// in-menu edits are saved by the panel's Confirm, others save at once).
fn toggle_grid(v: &mut ViewerState, paths: &SavePaths) {
	v.toggle_grid_display();
	persist_prefs_shortcut_change(v, paths);
}

/// Saves the current prefs when the preferences panel isn't open; while it
/// is, the panel shows live state already and its Apply does the saving.
fn persist_prefs_shortcut_change(v: &mut ViewerState, paths: &SavePaths) {
	if v.overlays.contains(&Overlay::Preferences) {
		return;
	}
	let mut desc = v.prefs.clone();
	if Saver::save_project_description(paths, &mut desc).is_ok() {
		v.prefs = desc;
	}
}

/// Whether a *free-text* field currently owns typing (so Ctrl+Space/Ctrl+G
/// must act as ordinary keystrokes rather than shortcuts). The preferences
/// panel's numeric fields are deliberately excluded -- they accept digits
/// only, and `PreferencesMenu.HandleKeyboardShortcuts` ran unconditionally
/// over every menu, so pause/grid toggling keeps working while one of them
/// is focused.
fn typing_into_free_text_field(v: &ViewerState) -> bool {
	match v.stack.keyboard_target() {
		Some(LayerId::Naming | LayerId::RomEditor | LayerId::SaveChip | LayerId::Search | LayerId::KeySelect) => true,
		Some(LayerId::Library) => v.library_creating_collection || v.library_renaming_collection,
		_ => false,
	}
}

/// Whether `s` may be appended to whichever prefs numeric field is focused:
/// digits only (`ValidateIntegerInput` rejects anything non-integer), capped
/// at 9 digits so the value always parses cleanly into an `i32`.
fn prefs_field_accepts(v: &ViewerState, s: &str) -> bool {
	if !s.chars().all(|c| c.is_ascii_digit()) {
		return false;
	}
	let len = match v.prefs_field_focus {
		Some(PrefValueField::ClockSpeed) => v.prefs_clock_text.chars().count(),
		Some(PrefValueField::TargetRate) => v.prefs_rate_text.chars().count(),
		None => return false,
	};
	len < 9
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn encode_modifiers_maps_each_winit_flag_to_its_sim_bit() {
		use sim::key_mods_bits;
		use winit::keyboard::ModifiersState;

		assert_eq!(encode_modifiers(ModifiersState::empty()), 0);
		assert_eq!(encode_modifiers(ModifiersState::SHIFT), key_mods_bits::SHIFT);
		assert_eq!(encode_modifiers(ModifiersState::CONTROL), key_mods_bits::CONTROL);
		assert_eq!(encode_modifiers(ModifiersState::ALT), key_mods_bits::ALT);
		assert_eq!(encode_modifiers(ModifiersState::SUPER), key_mods_bits::SUPER);

		let combo = ModifiersState::SHIFT | ModifiersState::CONTROL;
		assert_eq!(encode_modifiers(combo), key_mods_bits::SHIFT | key_mods_bits::CONTROL);
	}
}
