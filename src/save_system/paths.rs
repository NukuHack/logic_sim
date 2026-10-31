//! Path layout for saved data, ported from `DLS.SaveSystem.SavePaths`. Unlike the original (which
//! hangs everything off a single implicit global), this is a plain value you construct and pass
//! around, making it trivial to point at a temp directory in tests and leaving the choice of default
//! location up to the host application. On-disk layout mirrors the original: a `Projects/` directory
//! of named project folders, each holding `ProjectDescription.json` and a `Chips/` subfolder.

use std::io;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE_NAME: &str = "ProjectDescription.json";
const PROJECTS_DIR_NAME: &str = "Projects";
const DELETED_PROJECTS_DIR_NAME: &str = "Deleted Projects";
const CHIPS_DIR_NAME: &str = "Chips";
const DELETED_CHIPS_DIR_NAME: &str = "Deleted Chips";
const APP_SETTINGS_FILE_NAME: &str = "AppSettings.json";

/// Root-relative path layout for all saved data. Mirrors the *shape* of
/// `DLS.SaveSystem.SavePaths`; the actual root directory is up to the
/// caller (see `default_data_dir` for a reasonable non-Unity default).
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePaths {
	root: PathBuf,
}

impl SavePaths {
	pub fn new(root: impl Into<PathBuf>) -> Self {
		Self { root: root.into() }
	}

	/// A best-effort, non-Unity-specific default data directory using the `dirs` crate.
	/// Uses the appropriate platform-specific data directory:
	/// - Windows: `%APPDATA%\DigitalLogicSim`
	/// - macOS: `~/Library/Application Support/DigitalLogicSim`
	/// - Linux: `~/.local/share/DigitalLogicSim`
	/// - Falls back to `./DigitalLogicSimData` if no platform data directory can be determined.
	#[must_use]
	pub fn default_data_dir() -> PathBuf {
		dirs::data_dir().map_or_else(|| PathBuf::from(".").join("DigitalLogicSimData"), |d| d.join("DigitalLogicSim"))
	}

	/// The exact save-data directory the original Unity build of Digital Logic Sim uses
	/// (`Application.persistentDataPath`), so this port reads/writes the *same* projects a
	/// player already has on disk:
	/// - Windows: `%USERPROFILE%\AppData\LocalLow\SebastianLague\Digital-Logic-Sim\`
	/// - macOS: `~/Library/Application Support/SebastianLague/Digital-Logic-Sim/`
	/// - Linux: `~/.config/unity3d/SebastianLague/Digital-Logic-Sim/`
	/// - Falls back to `./Digital-Logic-Sim` (relative to the current working directory)
	///   if the relevant environment variables aren't set.
	///
	/// Unity's `Application.persistentDataPath` convention specifically (as
	/// opposed to `platform_data_dir`'s more generic "a reasonable place to
	/// put app data" used by `default_data_dir`): `LocalLow` (not `Roaming`)
	/// on Windows, and `~/.config/unity3d` (not XDG data home) on Linux.
	#[must_use]
	pub fn unity_persistent_data_dir() -> PathBuf {
		let path = {
			#[cfg(target_os = "windows")]
			{
				"AppData/LocalLow/SebastianLague/Digital-Logic-Sim"
			}
			#[cfg(target_os = "macos")]
			{
				"Library/Application Support/SebastianLague/Digital-Logic-Sim"
			}
			#[cfg(not(any(target_os = "windows", target_os = "macos")))]
			{
				".config/unity3d/SebastianLague/Digital-Logic-Sim"
			}
		};

		dirs::home_dir().map_or_else(|| PathBuf::from("Digital-Logic-Sim"), |home| home.join(path))
	}

	#[must_use]
	pub fn root(&self) -> &Path {
		&self.root
	}

	/// Opens the save-data root in the platform's file browser (`Main.OpenSaveDataFolderInFileBrowser`,
	/// bound to Ctrl+Shift+Alt+O). Fire-and-forget: the browser is spawned, not waited on.
	///
	/// # Errors
	/// If the folder doesn't exist, or the platform's opener can't be started.
	pub fn open_in_file_browser(&self) -> io::Result<()> {
		if !self.root.is_dir() {
			return Err(io::Error::new(io::ErrorKind::NotFound, format!("Path does not exist: {}", self.root.display())));
		}
		let opener = if cfg!(target_os = "windows") {
			"explorer"
		} else if cfg!(target_os = "macos") {
			"open"
		} else {
			"xdg-open"
		};
		std::process::Command::new(opener).arg(&self.root).spawn().map(|_| ())
	}

	/// # Errors
	/// propagates from `create_dir_all`
	pub fn ensure_directory_exists(path: &Path) -> io::Result<()> {
		std::fs::create_dir_all(path)
	}

	// ---- Path to save folder for all projects ----
	#[must_use]
	pub fn projects_path(&self) -> PathBuf {
		self.root.join(PROJECTS_DIR_NAME)
	}

	#[must_use]
	pub fn deleted_projects_path(&self) -> PathBuf {
		self.root.join(DELETED_PROJECTS_DIR_NAME)
	}

	#[must_use]
	pub fn app_settings_path(&self) -> PathBuf {
		self.root.join(APP_SETTINGS_FILE_NAME)
	}

	// ---- Path to save folder for a specific project ----
	#[must_use]
	pub fn project_path(&self, project_name: &str) -> PathBuf {
		self.projects_path().join(project_name)
	}

	#[must_use]
	pub fn deleted_project_path(&self, project_name: &str) -> PathBuf {
		self.deleted_projects_path().join(project_name)
	}

	#[must_use]
	pub fn chips_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(CHIPS_DIR_NAME)
	}

	#[must_use]
	pub fn deleted_chips_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(DELETED_CHIPS_DIR_NAME)
	}

	#[must_use]
	pub fn project_description_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(PROJECT_FILE_NAME)
	}
}

#[cfg(test)]
mod open_folder_tests {
	use super::*;

	#[test]
	fn opening_a_missing_save_folder_reports_not_found_instead_of_spawning_anything() {
		let paths = SavePaths::new(std::env::temp_dir().join("dls-definitely-not-here-4242"));
		let err = paths.open_in_file_browser().unwrap_err();
		assert_eq!(err.kind(), io::ErrorKind::NotFound);
	}
}
