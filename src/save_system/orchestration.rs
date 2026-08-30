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
		prefs_use_caching: true,
		all_custom_chip_names: Vec::new(),
		starred_list: default_starred_list(),
		chip_collections: default_chip_collections(),
		..ProjectDescription::default()
	};

	Saver::save_project_description(paths, &mut description)?;
	// `save_project_description` only creates the project's own directory; `Chips/` is otherwise created
	// lazily on first chip save, leaving a brand-new project unreadable (`json::load_project` hard-fails
	// on a missing `Chips/`). Create it up front so a new project is fully-formed and open-able.
	SavePaths::ensure_directory_exists(&paths.chips_path(project_name))?;
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
