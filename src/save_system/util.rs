//! Filename validation and small filesystem helpers, ported from
//! `DLS.SaveSystem.SaveUtils`.

use std::io;
use std::path::{Path, PathBuf};

/// Characters disallowed in project/chip names because they're illegal (or
/// behave strangely) as file/directory names on some operating systems.
/// Matches the original's `ForbiddenChars` set exactly.
const FORBIDDEN_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*', '.'];

/// Reserved device names on Windows -- matches the original's
/// `ReservedNames` list exactly.
const RESERVED_NAMES: &[&str] =
    &["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];

/// Mirrors `SaveUtils.NameContainsForbiddenChar`.
pub fn name_contains_forbidden_char(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars().any(|c| FORBIDDEN_CHARS.contains(&c))
}

fn is_reserved_file_name(name: &str) -> bool {
    let trimmed = name.trim();
    RESERVED_NAMES.iter().any(|reserved| trimmed.eq_ignore_ascii_case(reserved))
}

/// Mirrors `SaveUtils.ValidFileName`: true if `name` is safe to use as a
/// file/directory name on every operating system this game supports.
pub fn valid_file_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    !name_contains_forbidden_char(name) && !is_reserved_file_name(name)
}

/// Mirrors `SaveUtils.EnsureUniqueFileName`: if `original_path` already
/// exists, appends `_1`, `_2`, ... (before the extension) until a free path
/// is found.
pub fn ensure_unique_file_name(original_path: &Path) -> PathBuf {
    if !original_path.exists() {
        return original_path.to_path_buf();
    }

    let parent = original_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = original_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = original_path.extension().map(|e| e.to_string_lossy().into_owned());

    let mut duplicates = 0;
    loop {
        duplicates += 1;
        let candidate_name = match &ext {
            Some(ext) => format!("{stem}_{duplicates}.{ext}"),
            None => format!("{stem}_{duplicates}"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
}

/// Mirrors `SaveUtils.EnsureUniqueDirectoryName`: if `path` already exists,
/// appends `_1`, `_2`, ... until a free path is found.
pub fn ensure_unique_directory_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let mut duplicates = 0;
    loop {
        duplicates += 1;
        let candidate = append_to_file_name(path, &format!("_{duplicates}"));
        if !candidate.exists() {
            return candidate;
        }
    }
}

fn append_to_file_name(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.push_str(suffix);
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Mirrors `SaveUtils.CopyDirectory`.
pub fn copy_directory(source_dir: &Path, destination_dir: &Path, recursive: bool) -> io::Result<()> {
    if !source_dir.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Source directory not found: {}", source_dir.display())));
    }

    std::fs::create_dir_all(destination_dir)?;

    for entry in std::fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_file() {
            let target = destination_dir.join(entry.file_name());
            std::fs::copy(&path, &target)?;
        } else if file_type.is_dir() && recursive {
            let target = destination_dir.join(entry.file_name());
            copy_directory(&path, &target, true)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_system::test_util::temp_dir;

    #[test]
    fn forbidden_chars_are_detected() {
        for c in FORBIDDEN_CHARS {
            let name = format!("chip{c}name");
            assert!(name_contains_forbidden_char(&name), "expected {c:?} to be forbidden");
        }
        assert!(!name_contains_forbidden_char("valid-chip_name 123"));
        assert!(!name_contains_forbidden_char(""));
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
}
