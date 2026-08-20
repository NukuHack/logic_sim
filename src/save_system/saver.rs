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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::ChipType;
    use crate::save_system::loader::Loader;
    use crate::save_system::test_util::temp_dir;

    fn sample_description(name: &str) -> ProjectDescription {
        let mut d = ProjectDescription::default();
        d.project_name = name.to_string();
        d.creation_time = "2025-01-01T00:00:00.000Z".to_string();
        d
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
        chip.input_pins.push(crate::description::PinDescription::new("A", 0, crate::description::PinBitCount::Bit1));
        Saver::save_chip(&paths, "P", &chip).unwrap();

        assert!(paths.chips_path("P").join("MY GATE.json").is_file());

        let (library, errors) = crate::json::load_chip_library_from_dir(&paths.chips_path("P")).unwrap();
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
        Saver::save_chip(&paths, "P", &chip).unwrap();

        Saver::delete_chip(&paths, "P", "TEMP", true).unwrap();

        assert!(!chip_file_path(&paths, "P", "TEMP").exists());
        assert!(paths.deleted_chips_path("P").join("TEMP.json").is_file());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_chip_without_backup_removes_file_permanently() {
        let root = temp_dir("delete_chip_no_backup");
        let paths = SavePaths::new(&root);
        let chip = ChipDescription::new("TEMP", ChipType::Custom);
        Saver::save_chip(&paths, "P", &chip).unwrap();

        Saver::delete_chip(&paths, "P", "TEMP", false).unwrap();

        assert!(!chip_file_path(&paths, "P", "TEMP").exists());
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
}
