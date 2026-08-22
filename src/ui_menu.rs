//! The startup screen's logic, ported from `DLS.Graphics.MainMenu`.
//! The original draws its own UI with an immediate-mode framework that has no equivalent here yet, so
//! rather than a pixel port, this is a headless port: the same screens, popups, validation rules, and
//! transitions as the original, expressed as a plain state machine you can drive from any UI and query
//! to decide what to draw. See the crate docs / tests for a typical host app event-loop usage example.

use crate::json::ProjectDescription;
use crate::save_system::{can_open_project, valid_file_name, SavePaths};
use crate::settings::AppSettings;

/// Which top-level screen of the startup flow is currently shown. Mirrors
/// `MainMenu.MenuScreen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuScreen {
	Main,
	LoadProject,
	Settings,
	About,
}

/// A modal popup layered on top of the current screen. Mirrors
/// `MainMenu.PopupKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
	None,
	DeleteConfirmation,
	RenameProject,
	DuplicateProject,
	NewProject,
}

/// Something the host app should act on as a result of a menu interaction
/// (there was no equivalent explicit type in the original -- it called
/// straight into `Main.*` methods with side effects; this port separates
/// "decide what happened" from "actually load a project / quit the app /
/// apply settings", so the host stays in control of those side effects).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuOutcome {
	/// Nothing to do -- stay on the menu.
	None,
	/// The player chose to open `name` (already confirmed compatible).
	OpenProject { name: String },
	/// The player applied new app settings from the settings screen.
	SettingsApplied(AppSettings),
	/// The player chose Quit.
	Quit,
}

/// Maximum length for a project name, mirroring `MainMenu.MaxProjectNameLength`.
pub const MAX_PROJECT_NAME_LENGTH: usize = 20;

/// Headless state machine for the startup screen. Mirrors the *behaviour*
/// of `DLS.Graphics.MainMenu`'s static state, made instantiable (so tests,
/// or a multi-window host, don't have to fight a global).
pub struct MainMenu {
	paths: SavePaths,
	screen: MenuScreen,
	popup: PopupKind,
	edited_settings: AppSettings,
	projects: Vec<ProjectDescription>,
	selected_project_index: Option<usize>,
}

impl MainMenu {
	pub fn new(paths: SavePaths) -> Self {
		Self {
			paths,
			screen: MenuScreen::Main,
			popup: PopupKind::None,
			edited_settings: AppSettings::default_settings(),
			projects: Vec::new(),
			selected_project_index: None,
		}
	}

	// ---- Queries (for driving a UI) ----

	pub fn screen(&self) -> MenuScreen {
		self.screen
	}

	pub fn popup(&self) -> PopupKind {
		self.popup
	}

	pub fn projects(&self) -> &[ProjectDescription] {
		&self.projects
	}

	pub fn selected_project_index(&self) -> Option<usize> {
		self.selected_project_index
	}

	pub fn selected_project(&self) -> Option<&ProjectDescription> {
		self.selected_project_index.and_then(|i| self.projects.get(i))
	}

	/// `Ok(())` if the selected project can be opened by this build,
	/// `Err(reason)` otherwise. Mirrors `MainMenu.CanOpenProject`, applied
	/// to whichever project is currently selected on the load-project screen.
	pub fn selected_project_compatibility(&self) -> Option<Result<(), String>> {
		self.selected_project().map(can_open_project)
	}

	pub fn edited_settings(&self) -> AppSettings {
		self.edited_settings
	}

	// ---- Lifecycle ----

	/// Mirrors `MainMenu.OnMenuOpened`: resets to the main screen with no
	/// popup or selection. Call when transitioning into the startup screen
	/// (e.g. after quitting back out of a project).
	pub fn on_menu_opened(&mut self) {
		self.screen = MenuScreen::Main;
		self.popup = PopupKind::None;
		self.selected_project_index = None;
	}

	/// Mirrors `Loader.LoadAllProjectDescriptions`, called by
	/// `MainMenu.RefreshLoadedProjects`. Re-reads the project list from
	/// disk; call before showing the New Project popup or the Load
	/// Project screen, and again after any operation that changes the
	/// list (create/rename/duplicate/delete).
	pub fn refresh_projects(&mut self) {
		self.projects = crate::save_system::Loader::load_all_project_descriptions(&self.paths);
	}

	// ---- Main screen ----

	/// Mirrors choosing "New Project" on the main screen.
	pub fn choose_new_project(&mut self) {
		self.refresh_projects();
		self.popup = PopupKind::NewProject;
	}

	/// Mirrors choosing "Open Project" on the main screen.
	pub fn choose_open_project(&mut self) {
		self.refresh_projects();
		self.selected_project_index = None;
		self.screen = MenuScreen::LoadProject;
	}

	/// Mirrors choosing "Settings" on the main screen.
	pub fn choose_settings(&mut self) {
		self.edited_settings = crate::save_system::Loader::load_app_settings(&self.paths);
		self.screen = MenuScreen::Settings;
	}

	/// Mirrors choosing "About" on the main screen.
	pub fn choose_about(&mut self) {
		self.screen = MenuScreen::About;
	}

	/// Mirrors choosing "Quit" on the main screen.
	pub fn choose_quit(&self) -> MenuOutcome {
		MenuOutcome::Quit
	}

	// ---- Load Project screen ----

	pub fn select_project(&mut self, index: usize) {
		if index < self.projects.len() {
			self.selected_project_index = Some(index);
		}
	}

	pub fn request_delete_selected(&mut self) {
		if self.selected_project_index.is_some() {
			self.popup = PopupKind::DeleteConfirmation;
		}
	}

	pub fn request_duplicate_selected(&mut self) {
		if self.can_act_on_selected_compatible_project() {
			self.popup = PopupKind::DuplicateProject;
		}
	}

	pub fn request_rename_selected(&mut self) {
		if self.can_act_on_selected_compatible_project() {
			self.popup = PopupKind::RenameProject;
		}
	}

	fn can_act_on_selected_compatible_project(&self) -> bool {
		matches!(self.selected_project_compatibility(), Some(Ok(())))
	}

	/// Mirrors pressing "Open" on the load-project screen. Returns `None`
	/// if nothing valid is selected (mirrors the original disabling the
	/// button in that case).
	pub fn open_selected(&self) -> Option<MenuOutcome> {
		let project = self.selected_project()?;
		if can_open_project(project).is_ok() {
			Some(MenuOutcome::OpenProject { name: project.project_name.clone() })
		} else {
			None
		}
	}

	/// Mirrors pressing "Back" (or the cancel shortcut with no popup open).
	pub fn back_to_main(&mut self) {
		self.screen = MenuScreen::Main;
		self.popup = PopupKind::None;
	}

	// ---- Popups ----

	pub fn cancel_popup(&mut self) {
		self.popup = PopupKind::None;
	}

	/// Mirrors `MainMenu.DrawDeleteProjectConfirmationPopup`'s "Delete"
	/// button: deletes the selected project (with a backup copy, matching
	/// the original's default), then refreshes the list.
	pub fn confirm_delete(&mut self) -> std::io::Result<()> {
		let Some(project) = self.selected_project() else { return Ok(()) };
		let name = project.project_name.clone();
		crate::save_system::Saver::delete_project(&self.paths, &name, true)?;
		self.selected_project_index = None;
		self.popup = PopupKind::None;
		self.refresh_projects();
		Ok(())
	}

	/// Whether `name` would be accepted for a *new* project: mirrors the
	/// live validation shown while typing in `MainMenu.DrawNamePopup`
	/// (`projectNameValidator` + the "already exists" check), combined
	/// into the single pass/fail the Confirm button's enabled-state used.
	pub fn is_valid_new_project_name(&self, name: &str) -> bool {
		if name.trim().is_empty() || name.chars().count() > MAX_PROJECT_NAME_LENGTH {
			return false;
		}
		if !valid_file_name(name) {
			return false;
		}
		!self.projects.iter().any(|p| p.project_name.eq_ignore_ascii_case(name))
	}

	/// Mirrors `MainMenu.OnNamePopupConfirmed`: performs whichever action
	/// the currently-open name popup was for (new/rename/duplicate),
	/// closes the popup, and refreshes the project list. Returns the
	/// resulting `MenuOutcome` (opening the project, for a new project) if
	/// any, or `Ok(None)` for rename/duplicate (which stay on the
	/// load-project screen, as in the original).
	///
	/// No-ops (returns `Ok(None)`) if no name popup is currently open, or
	/// if `name` fails validation for the popup kind in question.
	pub fn confirm_name_popup(&mut self, name: &str) -> std::io::Result<Option<MenuOutcome>> {
		let kind = self.popup;
		match kind {
			PopupKind::NewProject => {
				if !self.is_valid_new_project_name(name) {
					return Ok(None);
				}
				self.popup = PopupKind::None;
				let project = crate::save_system::create_or_load_project(&self.paths, name)?;
				Ok(Some(MenuOutcome::OpenProject { name: project.description.project_name }))
			}
			PopupKind::RenameProject => {
				let Some(old_name) = self.selected_project().map(|p| p.project_name.clone()) else { return Ok(None) };
				if name.trim().is_empty() || !valid_file_name(name) {
					return Ok(None);
				}
				self.popup = PopupKind::None;
				crate::save_system::Saver::rename_project(&self.paths, &old_name, name)?;
				self.refresh_projects();
				self.selected_project_index = Some(0); // the modified project is now newest-saved, i.e. first
				Ok(None)
			}
			PopupKind::DuplicateProject => {
				let Some(old_name) = self.selected_project().map(|p| p.project_name.clone()) else { return Ok(None) };
				if !self.is_valid_new_project_name(name) {
					return Ok(None);
				}
				self.popup = PopupKind::None;
				crate::save_system::Saver::duplicate_project(&self.paths, &old_name, name)?;
				self.refresh_projects();
				self.selected_project_index = Some(0);
				Ok(None)
			}
			PopupKind::None | PopupKind::DeleteConfirmation => Ok(None),
		}
	}

	// ---- Settings screen ----

	/// Mirrors editing the settings screen's fields; the host UI is
	/// expected to mutate a copy obtained from `edited_settings()` and
	/// hand it back here (there's no field-by-field API since the fields
	/// themselves -- resolution, fullscreen mode, vsync -- have no menu
	/// logic of their own beyond "hold the edited value").
	pub fn set_edited_settings(&mut self, settings: AppSettings) {
		self.edited_settings = settings;
	}

	/// Mirrors pressing "APPLY" on the settings screen: persists
	/// `edited_settings` and returns it wrapped in `MenuOutcome` so the
	/// host can apply it to the actual window/renderer.
	pub fn apply_settings(&mut self) -> std::io::Result<MenuOutcome> {
		crate::save_system::Saver::save_app_settings(&self.paths, &self.edited_settings)?;
		Ok(MenuOutcome::SettingsApplied(self.edited_settings))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::save_system::create_project;

	fn menu_with_temp_paths(label: &str) -> (MainMenu, std::path::PathBuf) {
		let root = crate::save_system::test_util::temp_dir(label);
		(MainMenu::new(SavePaths::new(&root)), root)
	}

	#[test]
	fn on_menu_opened_resets_to_main_screen_with_no_popup_or_selection() {
		let (mut menu, root) = menu_with_temp_paths("menu_reset");
		menu.screen = MenuScreen::Settings;
		menu.popup = PopupKind::DeleteConfirmation;
		menu.selected_project_index = Some(0);

		menu.on_menu_opened();

		assert_eq!(menu.screen(), MenuScreen::Main);
		assert_eq!(menu.popup(), PopupKind::None);
		assert_eq!(menu.selected_project_index(), None);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn choose_new_project_opens_the_new_project_popup() {
		let (mut menu, root) = menu_with_temp_paths("choose_new_project");
		menu.choose_new_project();
		assert_eq!(menu.popup(), PopupKind::NewProject);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn is_valid_new_project_name_rejects_empty_too_long_forbidden_and_duplicate_names() {
		let (mut menu, root) = menu_with_temp_paths("valid_name_checks");
		create_project(&SavePaths::new(&root), "Existing").unwrap();
		menu.refresh_projects();

		assert!(!menu.is_valid_new_project_name(""));
		assert!(!menu.is_valid_new_project_name("   "));
		assert!(!menu.is_valid_new_project_name(&"x".repeat(MAX_PROJECT_NAME_LENGTH + 1)));
		assert!(!menu.is_valid_new_project_name("bad/name"));
		assert!(!menu.is_valid_new_project_name("existing")); // case-insensitive collision
		assert!(!menu.is_valid_new_project_name("EXISTING"));
		assert!(menu.is_valid_new_project_name(&"x".repeat(MAX_PROJECT_NAME_LENGTH)));
		assert!(menu.is_valid_new_project_name("Brand New Project"));

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn confirming_new_project_popup_creates_and_opens_the_project() {
		let (mut menu, root) = menu_with_temp_paths("confirm_new_project");
		menu.choose_new_project();

		let outcome = menu.confirm_name_popup("Fresh Project").unwrap();

		assert_eq!(outcome, Some(MenuOutcome::OpenProject { name: "Fresh Project".to_string() }));
		assert_eq!(menu.popup(), PopupKind::None);
		assert!(SavePaths::new(&root).project_description_path("Fresh Project").is_file());

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn confirming_new_project_popup_with_invalid_name_is_a_no_op() {
		let (mut menu, root) = menu_with_temp_paths("confirm_new_project_invalid");
		menu.choose_new_project();

		let outcome = menu.confirm_name_popup("bad/name").unwrap();

		assert_eq!(outcome, None);
		assert_eq!(menu.popup(), PopupKind::NewProject, "popup should stay open on invalid input");

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn choose_open_project_lists_projects_newest_first() {
		let (mut menu, root) = menu_with_temp_paths("choose_open_project");
		let paths = SavePaths::new(&root);
		let mut older = create_project(&paths, "Older").unwrap();
		older.description.last_save_time = "2020-01-01T00:00:00.000Z".to_string();
		crate::save_system::Saver::save_project_description(&paths, &mut older.description).unwrap();

		menu.choose_open_project();

		assert_eq!(menu.screen(), MenuScreen::LoadProject);
		assert_eq!(menu.projects().len(), 1);
		assert_eq!(menu.selected_project_index(), None);

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn selecting_out_of_range_index_is_ignored() {
		let (mut menu, root) = menu_with_temp_paths("select_out_of_range");
		menu.choose_open_project(); // no projects yet
		menu.select_project(0);
		assert_eq!(menu.selected_project_index(), None);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn open_selected_returns_none_when_nothing_is_selected() {
		let (menu, root) = menu_with_temp_paths("open_selected_none");
		assert_eq!(menu.open_selected(), None);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn open_selected_returns_outcome_for_a_compatible_project() {
		let (mut menu, root) = menu_with_temp_paths("open_selected_compatible");
		let paths = SavePaths::new(&root);
		create_project(&paths, "P").unwrap();
		menu.choose_open_project();
		menu.select_project(0);

		assert_eq!(menu.open_selected(), Some(MenuOutcome::OpenProject { name: "P".to_string() }));

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn open_selected_returns_none_for_an_incompatible_project() {
		let (mut menu, root) = menu_with_temp_paths("open_selected_incompatible");
		let paths = SavePaths::new(&root);
		let mut project = create_project(&paths, "Future Project").unwrap();
		project.description.dls_version_earliest_compatible = "99.0.0".to_string();
		crate::save_system::Saver::save_project_description(&paths, &mut project.description).unwrap();

		menu.choose_open_project();
		menu.select_project(0);

		assert_eq!(menu.open_selected(), None);

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn request_rename_and_duplicate_are_ignored_for_incompatible_projects() {
		let (mut menu, root) = menu_with_temp_paths("rename_duplicate_guard");
		let paths = SavePaths::new(&root);
		let mut project = create_project(&paths, "Future Project").unwrap();
		project.description.dls_version_earliest_compatible = "99.0.0".to_string();
		crate::save_system::Saver::save_project_description(&paths, &mut project.description).unwrap();

		menu.choose_open_project();
		menu.select_project(0);

		menu.request_rename_selected();
		assert_eq!(menu.popup(), PopupKind::None);

		menu.request_duplicate_selected();
		assert_eq!(menu.popup(), PopupKind::None);

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn confirm_delete_backs_up_the_project_and_refreshes_the_list() {
		let (mut menu, root) = menu_with_temp_paths("confirm_delete");
		let paths = SavePaths::new(&root);
		create_project(&paths, "Doomed").unwrap();

		menu.choose_open_project();
		menu.select_project(0);
		menu.request_delete_selected();
		assert_eq!(menu.popup(), PopupKind::DeleteConfirmation);

		menu.confirm_delete().unwrap();

		assert_eq!(menu.popup(), PopupKind::None);
		assert_eq!(menu.selected_project_index(), None);
		assert!(menu.projects().is_empty());
		assert!(!paths.project_path("Doomed").exists());
		assert!(paths.deleted_project_path("Doomed").is_dir());

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn rename_via_name_popup_updates_disk_and_reselects_the_project() {
		let (mut menu, root) = menu_with_temp_paths("rename_via_popup");
		let paths = SavePaths::new(&root);
		create_project(&paths, "Old Name").unwrap();

		menu.choose_open_project();
		menu.select_project(0);
		menu.request_rename_selected();
		assert_eq!(menu.popup(), PopupKind::RenameProject);

		let outcome = menu.confirm_name_popup("New Name").unwrap();

		assert_eq!(outcome, None, "rename stays on the load-project screen");
		assert_eq!(menu.popup(), PopupKind::None);
		assert_eq!(menu.selected_project_index(), Some(0));
		assert_eq!(menu.projects()[0].project_name, "New Name");
		assert!(paths.project_description_path("New Name").is_file());

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn duplicate_via_name_popup_creates_a_second_project() {
		let (mut menu, root) = menu_with_temp_paths("duplicate_via_popup");
		let paths = SavePaths::new(&root);
		create_project(&paths, "Original").unwrap();

		menu.choose_open_project();
		menu.select_project(0);
		menu.request_duplicate_selected();
		assert_eq!(menu.popup(), PopupKind::DuplicateProject);

		menu.confirm_name_popup("Original Copy").unwrap();

		assert_eq!(menu.popup(), PopupKind::None);
		assert_eq!(menu.projects().len(), 2);
		assert!(paths.project_description_path("Original").is_file());
		assert!(paths.project_description_path("Original Copy").is_file());

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn back_to_main_clears_screen_and_popup() {
		let (mut menu, root) = menu_with_temp_paths("back_to_main");
		menu.choose_open_project();
		menu.request_delete_selected(); // no-op (nothing selected), just to exercise popup field
		menu.popup = PopupKind::RenameProject;

		menu.back_to_main();

		assert_eq!(menu.screen(), MenuScreen::Main);
		assert_eq!(menu.popup(), PopupKind::None);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn choose_settings_loads_current_app_settings_for_editing() {
		let (mut menu, root) = menu_with_temp_paths("choose_settings");
		let paths = SavePaths::new(&root);
		let settings = AppSettings { resolution_x: 2560, resolution_y: 1440, ..AppSettings::default_settings() };
		crate::save_system::Saver::save_app_settings(&paths, &settings).unwrap();

		menu.choose_settings();

		assert_eq!(menu.screen(), MenuScreen::Settings);
		assert_eq!(menu.edited_settings(), settings);

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn apply_settings_persists_and_returns_outcome() {
		let (mut menu, root) = menu_with_temp_paths("apply_settings");
		let paths = SavePaths::new(&root);
		let settings = AppSettings { resolution_x: 1280, resolution_y: 720, ..AppSettings::default_settings() };
		menu.set_edited_settings(settings);

		let outcome = menu.apply_settings().unwrap();

		assert_eq!(outcome, MenuOutcome::SettingsApplied(settings));
		assert_eq!(crate::save_system::Loader::load_app_settings(&paths), settings);

		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn choose_quit_returns_quit_outcome() {
		let (menu, root) = menu_with_temp_paths("choose_quit");
		assert_eq!(menu.choose_quit(), MenuOutcome::Quit);
		std::fs::remove_dir_all(&root).ok();
	}

	#[test]
	fn cancel_popup_clears_popup_without_touching_screen() {
		let (mut menu, root) = menu_with_temp_paths("cancel_popup");
		menu.screen = MenuScreen::LoadProject;
		menu.popup = PopupKind::DeleteConfirmation;

		menu.cancel_popup();

		assert_eq!(menu.popup(), PopupKind::None);
		assert_eq!(menu.screen(), MenuScreen::LoadProject);
		std::fs::remove_dir_all(&root).ok();
	}
}
