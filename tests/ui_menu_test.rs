//! Startup-menu (`MainMenu`) integration tests: screen navigation,
//! project create/open/rename/duplicate/delete flows, version-guarded
//! selection, and settings editing -- driven entirely through the public
//! menu API against real temp directories.
//!
//! The three white-box tests that reach into `MainMenu`'s private fields
//! (forcing `screen`/`popup` into arbitrary states before asserting a
//! reset) stay inline in `src/ui_menu.rs` -- they cannot be expressed
//! through the public API.

use logic_sim::save_system::{create_project, SavePaths};
use logic_sim::settings::AppSettings;
use logic_sim::ui_menu::{MainMenu, MenuOutcome, MenuScreen, PopupKind, MAX_PROJECT_NAME_LENGTH};

/// Scratch-directory helper (the crate's own `test_util::temp_dir` is
/// unit-test-only).
fn temp_dir(label: &str) -> std::path::PathBuf {
	let pid = std::process::id();
	let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
	std::env::temp_dir().join(format!("dls_rust_integration_{label}_{pid}_{nanos}"))
}

fn menu_with_temp_paths(label: &str) -> (MainMenu, std::path::PathBuf) {
	let root = temp_dir(label);
	(MainMenu::new(SavePaths::new(&root)), root)
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
	logic_sim::Saver::save_project_description(&paths, &mut older.description).unwrap();

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
	logic_sim::Saver::save_project_description(&paths, &mut project.description).unwrap();

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
	logic_sim::Saver::save_project_description(&paths, &mut project.description).unwrap();

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
fn choose_settings_loads_current_app_settings_for_editing() {
	let (mut menu, root) = menu_with_temp_paths("choose_settings");
	let paths = SavePaths::new(&root);
	let settings = AppSettings { resolution_x: 2560, resolution_y: 1440, ..AppSettings::default_settings() };
	logic_sim::Saver::save_app_settings(&paths, &settings).unwrap();

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
	assert_eq!(logic_sim::Loader::load_app_settings(&paths), settings);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn choose_quit_returns_quit_outcome() {
	let (menu, root) = menu_with_temp_paths("choose_quit");
	assert_eq!(menu.choose_quit(), MenuOutcome::Quit);
	std::fs::remove_dir_all(&root).ok();
}
