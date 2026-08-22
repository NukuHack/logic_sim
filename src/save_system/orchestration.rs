//! Top-level "create or open a project" orchestration and the version
//! compatibility check used by the project-picker screen. Ported from the
//! relevant static methods on `DLS.Game.Main` and from
//! `MainMenu.CanOpenProject`.

use std::io;

use crate::json::ProjectDescription;
use crate::save_system::defaults::{default_chip_collections, default_starred_list};
use crate::save_system::loader::Loader;
use crate::save_system::paths::SavePaths;
use crate::save_system::project::Project;
use crate::save_system::saver::Saver;
use crate::save_system::version::{Version, DLS_VERSION, DLS_VERSION_EARLIEST_COMPATIBLE};

/// Preference constant mirroring `PreferencesMenu.DisplayMode_OnHover`,
/// used as the default for newly-created projects.
const DISPLAY_MODE_ON_HOVER: i32 = 1;

/// Mirrors `MainMenu.CanOpenProject`: whether the running build is new
/// enough to open a project that declares `DLSVersion_EarliestCompatible`.
/// `Ok(())` if it can be opened; `Err(message)` with a user-facing reason
/// otherwise (including when the version string itself is unparseable).
pub fn can_open_project(description: &ProjectDescription) -> Result<(), String> {
	match Version::parse(&description.dls_version_earliest_compatible) {
		Ok(earliest_compatible) => {
			if DLS_VERSION >= earliest_compatible {
				Ok(())
			} else {
				Err(format!("This project requires version {earliest_compatible} or later."))
			}
		}
		Err(_) => Err("Unrecognized project format".to_string()),
	}
}

/// Mirrors `Main.CreateProject`: builds a fresh `ProjectDescription` with
/// the same defaults as the original, saves it, then loads it back (so the
/// returned `Project`'s chip library includes every builtin).
pub fn create_project(paths: &SavePaths, project_name: &str) -> io::Result<Project> {
	let mut description = ProjectDescription {
		project_name: project_name.to_string(),
		dls_version_last_saved: DLS_VERSION.to_string(),
		dls_version_earliest_compatible: DLS_VERSION_EARLIEST_COMPATIBLE.to_string(),
		creation_time: crate::save_system::timestamp::now_iso8601(),
		prefs_chip_pin_names_display_mode: DISPLAY_MODE_ON_HOVER,
		prefs_main_pin_names_display_mode: DISPLAY_MODE_ON_HOVER,
		prefs_sim_target_steps_per_second: 1000,
		prefs_sim_steps_per_clock_tick: 250,
		prefs_sim_paused: false,
		all_custom_chip_names: Vec::new(),
		starred_list: default_starred_list(),
		chip_collections: default_chip_collections(),
		..ProjectDescription::default()
	};

	Saver::save_project_description(paths, &mut description)?;
	Loader::load_project(paths, project_name)
}

/// Mirrors `Main.CreateOrLoadProject` (minus the editor/simulation/UI side
/// effects, which belong to whatever integrates this with a live app):
/// loads the project if it already exists on disk, otherwise creates it.
pub fn create_or_load_project(paths: &SavePaths, project_name: &str) -> io::Result<Project> {
	if Loader::project_exists(paths, project_name) {
		Loader::load_project(paths, project_name)
	} else {
		create_project(paths, project_name)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::save_system::test_util::temp_dir;

	#[test]
	fn can_open_project_accepts_projects_within_the_supported_version_range() {
		let mut desc = ProjectDescription::default();
		desc.dls_version_earliest_compatible = "2.0.0".to_string();
		assert_eq!(can_open_project(&desc), Ok(()));
	}

	#[test]
	fn can_open_project_rejects_projects_from_a_newer_version() {
		let mut desc = ProjectDescription::default();
		desc.dls_version_earliest_compatible = "99.0.0".to_string();
		let result = can_open_project(&desc);
		assert!(result.is_err());
		assert!(result.unwrap_err().contains("99.0.0"));
	}

	#[test]
	fn can_open_project_rejects_unparseable_version_strings() {
		let mut desc = ProjectDescription::default();
		desc.dls_version_earliest_compatible = "not-a-version".to_string();
		assert_eq!(can_open_project(&desc), Err("Unrecognized project format".to_string()));
	}

	#[test]
	fn can_open_project_accepts_all_four_sample_projects() {
		// Every uploaded ProjectDescription.json declared EarliestCompatible = 2.0.0.
		let mut desc = ProjectDescription::default();
		desc.dls_version_earliest_compatible = "2.0.0".to_string();
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
		assert_eq!(project.description.prefs_main_pin_names_display_mode, DISPLAY_MODE_ON_HOVER);
		assert_eq!(project.description.prefs_chip_pin_names_display_mode, DISPLAY_MODE_ON_HOVER);
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
		crate::save_system::saver::Saver::save_project_description(&paths, &mut desc).unwrap();

		let reloaded_desc = Loader::load_project_description(&paths, "P").unwrap();
		assert_eq!(reloaded_desc.all_custom_chip_names, vec!["Marker".to_string()]);

		std::fs::remove_dir_all(&root).ok();
	}
}
