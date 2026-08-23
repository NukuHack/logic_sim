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
