//! Confirm handlers for the generic popups: the naming popup (project
//! rename, component/pin labels, pulse length), the ROM cell editor, and
//! the key-select popup -- each shared by its popup's Confirm *button*
//! and pressing Enter directly, so the two input paths can't drift
//! apart.

use crate::render::editor_ui;
use crate::viewer::state::{close_top_overlay, KeySelectPurpose, NamingPurpose, ViewerState};

/// Advances the wheel field at `row_index` (matching the row order
/// `editor_ui::build_preferences_panel` draws in) to its next option,
/// wrapping around.
pub(crate) fn cycle_pref(prefs: &mut crate::json::ProjectDescription, row_index: usize) {
	match row_index {
		0 => prefs.prefs_main_pin_names_display_mode = (prefs.prefs_main_pin_names_display_mode + 1) % 3,
		1 => prefs.prefs_chip_pin_names_display_mode = (prefs.prefs_chip_pin_names_display_mode + 1) % 3,
		2 => prefs.prefs_grid_display_mode = (prefs.prefs_grid_display_mode + 1) % 2,
		3 => prefs.prefs_snapping = (prefs.prefs_snapping + 1) % 3,
		4 => prefs.prefs_straight_wires = (prefs.prefs_straight_wires + 1) % 3,
		5 => prefs.prefs_sim_paused = !prefs.prefs_sim_paused,
		_ => {}
	}
}

/// Re-parses both numeric draft fields onto the live prefs, mirroring
/// `PreferencesMenu.DrawMenu`'s "assign changes immediately so can see
/// them take effect in background": `int.TryParse` semantics -- anything
/// unparseable counts as 0 (the target rate is clamped back up to >= 1
/// where it's consumed, via `ViewerState::target_ticks_per_second`).
/// Called after every keystroke into either field and by Apply.
pub(crate) fn apply_prefs_field_text(v: &mut ViewerState) {
	v.prefs.prefs_sim_steps_per_clock_tick = v.prefs_clock_text.parse::<i32>().unwrap_or(0);
	v.prefs.prefs_sim_target_steps_per_second = v.prefs_rate_text.parse::<i32>().unwrap_or(0);
}

/// Applies whatever's typed into `Overlay::Naming`'s text field, per
/// `v.naming_purpose` -- shared by the popup's Confirm button
/// (`EditorAction::ConfirmName`) and pressing Enter directly. Always
/// closes the popup and resets `naming_purpose` back to its default
/// afterwards, success or not.
pub(crate) fn confirm_naming_popup(v: &mut ViewerState, status: &mut Option<String>) {
	let trimmed = v.overlay_text_input.trim().to_string();
	let root_chip_name = v.root_chip_name.clone();

	match v.naming_purpose {
		NamingPurpose::RenameProject => {
			if !trimmed.is_empty() {
				v.project_name = trimmed;
			}
		}
		NamingPurpose::LabelComponent(id) => {
			if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
				sub.label = if trimmed.is_empty() { None } else { Some(trimmed) };
			}
		}
		NamingPurpose::LabelDevPin { is_input, id } => {
			let chip = v.library.get_mut(&root_chip_name);
			let pins = if is_input { &mut chip.input_pins } else { &mut chip.output_pins };
			if let Some(pin) = pins.iter_mut().find(|p| p.id == id) {
				if !trimmed.is_empty() {
					pin.name = trimmed;
				}
			}
		}
		NamingPurpose::ConfigurePulseDuration(id) => match trimmed.parse::<u32>() {
			Ok(ticks) => {
				if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
					// `Simulator::process_builtin_chip`'s `Pulse` arm indexes `internal_state` at three
					// fixed slots -- `[DURATION, TICKS_REMAINING, INPUT_OLD]`. Changing the configured
					// length also resets any in-flight pulse and forgets the last sampled input edge.
					sub.internal_data = Some(vec![ticks, 0, 0]);
				}
				v.rebuild_sim();
			}
			Err(_) => *status = Some("Pulse length must be a whole number of ticks".to_string()),
		},
	}

	close_top_overlay(v);
}

/// Parses a single ROM cell value, same rule as the old comma-list
/// editor: a leading `0x`/`0X` means hex, otherwise decimal.
fn parse_rom_word(text: &str) -> Option<u32> {
	let text = text.trim();
	if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
		u32::from_str_radix(hex, 16).ok()
	} else {
		text.parse::<u32>().ok()
	}
}

/// Commits `v.overlay_text_input` into the currently-selected cell of
/// the open ROM editor (`EditorAction::RomConfirmCell`), then advances
/// selection to the next cell (wrapping) and loads *its* value into the
/// text field -- lets the player type several values in a row without
/// re-clicking between each one. A parse failure leaves the selection
/// and text field untouched (so the player can just fix their typo)
/// rather than silently discarding it.
pub(crate) fn confirm_rom_cell(v: &mut ViewerState, status: &mut Option<String>) {
	let Some(editor) = v.rom_editor.as_mut() else { return };
	match parse_rom_word(&v.overlay_text_input) {
		Some(value) => {
			if let Some(cell) = editor.data.get_mut(editor.selected) {
				*cell = value;
			}
			editor.selected = (editor.selected + 1) % editor_ui::ROM_WORD_COUNT;
			v.overlay_text_input = editor.data[editor.selected].to_string();
		}
		None => *status = Some("ROM cell value must be a number (decimal or 0x hex)".to_string()),
	}
}

/// Writes the ROM editor's whole draft buffer back onto the subchip
/// (`EditorAction::RomApply`) and closes the popup. Any value still only
/// sitting in the text field (typed but not yet committed via "Set"/Enter)
/// is committed first, so clicking straight from typing to "Apply" isn't
/// a silent no-op for that last cell.
pub(crate) fn apply_rom_editor(v: &mut ViewerState, status: &mut Option<String>) {
	confirm_rom_cell(v, status);
	if let Some(editor) = v.rom_editor.take() {
		let root_chip_name = v.root_chip_name.clone();
		if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == editor.component_id) {
			sub.internal_data = Some(editor.data);
		}
		v.rebuild_sim();
	}
	close_top_overlay(v);
}

/// Applies whatever's chosen in `Overlay::KeySelect`, per
/// `v.key_select_purpose` -- shared by the popup's Confirm button
/// (`EditorAction::ConfirmKey`) and pressing Enter directly, mirroring
/// `confirm_naming_popup`.
pub(crate) fn confirm_key_select_popup(v: &mut ViewerState, status: &mut Option<String>) {
	if let Some(c) = v.overlay_key_choice {
		match v.key_select_purpose {
			KeySelectPurpose::Rebind => {
				// No actual keybind system exists to rebind yet -- this
				// just reports the choice back so the popup is usable
				// and testable end-to-end ahead of that being wired up.
				*status = Some(format!("Key '{c}' chosen (not yet wired to an action)"));
			}
			KeySelectPurpose::ConfigureKeyChar(id) => {
				let root_chip_name = v.root_chip_name.clone();
				if let Some(sub) = v.library.get_mut(&root_chip_name).sub_chips.iter_mut().find(|s| s.id == id) {
					sub.internal_data = Some(vec![c as u32]);
				}
				v.rebuild_sim();
			}
		}
	}
	close_top_overlay(v);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_rom_word_accepts_decimal_and_hex() {
		assert_eq!(parse_rom_word("42"), Some(42));
		assert_eq!(parse_rom_word(" 42 "), Some(42));
		assert_eq!(parse_rom_word("0xFF"), Some(255));
		assert_eq!(parse_rom_word("0X10"), Some(16));
		assert_eq!(parse_rom_word(""), None);
		assert_eq!(parse_rom_word("nope"), None);
	}

	#[test]
	fn cycle_pref_wraps_each_wheel_row_around_its_own_option_count() {
		let mut prefs = crate::json::ProjectDescription::default();

		cycle_pref(&mut prefs, 2); // grid: Off -> On
		assert_eq!(prefs.prefs_grid_display_mode, 1);
		cycle_pref(&mut prefs, 2); // On -> Off
		assert_eq!(prefs.prefs_grid_display_mode, 0);

		prefs.prefs_sim_paused = false;
		cycle_pref(&mut prefs, 5);
		assert!(prefs.prefs_sim_paused);

		cycle_pref(&mut prefs, 99); // out of range: no-op
	}
}
