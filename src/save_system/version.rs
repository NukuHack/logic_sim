//! Simple `major.minor.patch` version type, ported from the nested
//! `Main.Version` class in the original C# codebase. Used to decide whether
//! a saved project (or chip) is compatible with the running build, and to
//! stamp newly-saved files with the current version.

use std::fmt;

/// The current version of this port. Bump this (and update
/// `Cargo.toml`'s `version` alongside it, if desired) on release.
///
/// Kept in step with the upstream C# game's `Main.DLSVersion` at the time
/// this port was written; the two don't have to move in lockstep going
/// forward, but starting aligned means every file in the four sample
/// projects (`DLSVersion_LastSaved: "2.1.6"` / `"2.1.4"`) is already
/// compatible out of the box.
pub const DLS_VERSION: Version = Version::new(2, 1, 6);

/// The oldest project format this port promises to be able to open.
pub const DLS_VERSION_EARLIEST_COMPATIBLE: Version = Version::new(2, 0, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
	pub major: u32,
	pub minor: u32,
	pub patch: u32,
}

impl Version {
	pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
		Self { major, minor, patch }
	}

	/// Mirrors `Main.Version.ToInt()`. Kept for parity with the original
	/// (and because on-disk/UI code may want a single comparable integer),
	/// but note that ordinary `Version` comparisons (`<`, `>`, ...) don't
	/// need this -- the derived `Ord` already compares major/minor/patch
	/// lexicographically, which is correct even in the (very unlikely)
	/// case a minor or patch component reaches three digits, unlike this
	/// packed-integer form.
	pub fn to_int(self) -> i64 {
		self.major as i64 * 100_000 + self.minor as i64 * 1_000 + self.patch as i64
	}

	/// Mirrors `Main.Version.Parse`. Accepts exactly three dot-separated
	/// integer components, e.g. `"2.1.6"`.
	pub fn parse(s: &str) -> Result<Self, VersionParseError> {
		let mut parts = s.split('.');
		let major = parts.next().ok_or(VersionParseError)?.trim().parse().map_err(|_| VersionParseError)?;
		let minor = parts.next().ok_or(VersionParseError)?.trim().parse().map_err(|_| VersionParseError)?;
		let patch = parts.next().ok_or(VersionParseError)?.trim().parse().map_err(|_| VersionParseError)?;
		if parts.next().is_some() {
			return Err(VersionParseError);
		}
		Ok(Self { major, minor, patch })
	}

	/// Mirrors `Main.Version.TryParse`.
	pub fn try_parse(s: &str) -> Option<Self> {
		Self::parse(s).ok()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionParseError;

impl fmt::Display for VersionParseError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "invalid version string (expected \"major.minor.patch\")")
	}
}

impl std::error::Error for VersionParseError {}

impl fmt::Display for Version {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
	}
}
