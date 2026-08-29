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
use crate::{default_chip_collections, default_starred_list, ChipDescription, ChipType, SavePaths};

/// How long the transient status/error toast stays on screen before
/// dismissing itself -- no interaction required.
pub(crate) const STATUS_TOAST_LINGER: std::time::Duration = std::time::Duration::from_secs(7);

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
	/// The transient status/error toast's text (`None` = nothing shown).
	pub(crate) status: Option<String>,
	/// When the current `status` text first appeared -- what lets the
	/// toast auto-dismiss after [`STATUS_TOAST_LINGER`] seconds without
	/// any interaction. Restamped only when the text *changes* during an
	/// event (see [`App::note_status_maybe_changed`]).
	pub(crate) status_since: Option<std::time::Instant>,

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

	/// The app-wide buzzer-audio state handed to every opened viewer, plus
	/// its output stream (`None` where no audio device is available -- the
	/// app runs fine silently). Kept alive for the whole session like the
	/// original's ever-present `AudioUnity` component.
	pub(crate) audio: crate::audio::SharedAudioState,
	#[allow(dead_code)] // held (not read) purely to keep the stream playing
	pub(crate) audio_player: Option<crate::audio::AudioPlayer>,

	// --- bare-bones FPS counter: just counts frames and dumps a rate to
	// stdout every few seconds. Not exact (first print is a few seconds
	// of startup jitter included) -- good enough for eyeballing perf.
	pub(crate) fps_frames: u32,
	pub(crate) fps_since: std::time::Instant,
}

use crate::render::menu_ui::UiAction;

impl App {
	pub(crate) fn new(paths: SavePaths) -> Self {
		let mut menu = MainMenu::new(paths.clone());
		menu.on_menu_opened();

		let audio = crate::audio::default_shared_state();
		let audio_player = match crate::audio::spawn_player(std::sync::Arc::clone(&audio)) {
			Ok(player) => Some(player),
			Err(reason) => {
				eprintln!("audio disabled: {reason}");
				None
			}
		};

		App {
			paths,
			menu,
			screen: Screen::Menu,
			text_input: String::new(),
			status: None,
			status_since: None,
			state: None,
			viewport: Vec2::new(1280.0, 800.0),
			mouse_pos: Vec2::ZERO,
			modifiers: winit::keyboard::ModifiersState::empty(),
			menu_stack: UiStack::new(),
			audio,
			audio_player,
			fps_frames: 0,
			fps_since: std::time::Instant::now(),
		}
	}

	pub(crate) fn window_title(&self) -> String {
		match &self.screen {
			Screen::Menu => "Digital Logic Sim".to_string(),
			Screen::Viewer(v) => {
				// A view-only chip on screen extends the chip chain
				// ("project / root > viewed"), like the banner shows.
				let chip = match v.view_stack.last() {
					Some(top) => format!("{} > {}", v.root_chip_name, top.name),
					None => v.root_chip_name.clone(),
				};
				format!("Digital Logic Sim -- {} / {}", v.project_name, chip)
			}
		}
	}

	pub(crate) fn set_window_title(&self) {
		if let Some(state) = &self.state {
			state.window.set_title(&self.window_title());
		}
	}

	// ---- Screen transitions ----

	pub(crate) fn open_project(&mut self, name: &str) {
		// The compliant loader (`Loader::load_project`): loads only the
		// chips listed in `AllCustomChipNames` (never stray files), runs
		// `UpgradeHelper`'s pre-2.1.5 migrations, and lets a custom chip
		// shadow a same-named builtin -- exactly what the Unity build does.
		match crate::save_system::Loader::load_project(&self.paths, name) {
			Ok(project) => {
				let project_desc = project.description;
				let mut library = project.chip_library;

				// Every project opens onto a blank, unsaved chip rather than jumping back into whichever
				// custom chip happens to be "last" (or biggest) -- mirrors Ctrl+N rather than remembering a chip to reopen.
				let root_chip_name = unique_new_chip_name(&library);
				library.add(ChipDescription::new(&root_chip_name, ChipType::Custom));

				let mut v = ViewerState::new(name, library, root_chip_name.clone(), self.viewport, std::sync::Arc::clone(&self.audio));
				// That opening chip is a Ctrl+N-style draft too: it stays out of the
				// library sidebar and off disk until it's actually saved (Ctrl+S).
				v.mark_unsaved_draft(&root_chip_name);
				// In case modifier keys are already held down (e.g. Alt from the menu action that
				// opened this project) by the time the viewer appears, rather than only picking them up on the next change.
				v.sim.set_key_modifiers(encode_modifiers(self.modifiers));

				let mut prefs = project_desc;
				if prefs.chip_collections.is_empty() {
					prefs.chip_collections = default_chip_collections();
				}
				if prefs.starred_list.is_empty() {
					prefs.starred_list = default_starred_list();
				}
				v.prefs = prefs;
				// Projects saved from a debug build may still list the
				// dev-only builtins (dev.RAM-8, the bus termini); release
				// builds drop those rows from the palette on open -- see
				// `prune_hidden_chips_from_palette`.
				crate::viewer::library::prune_hidden_chips_from_palette(&mut v.prefs, &v.library);
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
		// The viewer (and with it the only thing advancing the audio mix)
		// is about to be dropped -- silence any sounding buzzer instead of
		// letting it drone on under the menu.
		let shared = std::sync::Arc::clone(&self.audio);
		shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).sim_audio.silence();
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
				use crate::FullScreenMode as E;
				let mut s = self.menu.edited_settings();
				s.fullscreen_mode = match s.fullscreen_mode {
					E::Windowed => E::FullScreenWindow,
					E::FullScreenWindow => E::MaximizedWindow,
					E::MaximizedWindow => E::ExclusiveFullScreen,
					E::ExclusiveFullScreen => E::Windowed,
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

	// ---- Status toast lifetime ----

	/// Stamps the toast's appearance clock when an event changed its text
	/// (or dropped it). Called once per event with the pre-event text, so
	/// the dozens of `*status = Some(...)` sites scattered across the
	/// viewer stay untouched while the timer still always restarts on a
	/// genuinely new message.
	pub(crate) fn note_status_maybe_changed(&mut self, before: &Option<String>) {
		match &self.status {
			// A changed message restarts the window unconditionally -- the
			// previous entry's clock may be nearly expired already.
			Some(now) if Some(now) != before.as_ref() => {
				self.status_since = Some(std::time::Instant::now());
			}
			None => self.status_since = None,
			_ => {}
		}
	}

	/// Auto-dismisses the toast once [`STATUS_TOAST_LINGER`] has passed
	/// since it appeared. Runs every redraw -- the render loop ticks even
	/// when nothing else happens, so the expiry needs no interaction.
	pub(crate) fn expire_status_toast(&mut self) {
		if let Some(since) = self.status_since {
			if since.elapsed() >= STATUS_TOAST_LINGER {
				self.status = None;
				self.status_since = None;
			}
		}
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

#[cfg(test)]
mod status_toast_tests {
	//! White-box: the toast's clock lives on `App` (the viewer's shared
	//! `&mut Option<String>` sites can't carry it), so its stamp/expire
	//! contract is driven directly against a real `App` here -- no GPU or
	//! event loop needed.

	use super::*;
	use crate::save_system::test_util::temp_dir;

	fn app() -> App {
		App::new(SavePaths::new(temp_dir("status_toast")))
	}

	/// Back-dates the appearance clock, simulating an old toast.
	fn age_toast(app: &mut App, seconds: u64) {
		app.status_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(seconds));
	}

	#[test]
	fn new_message_stamps_the_clock_and_old_message_expires() {
		let mut app = app();

		assert!(app.status_since.is_none());
		app.note_status_maybe_changed(&None);
		assert_eq!(app.status, None);
		assert!(app.status_since.is_none(), "nothing to time while no toast is up");

		// A message appears: the clock starts.
		app.status = Some("Saved 'X'".to_string());
		app.note_status_maybe_changed(&None);
		let stamped = app.status_since.expect("a fresh message starts the linger window");
		assert!(stamped.elapsed() < STATUS_TOAST_LINGER);

		// Once the window has passed, the very next redraw clears it.
		age_toast(&mut app, 8);
		app.expire_status_toast();
		assert_eq!(app.status, None, "the toast dismisses itself without any interaction");
		assert!(app.status_since.is_none());

		// ...and expiry before the window does nothing.
		app.status = Some("Failed: boom".to_string());
		app.note_status_maybe_changed(&None);
		app.expire_status_toast();
		assert_eq!(app.status.as_deref(), Some("Failed: boom"), "a fresh toast outlives the redraw that showed it");
	}

	#[test]
	fn clearing_and_replacing_behave_like_their_words() {
		let mut app = app();

		// Clearing the toast clears the clock.
		app.status = Some("hi".to_string());
		app.note_status_maybe_changed(&None);
		app.status = None;
		app.note_status_maybe_changed(&Some("hi".to_string()));
		assert!(app.status_since.is_none(), "no lingering timer after the text goes away");

		// Replacing one message with another restarts the window...
		app.status = Some("first".to_string());
		app.note_status_maybe_changed(&None);
		age_toast(&mut app, 9);
		app.status = Some("second".to_string());
		app.note_status_maybe_changed(&Some("first".to_string()));
		app.expire_status_toast();
		assert_eq!(app.status.as_deref(), Some("second"), "the new message's own 7s window applies, not the old one's");
		assert!(app.status_since.expect("restamped").elapsed() < STATUS_TOAST_LINGER);
	}

	#[test]
	fn the_linger_window_is_about_seven_seconds() {
		assert_eq!(STATUS_TOAST_LINGER.as_secs(), 7);
	}
}
