//! Builds drawable geometry + clickable hit-boxes for the app's startup screen (project picker),
//! driven by the headless [`crate::ui_menu::MainMenu`] state machine. This is the immediate-mode
//! glue between `MainMenu` (pure state, no drawing) and `render::gpu` (draws triangles/text, no app
//! logic). Everything here is plain data -- no wgpu types -- so it's fully unit-testable without a
//! GPU. Layout is done in screen pixel space; [`to_world`] converts to the world space `render::gpu` expects.

use crate::render::foundation::TextLabel;
use crate::render::theme;
use crate::render::ui_kit::{self, Frame, UiCtx};
use crate::structs::Vec2;
use crate::ui_menu::{MainMenu, MenuScreen, PopupKind};

pub use crate::render::ui_kit::{to_world, UiRect};

/// Something a click on a `UiButton` should cause the host app to do.
/// Mirrors (a UI-level view of) `MainMenu`'s methods -- `viewer::app`
/// matches on this and calls the corresponding `MainMenu` method / does
/// the corresponding app-level transition (e.g. actually opening a
/// project into the viewer).
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
	NewProject,
	OpenProjectScreen,
	SettingsScreen,
	AboutScreen,
	Quit,
	BackToMain,
	SelectProject(usize),
	OpenSelected,
	RenameSelected,
	DuplicateSelected,
	DeleteSelected,
	RefreshProjects,
	PopupConfirm,
	PopupCancel,
	ToggleVsync,
	CycleFullscreenMode,
	ApplySettings,
}

/// Hit-box of one clickable region of a [`MenuFrame`] -- see [`ui_kit::Button`].
pub type UiButton = ui_kit::Button<UiAction>;

/// Everything needed to draw one frame of the menu and to hit-test the
/// next mouse event against it. Its `text_field` is the hit-box of the
/// text-entry field for the currently-open name popup, if any (purely
/// informational right now -- the host treats "a name popup is open" as
/// enough to route keyboard input to it regardless of click-to-focus,
/// since there's at most one field on screen). See [`ui_kit::Frame`].
pub type MenuFrame = Frame<UiAction>;

const BUTTON_W: f32 = 260.0;
const BUTTON_H: f32 = 44.0;
const BUTTON_GAP: f32 = 14.0;
const FONT_SIZE: f32 = ui_kit::FONT_SIZE;
const TITLE_FONT_SIZE: f32 = 40.0;

/// Builds the full drawable + clickable frame for the current `MainMenu`
/// state. `text_input` is whatever the player has typed so far into the
/// currently-open name popup (ignored if no name popup is open).
/// `mouse` is the current cursor position in screen space, used purely to
/// compute `MenuFrame::hovered` / button hover colouring.
pub fn build(menu: &MainMenu, vw: f32, vh: f32, text_input: &str, mouse: Vec2) -> MenuFrame {
	let mut frame = build_screen(menu, vw, vh, mouse);

	if menu.popup() != PopupKind::None {
		build_popup(menu, vw, vh, text_input, &mut frame, mouse);
		// Resolve hover + enabled-gated colouring now that every button for
		// this frame is known.
		frame.hovered = ui_kit::hovered_button(&frame.buttons, mouse, false);
	}
	frame
}

/// Just the current screen (main menu / load-project list / settings /
/// about), *without* the popup baked in -- so a caller that wants the
/// popup guaranteed to composite (triangles *and* text) on top of this,
/// rather than sharing one triangles-then-text pass with it, can render
/// them as two separate layers. See `build_popup_frame`.
pub fn build_screen(menu: &MainMenu, vw: f32, vh: f32, mouse: Vec2) -> MenuFrame {
	let mut frame = MenuFrame::default();

	// Background fill so the menu fully occludes whatever was drawn
	// last frame (there's no depth buffer, so draw order is z-order --
	// this must be first).
	frame.geometry.add_rect(to_world(Vec2::new(vw / 2.0, vh / 2.0), vw, vh), Vec2::new(vw, vh), theme::BACKGROUND_COL);

	match menu.screen() {
		MenuScreen::Main => build_main_screen(menu, vw, vh, &mut frame, mouse),
		MenuScreen::LoadProject => build_load_project_screen(menu, vw, vh, &mut frame, mouse),
		MenuScreen::Settings => build_settings_screen(menu, vw, vh, &mut frame, mouse),
		MenuScreen::About => build_about_screen(vw, vh, &mut frame, mouse),
	}

	frame.hovered = ui_kit::hovered_button(&frame.buttons, mouse, false);
	frame
}

/// Just the popup (if one is open; an empty frame otherwise) as its own
/// standalone frame -- render this as a later/separate layer than
/// `build_screen`'s so it's guaranteed to composite fully (background
/// *and* its own text) on top of the screen underneath, and so it can be
/// hit-tested on its own ahead of (and instead of, when open) the
/// buttons underneath it. See `build_screen`'s docs.
pub fn build_popup_frame(menu: &MainMenu, vw: f32, vh: f32, text_input: &str, mouse: Vec2) -> MenuFrame {
	let mut frame = MenuFrame::default();
	if menu.popup() != PopupKind::None {
		build_popup(menu, vw, vh, text_input, &mut frame, mouse);
		frame.hovered = ui_kit::hovered_button(&frame.buttons, mouse, false);
	}
	frame
}

fn add_title(frame: &mut MenuFrame, vw: f32, vh: f32, y: f32, text: &str) {
	frame.geometry.labels.push(TextLabel {
		pos: to_world(Vec2::new(vw / 2.0, y), vw, vh),
		text: text.to_string(),
		colour: [1.0, 1.0, 1.0, 1.0],
		font_size: TITLE_FONT_SIZE,
		width: vw - 40.0,
	});
}

fn add_label(frame: &mut MenuFrame, ui: UiCtx, centre: Vec2, width: f32, text: &str, colour: theme::Rgba, font_size: f32) {
	ui_kit::add_label(frame, ui, centre, width, text, colour, font_size);
}

fn add_button(frame: &mut MenuFrame, ui: UiCtx, rect: UiRect, label: &str, action: UiAction, enabled: bool) {
	ui_kit::add_button(frame, ui, rect, label, action, enabled, None);
}

fn build_main_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	add_title(frame, vw, vh, 90.0, "Digital Logic Sim");

	let cx = vw / 2.0;
	let mut y = 220.0;
	let entries: [(&str, UiAction); 5] = [
		("New Project", UiAction::NewProject),
		("Open Project", UiAction::OpenProjectScreen),
		("Settings", UiAction::SettingsScreen),
		("About", UiAction::AboutScreen),
		("Quit", UiAction::Quit),
	];
	let _ = menu;
	for (label, action) in entries {
		let rect = UiRect::new(cx - BUTTON_W / 2.0, y, BUTTON_W, BUTTON_H);
		add_button(frame, ui, rect, label, action, true);
		y += BUTTON_H + BUTTON_GAP;
	}
}

fn build_load_project_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	add_title(frame, vw, vh, 60.0, "Load Project");

	let list_top = 120.0;
	let row_h = 40.0;
	let row_w = (vw - 80.0).min(760.0);
	let cx = vw / 2.0;

	if menu.projects().is_empty() {
		add_label(
			frame,
			ui,
			Vec2::new(cx, list_top + 30.0),
			row_w,
			"No projects yet -- create one from the main menu.",
			[0.8, 0.8, 0.8, 1.0],
			FONT_SIZE,
		);
	}

	for (i, project) in menu.projects().iter().enumerate() {
		let y = list_top + i as f32 * (row_h + 6.0);
		let rect = UiRect::new(cx - row_w / 2.0, y, row_w, row_h);
		let selected = menu.selected_project_index() == Some(i);
		let compatible = crate::save_system::can_open_project(project).is_ok();

		let bg = if selected {
			[0.35, 0.45, 0.6, 1.0]
		} else if rect.contains(mouse) {
			[0.4, 0.4, 0.44, 1.0]
		} else {
			[0.3, 0.3, 0.33, 1.0]
		};
		ui_kit::fill_rect(frame, ui, rect, bg);

		let text_colour = if compatible { theme::text_colour_for_background(bg) } else { [0.9, 0.35, 0.35, 1.0] };
		let label = if compatible {
			format!("{}   (saved {} ago)", project.project_name, crate::save_system::to_relative_time(&project.last_save_time))
		} else {
			format!("{}   (incompatible project version)", project.project_name)
		};
		add_label(frame, ui, rect.centre(), rect.w - 20.0, &label, text_colour, FONT_SIZE * 0.9);

		frame.buttons.push(UiButton { rect, action: UiAction::SelectProject(i), enabled: true });
	}

	let selected_compatible = matches!(menu.selected_project_compatibility(), Some(Ok(())));
	let toolbar_y = vh - BUTTON_H * 3.0;
	let mut x = cx - BUTTON_W;
	for (label, action, enabled) in [
		("Open", UiAction::OpenSelected, selected_compatible),
		("Rename", UiAction::RenameSelected, selected_compatible),
		("Duplicate", UiAction::DuplicateSelected, selected_compatible),
		("Delete", UiAction::DeleteSelected, menu.selected_project_index().is_some()),
	] {
		let rect = UiRect::new(x, toolbar_y, BUTTON_W / 2.0 - 4.0, BUTTON_H);
		add_button(frame, ui, rect, label, action, enabled);
		x += BUTTON_W / 2.0 + BUTTON_GAP / 2.0;
	}

	let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - BUTTON_H * 1.2, BUTTON_W, BUTTON_H);
	add_button(frame, ui, back_rect, "Back", UiAction::BackToMain, true);
}

fn build_settings_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	add_title(frame, vw, vh, 60.0, "Settings");
	let cx = vw / 2.0;
	let settings = menu.edited_settings();

	let vsync_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0, BUTTON_W, BUTTON_H);
	add_button(frame, ui, vsync_rect, &format!("VSync: {}", if settings.vsync_enabled { "On" } else { "Off" }), UiAction::ToggleVsync, true);

	let fs_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0 + BUTTON_H + BUTTON_GAP, BUTTON_W, BUTTON_H);
	add_button(frame, ui, fs_rect, &format!("Fullscreen: {:?}", settings.fullscreen_mode), UiAction::CycleFullscreenMode, true);

	let apply_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0 + 2.0 * (BUTTON_H + BUTTON_GAP), BUTTON_W, BUTTON_H);
	add_button(frame, ui, apply_rect, "Apply", UiAction::ApplySettings, true);

	let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - 30.0, BUTTON_W, BUTTON_H);
	add_button(frame, ui, back_rect, "Back", UiAction::BackToMain, true);
}

fn build_about_screen(vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	add_title(frame, vw, vh, 60.0, "About");
	let cx = vw / 2.0;
	add_label(
		frame,
		ui,
		Vec2::new(cx, 180.0),
		vw - 160.0,
		"A Rust port of Sebastian Lague's Digital Logic Sim (rendering + save system + project picker).",
		[0.85, 0.85, 0.85, 1.0],
		FONT_SIZE,
	);
	let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - 30.0, BUTTON_W, BUTTON_H);
	add_button(frame, ui, back_rect, "Back", UiAction::BackToMain, true);
}

fn build_popup(menu: &MainMenu, vw: f32, vh: f32, text_input: &str, frame: &mut MenuFrame, mouse: Vec2) {
	let ui = UiCtx::new(vw, vh, mouse);
	let panel_w = 420.0;
	let panel_h = 200.0;
	let cx = vw / 2.0;
	let cy = vh / 2.0;

	let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
	ui_kit::fill_rect(frame, ui, panel_rect, [0.18, 0.18, 0.2, 1.0]);

	let (title, is_name_popup) = match menu.popup() {
		PopupKind::NewProject => ("New Project", true),
		PopupKind::RenameProject => ("Rename Project", true),
		PopupKind::DuplicateProject => ("Duplicate Project", true),
		PopupKind::DeleteConfirmation => ("Delete Project?", false),
		PopupKind::None => ("", false),
	};
	add_label(frame, ui, Vec2::new(cx, panel_rect.y + 30.0), panel_w - 40.0, title, [1.0, 1.0, 1.0, 1.0], 22.0);

	if is_name_popup {
		let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 70.0, panel_w - 60.0, 36.0);
		ui_kit::text_field_row(frame, ui, field_rect, text_input, "", FONT_SIZE, 16.0);

		let valid = menu.popup() != PopupKind::NewProject || menu.is_valid_new_project_name(text_input);
		if !valid && !text_input.is_empty() {
			add_label(frame, ui, Vec2::new(cx, panel_rect.y + 118.0), panel_w - 40.0, "Invalid or already-used name", [0.9, 0.35, 0.35, 1.0], 14.0);
		}
	} else if let Some(project) = menu.selected_project() {
		add_label(
			frame,
			ui,
			Vec2::new(cx, panel_rect.y + 100.0),
			panel_w - 40.0,
			&format!("Delete '{}'? A backup copy will be kept.", project.project_name),
			[0.9, 0.9, 0.9, 1.0],
			15.0,
		);
	}

	let confirm_rect = UiRect::new(cx - BUTTON_W / 2.0 - 6.0 - 90.0, panel_rect.y + panel_h - 56.0, 180.0, 40.0).clamp_to(panel_rect);
	let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 56.0, 180.0, 40.0).clamp_to(panel_rect);
	let confirm_enabled =
		!is_name_popup || (menu.popup() != PopupKind::NewProject || menu.is_valid_new_project_name(text_input)) && !text_input.trim().is_empty();
	add_button(frame, ui, confirm_rect, "Confirm", UiAction::PopupConfirm, confirm_enabled);
	ui_kit::add_button(frame, ui, cancel_rect, "Cancel", UiAction::PopupCancel, true, Some(crate::render::theme::DANGEROUS_ACTION_COL));
}

/// A status/error line the host app can overlay near the bottom of the
/// screen regardless of which `MainMenu` screen/popup is currently shown
/// (e.g. "Failed to open project: ..."). Kept separate from [`build`]
/// itself since it's app-level transient state (an `io::Error` message),
/// not something `MainMenu` tracks.
pub fn status_label(vw: f32, vh: f32, message: &str) -> TextLabel {
	TextLabel {
		pos: to_world(Vec2::new(vw / 2.0, vh - 14.0), vw, vh),
		text: message.to_string(),
		colour: [0.95, 0.75, 0.3, 1.0],
		font_size: 14.0,
		width: vw - 40.0,
	}
}
