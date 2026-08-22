//! Application-level settings (window resolution, fullscreen mode, vsync),
//! saved at `<data dir>/AppSettings.json`. Mirrors `DLS.Description.AppSettings`
//! and the `FullScreenMode` enum it depends on (`UnityEngine.FullScreenMode`).
//!
//! This is intentionally decoupled from any particular windowing backend
//! (winit, etc): it's just the persisted data + its on-disk shape. Whatever
//! sets up the window is expected to translate `AppSettings` into whatever
//! its windowing library needs.

use serde::{Deserialize, Serialize};

/// Mirrors `UnityEngine.FullScreenMode`. The on-disk integer values are
/// Unity's own enum values (there's a deliberate "hole" at 2 -- that's not a
/// typo, it matches upstream so that AppSettings.json files written by the
/// original C# game round-trip correctly through this port).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FullScreenMode {
	ExclusiveFullScreen,
	#[default]
	FullScreenWindow,
	MaximizedWindow,
	Windowed,
}

impl FullScreenMode {
	pub fn to_int(self) -> i32 {
		match self {
			FullScreenMode::ExclusiveFullScreen => 0,
			FullScreenMode::FullScreenWindow => 1,
			FullScreenMode::MaximizedWindow => 3,
			FullScreenMode::Windowed => 4,
		}
	}

	/// Any value not matching a known variant (including the deliberately
	/// unused `2`) falls back to `FullScreenWindow`, matching the original's
	/// default full-screen behaviour.
	pub fn from_int(v: i32) -> Self {
		match v {
			0 => FullScreenMode::ExclusiveFullScreen,
			3 => FullScreenMode::MaximizedWindow,
			4 => FullScreenMode::Windowed,
			_ => FullScreenMode::FullScreenWindow,
		}
	}
}

/// Mirrors `DLS.Description.AppSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppSettings {
	pub resolution_x: i32,
	pub resolution_y: i32,
	pub fullscreen_mode: FullScreenMode,
	pub vsync_enabled: bool,
}

impl AppSettings {
	/// Mirrors `AppSettings.Default()`.
	pub fn default_settings() -> Self {
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

fn default_resolution_x() -> i32 {
	1920
}
fn default_resolution_y() -> i32 {
	1080
}
fn default_fullscreen_mode() -> i32 {
	FullScreenMode::FullScreenWindow.to_int()
}
fn default_vsync() -> bool {
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn default_settings_match_original() {
		let s = AppSettings::default_settings();
		assert_eq!(s.resolution_x, 1920);
		assert_eq!(s.resolution_y, 1080);
		assert_eq!(s.fullscreen_mode, FullScreenMode::FullScreenWindow);
		assert!(s.vsync_enabled);
	}

	#[test]
	fn fullscreen_mode_int_values_match_unity() {
		// These specific values (including the gap at 2) match
		// UnityEngine.FullScreenMode exactly -- don't "fix" the gap.
		assert_eq!(FullScreenMode::ExclusiveFullScreen.to_int(), 0);
		assert_eq!(FullScreenMode::FullScreenWindow.to_int(), 1);
		assert_eq!(FullScreenMode::MaximizedWindow.to_int(), 3);
		assert_eq!(FullScreenMode::Windowed.to_int(), 4);
	}

	#[test]
	fn fullscreen_mode_roundtrips_through_int() {
		for mode in [FullScreenMode::ExclusiveFullScreen, FullScreenMode::FullScreenWindow, FullScreenMode::MaximizedWindow, FullScreenMode::Windowed]
		{
			assert_eq!(FullScreenMode::from_int(mode.to_int()), mode);
		}
	}

	#[test]
	fn unknown_fullscreen_int_falls_back_to_fullscreen_window() {
		assert_eq!(FullScreenMode::from_int(2), FullScreenMode::FullScreenWindow);
		assert_eq!(FullScreenMode::from_int(999), FullScreenMode::FullScreenWindow);
		assert_eq!(FullScreenMode::from_int(-1), FullScreenMode::FullScreenWindow);
	}

	#[test]
	fn serialize_uses_original_field_names() {
		let s = AppSettings { resolution_x: 2560, resolution_y: 1440, fullscreen_mode: FullScreenMode::Windowed, vsync_enabled: false };
		let json = serialize_app_settings(&s).unwrap();
		assert!(json.contains("\"ResolutionX\": 2560"));
		assert!(json.contains("\"ResolutionY\": 1440"));
		assert!(json.contains("\"fullscreenMode\": 4"));
		assert!(json.contains("\"VSyncEnabled\": false"));
	}

	#[test]
	fn roundtrip_through_json() {
		let s = AppSettings { resolution_x: 1280, resolution_y: 720, fullscreen_mode: FullScreenMode::MaximizedWindow, vsync_enabled: true };
		let json = serialize_app_settings(&s).unwrap();
		let parsed = parse_app_settings(&json).unwrap();
		assert_eq!(parsed, s);
	}

	#[test]
	fn parses_a_hand_written_json_shape_like_the_c_sharp_game_would_write() {
		let json = r#"{
            "ResolutionX": 1920,
            "ResolutionY": 1080,
            "fullscreenMode": 1,
            "VSyncEnabled": true
        }"#;
		let parsed = parse_app_settings(json).unwrap();
		assert_eq!(parsed.resolution_x, 1920);
		assert_eq!(parsed.resolution_y, 1080);
		assert_eq!(parsed.fullscreen_mode, FullScreenMode::FullScreenWindow);
		assert!(parsed.vsync_enabled);
	}

	#[test]
	fn missing_fields_fall_back_to_defaults() {
		let parsed = parse_app_settings("{}").unwrap();
		assert_eq!(parsed, AppSettings::default_settings());
	}

	#[test]
	fn missing_fullscreen_mode_field_defaults_to_fullscreen_window_not_zero() {
		// Regression test: `#[serde(default)]` on an i32 field falls back
		// to `i32::default()` (0), which happens to decode as
		// `ExclusiveFullScreen` rather than the intended
		// `FullScreenWindow` default. Must use an explicit default fn.
		let json = r#"{"ResolutionX": 1920, "ResolutionY": 1080, "VSyncEnabled": true}"#;
		let parsed = parse_app_settings(json).unwrap();
		assert_eq!(parsed.fullscreen_mode, FullScreenMode::FullScreenWindow);
	}
}
