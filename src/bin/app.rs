//! The actual, integrated Digital Logic Sim app: `cargo run` opens a
//! project-picker startup screen (same on-disk layout and location as the
//! original Unity build -- see `SavePaths::unity_persistent_data_dir`),
//! lets you open an existing project or create a new one, then switches
//! the *same window* over to the chip viewer (`render::gpu` + the scene
//! builder) for whichever project you picked.
//!
//! This is the "host app" the doc comments in `ui_menu` (headless menu
//! logic) and `src/bin/viewer.rs` (viewer-only glue) describe as needing
//! to exist: it drives `MainMenu` from real mouse/keyboard events via
//! `render::menu_ui`, and reuses the exact same load/build/render
//! sequence `viewer.rs` uses once a project is opened.
//!
//! Usage:
//! ```text
//! cargo run                       # opens the picker at the default (Unity-compatible) save location
//! cargo run -- <path-to-data-dir> # opens the picker at a custom save-data root instead
//! ```
//! The `<path-to-data-dir>` is the *root* that contains `Projects/`, not a
//! project directory itself (that distinction is what lets the picker list
//! more than one project) -- if you want to jump straight into viewing one
//! project non-interactively, use `cargo run --bin viewer -- <project-dir>`
//! instead.
//!
//! Like `viewer.rs`, this needs a real GPU adapter + window/display server
//! to run, so it can't be exercised by `cargo test` in a headless sandbox.
//! Its non-GPU pieces (`ui_menu::MainMenu`, `render::menu_ui`) are
//! independently unit-tested and drive this file's control flow, so a bug
//! here is most likely in this glue rather than in the tested state
//! machine underneath it.

use logic_sim::json::ProjectDescription;
use logic_sim::render::camera::Camera;
use logic_sim::render::editor_ui::{self, EditorAction, EditorButton};
use logic_sim::render::gpu::Renderer;
use logic_sim::render::menu_ui::{self, UiAction};
use logic_sim::render::scene::{bounding_box, build_grid, build_scene, AllLow, SceneGeometry, SimulatorPinState};
use logic_sim::render::theme;
use logic_sim::sim::Simulator;
use logic_sim::structs::Vec2;
use logic_sim::ui_menu::{MainMenu, MenuOutcome, PopupKind};
use logic_sim::{default_chip_collections, load_project, register_all_builtins, ChipLibrary, SavePaths, Saver};
use std::path::PathBuf;
use std::sync::Arc;
use logic_sim::sim::key_mods_bits;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Which (if any) of the editor overlays from `render::editor_ui` is
/// currently open on top of the viewer. Only one at a time -- matches how
/// the original's popups/menus stack (library sidebar aside, which can
/// stay open alongside browsing, everything else here is modal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Library,
    Search,
    Preferences,
    Naming,
    KeySelect,
}

/// Convert winit's modifier state into the `Simulator::key_modifiers`
/// bitmask (see `key_mods_bits`), using winit's own boolean accessors
/// rather than its raw `bits()` value -- see the doc comment on
/// `key_mods_bits` for why.
fn encode_modifiers(mods: ModifiersState) -> u32 {
    let mut bits = 0u32;
    if mods.shift_key() {
        bits |= key_mods_bits::SHIFT;
    }
    if mods.control_key() {
        bits |= key_mods_bits::CONTROL;
    }
    if mods.alt_key() {
        bits |= key_mods_bits::ALT;
    }
    if mods.super_key() {
        bits |= key_mods_bits::SUPER;
    }
    bits
}

/// State specific to viewing/simulating one open project's chip, split out
/// so `App` can hold either this or the menu depending on `Screen`.
struct ViewerState {
    project_name: String,
    library: ChipLibrary,
    root_chip_name: String,
    sim: Simulator,
    camera: Camera,
    dragging: bool,
    last_cursor: Vec2,
    camera_fitted: bool,
    show_grid: bool,

    /// The project's saved prefs/collections, edited live by the
    /// preferences/library overlays and written back to disk on Apply.
    prefs: ProjectDescription,
    overlay: Overlay,
    /// Shared text buffer for whichever overlay currently has a text
    /// field open (search query, or the naming popup's text).
    overlay_text_input: String,
    /// Pending key choice for the key-select popup.
    overlay_key_choice: Option<char>,
    /// Hit-boxes from the overlay's *last drawn* frame -- same
    /// immediate-mode pattern as `App::last_menu_buttons`.
    last_overlay_buttons: Vec<EditorButton>,
}

impl ViewerState {
    fn rebuild_sim(&mut self) {
        let root_desc = self.library.get(&self.root_chip_name).clone();
        let held_keys = std::mem::take(&mut self.sim.held_keys);
        let key_modifiers = self.sim.key_modifiers;
        self.sim = Simulator::build(&root_desc, &self.library);
        self.sim.held_keys = held_keys;
        self.sim.key_modifiers = key_modifiers;
        self.camera_fitted = false;
    }
}

/// `editor_ui`'s builders lay out screen-pixel coordinates as if drawn
/// through a fixed camera positioned at `(vw/2, vh/2)` with `zoom = 1.0`
/// (see `menu_ui::to_world`, the same convention the main menu uses) --
/// appropriate for `Screen::Menu`, where that's exactly the camera used.
/// The viewer, though, draws its scene through `v.camera`, which pans and
/// zooms freely. Re-mapping each overlay vertex/label from "the pixel it
/// was drawn at under the fixed camera" to "the world point that lands on
/// that same pixel under `v.camera`" keeps overlays pinned to the screen
/// (constant position and size in pixels) no matter how far the chip
/// canvas underneath has been panned/zoomed, using one real render pass
/// instead of needing a second camera/pipeline in `render::gpu`.
fn pin_overlay_to_screen(mut geometry: SceneGeometry, camera: &Camera, vw: f32, vh: f32) -> SceneGeometry {
    let to_screen_px = |world: Vec2| Vec2::new(world.x, vh - world.y); // inverse of `menu_ui::to_world`, which is its own inverse
    for v in &mut geometry.triangles {
        v.pos = camera.screen_to_world(to_screen_px(v.pos));
    }
    for l in &mut geometry.labels {
        l.pos = camera.screen_to_world(to_screen_px(l.pos));
        l.font_size /= camera.zoom;
        l.width /= camera.zoom;
    }
    geometry
}

/// Advances the wheel field at `row_index` (matching the row order
/// `editor_ui::build_preferences_panel` draws in) to its next option,
/// wrapping around.
fn cycle_pref(prefs: &mut ProjectDescription, row_index: usize) {
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

/// Applies a click on one of the editor overlays. A free function (not an
/// `App` method) so it can be called from inside a `match &mut self.screen`
/// arm that's already holding `v`, while still touching the sibling
/// `self.paths` / `self.status` fields -- see `App::handle_mouse_button`.
fn apply_editor_action(v: &mut ViewerState, paths: &SavePaths, status: &mut Option<String>, action: EditorAction) {
    match action {
        EditorAction::ClosePopup => {
            v.overlay = Overlay::None;
            v.overlay_text_input.clear();
        }
        EditorAction::CyclePref(i) => cycle_pref(&mut v.prefs, i),
        EditorAction::ApplyPreferences => {
            v.show_grid = v.prefs.prefs_grid_display_mode == 1;
            let mut desc = v.prefs.clone();
            match Saver::save_project_description(paths, &mut desc) {
                Ok(()) => v.prefs = desc,
                Err(e) => *status = Some(format!("Failed to save preferences: {e}")),
            }
            v.overlay = Overlay::None;
        }
        EditorAction::SelectChip(name) => {
            if v.library.iter().any(|d| d.name == name) {
                v.root_chip_name = name;
                v.rebuild_sim();
            } else {
                *status = Some(format!("Chip '{name}' not found in library"));
            }
        }
        EditorAction::ToggleCollection(i) => {
            if let Some(c) = v.prefs.chip_collections.get_mut(i) {
                c.is_toggled_open = !c.is_toggled_open;
            }
        }
        EditorAction::UseChip(name) => {
            if v.library.iter().any(|d| d.name == name) {
                v.root_chip_name = name;
                v.rebuild_sim();
            } else {
                *status = Some(format!("Chip '{name}' not found in library"));
            }
            v.overlay = Overlay::None;
            v.overlay_text_input.clear();
        }
        EditorAction::ConfirmName => {
            let trimmed = v.overlay_text_input.trim().to_string();
            if !trimmed.is_empty() {
                v.project_name = trimmed;
            }
            v.overlay = Overlay::None;
        }
        EditorAction::ChooseKey(c) => v.overlay_key_choice = Some(c),
        EditorAction::ConfirmKey => {
            if let Some(c) = v.overlay_key_choice {
                // No actual keybind system exists to rebind yet -- this
                // just reports the choice back so the popup is usable
                // and testable end-to-end ahead of that being wired up.
                *status = Some(format!("Key '{c}' chosen (not yet wired to an action)"));
            }
            v.overlay = Overlay::None;
        }
    }
}

enum Screen {
    Menu,
    Viewer(ViewerState),
}

struct RenderState {
    window: Arc<Window>,
    renderer: Renderer,
}

struct App {
    paths: SavePaths,
    menu: MainMenu,
    screen: Screen,
    text_input: String,
    status: Option<String>,

    // Rendering / windowing (shared by both screens -- the menu and the
    // viewer are drawn into the same window/surface, just with different
    // scene-building code and a different logical camera).
    state: Option<RenderState>,
    viewport: (f32, f32),
    mouse_pos: Vec2,

    /// Current keyboard modifier state (updated from `WindowEvent::ModifiersChanged`,
    /// which winit reports independently of individual key press/release events).
    modifiers: ModifiersState,

    // Hit-boxes from the menu screen's *last drawn* frame, used by the
    // next mouse click (immediate-mode UI: layout is recomputed every
    // frame, so "what did I just draw" is also "what can be clicked").
    last_menu_buttons: Vec<menu_ui::UiButton>,
}

impl App {
    fn new(paths: SavePaths) -> Self {
        let mut menu = MainMenu::new(paths.clone());
        menu.on_menu_opened();
        App {
            paths,
            menu,
            screen: Screen::Menu,
            text_input: String::new(),
            status: None,
            state: None,
            viewport: (1280.0, 800.0),
            mouse_pos: Vec2::ZERO,
            modifiers: ModifiersState::empty(),
            last_menu_buttons: Vec::new(),
        }
    }

    fn window_title(&self) -> String {
        match &self.screen {
            Screen::Menu => "Digital Logic Sim".to_string(),
            Screen::Viewer(v) => format!("Digital Logic Sim -- {} / {}", v.project_name, v.root_chip_name),
        }
    }

    fn set_window_title(&self) {
        if let Some(state) = &self.state {
            state.window.set_title(&self.window_title());
        }
    }

    // ---- Screen transitions ----

    fn open_project(&mut self, name: &str) {
        let project_dir = self.paths.project_path(name);
        match load_project(&project_dir) {
            Ok((project_desc, mut library, errors)) => {
                for e in &errors {
                    eprintln!("warning: {e}");
                }
                register_all_builtins(&mut library);

                let root_chip_name = project_desc
                    .all_custom_chip_names
                    .last()
                    .cloned()
                    .or_else(|| {
                        library
                            .iter()
                            .filter(|d| d.chip_type == logic_sim::ChipType::Custom)
                            .max_by_key(|d| d.sub_chips.len())
                            .map(|d| d.name.clone())
                    })
                    .unwrap_or_else(|| "NAND".to_string());

                let root_desc = library.get(&root_chip_name).clone();
                let mut sim = Simulator::build(&root_desc, &library);
                // In case modifier keys are already held down (e.g. Alt from
                // the menu action that opened this project) by the time the
                // viewer appears, rather than only picking them up on the
                // next change.
                sim.key_modifiers = encode_modifiers(self.modifiers);
                let show_grid = project_desc.prefs_grid_display_mode == 1;

                let mut prefs = project_desc;
                if prefs.chip_collections.is_empty() {
                    prefs.chip_collections = default_chip_collections();
                }

                self.screen = Screen::Viewer(ViewerState {
                    project_name: name.to_string(),
                    library,
                    root_chip_name,
                    sim,
                    camera: Camera::new(self.viewport.0, self.viewport.1),
                    dragging: false,
                    last_cursor: Vec2::ZERO,
                    camera_fitted: false,
                    show_grid,
                    prefs,
                    overlay: Overlay::None,
                    overlay_text_input: String::new(),
                    overlay_key_choice: None,
                    last_overlay_buttons: Vec::new(),
                });
                self.status = None;
                self.set_window_title();
            }
            Err(e) => {
                self.status = Some(format!("Failed to open project '{name}': {e}"));
            }
        }
    }

    fn return_to_menu(&mut self) {
        self.screen = Screen::Menu;
        self.menu.on_menu_opened();
        self.set_window_title();
    }

    // ---- Menu action handling ----

    fn open_name_popup_with(&mut self, prefill: &str) {
        self.text_input = prefill.to_string();
    }

    fn handle_menu_action(&mut self, action: UiAction, event_loop: &ActiveEventLoop) {
        match action {
            UiAction::NewProject => {
                self.menu.choose_new_project();
                self.open_name_popup_with("");
            }
            UiAction::OpenProjectScreen => self.menu.choose_open_project(),
            UiAction::SettingsScreen => self.menu.choose_settings(),
            UiAction::AboutScreen => self.menu.choose_about(),
            UiAction::Quit => event_loop.exit(),
            UiAction::BackToMain => self.menu.back_to_main(),

            UiAction::SelectProject(i) => self.menu.select_project(i),
            UiAction::OpenSelected => {
                if let Some(MenuOutcome::OpenProject { name }) = self.menu.open_selected() {
                    self.open_project(&name);
                }
            }
            UiAction::RenameSelected => {
                let current = self.menu.selected_project().map(|p| p.project_name.clone()).unwrap_or_default();
                self.menu.request_rename_selected();
                if self.menu.popup() == PopupKind::RenameProject {
                    self.open_name_popup_with(&current);
                }
            }
            UiAction::DuplicateSelected => {
                self.menu.request_duplicate_selected();
                if self.menu.popup() == PopupKind::DuplicateProject {
                    self.open_name_popup_with("");
                }
            }
            UiAction::DeleteSelected => self.menu.request_delete_selected(),
            UiAction::RefreshProjects => self.menu.refresh_projects(),

            UiAction::PopupConfirm => self.confirm_popup(),
            UiAction::PopupCancel => {
                self.menu.cancel_popup();
                self.text_input.clear();
            }

            UiAction::ToggleVsync => {
                let mut s = self.menu.edited_settings();
                s.vsync_enabled = !s.vsync_enabled;
                self.menu.set_edited_settings(s);
            }
            UiAction::CycleFullscreenMode => {
                use logic_sim::FullScreenMode::*;
                let mut s = self.menu.edited_settings();
                s.fullscreen_mode = match s.fullscreen_mode {
                    Windowed => FullScreenWindow,
                    FullScreenWindow => MaximizedWindow,
                    MaximizedWindow => ExclusiveFullScreen,
                    ExclusiveFullScreen => Windowed,
                };
                self.menu.set_edited_settings(s);
            }
            UiAction::ApplySettings => {
                if let Err(e) = self.menu.apply_settings() {
                    self.status = Some(format!("Failed to save settings: {e}"));
                }
            }
        }
    }

    fn confirm_popup(&mut self) {
        match self.menu.popup() {
            PopupKind::DeleteConfirmation => {
                if let Err(e) = self.menu.confirm_delete() {
                    self.status = Some(format!("Failed to delete project: {e}"));
                }
            }
            PopupKind::NewProject | PopupKind::RenameProject | PopupKind::DuplicateProject => {
                match self.menu.confirm_name_popup(&self.text_input.clone()) {
                    Ok(Some(MenuOutcome::OpenProject { name })) => {
                        self.text_input.clear();
                        self.open_project(&name);
                    }
                    Ok(_) => self.text_input.clear(),
                    Err(e) => self.status = Some(format!("Failed: {e}")),
                }
            }
            PopupKind::None => {}
        }
    }

    // ---- Text input for name popups ----

    fn is_text_popup_open(&self) -> bool {
        matches!(self.menu.popup(), PopupKind::NewProject | PopupKind::RenameProject | PopupKind::DuplicateProject)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window_attrs =
            Window::default_attributes().with_title(self.window_title()).with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(window_attrs).expect("failed to create window"));

        let size = window.inner_size();
        self.viewport = (size.width as f32, size.height as f32);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).expect("failed to create surface");
        let renderer = pollster::block_on(Renderer::new(&instance, surface, size.width, size.height));

        self.state = Some(RenderState { window, renderer });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        if self.state.is_none() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(state) = self.state.as_mut() {
                    state.renderer.resize(size.width, size.height);
                }
                self.viewport = (size.width as f32, size.height as f32);
                if let Screen::Viewer(v) = &mut self.screen {
                    v.camera.resize_viewport(size.width as f32, size.height as f32);
                }
            }

            WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(event, event_loop),

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                if let Screen::Viewer(v) = &mut self.screen {
                    v.sim.key_modifiers = encode_modifiers(self.modifiers);
                }
            }

            // Physically-held keys don't generate a release event if focus
            // is lost while they're down (e.g. alt-tabbing away) -- without
            // this, a Key/KeyMods chip could get stuck "on" indefinitely.
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
                if let Screen::Viewer(v) = &mut self.screen {
                    v.sim.held_keys.clear();
                    v.sim.key_modifiers = 0;
                }
            }

            WindowEvent::MouseInput { state: btn_state, button: winit::event::MouseButton::Left, .. } => {
                self.handle_mouse_button(btn_state, event_loop);
            }

            WindowEvent::CursorMoved { position, .. } => {
                let cursor = Vec2::new(position.x as f32, position.y as f32);
                self.mouse_pos = cursor;
                if let Screen::Viewer(v) = &mut self.screen {
                    if v.dragging {
                        let before = v.camera.screen_to_world(v.last_cursor);
                        let after = v.camera.screen_to_world(cursor);
                        v.camera.pan(Vec2::new(before.x - after.x, before.y - after.y));
                    }
                    v.last_cursor = cursor;
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if let Screen::Viewer(v) = &mut self.screen {
                    let scroll = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 100.0) as f32,
                    };
                    let zoom_factor = 1.0 + scroll * 0.1;
                    v.camera.zoom_at(v.last_cursor, zoom_factor);
                }
            }

            WindowEvent::RedrawRequested => self.redraw(event_loop),

            _ => {}
        }
    }
}

impl App {
    fn handle_mouse_button(&mut self, btn_state: ElementState, event_loop: &ActiveEventLoop) {
        match &mut self.screen {
            Screen::Menu => {
                if btn_state == ElementState::Pressed {
                    let hit = self.last_menu_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
                    if let Some(action) = hit {
                        self.handle_menu_action(action, event_loop);
                    }
                }
            }
            Screen::Viewer(v) => {
                if btn_state == ElementState::Pressed && v.overlay != Overlay::None {
                    let hit = v.last_overlay_buttons.iter().find(|b| b.enabled && b.rect.contains(self.mouse_pos)).map(|b| b.action.clone());
                    if let Some(action) = hit {
                        apply_editor_action(v, &self.paths, &mut self.status, action);
                        return;
                    }
                    if v.overlay != Overlay::Library {
                        // Modal popup: swallow the click instead of
                        // letting it fall through to camera dragging.
                        return;
                    }
                }
                v.dragging = btn_state == ElementState::Pressed;
            }
        }
    }

    fn handle_key_event(&mut self, event: winit::event::KeyEvent, event_loop: &ActiveEventLoop) {
        // Feed the Key chip's held-key set on both press *and* release (not
        // just press, unlike the shortcut handling below) since it needs to
        // know when a key stops being held, not just when it starts.
        // The chip stores/compares its target letter in capitals, so a
        // basic lowercase 'a' keypress must also register as 'A' here.
        if let Key::Character(s) = &event.logical_key {
            if let Screen::Viewer(v) = &mut self.screen {
                if let Some(c) = s.chars().next() {
                    let c = c.to_ascii_uppercase();
                    match event.state {
                        ElementState::Pressed => {
                            v.sim.held_keys.insert(c);
                        }
                        ElementState::Released => {
                            v.sim.held_keys.remove(&c);
                        }
                    }
                }
            }
        }

        if event.state != ElementState::Pressed {
            return;
        }

        match &mut self.screen {
            Screen::Menu => {
                if self.is_text_popup_open() {
                    match &event.logical_key {
                        Key::Named(NamedKey::Backspace) => {
                            self.text_input.pop();
                        }
                        Key::Named(NamedKey::Enter) => self.confirm_popup(),
                        Key::Named(NamedKey::Escape) => {
                            self.menu.cancel_popup();
                            self.text_input.clear();
                        }
                        Key::Character(s) => {
                            if self.text_input.chars().count() < logic_sim::ui_menu::MAX_PROJECT_NAME_LENGTH {
                                self.text_input.push_str(s);
                            }
                        }
                        _ => {}
                    }
                } else if self.menu.popup() == PopupKind::DeleteConfirmation {
                    match &event.logical_key {
                        Key::Named(NamedKey::Enter) => self.confirm_popup(),
                        Key::Named(NamedKey::Escape) => self.menu.cancel_popup(),
                        _ => {}
                    }
                } else if event.logical_key == Key::Named(NamedKey::Escape) {
                    self.menu.back_to_main();
                }
            }
            Screen::Viewer(v) => match &event.logical_key {
                // ---- Text entry for the search / naming overlays ----
                Key::Named(NamedKey::Backspace) if matches!(v.overlay, Overlay::Search | Overlay::Naming) => {
                    v.overlay_text_input.pop();
                }
                Key::Named(NamedKey::Enter) if v.overlay == Overlay::Naming => {
                    let trimmed = v.overlay_text_input.trim().to_string();
                    if !trimmed.is_empty() {
                        v.project_name = trimmed;
                    }
                    v.overlay = Overlay::None;
                }
                Key::Named(NamedKey::Enter) if v.overlay == Overlay::KeySelect && v.overlay_key_choice.is_some() => {
                    v.overlay = Overlay::None;
                }
                Key::Character(s) if matches!(v.overlay, Overlay::Search | Overlay::Naming) => {
                    if v.overlay_text_input.chars().count() < 64 {
                        v.overlay_text_input.push_str(s);
                    }
                }
                // ---- Key-select overlay: capture the next alphanumeric key ----
                Key::Character(s) if v.overlay == Overlay::KeySelect => {
                    if let Some(c) = s.chars().next() {
                        let upper = c.to_ascii_uppercase();
                        if editor_ui::KEY_SELECT_ALLOWED_CHARS.contains(upper) {
                            v.overlay_key_choice = Some(upper);
                        }
                    }
                }
                // ---- Normal viewer shortcuts (only while nothing's open) ----
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("r") => v.rebuild_sim(),
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("f") => v.camera_fitted = !v.camera_fitted,
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("g") => v.show_grid = !v.show_grid,
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("p") => v.overlay = Overlay::Preferences,
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("n") => {
                    v.overlay = Overlay::Naming;
                    v.overlay_text_input = v.project_name.clone();
                }
                Key::Character(s) if v.overlay == Overlay::None && s.eq_ignore_ascii_case("k") => {
                    v.overlay = Overlay::KeySelect;
                    v.overlay_key_choice = None;
                }
                Key::Character(s) if v.overlay == Overlay::None && s.as_str() == "/" => {
                    v.overlay = Overlay::Search;
                    v.overlay_text_input.clear();
                }
                Key::Named(NamedKey::Tab) => {
                    v.overlay = if v.overlay == Overlay::Library { Overlay::None } else { Overlay::Library };
                }
                Key::Named(NamedKey::Escape) => {
                    if v.overlay != Overlay::None {
                        v.overlay = Overlay::None;
                        v.overlay_text_input.clear();
                    } else {
                        self.return_to_menu();
                    }
                }
                _ => {}
            },
        }

        let _ = event_loop;
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (vw, vh) = self.viewport;

        let scene = match &mut self.screen {
            Screen::Menu => {
                let mut frame = menu_ui::build(&self.menu, vw, vh, &self.text_input, self.mouse_pos);
                if let Some(msg) = &self.status {
                    frame.geometry.labels.push(menu_ui::status_label(vw, vh, msg));
                }
                self.last_menu_buttons = frame.buttons.clone();
                frame.geometry
            }
            Screen::Viewer(v) => {
                v.sim.run_simulation_step(&[]);

                let root_desc = v.library.get(&v.root_chip_name);
                let lookup = SimulatorPinState { sim: &v.sim, scope: v.sim.root() };
                let hover_world_pos = Some(v.camera.screen_to_world(self.mouse_pos));
                let chip_scene = build_scene(root_desc, &v.library, &lookup, hover_world_pos);

                if !v.camera_fitted {
                    let bounds = bounding_box(&chip_scene).or_else(|| bounding_box(&build_scene(root_desc, &v.library, &AllLow, None)));
                    if let Some((min, max)) = bounds {
                        v.camera.fit_to_bounds(min, max, 0.15);
                    }
                    v.camera_fitted = true;
                }

                let mut scene = if v.show_grid { build_grid(&v.camera, theme::GRID_COL) } else { SceneGeometry::default() };
                scene.triangles.extend(chip_scene.triangles);
                scene.labels.extend(chip_scene.labels);

                // Overlays are laid out in screen-pixel space by
                // `editor_ui` (see `pin_overlay_to_screen`'s doc comment)
                // -- remap that into `v.camera`'s current world space so
                // they stay pinned to the screen regardless of pan/zoom.
                if v.overlay != Overlay::None {
                    let overlay_frame = match v.overlay {
                        Overlay::Library => editor_ui::build_chip_library_panel(&v.prefs.chip_collections, Some(v.root_chip_name.as_str()), vw, vh, self.mouse_pos),
                        Overlay::Search => {
                            let mut names: Vec<String> = v.library.iter().map(|d| d.name.clone()).collect();
                            names.sort();
                            editor_ui::build_search_popup(&names, &v.overlay_text_input, vw, vh, self.mouse_pos)
                        }
                        Overlay::Preferences => editor_ui::build_preferences_panel(&v.prefs, vw, vh, self.mouse_pos),
                        Overlay::Naming => {
                            let confirm_enabled = !v.overlay_text_input.trim().is_empty();
                            editor_ui::build_simple_naming_popup("Rename project", &v.overlay_text_input, confirm_enabled, vw, vh, self.mouse_pos)
                        }
                        Overlay::KeySelect => editor_ui::build_key_select_popup(v.overlay_key_choice, vw, vh, self.mouse_pos),
                        Overlay::None => unreachable!(),
                    };
                    v.last_overlay_buttons = overlay_frame.buttons;
                    let pinned = pin_overlay_to_screen(overlay_frame.geometry, &v.camera, vw, vh);
                    scene.triangles.extend(pinned.triangles);
                    scene.labels.extend(pinned.labels);
                } else {
                    v.last_overlay_buttons.clear();
                }

                scene
            }
        };

        let camera = match &self.screen {
            Screen::Menu => Camera { position: Vec2::new(vw / 2.0, vh / 2.0), zoom: 1.0, viewport_width: vw, viewport_height: vh },
            Screen::Viewer(v) => v.camera,
        };

        if let Some(state) = self.state.as_mut() {
            match state.renderer.render(&scene, &camera, theme::BACKGROUND_COL) {
                Ok(()) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    let size = state.window.inner_size();
                    state.renderer.resize(size.width, size.height);
                }
                Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                Err(e) => eprintln!("render error: {e:?}"),
            }
            state.window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();

    let data_dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(SavePaths::unity_persistent_data_dir);
    eprintln!("using save data directory: {}", data_dir.display());
    SavePaths::ensure_directory_exists(&data_dir).ok();

    let mut app = App::new(SavePaths::new(data_dir));
    app.menu.refresh_projects();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.run_app(&mut app).expect("event loop error");
}
