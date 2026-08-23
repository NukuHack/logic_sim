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
const RESERVED_NAMES: &[&str] = &[
	"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
	"LPT6", "LPT7", "LPT8", "LPT9",
];

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
			// Read+write instead of `std::fs::copy`: these are always small
			// JSON documents, and glibc implements fs::copy via the
			// copy_file_range syscall, which isn't available everywhere
			// (e.g. under Miri's syscall shims).
			std::fs::write(&target, std::fs::read(&path)?)?;
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

	#[test]
	fn forbidden_chars_are_detected() {
		for c in FORBIDDEN_CHARS {
			let name = format!("chip{c}name");
			assert!(name_contains_forbidden_char(&name), "expected {c:?} to be forbidden");
		}
		assert!(!name_contains_forbidden_char("valid-chip_name 123"));
		assert!(!name_contains_forbidden_char(""));
	}
}
