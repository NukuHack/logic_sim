//! Builds drawable geometry + clickable hit-boxes for the in-editor overlays that sit on top of the
//! chip viewer, ported from the corresponding `DLS.Graphics.*Menu` classes. Same philosophy as
//! [`crate::render::menu_ui`]: plain data in, plain data out, no wgpu types, fully unit-testable
//! without a GPU. Holds no simulation or save-system logic of its own -- only builds frames to
//! draw/hit-test for the preferences, chip library, search/naming, key-select, ROM-editor, and save-chip popups.

use crate::json::ChipCollection;
use crate::json::ProjectDescription;
use crate::render::menu_ui::UiRect;
use crate::render::scene::TextLabel;
use crate::render::theme;
use crate::structs::Vec2;

pub use crate::render::menu_ui::to_world;

/// Something a click on one of these overlays should cause the host app
/// to do. Mirrors a UI-level view of the corresponding menu's behaviour,
/// analogous to `menu_ui::UiAction` but for the editor-side overlays.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorAction {
	ClosePopup,
	/// Preferences: cycle the wheel field at this index (0-based, in the
	/// order the panel draws them) to its next option.
	CyclePref(usize),
	ApplyPreferences,
	/// Chip library: highlight a row (by name). Purely a selection --
	/// the host no longer opens the chip just from this click; opening
	/// happens via that same row's right-click "Open" popup instead (see
	/// `render::context_menu`).
	SelectChip(String),
	/// Chip library: toggle a collection's open/closed state (by index
	/// into the collections slice passed to [`build_chip_library_panel`]).
	ToggleCollection(usize),
	/// Search popup: pick a chip from the filtered results (by name).
	UseChip(String),
	ConfirmName,
	/// Key select: choose this key (already upper-cased, alphanumeric).
	ChooseKey(char),
	ConfirmKey,
	/// ROM editor: select cell `usize` (0..256) for editing -- its
	/// current value gets loaded into the host's text-input buffer for
	/// [`build_rom_editor_popup`]'s text field.
	RomSelectCell(usize),
	/// ROM editor: commit whatever's typed in the text field into the
	/// currently-selected cell, then advance selection to the next cell
	/// (wrapping at 255 -> 0) so typing several values in a row doesn't
	/// need a click between each one.
	RomConfirmCell,
	/// ROM editor: write the whole edited buffer back to the subchip and
	/// close the popup.
	RomApply,
	/// Save-chip popup: commit the typed name -- either a plain
	/// overwrite/create (`SaveChipMode::Save`) or, when the name belongs
	/// to a *different* existing chip, a backup-then-overwrite
	/// (`SaveChipMode::Replace`). Which of those it actually does is
	/// resolved by the host at click time the same way it was resolved
	/// to choose which button/label to show -- see
	/// `build_save_chip_popup`'s docs.
	SaveChipConfirm,
	/// Save-chip popup (`SaveChipMode::SaveAsOrRename` only): save a
	/// *copy* of the current chip under the typed name, leaving the
	/// chip's existing on-disk file (under its current name) untouched.
	SaveChipSaveAs,
	/// Save-chip popup (`SaveChipMode::SaveAsOrRename` only): actually
	/// rename the chip -- moves its on-disk file to the typed name, no
	/// copy left behind under the old name.
	SaveChipRename,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorButton {
	pub rect: UiRect,
	pub action: EditorAction,
	pub enabled: bool,
}

/// Everything needed to draw one frame of an overlay and hit-test the
/// next mouse event against it. Analogous to `menu_ui::MenuFrame`.
#[derive(Debug, Default, Clone)]
pub struct EditorFrame {
	pub geometry: crate::render::scene::SceneGeometry,
	pub buttons: Vec<EditorButton>,
	/// Hit-box of the text-entry field, if this overlay has one.
	pub text_field: Option<UiRect>,
	pub hovered: Option<EditorAction>,
}

const FONT_SIZE: f32 = 18.0;
const TITLE_FONT_SIZE: f32 = 26.0;
const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 6.0;

/// Centre point of a `UiRect`, in the same screen-pixel space as its
/// `x`/`y`/`w`/`h` fields. `UiRect` doesn't expose its own `centre()`
/// publicly (it's private to `menu_ui`), so this is the local equivalent.
fn centre(r: &UiRect) -> Vec2 {
	Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn panel_bg(frame: &mut EditorFrame, vw: f32, vh: f32, rect: UiRect, colour: theme::Rgba) {
	frame.geometry.add_rect(to_world(centre(&rect), vw, vh), Vec2::new(rect.w, rect.h), colour);
}

fn add_label(frame: &mut EditorFrame, vw: f32, vh: f32, centre: Vec2, width: f32, text: &str, colour: theme::Rgba, font_size: f32) {
	frame.geometry.labels.push(TextLabel { pos: to_world(centre, vw, vh), text: text.to_string(), colour, font_size, width });
}

fn add_button(frame: &mut EditorFrame, vw: f32, vh: f32, rect: UiRect, label: &str, action: EditorAction, enabled: bool, mouse: Vec2) {
	let hovered = enabled && rect.contains(mouse);
	let bg = if !enabled {
		theme::PIN_INVALID_COL
	} else if hovered {
		[0.45, 0.45, 0.5, 1.0]
	} else {
		theme::CHIP_BODY_COL
	};
	panel_bg(frame, vw, vh, rect, bg);
	add_label(frame, vw, vh, centre(&rect), rect.w - 12.0, label, theme::text_colour_for_background(bg), FONT_SIZE);
	frame.buttons.push(EditorButton { rect, action, enabled });
}

/// Same as [`add_button`], but with an explicit base colour (before the
/// hover brightening) instead of the default grey -- used for the ROM
/// editor's "Set" (default already) and the save-chip popup's "Replace"
/// button, which is deliberately red (see `build_save_chip_popup`) since
/// it's a destructive action (backs up, then overwrites, a different
/// existing chip).
fn add_button_coloured(
	frame: &mut EditorFrame,
	vw: f32,
	vh: f32,
	rect: UiRect,
	label: &str,
	action: EditorAction,
	enabled: bool,
	mouse: Vec2,
	base_colour: theme::Rgba,
) {
	let hovered = enabled && rect.contains(mouse);
	let bg = if !enabled {
		theme::PIN_INVALID_COL
	} else if hovered {
		[(base_colour[0] + 0.12).min(1.0), (base_colour[1] + 0.12).min(1.0), (base_colour[2] + 0.12).min(1.0), base_colour[3]]
	} else {
		base_colour
	};
	panel_bg(frame, vw, vh, rect, bg);
	add_label(frame, vw, vh, centre(&rect), rect.w - 12.0, label, theme::text_colour_for_background(bg), FONT_SIZE);
	frame.buttons.push(EditorButton { rect, action, enabled });
}

fn finish(mut frame: EditorFrame, mouse: Vec2) -> EditorFrame {
	frame.hovered = frame.buttons.iter().find(|b| b.enabled && b.rect.contains(mouse)).map(|b| b.action.clone());
	frame
}

// ---------------------------------------------------------------------
// Preferences (`PreferencesMenu`)
// ---------------------------------------------------------------------

pub const PIN_DISPLAY_OPTIONS: [&str; 3] = ["Always", "On Hover", "Tab to Toggle"];
pub const GRID_DISPLAY_OPTIONS: [&str; 2] = ["Off", "On"];
pub const SNAPPING_OPTIONS: [&str; 3] = ["Hold Ctrl", "If Grid Shown", "Always"];
pub const STRAIGHT_WIRE_OPTIONS: [&str; 3] = ["Hold Shift", "If Grid Shown", "Always"];
pub const SIM_STATUS_OPTIONS: [&str; 2] = ["Active", "Paused"];

/// One row of the preferences panel: a label plus the currently-selected
/// option out of a fixed set (mirrors one `PreferencesMenu.DrawNextWheel`
/// call). `CyclePref(index)` where `index` is this row's position in
/// [`build_preferences_panel`]'s output advances `current` by one,
/// wrapping -- the host applies that back onto its own settings/prefs
/// struct and rebuilds the frame.
struct PrefRow<'a> {
	label: &'a str,
	options: &'a [&'a str],
	current: i32,
}

/// Builds the preferences overlay from a project's current prefs fields
/// (`ProjectDescription.Prefs_*`). Purely a display of the *current*
/// values plus next/cycle buttons -- the host owns applying a cycled
/// value back to its own copy of `desc` and re-calling this each frame,
/// same pattern as `menu_ui`'s settings screen.
pub fn build_preferences_panel(desc: &ProjectDescription, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = (vw * 0.6).clamp(360.0, 620.0);
	let cx = vw / 2.0;
	let top = vh * 0.12;
	let panel_rect = UiRect::new(cx - panel_w / 2.0, top - 40.0, panel_w, vh * 0.76);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.14, 0.14, 0.16, 0.97]);
	add_label(&mut frame, vw, vh, Vec2::new(cx, top - 10.0), panel_w - 40.0, "Preferences", [1.0, 1.0, 1.0, 1.0], TITLE_FONT_SIZE);

	let rows = [
		PrefRow { label: "Show I/O pin names", options: &PIN_DISPLAY_OPTIONS, current: desc.prefs_main_pin_names_display_mode },
		PrefRow { label: "Show chip pin names", options: &PIN_DISPLAY_OPTIONS, current: desc.prefs_chip_pin_names_display_mode },
		PrefRow { label: "Show grid", options: &GRID_DISPLAY_OPTIONS, current: desc.prefs_grid_display_mode },
		PrefRow { label: "Snap to grid", options: &SNAPPING_OPTIONS, current: desc.prefs_snapping },
		PrefRow { label: "Straight wires", options: &STRAIGHT_WIRE_OPTIONS, current: desc.prefs_straight_wires },
		PrefRow { label: "Sim status", options: &SIM_STATUS_OPTIONS, current: desc.prefs_sim_paused as i32 },
	];

	let field_w = panel_w * 0.4;
	let mut y = top + 30.0;
	for (i, row) in rows.iter().enumerate() {
		let label_x = panel_rect.x + 20.0;
		let field_rect = UiRect::new(panel_rect.x + panel_w - field_w - 20.0, y, field_w, ROW_H);
		add_label(
			&mut frame,
			vw,
			vh,
			Vec2::new(label_x + (panel_w - field_w - 60.0) / 2.0, y + ROW_H / 2.0),
			panel_w - field_w - 60.0,
			row.label,
			[0.9, 0.9, 0.9, 1.0],
			FONT_SIZE * 0.9,
		);
		let option_text = row.options.get(row.current as usize).copied().unwrap_or("?");
		add_button(&mut frame, vw, vh, field_rect, option_text, EditorAction::CyclePref(i), true, mouse);
		y += ROW_H + ROW_GAP;
	}

	let apply_rect = UiRect::new(cx - 90.0, panel_rect.y + panel_rect.h - 56.0, 180.0, 40.0);
	add_button(&mut frame, vw, vh, apply_rect, "Apply", EditorAction::ApplyPreferences, true, mouse);
	let close_rect = UiRect::new(cx - 90.0, panel_rect.y + panel_rect.h - 10.0, 180.0, 32.0);
	add_button(&mut frame, vw, vh, close_rect, "Close", EditorAction::ClosePopup, true, mouse);

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// Chip library (`ChipLibraryMenu`)
// ---------------------------------------------------------------------

/// Builds the left-hand chip palette: one collapsible header per
/// collection, and (for open collections) one clickable row per chip
/// inside it. Mirrors the read/select portion of `ChipLibraryMenu` --
/// drag-to-reorder, rename, and delete-collection aren't ported here
/// since they need no new *rendering* concepts beyond what's already
/// covered by the name/delete popups in `menu_ui`.
pub fn build_chip_library_panel(collections: &[ChipCollection], selected_chip: Option<&str>, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = 220.0_f32.min(vw * 0.3);
	let panel_rect = UiRect::new(0.0, 0.0, panel_w, vh);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.16, 0.16, 0.18, 0.98]);
	add_label(&mut frame, vw, vh, Vec2::new(panel_w / 2.0, 24.0), panel_w - 16.0, "Chips", [1.0, 1.0, 1.0, 1.0], TITLE_FONT_SIZE * 0.8);

	let row_w = panel_w - 16.0;
	let mut y = 50.0;
	for (ci, collection) in collections.iter().enumerate() {
		let header_rect = UiRect::new(8.0, y, row_w, ROW_H * 0.85);
		let arrow = if collection.is_toggled_open { "v" } else { ">" };
		let header_bg = if header_rect.contains(mouse) { [0.3, 0.3, 0.34, 1.0] } else { [0.24, 0.24, 0.27, 1.0] };
		panel_bg(&mut frame, vw, vh, header_rect, header_bg);
		add_label(
			&mut frame,
			vw,
			vh,
			centre(&header_rect),
			row_w - 16.0,
			&format!("{arrow} {}", collection.name),
			[0.85, 0.95, 0.85, 1.0],
			FONT_SIZE * 0.85,
		);
		frame.buttons.push(EditorButton { rect: header_rect, action: EditorAction::ToggleCollection(ci), enabled: true });
		y += ROW_H * 0.85 + 4.0;

		if collection.is_toggled_open {
			for chip_name in &collection.chips {
				let row_rect = UiRect::new(20.0, y, row_w - 12.0, ROW_H * 0.8);
				let is_selected = selected_chip == Some(chip_name.as_str());
				let bg = if is_selected {
					[0.35, 0.45, 0.6, 1.0]
				} else if row_rect.contains(mouse) {
					[0.32, 0.32, 0.36, 1.0]
				} else {
					[0.22, 0.22, 0.25, 1.0]
				};
				panel_bg(&mut frame, vw, vh, row_rect, bg);
				add_label(
					&mut frame,
					vw,
					vh,
					centre(&row_rect),
					row_rect.w - 12.0,
					chip_name,
					theme::text_colour_for_background(bg),
					FONT_SIZE * 0.8,
				);
				frame.buttons.push(EditorButton { rect: row_rect, action: EditorAction::SelectChip(chip_name.clone()), enabled: true });
				y += ROW_H * 0.8 + 3.0;
			}
		}
		y += 6.0;
	}

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// Search popup (`SearchPopup`)
// ---------------------------------------------------------------------

/// Builds the fullscreen chip-search overlay: a text field plus a
/// scrollable (here: simply clipped-to-viewport) list of chip names
/// containing `query` as a case-insensitive substring, matching
/// `SearchPopup`'s filtering.
pub fn build_search_popup(all_chip_names: &[String], query: &str, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = 420.0_f32.min(vw - 80.0);
	let cx = vw / 2.0;
	let top = vh * 0.07;

	let field_rect = UiRect::new(cx - panel_w / 2.0, top, panel_w, 36.0);
	frame.geometry.add_rect(to_world(centre(&field_rect), vw, vh), Vec2::new(field_rect.w, field_rect.h), [0.08, 0.08, 0.09, 1.0]);
	let shown = if query.is_empty() { "Search...|".to_string() } else { format!("{query}|") };
	add_label(&mut frame, vw, vh, centre(&field_rect), field_rect.w - 16.0, &shown, [1.0, 1.0, 1.0, 1.0], FONT_SIZE);
	frame.text_field = Some(field_rect);

	let needle = query.to_lowercase();
	let filtered: Vec<&String> = all_chip_names.iter().filter(|n| needle.is_empty() || n.to_lowercase().contains(&needle)).collect();

	let list_top = top + 36.0 + 10.0;
	let list_bottom = vh * 0.9;
	let mut y = list_top;
	for name in &filtered {
		if y + ROW_H > list_bottom {
			break; // rest is scrolled off; not represented since there's no scroll offset state to port yet
		}
		let row_rect = UiRect::new(cx - panel_w / 2.0, y, panel_w, ROW_H - 4.0);
		let bg = if row_rect.contains(mouse) { [0.32, 0.32, 0.36, 1.0] } else { [0.22, 0.22, 0.25, 1.0] };
		frame.geometry.add_rect(to_world(centre(&row_rect), vw, vh), Vec2::new(row_rect.w, row_rect.h), bg);
		add_label(&mut frame, vw, vh, centre(&row_rect), row_rect.w - 16.0, name, theme::text_colour_for_background(bg), FONT_SIZE * 0.9);
		frame.buttons.push(EditorButton { rect: row_rect, action: EditorAction::UseChip((*name).clone()), enabled: true });
		y += ROW_H;
	}

	if filtered.is_empty() {
		add_label(&mut frame, vw, vh, Vec2::new(cx, list_top + 20.0), panel_w, "No matching chips", [0.7, 0.7, 0.7, 1.0], FONT_SIZE * 0.9);
	}

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// Simple naming popup (`ChipLabelMenu`)
// ---------------------------------------------------------------------

/// Builds a small "type a name, Cancel/Confirm" popup -- the generic
/// shape used e.g. by `ChipLabelMenu` for labelling a sub-chip. `title`
/// is shown above the field (the original doesn't have one, but hosts
/// reusing this for more than one purpose need to tell them apart).
/// `confirm_enabled` mirrors the caller's own validation (e.g. max
/// length) -- this builder has no opinion on what makes a label valid.
pub fn build_simple_naming_popup(title: &str, text: &str, confirm_enabled: bool, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = 360.0;
	let panel_h = 150.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	if !title.is_empty() {
		add_label(&mut frame, vw, vh, Vec2::new(cx, panel_rect.y + 26.0), panel_w - 40.0, title, [1.0, 1.0, 1.0, 1.0], 20.0);
	}

	let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 46.0, panel_w - 60.0, 34.0);
	frame.geometry.add_rect(to_world(centre(&field_rect), vw, vh), Vec2::new(field_rect.w, field_rect.h), [0.08, 0.08, 0.09, 1.0]);
	let shown = if text.is_empty() { "|".to_string() } else { format!("{text}|") };
	add_label(&mut frame, vw, vh, centre(&field_rect), field_rect.w - 16.0, &shown, [1.0, 1.0, 1.0, 1.0], FONT_SIZE);
	frame.text_field = Some(field_rect);

	let confirm_rect = UiRect::new(cx - 186.0, panel_rect.y + panel_h - 46.0, 180.0, 36.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 46.0, 180.0, 36.0);
	add_button(&mut frame, vw, vh, confirm_rect, "Confirm", EditorAction::ConfirmName, confirm_enabled, mouse);
	add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// Key select popup (`RebindKeyChipMenu`)
// ---------------------------------------------------------------------

pub const KEY_SELECT_ALLOWED_CHARS: &str = "1234567890QWERTYUIOPASDFGHJKLZXCVBNM";

/// Builds the "press a key to rebind" popup. `chosen_key` is whatever
/// alphanumeric key is currently pending confirmation (the host updates
/// this from raw keyboard input using [`KEY_SELECT_ALLOWED_CHARS`] to
/// filter, same as the original, and re-calls this each frame).
pub fn build_key_select_popup(chosen_key: Option<char>, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = 320.0;
	let panel_h = 220.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(
		&mut frame,
		vw,
		vh,
		Vec2::new(cx, panel_rect.y + 30.0),
		panel_w - 30.0,
		"Press a key to rebind\n(alphanumeric only)",
		[1.0, 1.0, 1.0, 0.8],
		18.0,
	);

	let key_box = UiRect::new(cx - 35.0, panel_rect.y + 66.0, 70.0, 70.0);
	panel_bg(&mut frame, vw, vh, key_box, [0.1, 0.1, 0.1, 1.0]);
	let shown = chosen_key.map(|c| c.to_string()).unwrap_or_default();
	add_label(&mut frame, vw, vh, centre(&key_box), key_box.w, &shown, [1.0, 1.0, 1.0, 1.0], 27.0);

	let confirm_rect = UiRect::new(cx - 166.0, panel_rect.y + panel_h - 46.0, 160.0, 36.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 46.0, 160.0, 36.0);
	add_button(&mut frame, vw, vh, confirm_rect, "Confirm", EditorAction::ConfirmKey, chosen_key.is_some(), mouse);
	add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// ROM data editor (configuring a placed `Rom256x16`'s contents)
// ---------------------------------------------------------------------

/// Number of addressable words in a `Rom256x16` -- matches that chip
/// type's name (256 sixteen-bit words) and `SubChipDescription::internal_data`'s
/// expected length for one (see `sim::process_builtin_chip`'s `Rom256x16`
/// arm, which indexes `internal_state` by the read address 0..256).
pub const ROM_WORD_COUNT: usize = 256;
const ROM_GRID_COLS: usize = 16;
const ROM_GRID_ROWS: usize = ROM_WORD_COUNT / ROM_GRID_COLS;
const ROM_CELL_W: f32 = 42.0;
const ROM_CELL_H: f32 = 22.0;
const ROM_CELL_GAP: f32 = 2.0;

/// Builds the 256-cell ROM contents editor popup -- a proper grid (16x16,
/// one cell per address) rather than the plain comma-separated text blob
/// this used to be. `data` is the host's own working copy of all 256
/// words (`ViewerState` keeps this separately from the saved
/// `SubChipDescription::internal_data` until "Apply" is clicked, same
/// "edit a draft, commit on confirm" shape as every other overlay here);
/// `selected` is which cell's value `edit_text` currently represents.
///
/// Each cell shows its word in decimal; click one to select it (loads
/// its value into the text field for editing), type a new value, then
/// either click "Set" or press Enter (`EditorAction::RomConfirmCell`) to
/// commit it and move on to the next cell. Accepts a leading `0x`/`0X`
/// for hex input; displays decimal, to match the plain-number contents
/// most ROM programs actually use.
pub fn build_rom_editor_popup(data: &[u32], selected: usize, edit_text: &str, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();

	let grid_w = ROM_GRID_COLS as f32 * (ROM_CELL_W + ROM_CELL_GAP) - ROM_CELL_GAP;
	let grid_h = ROM_GRID_ROWS as f32 * (ROM_CELL_H + ROM_CELL_GAP) - ROM_CELL_GAP;
	let panel_w = grid_w + 40.0;
	let header_h = 92.0;
	let footer_h = 56.0;
	let panel_h = header_h + grid_h + footer_h;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(&mut frame, vw, vh, Vec2::new(cx, panel_rect.y + 20.0), panel_w - 30.0, "Configure ROM (256 x 16-bit)", [1.0, 1.0, 1.0, 1.0], 20.0);

	let selected = selected.min(ROM_WORD_COUNT - 1);
	let addr_label = format!("Address {selected} (0x{selected:02X})");
	add_label(&mut frame, vw, vh, Vec2::new(panel_rect.x + 90.0, panel_rect.y + 52.0), 150.0, &addr_label, [0.85, 0.85, 0.85, 1.0], 15.0);

	let field_rect = UiRect::new(panel_rect.x + panel_w - 190.0, panel_rect.y + 38.0, 100.0, 30.0);
	frame.geometry.add_rect(to_world(centre(&field_rect), vw, vh), Vec2::new(field_rect.w, field_rect.h), [0.08, 0.08, 0.09, 1.0]);
	let shown = if edit_text.is_empty() { "|".to_string() } else { format!("{edit_text}|") };
	add_label(&mut frame, vw, vh, centre(&field_rect), field_rect.w - 10.0, &shown, [1.0, 1.0, 1.0, 1.0], FONT_SIZE);
	frame.text_field = Some(field_rect);

	let set_rect = UiRect::new(panel_rect.x + panel_w - 80.0, panel_rect.y + 38.0, 60.0, 30.0);
	add_button(&mut frame, vw, vh, set_rect, "Set", EditorAction::RomConfirmCell, true, mouse);

	let grid_origin = Vec2::new(panel_rect.x + (panel_w - grid_w) / 2.0, panel_rect.y + header_h);
	for row in 0..ROM_GRID_ROWS {
		for col in 0..ROM_GRID_COLS {
			let idx = row * ROM_GRID_COLS + col;
			let cell_rect = UiRect::new(
				grid_origin.x + col as f32 * (ROM_CELL_W + ROM_CELL_GAP),
				grid_origin.y + row as f32 * (ROM_CELL_H + ROM_CELL_GAP),
				ROM_CELL_W,
				ROM_CELL_H,
			);
			let is_selected = idx == selected;
			let hovered = cell_rect.contains(mouse);
			let value = data.get(idx).copied().unwrap_or(0);
			let bg = if is_selected {
				[0.35, 0.5, 0.75, 1.0]
			} else if value != 0 {
				[0.3, 0.3, 0.34, 1.0]
			} else if hovered {
				[0.28, 0.28, 0.3, 1.0]
			} else {
				[0.14, 0.14, 0.16, 1.0]
			};
			panel_bg(&mut frame, vw, vh, cell_rect, bg);
			add_label(&mut frame, vw, vh, centre(&cell_rect), cell_rect.w - 4.0, &value.to_string(), theme::text_colour_for_background(bg), 11.0);
			frame.buttons.push(EditorButton { rect: cell_rect, action: EditorAction::RomSelectCell(idx), enabled: true });
		}
	}

	let apply_rect = UiRect::new(cx - 166.0, panel_rect.y + panel_h - 44.0, 160.0, 34.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 44.0, 160.0, 34.0);
	add_button(&mut frame, vw, vh, apply_rect, "Apply", EditorAction::RomApply, true, mouse);
	add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);

	finish(frame, mouse)
}

// ---------------------------------------------------------------------
// Save-chip popup (Ctrl+S)
// ---------------------------------------------------------------------

/// Which buttons [`build_save_chip_popup`] should offer, based on how the
/// currently-typed name compares to the chip's current on-disk identity
/// and the rest of the library -- computed by the host (it needs
/// `ChipLibrary`/`ViewerState` access this module deliberately doesn't
/// have) by comparing the typed name against `v.root_chip_name` and
/// `v.library`, then re-derived identically on both the "which buttons
/// to draw" side and the "what did this click actually mean" side, so
/// the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveChipMode {
	/// Typed name is exactly the chip's current identity (or, before
	/// anything's been typed, defaults to it) -- plain overwrite/create,
	/// no other chip is affected.
	Save,
	/// Typed name already belongs to a *different* chip that's saved to
	/// disk -- confirming backs that other chip up (see
	/// `Saver::delete_chip`'s `backup_in_deleted_folder`) and overwrites
	/// it with this one.
	Replace,
	/// Typed name is free and differs from the chip's current identity
	/// -- ambiguous by itself whether the player means "keep the
	/// original too, also save a copy here" (Save As) or "this chip is
	/// now called that" (Rename), so both are offered.
	SaveAsOrRename,
}

/// Builds the Ctrl+S popup. `current_name` is the chip's own current
/// identity (shown for context, e.g. "Saving: Full Adder"); `text` is
/// the typed name field's contents; `mode` (see its own docs) picks
/// which action buttons to show -- and, for `Replace`, colours it red
/// since it's destructive to *some other* chip's save file.
pub fn build_save_chip_popup(current_name: &str, text: &str, mode: SaveChipMode, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let mut frame = EditorFrame::default();
	let panel_w = 420.0;
	let panel_h = 178.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, vw, vh, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(
		&mut frame,
		vw,
		vh,
		Vec2::new(cx, panel_rect.y + 24.0),
		panel_w - 40.0,
		&format!("Save chip (currently: {current_name})"),
		[1.0, 1.0, 1.0, 1.0],
		18.0,
	);

	let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 48.0, panel_w - 60.0, 34.0);
	frame.geometry.add_rect(to_world(centre(&field_rect), vw, vh), Vec2::new(field_rect.w, field_rect.h), [0.08, 0.08, 0.09, 1.0]);
	let shown = if text.is_empty() { "|".to_string() } else { format!("{text}|") };
	add_label(&mut frame, vw, vh, centre(&field_rect), field_rect.w - 16.0, &shown, [1.0, 1.0, 1.0, 1.0], FONT_SIZE);
	frame.text_field = Some(field_rect);

	let hint = match mode {
		SaveChipMode::Save => "",
		SaveChipMode::Replace => "A different chip already has this name.",
		SaveChipMode::SaveAsOrRename => "Name changed -- keep both, or rename?",
	};
	if !hint.is_empty() {
		add_label(&mut frame, vw, vh, Vec2::new(cx, panel_rect.y + 92.0), panel_w - 40.0, hint, [0.85, 0.65, 0.4, 1.0], 14.0);
	}

	let confirm_enabled = !text.trim().is_empty();
	let button_y = panel_rect.y + panel_h - 46.0;
	match mode {
		SaveChipMode::Save => {
			let confirm_rect = UiRect::new(cx - 186.0, button_y, 180.0, 36.0);
			let cancel_rect = UiRect::new(cx + 6.0, button_y, 180.0, 36.0);
			add_button(&mut frame, vw, vh, confirm_rect, "Save", EditorAction::SaveChipConfirm, confirm_enabled, mouse);
			add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);
		}
		SaveChipMode::Replace => {
			let confirm_rect = UiRect::new(cx - 186.0, button_y, 180.0, 36.0);
			let cancel_rect = UiRect::new(cx + 6.0, button_y, 180.0, 36.0);
			add_button_coloured(
				&mut frame,
				vw,
				vh,
				confirm_rect,
				"Replace",
				EditorAction::SaveChipConfirm,
				confirm_enabled,
				mouse,
				[0.62, 0.18, 0.18, 1.0],
			);
			add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);
		}
		SaveChipMode::SaveAsOrRename => {
			let w = (panel_w - 60.0 - 16.0) / 3.0;
			let save_as_rect = UiRect::new(panel_rect.x + 30.0, button_y, w, 36.0);
			let rename_rect = UiRect::new(save_as_rect.x + w + 8.0, button_y, w, 36.0);
			let cancel_rect = UiRect::new(rename_rect.x + w + 8.0, button_y, w, 36.0);
			add_button(&mut frame, vw, vh, save_as_rect, "Save As", EditorAction::SaveChipSaveAs, confirm_enabled, mouse);
			add_button(&mut frame, vw, vh, rename_rect, "Rename", EditorAction::SaveChipRename, confirm_enabled, mouse);
			add_button(&mut frame, vw, vh, cancel_rect, "Cancel", EditorAction::ClosePopup, true, mouse);
		}
	}

	finish(frame, mouse)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::structs::Vec2;

	fn sample_desc() -> ProjectDescription {
		ProjectDescription {
			prefs_main_pin_names_display_mode: 1,
			prefs_chip_pin_names_display_mode: 0,
			prefs_grid_display_mode: 1,
			prefs_snapping: 2,
			prefs_straight_wires: 0,
			prefs_sim_paused: true,
			..Default::default()
		}
	}

	#[test]
	fn preferences_panel_has_one_cycle_button_per_row_plus_apply_and_close() {
		let frame = build_preferences_panel(&sample_desc(), 1280.0, 800.0, Vec2::ZERO);
		let cycle_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::CyclePref(_))).count();
		assert_eq!(cycle_count, 6);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ApplyPreferences));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
	}

	#[test]
	fn preferences_panel_shows_currently_selected_option_text() {
		let frame = build_preferences_panel(&sample_desc(), 1280.0, 800.0, Vec2::ZERO);
		// Row 0 is "Show I/O pin names" with mode 1 => "On Hover".
		assert!(frame.geometry.labels.iter().any(|l| l.text == "On Hover"));
		// Row 5 is "Sim status" with prefs_sim_paused = true => "Paused".
		assert!(frame.geometry.labels.iter().any(|l| l.text == "Paused"));
	}

	#[test]
	fn chip_library_panel_only_lists_chips_for_open_collections() {
		let collections = vec![
			ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into(), "OR".into()] },
			ChipCollection { name: "CLOSED".into(), is_toggled_open: false, chips: vec!["XOR".into()] },
		];
		let frame = build_chip_library_panel(&collections, None, 1280.0, 800.0, Vec2::ZERO);

		let select_actions: Vec<_> =
			frame.buttons.iter().filter_map(|b| if let EditorAction::SelectChip(n) = &b.action { Some(n.clone()) } else { None }).collect();
		assert_eq!(select_actions, vec!["AND".to_string(), "OR".to_string()]);

		let toggle_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::ToggleCollection(_))).count();
		assert_eq!(toggle_count, 2);
	}

	#[test]
	fn chip_library_panel_highlights_the_selected_chip() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
		let frame = build_chip_library_panel(&collections, Some("AND"), 1280.0, 800.0, Vec2::ZERO);
		let row = frame.buttons.iter().find(|b| b.action == EditorAction::SelectChip("AND".to_string())).unwrap();
		// Just confirm it's present/enabled; the selected-colour path is
		// exercised visually, not asserted on pixel colour here.
		assert!(row.enabled);
		assert_eq!(frame.hovered, None);
	}

	#[test]
	fn search_popup_filters_case_insensitively() {
		let names = vec!["AND".to_string(), "OR".to_string(), "NAND".to_string()];
		let frame = build_search_popup(&names, "an", 1280.0, 800.0, Vec2::ZERO);
		let shown: Vec<_> =
			frame.buttons.iter().filter_map(|b| if let EditorAction::UseChip(n) = &b.action { Some(n.clone()) } else { None }).collect();
		assert_eq!(shown, vec!["AND".to_string(), "NAND".to_string()]);
	}

	#[test]
	fn search_popup_with_empty_query_lists_everything() {
		let names = vec!["AND".to_string(), "OR".to_string()];
		let frame = build_search_popup(&names, "", 1280.0, 800.0, Vec2::ZERO);
		assert_eq!(frame.buttons.len(), 2);
		assert!(frame.text_field.is_some());
	}

	#[test]
	fn search_popup_shows_a_message_when_nothing_matches() {
		let names = vec!["AND".to_string()];
		let frame = build_search_popup(&names, "zzz", 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.buttons.iter().all(|b| !matches!(b.action, EditorAction::UseChip(_))));
		assert!(frame.geometry.labels.iter().any(|l| l.text.contains("No matching")));
	}

	#[test]
	fn simple_naming_popup_exposes_a_text_field_and_respects_confirm_enabled() {
		let frame = build_simple_naming_popup("Rename", "My Label", false, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.text_field.is_some());
		let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::ConfirmName).unwrap();
		assert!(!confirm.enabled);

		let frame_ok = build_simple_naming_popup("Rename", "My Label", true, 1280.0, 800.0, Vec2::ZERO);
		let confirm_ok = frame_ok.buttons.iter().find(|b| b.action == EditorAction::ConfirmName).unwrap();
		assert!(confirm_ok.enabled);
	}

	#[test]
	fn key_select_popup_disables_confirm_until_a_key_is_chosen() {
		let frame_none = build_key_select_popup(None, 1280.0, 800.0, Vec2::ZERO);
		let confirm_none = frame_none.buttons.iter().find(|b| b.action == EditorAction::ConfirmKey).unwrap();
		assert!(!confirm_none.enabled);
		assert!(frame_none.text_field.is_none());

		let frame_some = build_key_select_popup(Some('Q'), 1280.0, 800.0, Vec2::ZERO);
		let confirm_some = frame_some.buttons.iter().find(|b| b.action == EditorAction::ConfirmKey).unwrap();
		assert!(confirm_some.enabled);
		assert!(frame_some.geometry.labels.iter().any(|l| l.text == "Q"));
	}

	#[test]
	fn rom_editor_has_one_selectable_cell_per_word() {
		let data = vec![0u32; ROM_WORD_COUNT];
		let frame = build_rom_editor_popup(&data, 0, "0", 1280.0, 800.0, Vec2::ZERO);
		let cell_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::RomSelectCell(_))).count();
		assert_eq!(cell_count, ROM_WORD_COUNT);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::RomApply));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
		assert!(frame.text_field.is_some());
	}

	#[test]
	fn rom_editor_shows_selected_cells_value_in_its_text_field() {
		let mut data = vec![0u32; ROM_WORD_COUNT];
		data[5] = 1234;
		let frame = build_rom_editor_popup(&data, 5, "1234", 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.geometry.labels.iter().any(|l| l.text.contains("1234")));
		assert!(frame.geometry.labels.iter().any(|l| l.text.contains("Address 5")));
	}

	#[test]
	fn save_chip_mode_save_shows_single_confirm_button() {
		let frame = build_save_chip_popup("Full Adder", "Full Adder", SaveChipMode::Save, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipConfirm));
		assert!(!frame.buttons.iter().any(|b| matches!(b.action, EditorAction::SaveChipSaveAs | EditorAction::SaveChipRename)));
	}

	#[test]
	fn save_chip_mode_save_as_or_rename_shows_both_options() {
		let frame = build_save_chip_popup("Full Adder", "Full Adder 2", SaveChipMode::SaveAsOrRename, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipSaveAs));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::SaveChipRename));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ClosePopup));
	}

	#[test]
	fn save_chip_confirm_disabled_for_empty_name() {
		let frame = build_save_chip_popup("Full Adder", "", SaveChipMode::Save, 1280.0, 800.0, Vec2::ZERO);
		let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::SaveChipConfirm).unwrap();
		assert!(!confirm.enabled);
	}

	#[test]
	fn key_select_allowed_chars_are_alphanumeric_uppercase() {
		assert!(KEY_SELECT_ALLOWED_CHARS.chars().all(|c| c.is_ascii_alphanumeric() && (c.is_ascii_digit() || c.is_ascii_uppercase())));
	}
}
