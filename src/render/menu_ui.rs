//! Builds drawable geometry + clickable hit-boxes for the app's startup
//! screen (project picker), driven by the headless [`crate::ui_menu::MainMenu`]
//! state machine.
//!
//! This is the "immediate mode" glue between `MainMenu` (pure state, no
//! drawing) and `render::gpu` (draws triangles/text, no app logic): each
//! frame, [`build`] is called with the current `MainMenu`, the viewport
//! size, and whatever text the player has typed into the currently-open
//! name popup (if any), and returns both the geometry to draw *and* the
//! list of clickable rectangles to hit-test the next mouse click/hover
//! against. Everything here is plain data -- no wgpu types -- so, like
//! `render::scene`, it's fully unit-testable without a GPU.
//!
//! Coordinate convention: all layout is done in *screen* pixel space,
//! origin top-left, +x right, +y down (i.e. the same space window/cursor
//! events arrive in). [`to_world`] converts a screen point into the world
//! coordinates `render::gpu` expects, for a camera positioned so that
//! world space and screen space coincide 1:1 (see `menu_camera` in
//! `src/bin/app.rs`).

use crate::render::scene::{SceneGeometry, TextLabel};
use crate::render::theme;
use crate::structs::Vec2;
use crate::ui_menu::{MainMenu, MenuScreen, PopupKind};

/// Converts a screen-space point (origin top-left, +y down) into the
/// world-space point that lands there when drawn through a camera
/// positioned at `(vw / 2, vh / 2)` with `zoom = 1.0` (see `menu_camera`
/// in `src/bin/app.rs`). The inverse of what `Camera::world_to_screen`
/// computes for that same camera.
pub fn to_world(screen: Vec2, vw: f32, vh: f32) -> Vec2 {
    let _ = vw; // kept for symmetry / clarity at call sites, x maps 1:1
    Vec2::new(screen.x, vh - screen.y)
}

/// An axis-aligned rectangle in screen pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl UiRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }

    fn centre(&self) -> Vec2 {
        Vec2::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Something a click on a `UiButton` should cause the host app to do.
/// Mirrors (a UI-level view of) `MainMenu`'s methods -- `src/bin/app.rs`
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

#[derive(Debug, Clone, PartialEq)]
pub struct UiButton {
    pub rect: UiRect,
    pub action: UiAction,
    pub enabled: bool,
}

/// Everything needed to draw one frame of the menu and to hit-test the
/// next mouse event against it.
#[derive(Debug, Default, Clone)]
pub struct MenuFrame {
    pub geometry: SceneGeometry,
    pub buttons: Vec<UiButton>,
    /// Hit-box of the text-entry field for the currently-open name popup,
    /// if any (purely informational right now -- the host treats "a name
    /// popup is open" as enough to route keyboard input to it regardless
    /// of click-to-focus, since there's at most one field on screen).
    pub text_field: Option<UiRect>,
    /// The button currently under `mouse`, if any -- convenience mirror of
    /// `buttons.iter().find(|b| b.rect.contains(mouse))` so callers don't
    /// have to redo the search themselves for hover styling.
    pub hovered: Option<UiAction>,
}

const BUTTON_W: f32 = 260.0;
const BUTTON_H: f32 = 44.0;
const BUTTON_GAP: f32 = 14.0;
const FONT_SIZE: f32 = 18.0;
const TITLE_FONT_SIZE: f32 = 40.0;

fn button_colour(enabled: bool, hovered: bool) -> theme::Rgba {
    if !enabled {
        theme::PIN_INVALID_COL
    } else if hovered {
        [0.45, 0.45, 0.5, 1.0]
    } else {
        theme::CHIP_BODY_COL
    }
}

/// Builds the full drawable + clickable frame for the current `MainMenu`
/// state. `text_input` is whatever the player has typed so far into the
/// currently-open name popup (ignored if no name popup is open).
/// `mouse` is the current cursor position in screen space, used purely to
/// compute `MenuFrame::hovered` / button hover colouring.
pub fn build(menu: &MainMenu, vw: f32, vh: f32, text_input: &str, mouse: Vec2) -> MenuFrame {
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

    if menu.popup() != PopupKind::None {
        build_popup(menu, vw, vh, text_input, &mut frame, mouse);
    }

    // Resolve hover + enabled-gated colouring now that every button for
    // this frame is known.
    frame.hovered = frame.buttons.iter().find(|b| b.rect.contains(mouse)).map(|b| b.action.clone());
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

fn add_label(frame: &mut MenuFrame, vw: f32, vh: f32, centre_x: f32, y: f32, width: f32, text: &str, colour: theme::Rgba, font_size: f32) {
    frame.geometry.labels.push(TextLabel {
        pos: to_world(Vec2::new(centre_x, y), vw, vh),
        text: text.to_string(),
        colour,
        font_size,
        width,
    });
}

/// Draws one button, appends its hit-box to `frame.buttons`, and returns
/// whether the mouse is currently over it (for callers that want to react
/// immediately, e.g. disabling a hover state on a disabled button).
fn add_button(frame: &mut MenuFrame, vw: f32, vh: f32, rect: UiRect, label: &str, action: UiAction, enabled: bool, mouse: Vec2) {
    let hovered = enabled && rect.contains(mouse);
    let bg = button_colour(enabled, hovered);
    frame.geometry.add_rect(to_world(rect.centre(), vw, vh), Vec2::new(rect.w, rect.h), bg);
    add_label(frame, vw, vh, rect.centre().x, rect.centre().y, rect.w - 12.0, label, theme::text_colour_for_background(bg), FONT_SIZE);
    frame.buttons.push(UiButton { rect, action, enabled });
}

fn build_main_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
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
        add_button(frame, vw, vh, rect, label, action, true, mouse);
        y += BUTTON_H + BUTTON_GAP;
    }
}

fn build_load_project_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
    add_title(frame, vw, vh, 60.0, "Load Project");

    let list_top = 120.0;
    let row_h = 40.0;
    let row_w = (vw - 80.0).min(760.0);
    let cx = vw / 2.0;

    if menu.projects().is_empty() {
        add_label(frame, vw, vh, cx, list_top + 30.0, row_w, "No projects yet -- create one from the main menu.", [0.8, 0.8, 0.8, 1.0], FONT_SIZE);
    }

    for (i, project) in menu.projects().iter().enumerate() {
        let y = list_top + i as f32 * (row_h + 6.0);
        let rect = UiRect::new(cx - row_w / 2.0, y, row_w, row_h);
        let selected = menu.selected_project_index() == Some(i);
        let compatible = crate::save_system::can_open_project(project).is_ok();

        let bg = if selected { [0.35, 0.45, 0.6, 1.0] } else if rect.contains(mouse) { [0.4, 0.4, 0.44, 1.0] } else { [0.3, 0.3, 0.33, 1.0] };
        frame.geometry.add_rect(to_world(rect.centre(), vw, vh), Vec2::new(rect.w, rect.h), bg);

        let text_colour = if compatible { theme::text_colour_for_background(bg) } else { [0.9, 0.35, 0.35, 1.0] };
        let label = if compatible {
            format!("{}   (saved {})", project.project_name, project.last_save_time)
        } else {
            format!("{}   (incompatible project version)", project.project_name)
        };
        add_label(frame, vw, vh, rect.centre().x, rect.centre().y, rect.w - 20.0, &label, text_colour, FONT_SIZE * 0.9);

        frame.buttons.push(UiButton { rect, action: UiAction::SelectProject(i), enabled: true });
    }

    let selected_compatible = matches!(menu.selected_project_compatibility(), Some(Ok(())));
    let toolbar_y = vh - 80.0;
    let mut x = cx - (BUTTON_W * 2.0 + BUTTON_GAP * 1.5);
    for (label, action, enabled) in [
        ("Open", UiAction::OpenSelected, selected_compatible),
        ("Rename", UiAction::RenameSelected, selected_compatible),
        ("Duplicate", UiAction::DuplicateSelected, selected_compatible),
        ("Delete", UiAction::DeleteSelected, menu.selected_project_index().is_some()),
    ] {
        let rect = UiRect::new(x, toolbar_y, BUTTON_W / 2.0 - 4.0, BUTTON_H);
        add_button(frame, vw, vh, rect, label, action, enabled, mouse);
        x += BUTTON_W / 2.0 + BUTTON_GAP / 2.0;
    }

    let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - 30.0, BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, back_rect, "Back", UiAction::BackToMain, true, mouse);
}

fn build_settings_screen(menu: &MainMenu, vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
    add_title(frame, vw, vh, 60.0, "Settings");
    let cx = vw / 2.0;
    let settings = menu.edited_settings();

    let vsync_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0, BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, vsync_rect, &format!("VSync: {}", if settings.vsync_enabled { "On" } else { "Off" }), UiAction::ToggleVsync, true, mouse);

    let fs_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0 + BUTTON_H + BUTTON_GAP, BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, fs_rect, &format!("Fullscreen: {:?}", settings.fullscreen_mode), UiAction::CycleFullscreenMode, true, mouse);

    let apply_rect = UiRect::new(cx - BUTTON_W / 2.0, 160.0 + 2.0 * (BUTTON_H + BUTTON_GAP), BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, apply_rect, "Apply", UiAction::ApplySettings, true, mouse);

    let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - 30.0, BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, back_rect, "Back", UiAction::BackToMain, true, mouse);
}

fn build_about_screen(vw: f32, vh: f32, frame: &mut MenuFrame, mouse: Vec2) {
    add_title(frame, vw, vh, 60.0, "About");
    let cx = vw / 2.0;
    add_label(
        frame,
        vw,
        vh,
        cx,
        180.0,
        vw - 160.0,
        "A Rust port of Sebastian Lague's Digital Logic Sim (rendering + save system + project picker).",
        [0.85, 0.85, 0.85, 1.0],
        FONT_SIZE,
    );
    let back_rect = UiRect::new(cx - BUTTON_W / 2.0, vh - 30.0, BUTTON_W, BUTTON_H);
    add_button(frame, vw, vh, back_rect, "Back", UiAction::BackToMain, true, mouse);
}

fn build_popup(menu: &MainMenu, vw: f32, vh: f32, text_input: &str, frame: &mut MenuFrame, mouse: Vec2) {
    let panel_w = 420.0;
    let panel_h = 200.0;
    let cx = vw / 2.0;
    let cy = vh / 2.0;

    let panel_rect = UiRect::new(cx - panel_w / 2.0, cy - panel_h / 2.0, panel_w, panel_h);
    frame.geometry.add_rect(to_world(panel_rect.centre(), vw, vh), Vec2::new(panel_w, panel_h), [0.18, 0.18, 0.2, 1.0]);

    let (title, is_name_popup) = match menu.popup() {
        PopupKind::NewProject => ("New Project", true),
        PopupKind::RenameProject => ("Rename Project", true),
        PopupKind::DuplicateProject => ("Duplicate Project", true),
        PopupKind::DeleteConfirmation => ("Delete Project?", false),
        PopupKind::None => ("", false),
    };
    add_label(frame, vw, vh, cx, panel_rect.y + 30.0, panel_w - 40.0, title, [1.0, 1.0, 1.0, 1.0], 22.0);

    if is_name_popup {
        let field_rect = UiRect::new(cx - (panel_w - 60.0) / 2.0, panel_rect.y + 70.0, panel_w - 60.0, 36.0);
        frame.geometry.add_rect(to_world(field_rect.centre(), vw, vh), Vec2::new(field_rect.w, field_rect.h), [0.08, 0.08, 0.09, 1.0]);
        let shown = if text_input.is_empty() { "|".to_string() } else { format!("{text_input}|") };
        add_label(frame, vw, vh, field_rect.centre().x, field_rect.centre().y, field_rect.w - 16.0, &shown, [1.0, 1.0, 1.0, 1.0], FONT_SIZE);
        frame.text_field = Some(field_rect);

        let valid = menu.popup() != PopupKind::NewProject || menu.is_valid_new_project_name(text_input);
        if !valid && !text_input.is_empty() {
            add_label(frame, vw, vh, cx, panel_rect.y + 118.0, panel_w - 40.0, "Invalid or already-used name", [0.9, 0.35, 0.35, 1.0], 14.0);
        }
    } else if let Some(project) = menu.selected_project() {
        add_label(
            frame,
            vw,
            vh,
            cx,
            panel_rect.y + 100.0,
            panel_w - 40.0,
            &format!("Delete '{}'? A backup copy will be kept.", project.project_name),
            [0.9, 0.9, 0.9, 1.0],
            15.0,
        );
    }

    let confirm_rect = UiRect::new(cx - BUTTON_W / 2.0 - 6.0 - 90.0, panel_rect.y + panel_h - 56.0, 180.0, 40.0);
    let cancel_rect = UiRect::new(cx + 6.0, panel_rect.y + panel_h - 56.0, 180.0, 40.0);
    let confirm_enabled = !is_name_popup || (menu.popup() != PopupKind::NewProject || menu.is_valid_new_project_name(text_input)) && !text_input.trim().is_empty();
    add_button(frame, vw, vh, confirm_rect, "Confirm", UiAction::PopupConfirm, confirm_enabled, mouse);
    add_button(frame, vw, vh, cancel_rect, "Cancel", UiAction::PopupCancel, true, mouse);
}

/// A status/error line the host app can overlay near the bottom of the
/// screen regardless of which `MainMenu` screen/popup is currently shown
/// (e.g. "Failed to open project: ..."). Kept separate from [`build`]
/// itself since it's app-level transient state (an `io::Error` message),
/// not something `MainMenu` tracks.
pub fn status_label(vw: f32, vh: f32, message: &str) -> TextLabel {
    TextLabel { pos: to_world(Vec2::new(vw / 2.0, vh - 14.0), vw, vh), text: message.to_string(), colour: [0.95, 0.75, 0.3, 1.0], font_size: 14.0, width: vw - 40.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_system::{create_project, test_util::temp_dir, SavePaths};

    #[test]
    fn ui_rect_contains_checks_bounds_inclusively() {
        let r = UiRect::new(10.0, 10.0, 100.0, 20.0);
        assert!(r.contains(Vec2::new(10.0, 10.0)));
        assert!(r.contains(Vec2::new(110.0, 30.0)));
        assert!(r.contains(Vec2::new(60.0, 20.0)));
        assert!(!r.contains(Vec2::new(9.0, 20.0)));
        assert!(!r.contains(Vec2::new(60.0, 31.0)));
    }

    #[test]
    fn to_world_round_trips_through_camera_world_to_screen() {
        // Mirrors the camera setup `src/bin/app.rs` uses for the menu: a
        // camera centred on the viewport with zoom 1.0, so world space and
        // screen space coincide (with a y-flip, since world is y-up and
        // screen is y-down).
        let vw = 1280.0;
        let vh = 800.0;
        let cam = crate::render::camera::Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport_width: vw, viewport_height: vh };

        let screen = Vec2::new(300.0, 150.0);
        let world = to_world(screen, vw, vh);
        let back = cam.world_to_screen(world);

        assert!((back.x - screen.x).abs() < 1e-3);
        assert!((back.y - screen.y).abs() < 1e-3);
    }

    #[test]
    fn main_screen_has_five_buttons_with_expected_actions() {
        let root = temp_dir("menu_ui_main_screen");
        let menu = MainMenu::new(SavePaths::new(&root));
        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);

        let actions: Vec<_> = frame.buttons.iter().map(|b| b.action.clone()).collect();
        assert_eq!(
            actions,
            vec![UiAction::NewProject, UiAction::OpenProjectScreen, UiAction::SettingsScreen, UiAction::AboutScreen, UiAction::Quit]
        );
        assert!(frame.buttons.iter().all(|b| b.enabled));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_project_screen_lists_projects_as_clickable_rows() {
        let root = temp_dir("menu_ui_load_screen");
        let paths = SavePaths::new(&root);
        create_project(&paths, "Alpha").unwrap();
        create_project(&paths, "Beta").unwrap();

        let mut menu = MainMenu::new(paths);
        menu.choose_open_project();

        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        let select_actions: Vec<_> = frame.buttons.iter().filter(|b| matches!(b.action, UiAction::SelectProject(_))).collect();
        assert_eq!(select_actions.len(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_project_screen_toolbar_buttons_disabled_without_a_selection() {
        let root = temp_dir("menu_ui_toolbar_disabled");
        let paths = SavePaths::new(&root);
        create_project(&paths, "Alpha").unwrap();
        let mut menu = MainMenu::new(paths);
        menu.choose_open_project();

        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        let open_btn = frame.buttons.iter().find(|b| b.action == UiAction::OpenSelected).unwrap();
        let delete_btn = frame.buttons.iter().find(|b| b.action == UiAction::DeleteSelected).unwrap();
        assert!(!open_btn.enabled);
        assert!(!delete_btn.enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_project_screen_toolbar_buttons_enabled_once_a_compatible_project_is_selected() {
        let root = temp_dir("menu_ui_toolbar_enabled");
        let paths = SavePaths::new(&root);
        create_project(&paths, "Alpha").unwrap();
        let mut menu = MainMenu::new(paths);
        menu.choose_open_project();
        menu.select_project(0);

        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        let open_btn = frame.buttons.iter().find(|b| b.action == UiAction::OpenSelected).unwrap();
        let rename_btn = frame.buttons.iter().find(|b| b.action == UiAction::RenameSelected).unwrap();
        let delete_btn = frame.buttons.iter().find(|b| b.action == UiAction::DeleteSelected).unwrap();
        assert!(open_btn.enabled);
        assert!(rename_btn.enabled);
        assert!(delete_btn.enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn new_project_popup_shows_a_text_field_and_disables_confirm_for_invalid_names() {
        let root = temp_dir("menu_ui_new_project_popup");
        let paths = SavePaths::new(&root);
        let mut menu = MainMenu::new(paths);
        menu.choose_new_project();

        let frame_empty = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        assert!(frame_empty.text_field.is_some());
        let confirm = frame_empty.buttons.iter().find(|b| b.action == UiAction::PopupConfirm).unwrap();
        assert!(!confirm.enabled, "empty name should not be confirmable");

        let frame_valid = build(&menu, 1280.0, 800.0, "My New Project", Vec2::ZERO);
        let confirm = frame_valid.buttons.iter().find(|b| b.action == UiAction::PopupConfirm).unwrap();
        assert!(confirm.enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_confirmation_popup_has_no_text_field() {
        let root = temp_dir("menu_ui_delete_popup");
        let paths = SavePaths::new(&root);
        create_project(&paths, "Doomed").unwrap();
        let mut menu = MainMenu::new(paths);
        menu.choose_open_project();
        menu.select_project(0);
        menu.request_delete_selected();

        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        assert!(frame.text_field.is_none());
        assert!(frame.buttons.iter().any(|b| b.action == UiAction::PopupConfirm));
        assert!(frame.buttons.iter().any(|b| b.action == UiAction::PopupCancel));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hovered_reports_the_button_under_the_mouse() {
        let root = temp_dir("menu_ui_hovered");
        let menu = MainMenu::new(SavePaths::new(&root));
        let frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        let new_project_btn = frame.buttons.iter().find(|b| b.action == UiAction::NewProject).unwrap();
        let inside = Vec2::new(new_project_btn.rect.x + 5.0, new_project_btn.rect.y + 5.0);

        let frame_hovered = build(&menu, 1280.0, 800.0, "", inside);
        assert_eq!(frame_hovered.hovered, Some(UiAction::NewProject));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn status_label_is_positioned_near_the_bottom_of_the_viewport() {
        let label = status_label(1280.0, 800.0, "oh no");
        assert_eq!(label.text, "oh no");
        // world.y = vh - screen.y, and the label is placed near screen
        // bottom (small vh - y), so its world y should be small too.
        assert!(label.pos.y < 30.0);
    }

    #[test]
    fn about_and_settings_screens_render_a_back_button() {
        let root = temp_dir("menu_ui_about_settings");
        let paths = SavePaths::new(&root);
        let mut menu = MainMenu::new(paths);

        menu.choose_about();
        let about_frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        assert!(about_frame.buttons.iter().any(|b| b.action == UiAction::BackToMain));

        menu.choose_settings();
        let settings_frame = build(&menu, 1280.0, 800.0, "", Vec2::ZERO);
        assert!(settings_frame.buttons.iter().any(|b| b.action == UiAction::BackToMain));
        assert!(settings_frame.buttons.iter().any(|b| b.action == UiAction::ToggleVsync));

        std::fs::remove_dir_all(&root).ok();
    }
}
