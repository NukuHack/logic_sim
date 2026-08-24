//! The integrated application shell: one window that is either the
//! startup project-picker menu or an open project's chip viewer, plus
//! the screen transitions and menu-screen actions between them. Window
//! event handling lives in [`crate::viewer::events`], frame construction
//! in [`crate::viewer::frame`].

use crate::render::gpu::Renderer;
use crate::render::ui_stack::UiStack;
use crate::structs::Vec2;
use crate::ui_menu::{MainMenu, MenuOutcome, PopupKind};
use crate::viewer::input::encode_modifiers;
use crate::viewer::save_flow::unique_new_chip_name;
use crate::viewer::state::ViewerState;
use crate::{default_chip_collections, default_starred_list, load_project, register_all_builtins, ChipDescription, ChipType, SavePaths};

/// The window + wgpu renderer pair both screens draw into.
pub(crate) struct RenderState {
	pub(crate) window: std::sync::Arc<winit::window::Window>,
	pub(crate) renderer: Renderer,
}

pub(crate) enum Screen {
	Menu,
	/// Boxed so `Screen` itself stays small -- `ViewerState` is by far the
	/// biggest value in the app and would otherwise bloat every `Screen`
	/// (clippy::large_enum_variant).
	Viewer(Box<ViewerState>),
}

pub(crate) struct App {
	pub(crate) paths: SavePaths,
	pub(crate) menu: MainMenu,
	pub(crate) screen: Screen,
	pub(crate) text_input: String,
	pub(crate) status: Option<String>,

	// Rendering / windowing (shared by both screens -- the menu and the
	// viewer are drawn into the same window/surface, just with different
	// scene-building code and a different logical camera).
	pub(crate) state: Option<RenderState>,
	pub(crate) viewport: Vec2,
	pub(crate) mouse_pos: Vec2,

	/// Current keyboard modifier state (updated from `WindowEvent::ModifiersChanged`,
	/// which winit reports independently of individual key press/release events).
	pub(crate) modifiers: winit::keyboard::ModifiersState,

	/// The menu screen's UI stack as of the *last drawn* frame -- the
	/// screen itself at the bottom, the modal dialog on top of it (see
	/// `frame::build_menu_stack`). Immediate-mode, same as `ViewerState::stack`:
	/// every click/wheel event dispatches against what was just drawn.
	pub(crate) menu_stack: UiStack<UiAction>,
}

use crate::render::menu_ui::UiAction;

impl App {
	pub(crate) fn new(paths: SavePaths) -> Self {
		let mut menu = MainMenu::new(paths.clone());
		menu.on_menu_opened();
		App {
			paths,
			menu,
			screen: Screen::Menu,
			text_input: String::new(),
			status: None,
			state: None,
			viewport: Vec2::new(1280.0, 800.0),
			mouse_pos: Vec2::ZERO,
			modifiers: winit::keyboard::ModifiersState::empty(),
			menu_stack: UiStack::new(),
		}
	}

	pub(crate) fn window_title(&self) -> String {
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

	pub(crate) fn open_project(&mut self, name: &str) {
		let project_dir = self.paths.project_path(name);
		match load_project(&project_dir) {
			Ok((project_desc, mut library, errors)) => {
				for e in &errors {
					eprintln!("warning: {e}");
				}
				register_all_builtins(&mut library);

				// Every project opens onto a blank, unsaved chip rather than jumping back into whichever
				// custom chip happens to be "last" (or biggest) -- mirrors Ctrl+N rather than remembering a chip to reopen.
				let root_chip_name = unique_new_chip_name(&library);
				library.add(ChipDescription::new(&root_chip_name, ChipType::Custom));

				let mut v = ViewerState::new(name, library, root_chip_name.clone(), self.viewport);
				// In case modifier keys are already held down (e.g. Alt from the menu action that
				// opened this project) by the time the viewer appears, rather than only picking them up on the next change.
				v.sim.key_modifiers = encode_modifiers(self.modifiers);

				let mut prefs = project_desc;
				if prefs.chip_collections.is_empty() {
					prefs.chip_collections = default_chip_collections();
				}
				if prefs.starred_list.is_empty() {
					prefs.starred_list = default_starred_list();
				}
				v.prefs = prefs;
				v.sync_sim_clock_pref();

				self.screen = Screen::Viewer(Box::new(v));
				self.status = None;
				self.set_window_title();
			}
			Err(e) => {
				self.status = Some(format!("Failed to open project '{name}': {e}"));
			}
		}
	}

	pub(crate) fn return_to_menu(&mut self) {
		self.screen = Screen::Menu;
		self.menu.on_menu_opened();
		self.set_window_title();
	}

	// ---- Menu action handling ----

	fn open_name_popup_with(&mut self, prefill: &str) {
		self.text_input = prefill.to_string();
	}

	pub(crate) fn handle_menu_action(&mut self, action: UiAction, event_loop: &winit::event_loop::ActiveEventLoop) {
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
				use crate::FullScreenMode::*;
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

	pub(crate) fn confirm_popup(&mut self) {
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

	pub(crate) fn is_text_popup_open(&self) -> bool {
		matches!(self.menu.popup(), PopupKind::NewProject | PopupKind::RenameProject | PopupKind::DuplicateProject)
	}
}

/// Creates the wgpu renderer for the app's single window. Split from
/// `resumed` so the (blocking) GPU setup stays readable.
pub(crate) fn create_render_state(window: std::sync::Arc<winit::window::Window>, size: winit::dpi::PhysicalSize<u32>) -> RenderState {
	let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
	let surface = instance.create_surface(window.clone()).expect("failed to create surface");
	let renderer = pollster::block_on(Renderer::new(&instance, surface, size.width, size.height));
	RenderState { window, renderer }
}

/// Entry point: sets up logging + save paths, then runs the event loop.
pub fn run() -> Result<(), winit::error::EventLoopError> {
	env_logger::init();

	let data_dir = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(SavePaths::unity_persistent_data_dir);
	eprintln!("using save data directory: {}", data_dir.display());
	SavePaths::ensure_directory_exists(&data_dir).ok();

	let mut app = App::new(SavePaths::new(data_dir));
	app.menu.refresh_projects();

	let event_loop = winit::event_loop::EventLoop::new()?;
	event_loop.run_app(&mut app)
}
