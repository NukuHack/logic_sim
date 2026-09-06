//! Application-level settings (window resolution, fullscreen mode, vsync), saved at
//! `<data dir>/AppSettings.json`. Mirrors `DLS.Description.AppSettings` and the `FullScreenMode` enum
//! it depends on. This is intentionally decoupled from any particular windowing backend (winit, etc):
//! it's just the persisted data plus its on-disk shape. Whatever sets up the window is expected to
//! translate `AppSettings` into whatever its windowing library needs.

use logic_sim_macros::ConstFromPrimitive;
use serde::{Deserialize, Serialize};

/// Mirrors `UnityEngine.FullScreenMode`. The on-disk integer values are
/// Unity's own enum values (there's a deliberate "hole" at 2 -- that's not a
/// typo, it matches upstream so that AppSettings.json files written by the
/// original C# game round-trip correctly through this port).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, ConstFromPrimitive)]
#[repr(i32)]
pub enum FullScreenMode {
	ExclusiveFullScreen = 0,
	#[default]
	FullScreenWindow = 1,
	MaximizedWindow = 3,
	Windowed = 4,
}

impl FullScreenMode {
	/// Convert to the integer representation used on disk.
	pub const fn to_int(&self) -> i32 {
		*self as i32
	}
	/// Reconstruct from an integer, matching the original C# enum order.
	/// Invalid values fall back to `Custom`.
	pub const fn from_int(v: i32) -> Self {
		Self::from_primitive(v)
	}
}

/// Mirrors `DLS.Description.AppSettings`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AppSettings {
	pub resolution_x: i32,
	pub resolution_y: i32,
	pub fullscreen_mode: FullScreenMode,
	pub vsync_enabled: bool,
}

impl AppSettings {
	/// Mirrors `AppSettings.Default()`.
	pub const fn default_settings() -> Self {
		Self { resolution_x: 1920, resolution_y: 1080, fullscreen_mode: FullScreenMode::FullScreenWindow, vsync_enabled: true }
	}
}

impl Default for AppSettings {
	fn default() -> Self {
		Self::default_settings()
	}
}

/// On-disk shape of AppSettings.json. Field names match the original C#
/// struct's field names exactly (including the lowercase `fullscreenMode`,
/// which is how it was declared there) so files are interchangeable between
/// the two implementations.
#[derive(Debug, Serialize, Deserialize)]
struct JsonAppSettings {
	#[serde(rename = "ResolutionX", default = "default_resolution_x")]
	resolution_x: i32,
	#[serde(rename = "ResolutionY", default = "default_resolution_y")]
	resolution_y: i32,
	#[serde(rename = "fullscreenMode", default = "default_fullscreen_mode")]
	fullscreen_mode: i32,
	#[serde(rename = "VSyncEnabled", default = "default_vsync")]
	vsync_enabled: bool,
}

const fn default_resolution_x() -> i32 {
	1920
}
const fn default_resolution_y() -> i32 {
	1080
}
const fn default_fullscreen_mode() -> i32 {
	FullScreenMode::FullScreenWindow.to_int()
}
const fn default_vsync() -> bool {
	true
}

impl From<AppSettings> for JsonAppSettings {
	fn from(s: AppSettings) -> Self {
		Self {
			resolution_x: s.resolution_x,
			resolution_y: s.resolution_y,
			fullscreen_mode: s.fullscreen_mode.to_int(),
			vsync_enabled: s.vsync_enabled,
		}
	}
}

impl From<JsonAppSettings> for AppSettings {
	fn from(j: JsonAppSettings) -> Self {
		Self {
			resolution_x: j.resolution_x,
			resolution_y: j.resolution_y,
			fullscreen_mode: FullScreenMode::from_int(j.fullscreen_mode),
			vsync_enabled: j.vsync_enabled,
		}
	}
}

pub fn parse_app_settings(json: &str) -> serde_json::Result<AppSettings> {
	let raw: JsonAppSettings = serde_json::from_str(json)?;
	Ok(raw.into())
}

pub fn serialize_app_settings(settings: &AppSettings) -> serde_json::Result<String> {
	let raw: JsonAppSettings = (*settings).into();
	serde_json::to_string_pretty(&raw)
}
