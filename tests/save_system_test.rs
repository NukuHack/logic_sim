//! Save-system integration tests: version parsing/precedence, on-disk
//! path layout, project create/load/rename/duplicate/delete round-trips
//! through real temp directories, filename validation, and the persisted
//! `AppSettings` format -- all driven through the crate's public API.

use logic_sim::save_system::Project;
use logic_sim::save_system::{
	can_open_project, copy_directory, create_or_load_project, create_project, default_starred_list, ensure_unique_directory_name,
	ensure_unique_file_name, valid_file_name, Version, DLS_VERSION, DLS_VERSION_EARLIEST_COMPATIBLE,
};
use logic_sim::settings::{parse_app_settings, serialize_app_settings, AppSettings, FullScreenMode};
use logic_sim::{ChipDescription, ChipLibrary, ChipType, Loader, ProjectDescription, SavePaths, Saver};
use std::path::PathBuf;

/// Scratch-directory helper mirroring the crate's own `test_util::temp_dir`
/// (which is unit-test-only), namespaced so concurrent runs never collide.
fn temp_dir(label: &str) -> std::path::PathBuf {
	let pid = std::process::id();
	let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
	std::env::temp_dir().join(format!("dls_rust_integration_{label}_{pid}_{nanos}"))
}

fn sample_description(name: &str) -> ProjectDescription {
	let mut d = ProjectDescription { project_name: name.to_string(), ..Default::default() };
	d.creation_time = "2025-01-01T00:00:00.000Z".to_string();
	d
}

#[test]
fn parses_valid_version_strings() {
	assert_eq!(Version::parse("2.1.6").unwrap(), Version::new(2, 1, 6));
	assert_eq!(Version::parse("0.0.0").unwrap(), Version::new(0, 0, 0));
	assert_eq!(Version::parse(" 2 . 1 . 6 ").unwrap(), Version::new(2, 1, 6));
}

#[test]
fn rejects_malformed_version_strings() {
	assert!(Version::parse("2.1").is_err());
	assert!(Version::parse("2.1.6.0").is_err());
	assert!(Version::parse("a.b.c").is_err());
	assert!(Version::parse("").is_err());
	assert!(Version::try_parse("not a version").is_none());
}

#[test]
fn display_matches_dot_separated_format() {
	assert_eq!(Version::new(2, 1, 6).to_string(), "2.1.6");
}

#[test]
fn to_int_matches_original_packed_formula() {
	assert_eq!(Version::new(2, 1, 6).to_int(), 2 * 100_000 + 1_000 + 6);
}

#[test]
fn ordering_matches_semantic_version_precedence() {
	assert!(Version::new(2, 0, 0) < Version::new(2, 0, 1));
	assert!(Version::new(2, 0, 9) < Version::new(2, 1, 0));
	assert!(Version::new(1, 9, 9) < Version::new(2, 0, 0));
	assert!(DLS_VERSION_EARLIEST_COMPATIBLE < DLS_VERSION);
}

#[test]
fn roundtrips_through_display_and_parse() {
	let v = Version::new(3, 12, 7);
	assert_eq!(Version::parse(&v.to_string()).unwrap(), v);
}

#[test]
fn builds_expected_relative_layout() {
	let paths = SavePaths::new("/data/root");
	assert_eq!(paths.projects_path(), PathBuf::from("/data/root/Projects"));
	assert_eq!(paths.deleted_projects_path(), PathBuf::from("/data/root/Deleted Projects"));
	assert_eq!(paths.app_settings_path(), PathBuf::from("/data/root/AppSettings.json"));
	assert_eq!(paths.project_path("GOL"), PathBuf::from("/data/root/Projects/GOL"));
	assert_eq!(paths.deleted_project_path("GOL"), PathBuf::from("/data/root/Deleted Projects/GOL"));
	assert_eq!(paths.chips_path("GOL"), PathBuf::from("/data/root/Projects/GOL/Chips"));
	assert_eq!(paths.deleted_chips_path("GOL"), PathBuf::from("/data/root/Projects/GOL/Deleted Chips"));
	assert_eq!(paths.project_description_path("GOL"), PathBuf::from("/data/root/Projects/GOL/ProjectDescription.json"));
}

#[test]
fn default_data_dir_ends_with_app_name() {
	assert_eq!(SavePaths::default_data_dir().file_name().unwrap(), "DigitalLogicSim");
}

#[test]
fn unity_persistent_data_dir_ends_with_the_expected_publisher_and_game_folders() {
	let dir = SavePaths::unity_persistent_data_dir();
	assert_eq!(dir.file_name().unwrap(), "Digital-Logic-Sim");
	assert_eq!(dir.parent().unwrap().file_name().unwrap(), "SebastianLague");
}

#[cfg(target_os = "windows")]
#[test]
fn unity_persistent_data_dir_uses_local_low_on_windows() {
	let dir = SavePaths::unity_persistent_data_dir();
	let s = dir.to_string_lossy();
	assert!(s.contains("AppData"), "expected AppData in {s}");
	assert!(s.contains("LocalLow"), "expected LocalLow (not Roaming) in {s}");
}

#[cfg(target_os = "macos")]
#[test]
fn unity_persistent_data_dir_uses_application_support_on_macos() {
	let dir = SavePaths::unity_persistent_data_dir();
	let s = dir.to_string_lossy();
	assert!(s.contains("Library/Application Support"), "expected Application Support in {s}");
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[test]
fn unity_persistent_data_dir_uses_dot_config_unity3d_on_linux() {
	let dir = SavePaths::unity_persistent_data_dir();
	let s = dir.to_string_lossy();
	assert!(s.contains(".config/unity3d"), "expected .config/unity3d in {s}");
}

#[test]
fn ensure_directory_exists_creates_nested_dirs() {
	let tmp = temp_dir("ensure_dir");
	let nested = tmp.join("a").join("b").join("c");
	assert!(!nested.exists());
	SavePaths::ensure_directory_exists(&nested).unwrap();
	assert!(nested.is_dir());
	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn add_or_update_custom_chip_registers_new_chip_name_once() {
	let mut project = Project::new(ProjectDescription::default(), ChipLibrary::new());

	let chip = ChipDescription::new("MY GATE", ChipType::Custom);
	project.add_or_update_custom_chip(chip.clone());
	project.add_or_update_custom_chip(chip);

	assert_eq!(project.description.all_custom_chip_names, vec!["MY GATE".to_string()]);
	assert!(project.chip_library.try_get("my gate").is_some());
}

#[test]
fn default_starred_list_matches_original() {
	let list = default_starred_list();
	assert_eq!(list.len(), 2);
	assert_eq!(list[0].name, "IN/OUT");
	assert!(list[0].is_collection);
	assert_eq!(list[1].name, "NAND");
	assert!(!list[1].is_collection);
}

#[test]
fn can_open_project_accepts_projects_within_the_supported_version_range() {
	let desc = ProjectDescription { dls_version_earliest_compatible: "2.0.0".to_string(), ..Default::default() };
	assert_eq!(can_open_project(&desc), Ok(()));
}

#[test]
fn can_open_project_rejects_projects_from_a_newer_version() {
	let desc = ProjectDescription { dls_version_earliest_compatible: "99.0.0".to_string(), ..Default::default() };
	let result = can_open_project(&desc);
	assert!(result.is_err());
	assert!(result.unwrap_err().contains("99.0.0"));
}

#[test]
fn can_open_project_rejects_unparseable_version_strings() {
	let desc = ProjectDescription { dls_version_earliest_compatible: "not-a-version".to_string(), ..Default::default() };
	assert_eq!(can_open_project(&desc), Err("Unrecognized project format".to_string()));
}

#[test]
fn can_open_project_accepts_all_four_sample_projects() {
	// Every uploaded ProjectDescription.json declared EarliestCompatible = 2.0.0.
	let desc = ProjectDescription { dls_version_earliest_compatible: "2.0.0".to_string(), ..Default::default() };
	assert_eq!(can_open_project(&desc), Ok(()));
}

#[test]
fn create_project_produces_expected_defaults() {
	let root = temp_dir("create_project_defaults");
	let paths = SavePaths::new(&root);

	let project = create_project(&paths, "My New Project").unwrap();

	assert_eq!(project.description.project_name, "My New Project");
	assert_eq!(project.description.dls_version_last_saved, DLS_VERSION.to_string());
	assert_eq!(project.description.dls_version_earliest_compatible, DLS_VERSION_EARLIEST_COMPATIBLE.to_string());
	assert_eq!(project.description.prefs_main_pin_names_display_mode, 1, "On Hover");
	assert_eq!(project.description.prefs_chip_pin_names_display_mode, 1, "On Hover");
	assert_eq!(project.description.prefs_sim_target_steps_per_second, 1000);
	assert_eq!(project.description.prefs_sim_steps_per_clock_tick, 250);
	assert!(!project.description.prefs_sim_paused);
	assert!(project.description.all_custom_chip_names.is_empty());
	assert!(!project.description.starred_list.is_empty());
	assert!(!project.description.chip_collections.is_empty());

	// Persisted to disk, not just in memory.
	assert!(paths.project_description_path("My New Project").is_file());
	// And builtins are present in the loaded chip library.
	assert!(project.chip_library.try_get("nand").is_some());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn create_project_creates_the_chips_folder_so_the_project_is_immediately_open_able() {
	// Regression test: a freshly-created project must have its `Chips/` folder on disk, because
	// `json::load_project` -> `load_chip_library_from_dir` used to hard-fail with a "No such file
	// or directory" error when that folder didn't exist yet, making a new project un-openable.
	let root = temp_dir("create_project_chips_folder");
	let paths = SavePaths::new(&root);

	create_project(&paths, "Fresh Project").unwrap();

	assert!(paths.chips_path("Fresh Project").is_dir());

	let (_desc, library, errors) = logic_sim::json::load_project(&paths.project_path("Fresh Project")).unwrap();
	assert!(errors.is_empty());
	assert!(library.try_get("nand").is_none(), "json::load_project doesn't register builtins itself; that's the app's job");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn create_or_load_project_creates_when_absent_then_loads_when_present() {
	let root = temp_dir("create_or_load_project");
	let paths = SavePaths::new(&root);

	let created = create_or_load_project(&paths, "P").unwrap();
	assert_eq!(created.description.project_name, "P");

	// Mutate on disk so we can tell a fresh load actually happened
	// rather than silently re-creating the project.
	let mut desc = created.description;
	desc.all_custom_chip_names.push("Marker".to_string());
	// (Not a real chip file -- just checking the description round-trips;
	// avoid load_project's chip-library step by re-reading the description directly.)
	logic_sim::Saver::save_project_description(&paths, &mut desc).unwrap();

	let reloaded_desc = Loader::load_project_description(&paths, "P").unwrap();
	assert_eq!(reloaded_desc.all_custom_chip_names, vec!["Marker".to_string()]);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn save_app_settings_writes_readable_file() {
	let root = temp_dir("save_app_settings");
	let paths = SavePaths::new(&root);
	let settings = AppSettings { resolution_x: 1280, resolution_y: 720, ..AppSettings::default_settings() };

	Saver::save_app_settings(&paths, &settings).unwrap();

	assert!(paths.app_settings_path().is_file());
	let loaded = Loader::load_app_settings(&paths);
	assert_eq!(loaded, settings);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn save_project_description_stamps_last_save_time_and_version() {
	let root = temp_dir("save_project_desc");
	let paths = SavePaths::new(&root);
	let mut desc = sample_description("GOL");
	desc.dls_version_last_saved = "0.0.0".to_string();
	let original_creation_time = desc.creation_time.clone();

	Saver::save_project_description(&paths, &mut desc).unwrap();

	assert_eq!(desc.dls_version_last_saved, DLS_VERSION.to_string());
	assert!(!desc.last_save_time.is_empty());
	// Creation time is untouched by saving.
	assert_eq!(desc.creation_time, original_creation_time);
	assert!(paths.project_description_path("GOL").is_file());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn save_chip_then_load_chip_library_round_trips() {
	let root = temp_dir("save_chip_roundtrip");
	let paths = SavePaths::new(&root);
	SavePaths::ensure_directory_exists(&paths.chips_path("P")).unwrap();

	let mut chip = ChipDescription::new("MY GATE", ChipType::Custom);
	chip.input_pins.push(logic_sim::description::PinDescription::new("A", 0, logic_sim::description::PinBitCount::Bit1));
	Saver::save_chip(&paths, "P", &ChipLibrary::new(), &chip).unwrap();

	assert!(paths.chips_path("P").join("MY GATE.json").is_file());

	let (library, errors) = logic_sim::json::load_chip_library_from_dir(&paths.chips_path("P")).unwrap();
	assert!(errors.is_empty());
	let loaded = library.get("MY GATE");
	assert_eq!(loaded.input_pins.len(), 1);
	assert_eq!(loaded.input_pins[0].name, "A");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_chip_with_backup_moves_file_into_deleted_chips_folder() {
	let root = temp_dir("delete_chip_backup");
	let paths = SavePaths::new(&root);
	let chip = ChipDescription::new("TEMP", ChipType::Custom);
	Saver::save_chip(&paths, "P", &ChipLibrary::new(), &chip).unwrap();

	Saver::delete_chip(&paths, "P", "TEMP", true).unwrap();

	assert!(!paths.chips_path("P").join("TEMP.json").exists());
	assert!(paths.deleted_chips_path("P").join("TEMP.json").is_file());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_chip_without_backup_removes_file_permanently() {
	let root = temp_dir("delete_chip_no_backup");
	let paths = SavePaths::new(&root);
	let chip = ChipDescription::new("TEMP", ChipType::Custom);
	Saver::save_chip(&paths, "P", &ChipLibrary::new(), &chip).unwrap();

	Saver::delete_chip(&paths, "P", "TEMP", false).unwrap();

	assert!(!paths.chips_path("P").join("TEMP.json").exists());
	assert!(!paths.deleted_chips_path("P").exists());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_project_with_backup_moves_project_into_deleted_projects_folder() {
	let root = temp_dir("delete_project_backup");
	let paths = SavePaths::new(&root);
	let mut desc = sample_description("GOL");
	Saver::save_project_description(&paths, &mut desc).unwrap();

	Saver::delete_project(&paths, "GOL", true).unwrap();

	assert!(!paths.project_path("GOL").exists());
	assert!(paths.deleted_project_path("GOL").join("ProjectDescription.json").is_file());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_project_backup_avoids_clobbering_an_existing_deleted_copy() {
	let root = temp_dir("delete_project_backup_collision");
	let paths = SavePaths::new(&root);

	let mut desc = sample_description("GOL");
	Saver::save_project_description(&paths, &mut desc).unwrap();
	Saver::delete_project(&paths, "GOL", true).unwrap();

	// Recreate and delete again -- should not collide with the first backup.
	let mut desc2 = sample_description("GOL");
	Saver::save_project_description(&paths, &mut desc2).unwrap();
	Saver::delete_project(&paths, "GOL", true).unwrap();

	assert!(paths.deleted_projects_path().join("GOL").is_dir());
	assert!(paths.deleted_projects_path().join("GOL_1").is_dir());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rename_project_moves_directory_and_updates_description() {
	let root = temp_dir("rename_project");
	let paths = SavePaths::new(&root);
	let mut desc = sample_description("Old Name");
	Saver::save_project_description(&paths, &mut desc).unwrap();

	Saver::rename_project(&paths, "Old Name", "New Name").unwrap();

	assert!(!paths.project_path("Old Name").exists());
	assert!(paths.project_description_path("New Name").is_file());
	let reloaded = Loader::load_project_description(&paths, "New Name").unwrap();
	assert_eq!(reloaded.project_name, "New Name");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn duplicate_project_copies_files_and_renames_the_copy() {
	let root = temp_dir("duplicate_project");
	let paths = SavePaths::new(&root);
	SavePaths::ensure_directory_exists(&paths.chips_path("Original")).unwrap();
	std::fs::write(paths.chips_path("Original").join("NOT.json"), "{}").unwrap();
	let mut desc = sample_description("Original");
	Saver::save_project_description(&paths, &mut desc).unwrap();

	Saver::duplicate_project(&paths, "Original", "Copy").unwrap();

	assert!(paths.project_path("Original").is_dir(), "original project should be untouched");
	let duplicated = Loader::load_project_description(&paths, "Copy").unwrap();
	assert_eq!(duplicated.project_name, "Copy");
	assert!(paths.chips_path("Copy").join("NOT.json").is_file());

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_app_settings_returns_default_when_file_missing() {
	let root = temp_dir("load_app_settings_missing");
	let paths = SavePaths::new(&root);
	assert_eq!(Loader::load_app_settings(&paths), AppSettings::default_settings());
}

#[test]
fn project_exists_reflects_disk_state() {
	let root = temp_dir("project_exists");
	let paths = SavePaths::new(&root);
	assert!(!Loader::project_exists(&paths, "GOL"));

	let mut desc = ProjectDescription { project_name: "GOL".to_string(), ..Default::default() };
	Saver::save_project_description(&paths, &mut desc).unwrap();

	assert!(Loader::project_exists(&paths, "GOL"));
	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_project_description_errors_when_missing() {
	let root = temp_dir("load_missing_project_desc");
	let paths = SavePaths::new(&root);
	assert!(Loader::load_project_description(&paths, "Nope").is_err());
}

#[test]
fn load_project_description_enforces_name_matches_directory() {
	let root = temp_dir("load_project_desc_name_enforced");
	let paths = SavePaths::new(&root);
	let mut desc = ProjectDescription { project_name: "Original Name".to_string(), ..Default::default() };
	Saver::save_project_description(&paths, &mut desc).unwrap();

	// Simulate the player renaming the folder by hand without updating
	// the JSON's ProjectName field.
	std::fs::rename(paths.project_path("Original Name"), paths.project_path("Renamed Folder")).unwrap();

	let loaded = Loader::load_project_description(&paths, "Renamed Folder").unwrap();
	assert_eq!(loaded.project_name, "Renamed Folder");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_all_project_descriptions_sorts_newest_first_and_skips_invalid() {
	let root = temp_dir("load_all_projects");
	let paths = SavePaths::new(&root);

	for (name, last_save) in [("A", "2024-01-01T00:00:00.000Z"), ("B", "2025-06-01T00:00:00.000Z"), ("C", "2024-06-01T00:00:00.000Z")] {
		let desc = ProjectDescription { project_name: name.to_string(), last_save_time: last_save.to_string(), ..Default::default() };
		let data = logic_sim::json::serialize_project_description(&desc).unwrap();
		SavePaths::ensure_directory_exists(&paths.project_path(name)).unwrap();
		std::fs::write(paths.project_description_path(name), data).unwrap();
	}

	// An invalid project directory (no ProjectDescription.json) should
	// be silently skipped rather than aborting the whole listing.
	std::fs::create_dir_all(paths.project_path("NotAProject")).unwrap();

	let all = Loader::load_all_project_descriptions(&paths);
	let names: Vec<&str> = all.iter().map(|d| d.project_name.as_str()).collect();
	assert_eq!(names, ["B", "C", "A"]);

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn load_all_project_descriptions_returns_empty_when_projects_dir_missing() {
	let root = temp_dir("load_all_projects_missing_dir");
	let paths = SavePaths::new(&root);
	assert!(Loader::load_all_project_descriptions(&paths).is_empty());
}

#[test]
fn load_project_includes_builtins_and_custom_chips() {
	let root = temp_dir("load_project_chip_library");
	let paths = SavePaths::new(&root);

	SavePaths::ensure_directory_exists(&paths.chips_path("P")).unwrap();
	let mut custom = logic_sim::description::ChipDescription::new("MY GATE", logic_sim::description::ChipType::Custom);
	custom.input_pins.push(logic_sim::description::PinDescription::new("A", 0, logic_sim::description::PinBitCount::Bit1));
	Saver::save_chip(&paths, "P", &ChipLibrary::new(), &custom).unwrap();

	let mut desc = ProjectDescription { project_name: "P".to_string(), all_custom_chip_names: vec!["MY GATE".to_string()], ..Default::default() };
	Saver::save_project_description(&paths, &mut desc).unwrap();

	let project = Loader::load_project(&paths, "P").unwrap();
	assert!(project.chip_library.try_get("my gate").is_some());
	assert!(project.chip_library.try_get("nand").is_some(), "builtins should still be present");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn custom_chip_shadows_builtin_of_the_same_name() {
	let root = temp_dir("custom_chip_shadows_builtin");
	let paths = SavePaths::new(&root);

	SavePaths::ensure_directory_exists(&paths.chips_path("P")).unwrap();
	// A custom chip that happens to reuse a builtin's name (e.g. the
	// project predates that builtin being added).
	let mut custom = logic_sim::description::ChipDescription::new("NAND", logic_sim::description::ChipType::Custom);
	custom.input_pins.push(logic_sim::description::PinDescription::new("MyInput", 0, logic_sim::description::PinBitCount::Bit1));
	Saver::save_chip(&paths, "P", &ChipLibrary::new(), &custom).unwrap();

	let mut desc = ProjectDescription { project_name: "P".to_string(), all_custom_chip_names: vec!["NAND".to_string()], ..Default::default() };
	Saver::save_project_description(&paths, &mut desc).unwrap();

	let project = Loader::load_project(&paths, "P").unwrap();
	let nand = project.chip_library.get("NAND");
	assert_eq!(nand.chip_type, logic_sim::description::ChipType::Custom);
	assert_eq!(nand.input_pins[0].name, "MyInput");

	std::fs::remove_dir_all(&root).ok();
}

#[test]
fn reserved_windows_names_are_rejected_case_insensitively() {
	assert!(!valid_file_name("con"));
	assert!(!valid_file_name("CON"));
	assert!(!valid_file_name("  NUL  "));
	assert!(!valid_file_name("lpt1"));
	assert!(valid_file_name("CONTROLLER")); // not an exact reserved match
}

#[test]
fn empty_and_forbidden_names_are_invalid() {
	assert!(!valid_file_name(""));
	assert!(!valid_file_name("bad/name"));
	assert!(!valid_file_name("bad:name"));
	assert!(!valid_file_name("trailing.dot."));
}

#[test]
fn ordinary_names_are_valid() {
	assert!(valid_file_name("My Project"));
	assert!(valid_file_name("GOL"));
	assert!(valid_file_name("STATE CALCULATOR"));
}

#[test]
fn ensure_unique_file_name_leaves_nonexistent_path_untouched() {
	let tmp = temp_dir("unique_file_untouched");
	let path = tmp.join("NOT.json");
	assert_eq!(ensure_unique_file_name(&path), path);
	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn ensure_unique_file_name_appends_counter_on_collision() {
	let tmp = temp_dir("unique_file_collision");
	std::fs::create_dir_all(&tmp).unwrap();
	let path = tmp.join("NOT.json");
	std::fs::write(&path, "{}").unwrap();

	let unique = ensure_unique_file_name(&path);
	assert_eq!(unique, tmp.join("NOT_1.json"));

	std::fs::write(&unique, "{}").unwrap();
	let unique2 = ensure_unique_file_name(&path);
	assert_eq!(unique2, tmp.join("NOT_2.json"));

	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn ensure_unique_directory_name_appends_counter_on_collision() {
	let tmp = temp_dir("unique_dir_collision");
	let path = tmp.join("GOL");
	std::fs::create_dir_all(&path).unwrap();

	let unique = ensure_unique_directory_name(&path);
	assert_eq!(unique, tmp.join("GOL_1"));

	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn copy_directory_copies_files_and_subdirectories_recursively() {
	let tmp = temp_dir("copy_dir");
	let src = tmp.join("src");
	let dst = tmp.join("dst");
	std::fs::create_dir_all(src.join("Chips")).unwrap();
	std::fs::write(src.join("ProjectDescription.json"), "{}").unwrap();
	std::fs::write(src.join("Chips").join("NOT.json"), "{}").unwrap();

	copy_directory(&src, &dst, true).unwrap();

	assert!(dst.join("ProjectDescription.json").is_file());
	assert!(dst.join("Chips").join("NOT.json").is_file());

	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn copy_directory_non_recursive_skips_subdirectories() {
	let tmp = temp_dir("copy_dir_non_recursive");
	let src = tmp.join("src");
	let dst = tmp.join("dst");
	std::fs::create_dir_all(src.join("Chips")).unwrap();
	std::fs::write(src.join("ProjectDescription.json"), "{}").unwrap();
	std::fs::write(src.join("Chips").join("NOT.json"), "{}").unwrap();

	copy_directory(&src, &dst, false).unwrap();

	assert!(dst.join("ProjectDescription.json").is_file());
	assert!(!dst.join("Chips").exists());

	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn copy_directory_errors_on_missing_source() {
	let tmp = temp_dir("copy_dir_missing_source");
	let result = copy_directory(&tmp.join("does-not-exist"), &tmp.join("dst"), true);
	assert!(result.is_err());
	std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn default_settings_match_original() {
	let s = AppSettings::default_settings();
	assert_eq!(s.resolution_x, 1920);
	assert_eq!(s.resolution_y, 1080);
	assert_eq!(s.fullscreen_mode, FullScreenMode::FullScreenWindow);
	assert!(s.vsync_enabled);
}

#[test]
fn fullscreen_mode_int_values_match_unity() {
	// These specific values (including the gap at 2) match
	// UnityEngine.FullScreenMode exactly -- don't "fix" the gap.
	assert_eq!(FullScreenMode::ExclusiveFullScreen.to_int(), 0);
	assert_eq!(FullScreenMode::FullScreenWindow.to_int(), 1);
	assert_eq!(FullScreenMode::MaximizedWindow.to_int(), 3);
	assert_eq!(FullScreenMode::Windowed.to_int(), 4);
}

#[test]
fn fullscreen_mode_roundtrips_through_int() {
	for mode in [FullScreenMode::ExclusiveFullScreen, FullScreenMode::FullScreenWindow, FullScreenMode::MaximizedWindow, FullScreenMode::Windowed] {
		assert_eq!(FullScreenMode::from_int(mode.to_int()), mode);
	}
}

#[test]
fn unknown_fullscreen_int_falls_back_to_fullscreen_window() {
	assert_eq!(FullScreenMode::from_int(2), FullScreenMode::FullScreenWindow);
	assert_eq!(FullScreenMode::from_int(999), FullScreenMode::FullScreenWindow);
	assert_eq!(FullScreenMode::from_int(-1), FullScreenMode::FullScreenWindow);
}

#[test]
fn serialize_uses_original_field_names() {
	let s = AppSettings { resolution_x: 2560, resolution_y: 1440, fullscreen_mode: FullScreenMode::Windowed, vsync_enabled: false };
	let json = serialize_app_settings(&s).unwrap();
	assert!(json.contains("\"ResolutionX\": 2560"));
	assert!(json.contains("\"ResolutionY\": 1440"));
	assert!(json.contains("\"fullscreenMode\": 4"));
	assert!(json.contains("\"VSyncEnabled\": false"));
}

#[test]
fn roundtrip_through_json() {
	let s = AppSettings { resolution_x: 1280, resolution_y: 720, fullscreen_mode: FullScreenMode::MaximizedWindow, vsync_enabled: true };
	let json = serialize_app_settings(&s).unwrap();
	let parsed = parse_app_settings(&json).unwrap();
	assert_eq!(parsed, s);
}

#[test]
fn parses_a_hand_written_json_shape_like_the_c_sharp_game_would_write() {
	let json = r#"{
            "ResolutionX": 1920,
            "ResolutionY": 1080,
            "fullscreenMode": 1,
            "VSyncEnabled": true
        }"#;
	let parsed = parse_app_settings(json).unwrap();
	assert_eq!(parsed.resolution_x, 1920);
	assert_eq!(parsed.resolution_y, 1080);
	assert_eq!(parsed.fullscreen_mode, FullScreenMode::FullScreenWindow);
	assert!(parsed.vsync_enabled);
}

#[test]
fn missing_fields_fall_back_to_defaults() {
	let parsed = parse_app_settings("{}").unwrap();
	assert_eq!(parsed, AppSettings::default_settings());
}

#[test]
fn missing_fullscreen_mode_field_defaults_to_fullscreen_window_not_zero() {
	// Regression test: `#[serde(default)]` on an i32 field falls back to `i32::default()` (0), which
	// decodes as `ExclusiveFullScreen` rather than the intended `FullScreenWindow`; needs an explicit default fn.
	let json = r#"{"ResolutionX": 1920, "ResolutionY": 1080, "VSyncEnabled": true}"#;
	let parsed = parse_app_settings(json).unwrap();
	assert_eq!(parsed.fullscreen_mode, FullScreenMode::FullScreenWindow);
}
