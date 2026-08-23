//! Reads app settings, project descriptions, and chip libraries from disk.
//! Ported from `DLS.SaveSystem.Loader`.

use std::io;

use crate::builtins;
use crate::description::ChipLibrary;
use crate::json::{parse_project_description, ProjectDescription};
use crate::save_system::paths::SavePaths;
use crate::save_system::project::Project;
use crate::settings::{parse_app_settings, AppSettings};

pub struct Loader;

impl Loader {
	/// Mirrors `Loader.LoadAppSettings`: falls back to
	/// `AppSettings::default_settings()` if no settings file exists yet, or
	/// if it can't be parsed (matching the original's "start fresh rather
	/// than crash on a corrupt settings file" behaviour).
	pub fn load_app_settings(paths: &SavePaths) -> AppSettings {
		match std::fs::read_to_string(paths.app_settings_path()) {
			Ok(text) => parse_app_settings(&text).unwrap_or_default(),
			Err(_) => AppSettings::default_settings(),
		}
	}

	/// Mirrors `Loader.ProjectExists`.
	pub fn project_exists(paths: &SavePaths, project_name: &str) -> bool {
		paths.project_description_path(project_name).is_file()
	}

	/// Mirrors `Loader.LoadProjectDescription`. Errors if no project
	/// description file exists at the expected path.
	pub fn load_project_description(paths: &SavePaths, project_name: &str) -> io::Result<ProjectDescription> {
		let path = paths.project_description_path(project_name);
		let text = std::fs::read_to_string(&path)
			.map_err(|e| io::Error::new(e.kind(), format!("No project description found at {}: {e}", path.display())))?;

		let mut desc = parse_project_description(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
		// Enforce name == directory name, in case the player edited the
		// file by hand -- operations like deleting/renaming projects rely
		// on this invariant.
		desc.project_name = project_name.to_string();
		Ok(desc)
	}

	/// Mirrors `Loader.LoadAllProjectDescriptions`: every project under
	/// `<root>/Projects/`, sorted newest-`LastSaveTime`-first. Directories
	/// that fail to load (missing/corrupt `ProjectDescription.json`) are
	/// silently skipped, matching the original.
	pub fn load_all_project_descriptions(paths: &SavePaths) -> Vec<ProjectDescription> {
		let mut descriptions = Vec::new();

		let Ok(entries) = std::fs::read_dir(paths.projects_path()) else {
			return descriptions;
		};

		for entry in entries.flatten() {
			if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
				continue;
			}
			let Some(project_name) = entry.file_name().to_str().map(str::to_owned) else { continue };
			if let Ok(desc) = Self::load_project_description(paths, &project_name) {
				descriptions.push(desc);
			}
		}

		// Newest first, matching `LastSaveTime` string comparison in the
		// original (ISO-8601 timestamps sort correctly as plain strings).
		descriptions.sort_by(|a, b| b.last_save_time.cmp(&a.last_save_time));
		descriptions
	}

	/// Mirrors `Loader.LoadProject`: description + chip library (custom
	/// chips from disk, plus every builtin chip not shadowed by a
	/// same-named custom chip).
	pub fn load_project(paths: &SavePaths, project_name: &str) -> io::Result<Project> {
		let description = Self::load_project_description(paths, project_name)?;
		let chip_library = Self::load_chip_library(paths, &description)?;
		Ok(Project::new(description, chip_library))
	}

	fn load_chip_library(paths: &SavePaths, description: &ProjectDescription) -> io::Result<ChipLibrary> {
		let chips_dir = paths.chips_path(&description.project_name);

		if !description.all_custom_chip_names.is_empty() && !chips_dir.is_dir() {
			return Err(io::Error::new(io::ErrorKind::NotFound, format!("Chips directory not found: {}", chips_dir.display())));
		}

		let mut library = ChipLibrary::new();
		let mut custom_names_lower = std::collections::HashSet::new();

		for chip_name in &description.all_custom_chip_names {
			let chip_path = chips_dir.join(format!("{chip_name}.json"));
			let text = std::fs::read_to_string(&chip_path)?;
			let chip_desc = crate::json::parse_chip_description(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
			custom_names_lower.insert(chip_desc.name.to_ascii_lowercase());
			library.add(chip_desc);
		}

		// If a builtin chip name conflicts with a custom chip, exclude the builtin so the custom chip
		// wins (mirrors `Loader.LoadChipLibrary`'s TODO-flagged behaviour: this is a silent shadow,
		// not a rename prompt).
		for builtin_desc in builtins::create_all() {
			if !custom_names_lower.contains(&builtin_desc.name.to_ascii_lowercase()) {
				library.add(builtin_desc);
			}
		}

		Ok(library)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::save_system::saver::Saver;
	use crate::save_system::test_util::temp_dir;

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
			let data = crate::json::serialize_project_description(&desc).unwrap();
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
		let mut custom = crate::description::ChipDescription::new("MY GATE", crate::description::ChipType::Custom);
		custom.input_pins.push(crate::description::PinDescription::new("A", 0, crate::description::PinBitCount::Bit1));
		Saver::save_chip(&paths, "P", &custom).unwrap();

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
		let mut custom = crate::description::ChipDescription::new("NAND", crate::description::ChipType::Custom);
		custom.input_pins.push(crate::description::PinDescription::new("MyInput", 0, crate::description::PinBitCount::Bit1));
		Saver::save_chip(&paths, "P", &custom).unwrap();

		let mut desc = ProjectDescription { project_name: "P".to_string(), all_custom_chip_names: vec!["NAND".to_string()], ..Default::default() };
		Saver::save_project_description(&paths, &mut desc).unwrap();

		let project = Loader::load_project(&paths, "P").unwrap();
		let nand = project.chip_library.get("NAND");
		assert_eq!(nand.chip_type, crate::description::ChipType::Custom);
		assert_eq!(nand.input_pins[0].name, "MyInput");

		std::fs::remove_dir_all(&root).ok();
	}
}
