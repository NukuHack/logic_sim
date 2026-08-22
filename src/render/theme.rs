//! Colour palette, ported from `DLS.Graphics.DrawSettings.CreateTheme()`.
//! Kept as plain `[f32; 4]` RGBA (0..1) so it can feed straight into a wgpu
//! vertex colour attribute without any GPU-specific types.

use crate::{description::Color, pin_state::LogicState};

pub type Rgba = [f32; 4];

const fn rgb(r: f32, g: f32, b: f32) -> Rgba {
	[r, g, b, 1.0]
}
#[allow(dead_code)] // kept for reference / future support
const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
	[r, g, b, a]
}

/// 8-entry "lit" (logic-high) state colour palette, index chosen
/// per-pin/-wire by the project (e.g. for colour-coding buses). Index 7 is
/// the "white" fallback. The low/off variant of each of these is no longer
/// a separate hand-tuned table -- it's derived from this one via `dim`, so
/// there's a single source of truth per palette index.
pub const COLORS: [Rgba; 8] = [
	rgb(0.95, 0.3, 0.31),  // Red
	rgb(0.92, 0.44, 0.12), // Orange
	rgb(0.98, 0.76, 0.26), // Yellow
	rgb(0.25, 0.66, 0.31), // Green
	rgb(0.2, 0.5, 1.0),    // Blue
	rgb(0.6, 0.4, 0.98),   // Purple
	rgb(0.84, 0.33, 0.9),  // Pink
	rgb(0.9, 0.9, 0.9),    // White
];

/// Colour for a pin/wire carrying a multi-bit value where individual state
/// bits disagree, or for an explicitly disconnected pin
/// (`pin_state::LOGIC_DISCONNECTED`). Always flat black, regardless of the
/// pin/wire's palette index.
pub const STATE_DISCONNECTED_COL: Rgba = [0.0, 0.0, 0.0, 1.0];

/// How much of a lit colour's brightness survives in its "off" (logic-low)
/// variant. Applied uniformly in `dim` rather than hand-tuning a second
/// 8-entry table -- a reasonably darker variant of whatever the pin/wire's
/// "on" colour is, per palette index.
const LOW_STATE_BRIGHTNESS: f32 = 0.3;

/// Darkens `c` down to its "off" brightness, preserving hue/alpha. Used to
/// derive a pin/wire's logic-low colour from its logic-high colour instead
/// of maintaining a separate low-colour lookup table.
pub fn dim(c: Rgba) -> Rgba {
	[c[0] * LOW_STATE_BRIGHTNESS, c[1] * LOW_STATE_BRIGHTNESS, c[2] * LOW_STATE_BRIGHTNESS, c[3]]
}

pub const PIN_COL: Rgba = [0.0, 0.0, 0.0, 1.0];
pub const PIN_HIGHLIGHT_COL: Rgba = [1.0, 1.0, 1.0, 1.0];
pub const PIN_INVALID_COL: Rgba = rgb(0.15, 0.15, 0.15);

pub const BACKGROUND_COL: Rgba = rgb(66.0 / 255.0, 66.0 / 255.0, 69.0 / 255.0);
pub const GRID_COL: Rgba = rgb(49.0 / 255.0, 49.0 / 255.0, 51.0 / 255.0);

pub const CHIP_BODY_COL: Rgba = rgb(0.55, 0.55, 0.58);
pub const CHIP_OUTLINE_COL: Rgba = rgb(0.15, 0.15, 0.16);

/// Colours for a 7-segment display's segments: `[Off, On, Highlight]` for
/// palette A (the `COL` pin low), followed by the same 3 for palette B
/// (`COL` pin high). Mirrors `ThemeDLS.SevenSegCols` exactly.
pub const SEVEN_SEG_COLS: [Rgba; 6] =
	[rgb(0.1, 0.09, 0.09), rgb(1.0, 0.32, 0.28), rgb(0.19, 0.15, 0.15), rgb(0.09, 0.09, 0.1), rgb(0.0, 0.61, 1.0), rgb(0.15, 0.15, 0.19)];

/// World-space font size (grid units) used for a subchip's name label.
/// Mirrors `DrawSettings.FontSizeChipName`.
pub const FONT_SIZE_CHIP_NAME: f32 = 0.2;

/// Text colour used for a hover-triggered name label (a pin's or a
/// component's), drawn directly over the background/grid rather than over
/// a component body -- so, unlike `text_colour_for_background`, it isn't
/// picked per-background; it just needs to read clearly against the dark
/// `BACKGROUND_COL`/`GRID_COL`.
pub const HOVER_LABEL_COL: Rgba = rgb(0.95, 0.95, 0.95);

/// Perceptual (Rec. 709) luminance of an RGBA colour, ignoring alpha.
/// Mirrors `ColHelper.Luminance`.
pub fn luminance(c: Rgba) -> f32 {
	0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Black or white text colour that reads legibly against `bg`, mirroring
/// `ColHelper.ShouldUseBlackText` (threshold 0.57). The original also blends
/// in a desaturated/lightened variant of the body colour for low-saturation
/// backgrounds (`DevSceneDrawer.DrawSubChip`'s `nameTextCol` lerp) -- that
/// nuance is skipped here in favour of a plain black/white pick, which stays
/// legible on every body colour.
pub fn text_colour_for_background(bg: Rgba) -> Rgba {
	if luminance(bg) > 0.57 {
		[0.0, 0.0, 0.0, 1.0]
	} else {
		[1.0, 1.0, 1.0, 1.0]
	}
}

/// Colour for one of the 8 state-palette indices in a given logic state,
/// clamped like `DrawSettings.GetStateColour` (no hover state here --
/// that's an editor interaction concern, not a first-pass rendering
/// concern). A pin/wire that's `LogicState::Disconnected` always renders
/// flat black regardless of `palette_index`, matching
/// `LOGIC_DISCONNECTED`/`DrawSettings.StateDisconnectedCol`; `Low` is a
/// darker variant of the same palette index's `High` colour (via `dim`),
/// not a separately hand-tuned colour.
pub fn state_colour(state: LogicState, color: Color) -> Rgba {
	if state == LogicState::Disconnected {
		return STATE_DISCONNECTED_COL;
	}
	let high = color.to_rgba();
	if state == LogicState::High {
		high
	} else {
		dim(high)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn state_colour_picks_high_or_dimmed_low() {
		assert_eq!(state_colour(LogicState::High, Color::default()), COLORS[0]);
		assert_eq!(state_colour(LogicState::Low, Color::default()), dim(COLORS[0]));
	}

	#[test]
	fn state_colour_disconnected_is_always_black_regardless_of_index() {
		assert_eq!(state_colour(LogicState::Disconnected, Color::from_int(0)), STATE_DISCONNECTED_COL);
		assert_eq!(state_colour(LogicState::Disconnected, Color::from_int(3)), STATE_DISCONNECTED_COL);
	}

	#[test]
	fn state_colour_clamps_out_of_range_index() {
		assert_eq!(state_colour(LogicState::High, Color::White), COLORS[7]);
	}

	#[test]
	fn dim_darkens_but_preserves_hue_ratio_and_alpha() {
		let c = [0.8, 0.4, 0.2, 1.0];
		let d = dim(c);
		assert!(d[0] < c[0] && d[1] < c[1] && d[2] < c[2]);
		assert_eq!(d[3], c[3]);
		// Hue ratio preserved (uniform scale factor across channels).
		assert!((d[0] / c[0] - d[1] / c[1]).abs() < 1e-6);
	}

	#[test]
	fn text_colour_is_black_on_light_background() {
		assert_eq!(text_colour_for_background([1.0, 1.0, 1.0, 1.0]), [0.0, 0.0, 0.0, 1.0]);
	}

	#[test]
	fn text_colour_is_white_on_dark_background() {
		assert_eq!(text_colour_for_background([0.05, 0.05, 0.05, 1.0]), [1.0, 1.0, 1.0, 1.0]);
	}
}
