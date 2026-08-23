//! Writes app settings, project descriptions, and chip files to disk.
//! Ported from `DLS.SaveSystem.Saver`.

use std::io;
use std::path::Path;

use crate::description::ChipDescription;
use crate::json::{serialize_chip_description, ProjectDescription};
use crate::save_system::loader::Loader;
use crate::save_system::paths::SavePaths;
use crate::save_system::timestamp::now_iso8601;
use crate::save_system::util::{copy_directory, ensure_unique_file_name};
use crate::save_system::version::DLS_VERSION;
use crate::settings::{serialize_app_settings, AppSettings};

pub struct Saver;

impl Saver {
	/// Mirrors `Saver.SaveAppSettings`.
	pub fn save_app_settings(paths: &SavePaths, settings: &AppSettings) -> io::Result<()> {
		let data = serialize_app_settings(settings).map_err(json_err)?;
		write_to_file(&data, &paths.app_settings_path())
	}

	/// Mirrors `Saver.SaveProjectDescription`: stamps `LastSaveTime` and the
	/// current version onto `description` before writing it out. Note that
	/// `DLSVersion_EarliestCompatible` is intentionally left untouched --
	/// mirroring the original, which only ever sets it when *creating* a
	/// project (see `create_project`), so re-saving an old project doesn't
	/// silently raise the version required to open it again.
	pub fn save_project_description(paths: &SavePaths, description: &mut ProjectDescription) -> io::Result<()> {
		description.last_save_time = now_iso8601();
		description.dls_version_last_saved = DLS_VERSION.to_string();

		let data = crate::json::serialize_project_description(description).map_err(json_err)?;
		write_to_file(&data, &paths.project_description_path(&description.project_name))
	}

	/// Mirrors `Saver.RenameProject`.
	pub fn rename_project(paths: &SavePaths, name_old: &str, name_new: &str) -> io::Result<()> {
		let mut desc = Loader::load_project_description(paths, name_old)?;
		desc.project_name = name_new.to_string();
		std::fs::rename(paths.project_path(name_old), paths.project_path(name_new))?;
		Self::save_project_description(paths, &mut desc)
	}

	/// Mirrors `Saver.DuplicateProject`.
	pub fn duplicate_project(paths: &SavePaths, name_original: &str, name_duplicate: &str) -> io::Result<()> {
		copy_directory(&paths.project_path(name_original), &paths.project_path(name_duplicate), true)?;
		let mut desc_new = Loader::load_project_description(paths, name_duplicate)?;
		desc_new.project_name = name_duplicate.to_string();
		Self::save_project_description(paths, &mut desc_new)
	}

	/// Mirrors `Saver.SaveChip`.
	pub fn save_chip(paths: &SavePaths, project_name: &str, chip_description: &ChipDescription) -> io::Result<()> {
		let data = serialize_chip_description(chip_description).map_err(json_err)?;
		write_to_file(&data, &chip_file_path(paths, project_name, &chip_description.name))
	}

	/// Mirrors `Saver.DeleteChip`: deletes the chip's save file, optionally
	/// keeping a backup copy in `<project>/Deleted Chips/`.
	pub fn delete_chip(paths: &SavePaths, project_name: &str, chip_name: &str, backup_in_deleted_folder: bool) -> io::Result<()> {
		let file_path = chip_file_path(paths, project_name, chip_name);

		if backup_in_deleted_folder {
			let deleted_dir = paths.deleted_chips_path(project_name);
			let deleted_file_path = ensure_unique_file_name(&deleted_dir.join(format!("{chip_name}.json")));
			SavePaths::ensure_directory_exists(&deleted_dir)?;
			std::fs::rename(&file_path, &deleted_file_path)
		} else {
			std::fs::remove_file(&file_path)
		}
	}

	/// Mirrors `Saver.DeleteProject`.
	pub fn delete_project(paths: &SavePaths, project_name: &str, backup_in_deleted_folder: bool) -> io::Result<()> {
		let project_path = paths.project_path(project_name);

		if backup_in_deleted_folder {
			SavePaths::ensure_directory_exists(&paths.deleted_projects_path())?;
			let deleted_path = crate::save_system::util::ensure_unique_directory_name(&paths.deleted_project_path(project_name));
			std::fs::rename(&project_path, &deleted_path)
		} else {
			std::fs::remove_dir_all(&project_path)
		}
	}
}

fn chip_file_path(paths: &SavePaths, project_name: &str, chip_name: &str) -> std::path::PathBuf {
	paths.chips_path(project_name).join(format!("{chip_name}.json"))
}

fn write_to_file(data: &str, path: &Path) -> io::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	std::fs::write(path, data)
}

fn json_err(e: serde_json::Error) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, e)
}
