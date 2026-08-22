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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePaths {
	root: PathBuf,
}

impl SavePaths {
	pub fn new(root: impl Into<PathBuf>) -> Self {
		Self { root: root.into() }
	}

	/// A best-effort, non-Unity-specific default data directory:
	/// `<platform data dir>/DigitalLogicSim`. Falls back to
	/// `./DigitalLogicSimData` if no platform data directory can be
	/// determined (e.g. relevant env vars are unset).
	pub fn default_data_dir() -> PathBuf {
		platform_data_dir().unwrap_or_else(|| PathBuf::from(".")).join("DigitalLogicSim")
	}

	/// The exact save-data directory the original Unity build of Digital
	/// Logic Sim uses (`Application.persistentDataPath`), so this port
	/// reads/writes the *same* projects a player already has on disk
	/// instead of a parallel `DigitalLogicSim` folder next to it:
	///
	/// - Windows: `%USERPROFILE%\AppData\LocalLow\SebastianLague\Digital-Logic-Sim\`
	/// - macOS:   `~/Library/Application Support/SebastianLague/Digital-Logic-Sim/`
	/// - Linux:   `~/.config/unity3d/SebastianLague/Digital-Logic-Sim/`
	///
	/// Falls back to `./Digital-Logic-Sim` (relative to the current
	/// working directory) if the relevant environment variables aren't
	/// set, so the app still has *somewhere* writable to start from
	/// rather than failing outright.
	pub fn unity_persistent_data_dir() -> PathBuf {
		unity_persistent_data_dir_impl().unwrap_or_else(|| PathBuf::from("Digital-Logic-Sim"))
	}

	pub fn root(&self) -> &Path {
		&self.root
	}

	pub fn ensure_directory_exists(path: &Path) -> io::Result<()> {
		std::fs::create_dir_all(path)
	}

	// ---- Path to save folder for all projects ----

	pub fn projects_path(&self) -> PathBuf {
		self.root.join(PROJECTS_DIR_NAME)
	}

	pub fn deleted_projects_path(&self) -> PathBuf {
		self.root.join(DELETED_PROJECTS_DIR_NAME)
	}

	pub fn app_settings_path(&self) -> PathBuf {
		self.root.join(APP_SETTINGS_FILE_NAME)
	}

	// ---- Path to save folder for a specific project ----

	pub fn project_path(&self, project_name: &str) -> PathBuf {
		self.projects_path().join(project_name)
	}

	pub fn deleted_project_path(&self, project_name: &str) -> PathBuf {
		self.deleted_projects_path().join(project_name)
	}

	pub fn chips_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(CHIPS_DIR_NAME)
	}

	pub fn deleted_chips_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(DELETED_CHIPS_DIR_NAME)
	}

	pub fn project_description_path(&self, project_name: &str) -> PathBuf {
		self.project_path(project_name).join(PROJECT_FILE_NAME)
	}
}

fn platform_data_dir() -> Option<PathBuf> {
	#[cfg(target_os = "windows")]
	{
		std::env::var_os("APPDATA").map(PathBuf::from)
	}
	#[cfg(target_os = "macos")]
	{
		std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
	}
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	{
		std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
	}
}

/// Unity's `Application.persistentDataPath` convention specifically (as
/// opposed to `platform_data_dir`'s more generic "a reasonable place to
/// put app data" used by `default_data_dir`): `LocalLow` (not `Roaming`)
/// on Windows, and `~/.config/unity3d` (not XDG data home) on Linux.
fn unity_persistent_data_dir_impl() -> Option<PathBuf> {
	#[cfg(target_os = "windows")]
	{
		// `%APPDATA%` is `...\AppData\Roaming`; Unity's LocalLow folder is a sibling of `Roaming`/`Local`
		// under `...\AppData\` and (unlike them) has no dedicated env var, so it's derived from `%USERPROFILE%`.
		std::env::var_os("USERPROFILE")
			.map(|home| PathBuf::from(home).join("AppData").join("LocalLow").join("SebastianLague").join("Digital-Logic-Sim"))
	}
	#[cfg(target_os = "macos")]
	{
		std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/SebastianLague/Digital-Logic-Sim"))
	}
	#[cfg(not(any(target_os = "windows", target_os = "macos")))]
	{
		std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/unity3d/SebastianLague/Digital-Logic-Sim"))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
		let tmp = crate::save_system::test_util::temp_dir("ensure_dir");
		let nested = tmp.join("a").join("b").join("c");
		assert!(!nested.exists());
		SavePaths::ensure_directory_exists(&nested).unwrap();
		assert!(nested.is_dir());
		std::fs::remove_dir_all(&tmp).ok();
	}
}
