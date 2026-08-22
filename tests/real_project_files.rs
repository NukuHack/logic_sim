//! Integration test against four real `ProjectDescription.json` files saved by the original C#
//! Digital Logic Sim (projects: GOL, MainTest, Snake, ZHT90), checked into `tests/fixtures/Projects/`
//! verbatim. The goal is to prove this port's save/load system is actually backwards compatible
//! with files the original game produces -- not just with hand-written test fixtures.

use logic_sim::{can_open_project, parse_project_description, Loader, SavePaths, Saver};

const GOL: &str = include_str!("fixtures/Projects/GOL/ProjectDescription.json");
const MAIN_TEST: &str = include_str!("fixtures/Projects/MainTest/ProjectDescription.json");
const SNAKE: &str = include_str!("fixtures/Projects/Snake/ProjectDescription.json");
const ZHT90: &str = include_str!("fixtures/Projects/ZHT90/ProjectDescription.json");

fn temp_dir(label: &str) -> std::path::PathBuf {
	let pid = std::process::id();
	let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
	std::env::temp_dir().join(format!("dls_rust_integration_{label}_{pid}_{nanos}"))
}

#[test]
fn all_four_real_project_files_parse_without_error() {
	for (label, json) in [("GOL", GOL), ("MainTest", MAIN_TEST), ("Snake", SNAKE), ("ZHT90", ZHT90)] {
		let result = parse_project_description(json);
		assert!(result.is_ok(), "{label} failed to parse: {:?}", result.err());
	}
}

#[test]
fn gol_project_fields_match_the_saved_file() {
	let desc = parse_project_description(GOL).unwrap();
	assert_eq!(desc.project_name, "GOL");
	assert_eq!(desc.dls_version_last_saved, "2.1.6");
	assert_eq!(desc.dls_version_earliest_compatible, "2.0.0");
	assert_eq!(desc.prefs_sim_target_steps_per_second, 1000);
	assert_eq!(desc.prefs_sim_steps_per_clock_tick, 4);
	assert!(!desc.prefs_sim_paused);
	assert_eq!(desc.all_custom_chip_names.len(), 25);
	assert!(desc.all_custom_chip_names.contains(&"GOL".to_string()));
	assert!(desc.all_custom_chip_names.contains(&"CELLS 64x64 WW".to_string()));

	// Starred list mixes collections and individual chips.
	assert!(desc.is_starred("IN/OUT", true));
	assert!(desc.is_starred("GOL", false));
	assert!(!desc.is_starred("GOL", true)); // it's starred as a chip, not a collection

	// Chip collections preserve nested chip lists and open/closed state.
	let gol_collection = desc.chip_collections.iter().find(|c| c.name == "GAME OF LIFE").unwrap();
	assert!(gol_collection.is_toggled_open);
	assert!(gol_collection.chips.contains(&"CELLS 64x64 WRAPPER".to_string()));
}

#[test]
fn main_test_project_has_expected_scale() {
	let desc = parse_project_description(MAIN_TEST).unwrap();
	assert_eq!(desc.project_name, "MainTest");
	assert_eq!(desc.prefs_sim_target_steps_per_second, 150_000);
	assert_eq!(desc.all_custom_chip_names.len(), 51);
	assert!(desc.all_custom_chip_names.contains(&"CONTROL UNIT".to_string()));
	// A single-character chip name -- edge case for name-based lookups.
	assert!(desc.all_custom_chip_names.contains(&"#".to_string()));
}

#[test]
fn snake_project_prefs_and_starred_list_round_trip() {
	let desc = parse_project_description(SNAKE).unwrap();
	assert_eq!(desc.project_name, "Snake");
	assert_eq!(desc.dls_version_last_saved, "2.1.4");
	assert_eq!(desc.prefs_snapping, 2);
	assert_eq!(desc.prefs_straight_wires, 2);
	assert!(desc.is_starred("OTHER", true));
	assert!(desc.is_starred("1-Reg", false));
}

#[test]
fn zht90_project_has_largest_chip_set_and_deep_collections() {
	let desc = parse_project_description(ZHT90).unwrap();
	assert_eq!(desc.project_name, "ZHT90");
	assert_eq!(desc.all_custom_chip_names.len(), 79);
	assert_eq!(desc.chip_collections.len(), 9);
	let calc = desc.chip_collections.iter().find(|c| c.name == "CALC").unwrap();
	assert!(calc.chips.contains(&"Program Counter".to_string()));
}

#[test]
fn all_four_real_projects_are_openable_by_this_port() {
	for (label, json) in [("GOL", GOL), ("MainTest", MAIN_TEST), ("Snake", SNAKE), ("ZHT90", ZHT90)] {
		let desc = parse_project_description(json).unwrap();
		assert_eq!(can_open_project(&desc), Ok(()), "{label} should be openable");
	}
}

#[test]
fn round_tripping_a_real_project_file_preserves_its_data() {
	let original = parse_project_description(GOL).unwrap();
	let reserialized = logic_sim::serialize_project_description(&original).unwrap();
	let reparsed = parse_project_description(&reserialized).unwrap();

	assert_eq!(reparsed.project_name, original.project_name);
	assert_eq!(reparsed.all_custom_chip_names, original.all_custom_chip_names);
	assert_eq!(reparsed.starred_list, original.starred_list);
	assert_eq!(reparsed.chip_collections, original.chip_collections);
}

/// End-to-end through the actual save/load system: drop a real
/// `ProjectDescription.json` (as the C# game would have written it) into
/// a project directory on disk, then load it exactly the way the startup
/// screen's "Open Project" flow does.
#[test]
fn loader_reads_a_real_c_sharp_project_description_from_disk() {
	let root = temp_dir("real_project_from_disk");
	let paths = SavePaths::new(&root);
	SavePaths::ensure_directory_exists(&paths.project_path("GOL")).unwrap();
	std::fs::write(paths.project_description_path("GOL"), GOL).unwrap();

	let loaded = Loader::load_project_description(&paths, "GOL").unwrap();
	assert_eq!(loaded.project_name, "GOL");
	assert_eq!(loaded.dls_version_last_saved, "2.1.6");
	assert_eq!(can_open_project(&loaded), Ok(()));

	// And it shows up in the project picker's listing.
	let all = Loader::load_all_project_descriptions(&paths);
	assert_eq!(all.len(), 1);
	assert_eq!(all[0].project_name, "GOL");

	std::fs::remove_dir_all(&root).ok();
}

/// Re-saving a project originally written by the C# game (via `Saver`)
/// should only touch `LastSaveTime` / `DLSVersion_LastSaved` -- everything
/// else the player set up (custom chip list, starred items, collections,
/// prefs) must survive untouched.
#[test]
fn resaving_a_real_c_sharp_project_preserves_player_data() {
	let root = temp_dir("resave_real_project");
	let paths = SavePaths::new(&root);
	SavePaths::ensure_directory_exists(&paths.project_path("ZHT90")).unwrap();
	std::fs::write(paths.project_description_path("ZHT90"), ZHT90).unwrap();

	let mut desc = Loader::load_project_description(&paths, "ZHT90").unwrap();
	let original_custom_chips = desc.all_custom_chip_names.clone();
	let original_starred = desc.starred_list.clone();
	let original_collections = desc.chip_collections.clone();
	let original_prefs_snapping = desc.prefs_snapping;

	Saver::save_project_description(&paths, &mut desc).unwrap();

	let reloaded = Loader::load_project_description(&paths, "ZHT90").unwrap();
	assert_eq!(reloaded.all_custom_chip_names, original_custom_chips);
	assert_eq!(reloaded.starred_list, original_starred);
	assert_eq!(reloaded.chip_collections, original_collections);
	assert_eq!(reloaded.prefs_snapping, original_prefs_snapping);
	assert_eq!(reloaded.dls_version_last_saved, logic_sim::DLS_VERSION.to_string());

	std::fs::remove_dir_all(&root).ok();
}
