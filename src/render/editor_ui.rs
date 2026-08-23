//! Builds drawable geometry + clickable hit-boxes for the in-editor overlays that sit on top of the
//! chip viewer, ported from the corresponding `DLS.Graphics.*Menu` classes. Same philosophy as
//! [`crate::render::menu_ui`]: plain data in, plain data out, no wgpu types, fully unit-testable
//! without a GPU. Holds no simulation or save-system logic of its own -- only builds frames to
//! draw/hit-test for the preferences, chip library, search/naming, key-select, ROM-editor, and save-chip popups.

use crate::json::ChipCollection;
use crate::json::ProjectDescription;
use crate::json::StarredItem;
use crate::render::theme;
use crate::render::ui_kit::{self, Frame, UiCtx, UiRect};
use crate::structs::Vec2;
use std::collections::HashSet;

pub use crate::render::ui_kit::to_world;

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
	/// Chip library (real 3-panel layout, `ChipLibraryMenu`): click a
	/// collection's header -- selects it and toggles its open/closed
	/// state in one click. This port has no ctrl-held "select without
	/// toggling" modifier to preserve, unlike the original.
	SelectCollection(usize),
	/// Chip library: click a chip row inside an open collection.
	SelectChipRow {
		collection: usize,
		chip: usize,
	},
	/// Chip library: click a row in the starred list.
	SelectStarredRow(usize),
	/// Chip library: star/unstar whatever's currently selected (`name`,
	/// `is_collection` describe the target directly rather than making
	/// the host re-derive them from its own selection state).
	ToggleStarred {
		name: String,
		is_collection: bool,
	},
	/// Chip library: move the selected chip/collection/starred row one
	/// step within its own list. `true` = move down, `false` = up.
	MoveSelectedStep(bool),
	/// Chip library: move the selected chip to the previous/next
	/// collection outright (`JUMP UP`/`JUMP DOWN` in the original) --
	/// only offered once it's already at the start/end of its own
	/// collection's chip list.
	MoveSelectedJump(bool),
	/// Chip library: open the selected chip for editing (mirrors the
	/// original's "OPEN").
	OpenSelectedChip(String),
	/// Chip library: pick up the selected chip for placement (mirrors the
	/// original's "USE") -- the host attaches it to the cursor as a
	/// translucent preview, dropped as a real subchip on the next canvas
	/// click that lands on free space. Unlike `OpenSelectedChip`/
	/// `RequestDeleteChip`, offered for builtins too, since any chip type
	/// (not just custom ones) can be placed on the canvas.
	PlaceChip(String),
	/// Chip library: ask to delete the selected chip -- opens the inline
	/// confirmation panel, doesn't delete anything itself.
	RequestDeleteChip(String),
	/// Chip library: begin typing a new collection's name.
	BeginNewCollection,
	/// Chip library: begin renaming the selected collection.
	BeginRenameCollection,
	/// Chip library: ask to delete the selected collection -- opens the
	/// inline confirmation panel (or, for an already-empty collection,
	/// the host deletes it immediately without one -- see
	/// `apply_editor_action`'s handling of this in `bin/app.rs`).
	RequestDeleteCollection,
	/// Chip library: commit whatever's typed in the new/rename-collection
	/// text field.
	ConfirmCollectionName,
	/// Chip library: cancel whatever inline popup (new/rename collection,
	/// delete confirmation) is currently open, without leaving the
	/// library itself -- distinct from [`EditorAction::ClosePopup`],
	/// which the host uses to close a whole modal overlay.
	CancelLibraryPopup,
	/// Chip library: confirm the pending chip/collection deletion.
	ConfirmDelete,
	/// Chip library: leave the library and return to the plain viewer.
	ExitLibrary,
	/// Bottom bar: click a starred collection's button -- opens (or, if
	/// it's already open, closes) [`build_starred_collection_popup`] for
	/// it. `String` is that collection's name, matching how the popup
	/// itself is keyed (see `bin/app.rs`'s bottom-bar state).
	ToggleStarredCollectionPopup(String),
	/// Bottom bar: close whatever starred-collection flyout is open,
	/// without opening a different one -- a click outside it, or Esc.
	CloseStarredCollectionPopup,
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

/// Hit-box of one clickable region of an [`EditorFrame`] -- see [`ui_kit::Button`].
pub type EditorButton = ui_kit::Button<EditorAction>;

/// Everything needed to draw one frame of an overlay and hit-test the
/// next mouse event against it. Analogous to `menu_ui::MenuFrame`. See [`ui_kit::Frame`].
pub type EditorFrame = Frame<EditorAction>;

const FONT_SIZE: f32 = ui_kit::FONT_SIZE;
const TITLE_FONT_SIZE: f32 = 26.0;
const ROW_H: f32 = 34.0;
const ROW_GAP: f32 = 6.0;

fn panel_bg(frame: &mut EditorFrame, ui: UiCtx, rect: UiRect, colour: theme::Rgba) {
	ui_kit::fill_rect(frame, ui, rect, colour);
}

fn add_label(frame: &mut EditorFrame, ui: UiCtx, centre: Vec2, width: f32, text: &str, colour: theme::Rgba, font_size: f32) {
	ui_kit::add_label(frame, ui, centre, width, text, colour, font_size);
}

fn add_button(frame: &mut EditorFrame, ui: UiCtx, rect: UiRect, label: &str, action: EditorAction, enabled: bool) {
	ui_kit::add_button(frame, ui, rect, label, action, enabled, None);
}

/// Same as [`add_button`], but with an explicit base colour (before the
/// hover brightening) instead of the default grey -- used for the
/// save-chip popup's "Replace" button, which is deliberately red (see
/// `build_save_chip_popup`) since it's a destructive action (backs up,
/// then overwrites, a different existing chip).
fn add_button_coloured(frame: &mut EditorFrame, ui: UiCtx, rect: UiRect, label: &str, action: EditorAction, enabled: bool, base_colour: theme::Rgba) {
	ui_kit::add_button(frame, ui, rect, label, action, enabled, Some(base_colour));
}

fn finish(frame: EditorFrame, ui: UiCtx) -> EditorFrame {
	ui_kit::finish(frame, ui.mouse)
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
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let panel_w = (vw * 0.6).clamp(360.0, 620.0);
	let cx = vw / 2.0;
	let top = vh * 0.12;
	let panel_rect = UiRect::new(cx - panel_w / 2.0, top - 40.0, panel_w, vh * 0.76);
	panel_bg(&mut frame, ui, panel_rect, [0.14, 0.14, 0.16, 0.97]);
	add_label(&mut frame, ui, Vec2::new(cx, top - 10.0), panel_w - 40.0, "Preferences", [1.0, 1.0, 1.0, 1.0], TITLE_FONT_SIZE);

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
			ui,
			Vec2::new(label_x + (panel_w - field_w - 60.0) / 2.0, y + ROW_H / 2.0),
			panel_w - field_w - 60.0,
			row.label,
			[0.9, 0.9, 0.9, 1.0],
			FONT_SIZE * 0.9,
		);
		let option_text = row.options.get(row.current as usize).copied().unwrap_or("?");
		add_button(&mut frame, ui, field_rect, option_text, EditorAction::CyclePref(i), true);
		y += ROW_H + ROW_GAP;
	}

	let apply_rect = UiRect::new(cx - 90.0, panel_rect.y + panel_rect.h - 56.0, 180.0, 40.0);
	add_button(&mut frame, ui, apply_rect, "Apply", EditorAction::ApplyPreferences, true);
	let close_rect = UiRect::new(cx - 90.0, panel_rect.y + panel_rect.h - 10.0, 180.0, 32.0);
	add_button(&mut frame, ui, close_rect, "Close", EditorAction::ClosePopup, true);

	finish(frame, ui)
}

// ---------------------------------------------------------------------
// Chip library (`ChipLibraryMenu`)
// ---------------------------------------------------------------------

/// Which row of the chip-library panel is currently selected, if any --
/// mirrors `ChipLibraryMenu`'s three separate `selected*` index fields,
/// kept together here since exactly one (or none) can be active at once,
/// the same invariant the original enforces by hand across three ints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySelection {
	#[default]
	None,
	/// A collection header, `collections[.0]`.
	Collection(usize),
	/// A chip row, `collections[.0].chips[.1]`.
	Chip(usize, usize),
	/// A row in the starred list, `starred_list[.0]`.
	Starred(usize),
}

/// Everything [`build_chip_library_panel`] needs to draw one frame --
/// bundled into a struct (rather than one parameter apiece) purely
/// because there are enough of them that a positional call would be
/// unreadable at the call site; the host still owns every field, same
/// "plain data in, plain data out" shape as the rest of this module.
pub struct ChipLibraryState<'a> {
	pub collections: &'a [ChipCollection],
	pub starred_list: &'a [StarredItem],
	pub selection: LibrarySelection,
	/// Whether the selected chip is a player-authored chip (as opposed
	/// to a builtin) -- gates the OPEN/DELETE buttons, mirrors
	/// `!ChipLibrary.IsBuiltinChip`. Ignored unless `selection` is a
	/// [`LibrarySelection::Chip`] or a non-collection starred row.
	pub selected_chip_is_custom: bool,
	/// Whether placing the selected chip as a new subchip inside the
	/// chip currently open on the canvas would create a recursive cycle
	/// -- it either *is* that chip, or already contains it somewhere
	/// inside its own definition. Precomputed by the host (same reason
	/// as `selected_chip_is_custom`'s docs -- this module has no
	/// `ChipLibrary` access to walk the dependency tree itself; see
	/// `bin/app.rs`'s `would_create_cycle`). Gates the "USE" button the
	/// same way `selected_chip_is_custom` gates OPEN/DELETE. Ignored
	/// unless `selection` is a [`LibrarySelection::Chip`] or a
	/// non-collection starred row.
	pub selected_chip_would_cycle: bool,
	pub creating_collection: bool,
	pub renaming_collection: bool,
	/// Live text of the new/rename-collection input field.
	pub name_field_text: &'a str,
	pub confirming_chip_delete: bool,
	pub confirming_collection_delete: bool,
	/// Precomputed by the host (it needs `ChipLibrary` access this
	/// module deliberately doesn't have -- see `CreateDeleteConfirmationMessage`
	/// in the original) -- shown verbatim above the DELETE confirmation buttons.
	pub delete_confirm_message: &'a str,
}

const LIBRARY_STARRED_WIDTH_T: f32 = 0.32;
const LIBRARY_COLLECTIONS_WIDTH_T: f32 = 0.34;
const LIBRARY_PANEL_GAP: f32 = 10.0;

fn is_starred(starred_list: &[StarredItem], name: &str, is_collection: bool) -> bool {
	starred_list.iter().any(|item| item.is_collection == is_collection && item.name.eq_ignore_ascii_case(name))
}

/// Draws one row of evenly-sized full-width buttons (the shape
/// `DrawHorizontalButtonGroup` uses throughout the original's selected-item
/// panel), returning the y just below the row so callers can keep
/// stacking without hand-computing offsets themselves.
fn button_row(frame: &mut EditorFrame, ui: UiCtx, x: f32, y: f32, width: f32, buttons: &[(&str, EditorAction, bool)]) -> f32 {
	let gap = 6.0;
	let n = buttons.len() as f32;
	let w = (width - gap * (n - 1.0)) / n;
	let mut bx = x;
	for (label, action, enabled) in buttons {
		add_button(frame, ui, UiRect::new(bx, y, w, ROW_H), label, action.clone(), *enabled);
		bx += w + gap;
	}
	y + ROW_H + 8.0
}

fn library_panel_header(frame: &mut EditorFrame, ui: UiCtx, rect: UiRect, title: &str) {
	panel_bg(frame, ui, rect, [0.11, 0.11, 0.12, 1.0]);
	add_label(frame, ui, rect.centre(), rect.w - 12.0, title, [0.24, 0.82, 0.41, 1.0], FONT_SIZE * 0.85);
}

/// Builds the "real" three-panel chip library overlay: a STARRED list on
/// the left, a COLLECTIONS tree (headers + their chips, when open) in the
/// middle, and a detail panel on the right showing whatever row is
/// currently selected -- star/unstar, reorder, a "USE" action that picks
/// the chip up for placement, and open/delete (chips) or rename/delete
/// (collections) actions for it. Mirrors `ChipLibraryMenu.DrawMenu`'s
/// three side-by-side panels; the delete confirmation and new/rename-collection
/// name field are drawn inline at the bottom of the detail column, same
/// as the original.
pub fn build_chip_library_panel(state: &ChipLibraryState, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	panel_bg(&mut frame, ui, UiRect::new(0.0, 0.0, vw, vh), [0.0, 0.0, 0.0, 0.55]);

	let pad = 24.0;
	let top = 20.0;
	let total_w = vw - pad * 2.0 - LIBRARY_PANEL_GAP * 2.0;
	let panel_h = vh - top - 20.0;
	let starred_w = total_w * LIBRARY_STARRED_WIDTH_T;
	let collections_w = total_w * LIBRARY_COLLECTIONS_WIDTH_T;
	let detail_w = total_w - starred_w - collections_w;

	let starred_x = pad;
	let collections_x = starred_x + starred_w + LIBRARY_PANEL_GAP;
	let detail_x = collections_x + collections_w + LIBRARY_PANEL_GAP;

	build_starred_panel(&mut frame, vw, vh, UiRect::new(starred_x, top, starred_w, panel_h), state, mouse);
	build_collections_panel(&mut frame, vw, vh, UiRect::new(collections_x, top, collections_w, panel_h), state, mouse);
	build_detail_panel(&mut frame, vw, vh, UiRect::new(detail_x, top, detail_w, panel_h), state, mouse);

	finish(frame, ui)
}

fn build_starred_panel(frame: &mut EditorFrame, vw: f32, vh: f32, rect: UiRect, state: &ChipLibraryState, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	panel_bg(frame, ui, rect, [0.16, 0.16, 0.18, 0.98]);
	let header_h = 30.0;
	library_panel_header(frame, ui, UiRect::new(rect.x, rect.y, rect.w, header_h), "STARRED");

	let mut y = rect.y + header_h + 8.0;
	let row_w = rect.w - 16.0;
	for (i, item) in state.starred_list.iter().enumerate() {
		if y + ROW_H > rect.y + rect.h {
			break; // rest is scrolled off; no scroll offset state ported yet, matches `build_search_popup`
		}
		let row_rect = UiRect::new(rect.x + 8.0, y, row_w, ROW_H - 4.0);
		let is_selected = state.selection == LibrarySelection::Starred(i);
		let bg = if is_selected {
			[0.35, 0.45, 0.6, 1.0]
		} else if row_rect.contains(mouse) {
			[0.32, 0.32, 0.36, 1.0]
		} else {
			[0.22, 0.22, 0.25, 1.0]
		};
		panel_bg(frame, ui, row_rect, bg);
		let label = if item.is_collection { format!("[{}]", item.name) } else { item.name.clone() };
		add_label(frame, ui, row_rect.centre(), row_rect.w - 12.0, &label, theme::text_colour_for_background(bg), FONT_SIZE * 0.85);
		frame.buttons.push(EditorButton { rect: row_rect, action: EditorAction::SelectStarredRow(i), enabled: true });
		y += ROW_H;
	}
}

fn build_collections_panel(frame: &mut EditorFrame, vw: f32, vh: f32, rect: UiRect, state: &ChipLibraryState, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	panel_bg(frame, ui, rect, [0.16, 0.16, 0.18, 0.98]);
	let header_h = 30.0;
	library_panel_header(frame, ui, UiRect::new(rect.x, rect.y, rect.w, header_h), "COLLECTIONS");

	let mut y = rect.y + header_h + 8.0;
	let row_w = rect.w - 16.0;
	let bottom = rect.y + rect.h;
	'collections: for (ci, collection) in state.collections.iter().enumerate() {
		if y + ROW_H > bottom {
			break;
		}
		let header_rect = UiRect::new(rect.x + 8.0, y, row_w, ROW_H);
		let is_selected = state.selection == LibrarySelection::Collection(ci);
		let arrow = if collection.is_toggled_open { "v" } else { ">" };
		let bg = if is_selected {
			[0.35, 0.45, 0.6, 1.0]
		} else if header_rect.contains(mouse) {
			[0.3, 0.3, 0.34, 1.0]
		} else {
			[0.24, 0.24, 0.27, 1.0]
		};
		panel_bg(frame, ui, header_rect, bg);
		add_label(
			frame,
			ui,
			header_rect.centre(),
			row_w - 16.0,
			&format!("{arrow} {}", collection.name),
			theme::text_colour_for_background(bg),
			FONT_SIZE * 0.85,
		);
		frame.buttons.push(EditorButton { rect: header_rect, action: EditorAction::SelectCollection(ci), enabled: true });
		y += ROW_H + 3.0;

		if collection.is_toggled_open {
			for (chi, chip_name) in collection.chips.iter().enumerate() {
				if y + ROW_H * 0.85 > bottom {
					break 'collections;
				}
				let row_rect = UiRect::new(rect.x + 20.0, y, row_w - 12.0, ROW_H * 0.85);
				let is_chip_selected = state.selection == LibrarySelection::Chip(ci, chi);
				let bg = if is_chip_selected {
					[0.35, 0.45, 0.6, 1.0]
				} else if row_rect.contains(mouse) {
					[0.32, 0.32, 0.36, 1.0]
				} else {
					[0.22, 0.22, 0.25, 1.0]
				};
				panel_bg(frame, ui, row_rect, bg);
				add_label(frame, ui, row_rect.centre(), row_rect.w - 12.0, chip_name, theme::text_colour_for_background(bg), FONT_SIZE * 0.8);
				frame.buttons.push(EditorButton { rect: row_rect, action: EditorAction::SelectChipRow { collection: ci, chip: chi }, enabled: true });
				y += ROW_H * 0.85 + 3.0;
			}
		}
		y += 6.0;
	}
}

fn build_detail_panel(frame: &mut EditorFrame, vw: f32, vh: f32, rect: UiRect, state: &ChipLibraryState, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	panel_bg(frame, ui, rect, [0.16, 0.16, 0.18, 0.98]);
	let inner_x = rect.x + 12.0;
	let inner_w = rect.w - 24.0;
	let mut y = rect.y + 12.0;

	if state.confirming_chip_delete || state.confirming_collection_delete {
		add_label(
			frame,
			ui,
			Vec2::new(inner_x + inner_w / 2.0, y + 30.0),
			inner_w,
			state.delete_confirm_message,
			[0.95, 0.8, 0.4, 1.0],
			FONT_SIZE * 0.85,
		);
		y += 90.0;
		y = button_row(
			frame,
			ui,
			inner_x,
			y,
			inner_w,
			&[("CANCEL", EditorAction::CancelLibraryPopup, true), ("DELETE", EditorAction::ConfirmDelete, true)],
		);
		let _ = y;
		return;
	}

	match state.selection {
		LibrarySelection::Chip(ci, chi) => {
			let Some(collection) = state.collections.get(ci) else { return };
			let Some(chip_name) = collection.chips.get(chi) else { return };

			add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, y + 12.0), inner_w, &collection.name, [0.75, 0.9, 0.8, 1.0], FONT_SIZE * 0.8);
			y += 28.0;
			add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, y + 14.0), inner_w, chip_name, [1.0, 1.0, 1.0, 1.0], TITLE_FONT_SIZE * 0.75);
			y += 34.0;

			let starred = is_starred(state.starred_list, chip_name, false);
			let star_label = if starred { "REMOVE FROM STARRED" } else { "ADD TO STARRED" };
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[(star_label, EditorAction::ToggleStarred { name: chip_name.clone(), is_collection: false }, true)],
			);

			y = button_row(frame, ui, inner_x, y, inner_w, &[("USE", EditorAction::PlaceChip(chip_name.clone()), !state.selected_chip_would_cycle)]);

			let can_step_up = chi > 0;
			let can_step_down = chi < collection.chips.len() - 1;
			let can_jump_up = ci > 0;
			let can_jump_down = ci < state.collections.len() - 1;
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[
					("MOVE UP", EditorAction::MoveSelectedStep(false), can_step_up || can_jump_up),
					("MOVE DOWN", EditorAction::MoveSelectedStep(true), can_step_down || can_jump_down),
				],
			);
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[
					("JUMP UP", EditorAction::MoveSelectedJump(false), can_jump_up),
					("JUMP DOWN", EditorAction::MoveSelectedJump(true), can_jump_down),
				],
			);
			button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[
					("OPEN", EditorAction::OpenSelectedChip(chip_name.clone()), state.selected_chip_is_custom),
					("DELETE", EditorAction::RequestDeleteChip(chip_name.clone()), state.selected_chip_is_custom),
				],
			);
		}
		LibrarySelection::Collection(ci) => {
			let Some(collection) = state.collections.get(ci) else { return };
			add_label(
				frame,
				ui,
				Vec2::new(inner_x + inner_w / 2.0, y + 14.0),
				inner_w,
				&collection.name,
				[1.0, 1.0, 1.0, 1.0],
				TITLE_FONT_SIZE * 0.75,
			);
			y += 34.0;

			let starred = is_starred(state.starred_list, &collection.name, true);
			let star_label = if starred { "REMOVE FROM STARRED" } else { "ADD TO STARRED" };
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[(star_label, EditorAction::ToggleStarred { name: collection.name.clone(), is_collection: true }, true)],
			);
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[
					("MOVE UP", EditorAction::MoveSelectedStep(false), ci > 0),
					("MOVE DOWN", EditorAction::MoveSelectedStep(true), ci < state.collections.len() - 1),
				],
			);
			let can_edit = !collection.name.eq_ignore_ascii_case("OTHER");
			button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[("RENAME", EditorAction::BeginRenameCollection, can_edit), ("DELETE", EditorAction::RequestDeleteCollection, can_edit)],
			);
		}
		LibrarySelection::Starred(i) => {
			let Some(item) = state.starred_list.get(i) else { return };
			add_label(frame, ui, Vec2::new(inner_x + inner_w / 2.0, y + 14.0), inner_w, &item.name, [1.0, 1.0, 1.0, 1.0], TITLE_FONT_SIZE * 0.75);
			y += 34.0;

			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[("REMOVE FROM STARRED", EditorAction::ToggleStarred { name: item.name.clone(), is_collection: item.is_collection }, true)],
			);
			y = button_row(
				frame,
				ui,
				inner_x,
				y,
				inner_w,
				&[
					("MOVE UP", EditorAction::MoveSelectedStep(false), i > 0),
					("MOVE DOWN", EditorAction::MoveSelectedStep(true), i < state.starred_list.len() - 1),
				],
			);
			if !item.is_collection {
				button_row(
					frame,
					ui,
					inner_x,
					y,
					inner_w,
					&[
						("OPEN", EditorAction::OpenSelectedChip(item.name.clone()), state.selected_chip_is_custom),
						("DELETE", EditorAction::RequestDeleteChip(item.name.clone()), state.selected_chip_is_custom),
					],
				);
			}
		}
		LibrarySelection::None => {}
	}

	// New-collection / rename-collection controls anchored to the bottom of the panel.
	let footer_h = if state.creating_collection || state.renaming_collection { 96.0 } else { 84.0 };
	let footer_y = rect.y + rect.h - footer_h;

	if state.creating_collection || state.renaming_collection {
		let field_rect = UiRect::new(inner_x, footer_y, inner_w, 30.0);
		ui_kit::text_field_row(frame, ui, field_rect, state.name_field_text, "", FONT_SIZE, 12.0);
		let confirm_enabled = !state.name_field_text.trim().is_empty();
		let confirm_label = if state.renaming_collection { "RENAME" } else { "CREATE" };
		button_row(
			frame,
			ui,
			inner_x,
			footer_y + 36.0,
			inner_w,
			&[("CANCEL", EditorAction::CancelLibraryPopup, true), (confirm_label, EditorAction::ConfirmCollectionName, confirm_enabled)],
		);
	} else {
		add_button(frame, ui, UiRect::new(inner_x, footer_y, inner_w, 32.0), "NEW COLLECTION", EditorAction::BeginNewCollection, true);
		add_button(frame, ui, UiRect::new(inner_x, footer_y + 40.0, inner_w, 32.0), "EXIT LIBRARY", EditorAction::ExitLibrary, true);
	}
}

// ---------------------------------------------------------------------
// Search popup (`SearchPopup`)
// ---------------------------------------------------------------------

/// Builds the fullscreen chip-search overlay: a text field plus a
/// scrollable (here: simply clipped-to-viewport) list of chip names
/// containing `query` as a case-insensitive substring, matching
/// `SearchPopup`'s filtering.
pub fn build_search_popup(all_chip_names: &[String], query: &str, vw: f32, vh: f32, mouse: Vec2) -> EditorFrame {
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let panel_w = 420.0_f32.min(vw - 80.0);
	let cx = vw / 2.0;
	let top = vh * 0.07;

	let field_rect = UiRect::new(cx - panel_w / 2.0, top, panel_w, 36.0);
	ui_kit::text_field_row(&mut frame, ui, field_rect, query, "Search...", FONT_SIZE, 16.0);

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
		ui_kit::fill_rect(&mut frame, ui, row_rect, bg);
		add_label(&mut frame, ui, row_rect.centre(), row_rect.w - 16.0, name, theme::text_colour_for_background(bg), FONT_SIZE * 0.9);
		frame.buttons.push(EditorButton { rect: row_rect, action: EditorAction::UseChip((*name).clone()), enabled: true });
		y += ROW_H;
	}

	if filtered.is_empty() {
		add_label(&mut frame, ui, Vec2::new(cx, list_top + 20.0), panel_w, "No matching chips", [0.7, 0.7, 0.7, 1.0], FONT_SIZE * 0.9);
	}

	finish(frame, ui)
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
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let panel_w = 360.0;
	let panel_h = 150.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, ui, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	if !title.is_empty() {
		add_label(&mut frame, ui, Vec2::new(cx, panel_rect.y + 26.0), panel_w - 40.0, title, [1.0, 1.0, 1.0, 1.0], 20.0);
	}

	let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 46.0, panel_w - 60.0, 34.0);
	ui_kit::text_field_row(&mut frame, ui, field_rect, text, "", FONT_SIZE, 16.0);

	let confirm_rect = UiRect::new(cx - 186.0, panel_rect.y + panel_h - 46.0, 180.0, 36.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 46.0, 180.0, 36.0);
	add_button(&mut frame, ui, confirm_rect, "Confirm", EditorAction::ConfirmName, confirm_enabled);
	add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);

	finish(frame, ui)
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
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let panel_w = 320.0;
	let panel_h = 220.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, ui, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(
		&mut frame,
		ui,
		Vec2::new(cx, panel_rect.y + 30.0),
		panel_w - 30.0,
		"Press a key to rebind\n(alphanumeric only)",
		[1.0, 1.0, 1.0, 0.8],
		18.0,
	);

	let key_box = UiRect::new(cx - 35.0, panel_rect.y + 66.0, 70.0, 70.0);
	panel_bg(&mut frame, ui, key_box, [0.1, 0.1, 0.1, 1.0]);
	let shown = chosen_key.map(|c| c.to_string()).unwrap_or_default();
	add_label(&mut frame, ui, key_box.centre(), key_box.w, &shown, [1.0, 1.0, 1.0, 1.0], 27.0);

	let confirm_rect = UiRect::new(cx - 166.0, panel_rect.y + panel_h - 46.0, 160.0, 36.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 46.0, 160.0, 36.0);
	add_button(&mut frame, ui, confirm_rect, "Confirm", EditorAction::ConfirmKey, chosen_key.is_some());
	add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);

	finish(frame, ui)
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
	let ui = UiCtx::new(vw, vh, mouse);
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
	panel_bg(&mut frame, ui, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(&mut frame, ui, Vec2::new(cx, panel_rect.y + 20.0), panel_w - 30.0, "Configure ROM (256 x 16-bit)", [1.0, 1.0, 1.0, 1.0], 20.0);

	let selected = selected.min(ROM_WORD_COUNT - 1);
	let addr_label = format!("Address {selected} (0x{selected:02X})");
	add_label(&mut frame, ui, Vec2::new(panel_rect.x + 90.0, panel_rect.y + 52.0), 150.0, &addr_label, [0.85, 0.85, 0.85, 1.0], 15.0);

	let field_rect = UiRect::new(panel_rect.x + panel_w - 190.0, panel_rect.y + 38.0, 100.0, 30.0);
	ui_kit::text_field_row(&mut frame, ui, field_rect, edit_text, "", FONT_SIZE, 10.0);

	let set_rect = UiRect::new(panel_rect.x + panel_w - 80.0, panel_rect.y + 38.0, 60.0, 30.0);
	add_button(&mut frame, ui, set_rect, "Set", EditorAction::RomConfirmCell, true);

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
			panel_bg(&mut frame, ui, cell_rect, bg);
			add_label(&mut frame, ui, cell_rect.centre(), cell_rect.w - 4.0, &value.to_string(), theme::text_colour_for_background(bg), 11.0);
			frame.buttons.push(EditorButton { rect: cell_rect, action: EditorAction::RomSelectCell(idx), enabled: true });
		}
	}

	let apply_rect = UiRect::new(cx - 166.0, panel_rect.y + panel_h - 44.0, 160.0, 34.0);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 44.0, 160.0, 34.0);
	add_button(&mut frame, ui, apply_rect, "Apply", EditorAction::RomApply, true);
	add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);

	finish(frame, ui)
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
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let panel_w = 420.0;
	let panel_h = 178.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	panel_bg(&mut frame, ui, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	add_label(
		&mut frame,
		ui,
		Vec2::new(cx, panel_rect.y + 24.0),
		panel_w - 40.0,
		&format!("Save chip (currently: {current_name})"),
		[1.0, 1.0, 1.0, 1.0],
		18.0,
	);

	let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 48.0, panel_w - 60.0, 34.0);
	ui_kit::text_field_row(&mut frame, ui, field_rect, text, "", FONT_SIZE, 16.0);

	let hint = match mode {
		SaveChipMode::Save => "",
		SaveChipMode::Replace => "A different chip already has this name.",
		SaveChipMode::SaveAsOrRename => "Name changed -- keep both, or rename?",
	};
	if !hint.is_empty() {
		add_label(&mut frame, ui, Vec2::new(cx, panel_rect.y + 92.0), panel_w - 40.0, hint, [0.85, 0.65, 0.4, 1.0], 14.0);
	}

	let confirm_enabled = !text.trim().is_empty();
	let button_y = panel_rect.y + panel_h - 46.0;
	match mode {
		SaveChipMode::Save => {
			let confirm_rect = UiRect::new(cx - 186.0, button_y, 180.0, 36.0);
			let cancel_rect = UiRect::new(cx + 6.0, button_y, 180.0, 36.0);
			add_button(&mut frame, ui, confirm_rect, "Save", EditorAction::SaveChipConfirm, confirm_enabled);
			add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);
		}
		SaveChipMode::Replace => {
			let confirm_rect = UiRect::new(cx - 186.0, button_y, 180.0, 36.0);
			let cancel_rect = UiRect::new(cx + 6.0, button_y, 180.0, 36.0);
			add_button_coloured(&mut frame, ui, confirm_rect, "Replace", EditorAction::SaveChipConfirm, confirm_enabled, [0.62, 0.18, 0.18, 1.0]);
			add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);
		}
		SaveChipMode::SaveAsOrRename => {
			let w = (panel_w - 60.0 - 16.0) / 3.0;
			let save_as_rect = UiRect::new(panel_rect.x + 30.0, button_y, w, 36.0);
			let rename_rect = UiRect::new(save_as_rect.x + w + 8.0, button_y, w, 36.0);
			let cancel_rect = UiRect::new(rename_rect.x + w + 8.0, button_y, w, 36.0);
			add_button(&mut frame, ui, save_as_rect, "Save As", EditorAction::SaveChipSaveAs, confirm_enabled);
			add_button(&mut frame, ui, rename_rect, "Rename", EditorAction::SaveChipRename, confirm_enabled);
			add_button(&mut frame, ui, cancel_rect, "Cancel", EditorAction::ClosePopup, true);
		}
	}

	finish(frame, ui)
}

// ---------------------------------------------------------------------
// Starred bottom bar (`BottomBarUI`)
// ---------------------------------------------------------------------

/// Screen-pixel height of [`build_starred_bottom_bar`]'s strip -- the
/// host uses this to know how much room at the bottom of the window it
/// occupies (e.g. to keep it from covering anything else drawn there).
pub const BOTTOM_BAR_HEIGHT: f32 = 44.0;
const BOTTOM_BAR_BTN_GAP: f32 = 6.0;
pub const BOTTOM_BAR_BTN_PAD: f32 = 8.0;

/// Builds the persistent bottom bar of starred chips/collections --
/// mirrors the chip-button strip half of `BottomBarUI.DrawBottomBar`.
/// Its "MENU" dropdown (New/Save/Find/Library/Prefs/Quit) isn't ported
/// here since every one of those already has its own keyboard shortcut
/// in this port (see `bin/app.rs`'s shortcut handling), so the bar's
/// only new surface is starred access.
///
/// `scroll_x` is the bar's horizontal scroll offset in pixels: with more
/// starred items than fit the window width, the host scrolls the strip
/// (the UI stack routes wheel events over the bar to it instead of
/// letting them reach the canvas zoom underneath) and passes the clamped
/// offset back in here, shifting every button left by that amount.
///
/// A plain starred chip's button (left click) picks it up for placement
/// -- same as the library's "USE" button, see [`EditorAction::PlaceChip`]
/// -- and mirrors the original's `StartPlacing`. Greyed out (same
/// treatment a builtin's "Open" gets) when placing it into the currently
/// open chip would create a recursive cycle -- `cycle_blocked` is a
/// precomputed, case-insensitive set of such chip names (see
/// `bin/app.rs`'s `would_create_cycle`; this module has no `ChipLibrary`
/// access of its own to work it out). Right-clicking it instead opens a
/// small popup offering "Open" (switch to editing its definition) and
/// "Un-star", handled by the host the same way it handles every other
/// right-click popup (`bin/app.rs`'s
/// `handle_right_mouse_button`/`apply_context_menu_action`) -- this
/// module only draws/hit-tests the bar itself and has no popup state of
/// its own. A starred collection's button instead toggles
/// [`build_starred_collection_popup`] for it, same as clicking a
/// collection button in the original opens/closes its flyout rather than
/// acting directly.
/// Builds the persistent bottom bar of starred chips/collections --
/// mirrors the chip-button strip half of `BottomBarUI.DrawBottomBar`.
/// Its "MENU" dropdown (New/Save/Find/Library/Prefs/Quit) isn't ported
/// here since every one of those already has its own keyboard shortcut
/// in this port (see `bin/app.rs`'s shortcut handling), so the bar's
/// only new surface is starred access.
///
/// `scroll_x` is the bar's horizontal scroll offset in pixels: with more
/// starred items than fit the window width, the host scrolls the strip
/// (the UI stack routes wheel events over the bar to it instead of
/// letting them reach the canvas zoom underneath) and passes the clamped
/// offset back in here, shifting every button left by that amount.
///
/// A plain starred chip's button (left click) picks it up for placement
/// -- same as the library's "USE" button, see [`EditorAction::PlaceChip`]
/// -- and mirrors the original's `StartPlacing`. Greyed out (same
/// treatment a builtin's "Open" gets) when placing it into the currently
/// open chip would create a recursive cycle -- `cycle_blocked` is a
/// precomputed, case-insensitive set of such chip names (see
/// `bin/app.rs`'s `would_create_cycle`; this module has no `ChipLibrary`
/// access of its own to work it out). Right-clicking it instead opens a
/// small popup offering "Open" (switch to editing its definition) and
/// "Un-star", handled by the host the same way it handles every other
/// right-click popup (`bin/app.rs`'s
/// `handle_right_mouse_button`/`apply_context_menu_action`) -- this
/// module only draws/hit-tests the bar itself and has no popup state of
/// its own. A starred collection's button instead toggles
/// [`build_starred_collection_popup`] for it, same as clicking a
/// collection button in the original opens/closes its flyout rather than
/// acting directly.
pub fn build_starred_bottom_bar(
	starred_list: &[StarredItem],
	open_collection: Option<&str>,
	enabled: bool,
	cycle_blocked: &HashSet<String>,
	scroll_x: f32,
	ui: UiCtx,
) -> EditorFrame {
	let vw = ui.vw;
	let vh = ui.vh;
	let mut frame = EditorFrame::default();
	let bar_rect = UiRect::new(0.0, vh - BOTTOM_BAR_HEIGHT, vw, BOTTOM_BAR_HEIGHT);
	frame.panel = Some(bar_rect);
	panel_bg(&mut frame, ui, bar_rect, [0.13, 0.13, 0.14, 1.0]);

	let mut x = BOTTOM_BAR_BTN_PAD - scroll_x;
	let y = bar_rect.y + 4.0;
	let h = BOTTOM_BAR_HEIGHT - 8.0;
	for item in starred_list {
		let is_open = item.is_collection && open_collection == Some(item.name.as_str());
		let label = if item.is_collection { format!("{} v", item.name) } else { item.name.clone() };
		let w = (label.chars().count() as f32 * 8.5 + 24.0).clamp(60.0, 220.0);
		let rect = UiRect::new(x, y, w, h);
		let (action, row_enabled) = if item.is_collection {
			(EditorAction::ToggleStarredCollectionPopup(item.name.clone()), enabled)
		} else {
			(EditorAction::PlaceChip(item.name.clone()), enabled && !cycle_blocked.contains(&item.name.to_ascii_lowercase()))
		};
		if is_open {
			add_button_coloured(&mut frame, ui, rect, &label, action, row_enabled, [0.3, 0.42, 0.58, 1.0]);
		} else {
			add_button(&mut frame, ui, rect, &label, action, row_enabled);
		}
		x += w + BOTTOM_BAR_BTN_GAP;
	}

	finish(frame, ui)
}

/// Builds the flyout listing one starred collection's chips, opened by
/// clicking its button in [`build_starred_bottom_bar`]. Mirrors
/// `BottomBarUI.DrawCollectionsPopup`, simplified to a single column that
/// stops once it runs out of vertical room rather than wrapping into a
/// second column near the top of the screen -- the same "rest is
/// scrolled off, no offset state ported yet" simplification
/// `build_search_popup` already makes elsewhere in this module. Anchored
/// to grow upward from just above the bar, at `anchor_x` (the left edge
/// of the collection's own button in the bar, so the flyout lines up
/// under/over it). Each row is a chip picked up for placement on left
/// click (`EditorAction::PlaceChip`, same as the bar's own plain-chip
/// buttons) -- greyed out under the same `cycle_blocked` rule
/// [`build_starred_bottom_bar`] uses -- with a right-click "Open" popup,
/// but, being *inside* a collection rather than a bare starred chip, no
/// "Un-star" option (see [`build_starred_bottom_bar`]'s docs).
pub fn build_starred_collection_popup(
	collection: &ChipCollection,
	anchor_x: f32,
	enabled: bool,
	cycle_blocked: &HashSet<String>,
	vw: f32,
	vh: f32,
	mouse: Vec2,
) -> EditorFrame {
	let ui = UiCtx::new(vw, vh, mouse);
	let mut frame = EditorFrame::default();
	let w = 180.0_f32.min(vw - 40.0);
	let row_h = 30.0;
	let bottom = vh - BOTTOM_BAR_HEIGHT - 4.0;
	let x = anchor_x.clamp(4.0, vw - w - 4.0);

	let visible_rows = collection.chips.len().min(((bottom - 4.0) / row_h).floor().max(0.0) as usize);
	let top = bottom - visible_rows as f32 * row_h;
	let panel_rect = UiRect::new(x - 4.0, top, w + 8.0, bottom - top);
	frame.panel = Some(panel_rect);
	panel_bg(&mut frame, ui, panel_rect, [0.13, 0.13, 0.14, 0.98]);

	let mut y = bottom - row_h;
	for chip_name in collection.chips.iter().take(visible_rows) {
		let rect = UiRect::new(x, y, w, row_h - 4.0);
		let row_enabled = enabled && !cycle_blocked.contains(&chip_name.to_ascii_lowercase());
		add_button(&mut frame, ui, rect, chip_name, EditorAction::PlaceChip(chip_name.clone()), row_enabled);
		y -= row_h;
	}

	finish(frame, ui)
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

	fn sample_library_state<'a>(collections: &'a [ChipCollection], starred: &'a [StarredItem], selection: LibrarySelection) -> ChipLibraryState<'a> {
		ChipLibraryState {
			collections,
			starred_list: starred,
			selection,
			selected_chip_is_custom: true,
			selected_chip_would_cycle: false,
			creating_collection: false,
			renaming_collection: false,
			name_field_text: "",
			confirming_chip_delete: false,
			confirming_collection_delete: false,
			delete_confirm_message: "",
		}
	}

	#[test]
	fn chip_library_panel_only_lists_chips_for_open_collections() {
		let collections = vec![
			ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into(), "OR".into()] },
			ChipCollection { name: "CLOSED".into(), is_toggled_open: false, chips: vec!["XOR".into()] },
		];
		let state = sample_library_state(&collections, &[], LibrarySelection::None);
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

		let select_actions: Vec<_> = frame
			.buttons
			.iter()
			.filter_map(|b| if let EditorAction::SelectChipRow { collection, chip } = &b.action { Some((*collection, *chip)) } else { None })
			.collect();
		assert_eq!(select_actions, vec![(0, 0), (0, 1)]);

		let toggle_count = frame.buttons.iter().filter(|b| matches!(b.action, EditorAction::SelectCollection(_))).count();
		assert_eq!(toggle_count, 2);
	}

	#[test]
	fn chip_library_panel_shows_open_and_delete_for_the_selected_chip() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
		let state = sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0));
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
		let open_btn = frame.buttons.iter().find(|b| b.action == EditorAction::OpenSelectedChip("AND".to_string())).unwrap();
		assert!(open_btn.enabled);
		let delete_btn = frame.buttons.iter().find(|b| b.action == EditorAction::RequestDeleteChip("AND".to_string())).unwrap();
		assert!(delete_btn.enabled);
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

	#[test]
	fn chip_library_star_button_reads_add_or_remove_depending_on_starred_state() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
		let starred = vec![StarredItem::new("AND", false)];
		let state = sample_library_state(&collections, &starred, LibrarySelection::Chip(0, 0));
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ToggleStarred { name: "AND".to_string(), is_collection: false }));
		assert!(frame.geometry.labels.iter().any(|l| l.text == "REMOVE FROM STARRED"));
	}

	#[test]
	fn chip_library_move_buttons_disabled_at_the_ends_of_a_collection() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into(), "OR".into()] }];
		let state = sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0));
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
		let up = frame.buttons.iter().find(|b| b.action == EditorAction::MoveSelectedStep(false)).unwrap();
		assert!(!up.enabled, "first chip in the only collection can't move up or jump");
		let down = frame.buttons.iter().find(|b| b.action == EditorAction::MoveSelectedStep(true)).unwrap();
		assert!(down.enabled, "second chip in the collection is still below it");
	}

	#[test]
	fn chip_library_new_collection_footer_shows_a_text_field_when_creating() {
		let state = ChipLibraryState {
			creating_collection: true,
			name_field_text: "My Collection",
			..sample_library_state(&[], &[], LibrarySelection::None)
		};
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.text_field.is_some());
		let confirm = frame.buttons.iter().find(|b| b.action == EditorAction::ConfirmCollectionName).unwrap();
		assert!(confirm.enabled);
	}

	#[test]
	fn chip_library_delete_confirmation_hides_the_normal_detail_buttons() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
		let state = ChipLibraryState {
			confirming_chip_delete: true,
			delete_confirm_message: "Are you sure?",
			..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0))
		};
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ConfirmDelete));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::CancelLibraryPopup));
		assert!(!frame.buttons.iter().any(|b| matches!(b.action, EditorAction::OpenSelectedChip(_))));
	}

	#[test]
	fn chip_library_detail_panel_offers_use_for_both_custom_and_builtin_chips() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["AND".into()] }];
		let state = ChipLibraryState { selected_chip_is_custom: false, ..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0)) };
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

		let use_button = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("AND".to_string())).unwrap();
		assert!(use_button.enabled, "USE should place builtins too, unlike OPEN/DELETE");
	}

	#[test]
	fn chip_library_detail_panel_greys_out_use_for_a_chip_that_would_cycle() {
		let collections = vec![ChipCollection { name: "OPEN".into(), is_toggled_open: true, chips: vec!["SubCircuit".into()] }];
		let state = ChipLibraryState { selected_chip_would_cycle: true, ..sample_library_state(&collections, &[], LibrarySelection::Chip(0, 0)) };
		let frame = build_chip_library_panel(&state, 1280.0, 800.0, Vec2::ZERO);

		let use_button = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("SubCircuit".to_string())).unwrap();
		assert!(!use_button.enabled);
	}

	#[test]
	fn starred_bottom_bar_has_one_button_per_starred_item() {
		let starred = vec![StarredItem::new("AND", false), StarredItem::new("Basics", true)];
		let frame = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::PlaceChip("AND".to_string())));
		assert!(frame.buttons.iter().any(|b| b.action == EditorAction::ToggleStarredCollectionPopup("Basics".to_string())));
		assert_eq!(frame.panel, Some(UiRect::new(0.0, 800.0 - BOTTOM_BAR_HEIGHT, 1280.0, BOTTOM_BAR_HEIGHT)));
	}

	#[test]
	fn starred_bottom_bar_buttons_disabled_when_not_editable() {
		let starred = vec![StarredItem::new("AND", false)];
		let frame = build_starred_bottom_bar(&starred, None, false, &HashSet::new(), 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
		assert!(frame.buttons.iter().all(|b| !b.enabled));
	}

	#[test]
	fn starred_bottom_bar_scroll_x_shifts_buttons_left() {
		let starred: Vec<_> = (1..=6).map(|i| StarredItem::new(format!("ChipName{i}"), false)).collect();
		let unscrolled = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 0.0, UiCtx::new(600.0, 800.0, Vec2::ZERO));
		let scrolled = build_starred_bottom_bar(&starred, None, true, &HashSet::new(), 120.0, UiCtx::new(600.0, 800.0, Vec2::ZERO));
		for (a, b) in unscrolled.buttons.iter().zip(&scrolled.buttons) {
			assert!((a.rect.x - 120.0 - b.rect.x).abs() < 1e-3, "every button shifts left by the scroll offset");
			assert!((a.rect.y - b.rect.y).abs() < 1e-3 && a.rect.w == b.rect.w);
		}
	}

	#[test]
	fn starred_bottom_bar_greys_out_a_chip_that_would_cycle() {
		let starred = vec![StarredItem::new("AND", false), StarredItem::new("SubCircuit", false)];
		let blocked: HashSet<String> = ["subcircuit".to_string()].into_iter().collect();
		let frame = build_starred_bottom_bar(&starred, None, true, &blocked, 0.0, UiCtx::new(1280.0, 800.0, Vec2::ZERO));
		let and_btn = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("AND".to_string())).unwrap();
		let sub_btn = frame.buttons.iter().find(|b| b.action == EditorAction::PlaceChip("SubCircuit".to_string())).unwrap();
		assert!(and_btn.enabled);
		assert!(!sub_btn.enabled);
	}

	#[test]
	fn starred_collection_popup_has_one_button_per_chip_and_exposes_its_panel() {
		let collection = ChipCollection::new("Basics", ["AND", "OR", "NOT"]);
		let frame = build_starred_collection_popup(&collection, 20.0, true, &HashSet::new(), 1280.0, 800.0, Vec2::ZERO);
		let names: Vec<_> =
			frame.buttons.iter().filter_map(|b| if let EditorAction::PlaceChip(n) = &b.action { Some(n.clone()) } else { None }).collect();
		assert_eq!(names, vec!["AND".to_string(), "OR".to_string(), "NOT".to_string()]);

		// The panel rect is what the host turns into the flyout layer's capture region -- it must
		// cover every row so clicks on the padding between/around rows belong to the flyout.
		let panel = frame.panel.expect("the flyout exposes its background rect");
		for b in &frame.buttons {
			assert!(panel.x <= b.rect.x && b.rect.x + b.rect.w <= panel.x + panel.w);
			assert!(panel.y <= b.rect.y && b.rect.y + b.rect.h <= panel.y + panel.h);
		}
	}
}
