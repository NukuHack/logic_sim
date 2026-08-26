//! Pure geometry/layout math, ported from the original C#'s `DLS.Graphics.DrawSettings` (world-space
//! constants), `DLS.Game.SubChipHelper` (pin layout / min chip size), and `DLS.Game.GridHelper` (grid
//! snapping). Deliberately kept free of any wgpu/GPU types so it can be unit tested without a graphics
//! device. The renderer proper (`render::gpu`) turns the output of this module into vertex buffers.

use crate::description::{ChipType, NameLocation, PinBitCount};
use crate::structs::Vec2;
use crate::ChipDescription;

// ---- World draw settings (DrawSettings.cs) ----------------------------------

pub const GRID_SIZE: f32 = 0.125;
pub use crate::description::PIN_RADIUS;
pub const PIN_SEGMENTS: u32 = 20;

pub const SUB_CHIP_PIN_INSET: f32 = 0.015;
pub const CHIP_OUTLINE_WIDTH: f32 = 0.05;
pub const WIRE_THICKNESS: f32 = 0.025;
pub const WIRE_HIGHLIGHTED_THICKNESS: f32 = WIRE_THICKNESS + 0.012;

/// World-space thickness of background grid lines. Mirrors `DrawSettings.GridThickness`.
pub const GRID_THICKNESS: f32 = 0.0035;

/// Minimum on-screen size of a chip body that has no pins/name to size it.
pub const MIN_CHIP_SIZE: f32 = GRID_SIZE;

/// Stacks `pins` from the top downward along one edge of a chip and returns
/// (total chip height, per-pin grid-space y offset from the chip's
/// vertical centre). Offsets are centred so they land symmetrically inside
/// a chip body rect that is itself centred on the chip's position and
/// spans `[-height/2, +height/2]` -- without this centring, the pin stack
/// (which is naturally built top-down starting from an arbitrary y=0
/// reference) ends up sitting entirely in the bottom half of the body,
/// leaving the top half empty. Direct port of
/// `SubChipHelper.CalculateDefaultPinLayout`, plus this centring step.
pub fn calculate_default_pin_layout(pins: &[PinBitCount]) -> (f32, Vec<f32>) {
	let mut grid_y: i32 = 0; // top, before centring
	let mut pin_grid_y_vals = Vec::with_capacity(pins.len());

	for &bit_count in pins {
		let pin_h = bit_count.pin_grid_height();
		pin_grid_y_vals.push(grid_y as f32 - pin_h as f32 / 2.0);
		grid_y -= pin_h;
	}

	let total_grid_units = grid_y.unsigned_abs() as f32;
	let shift = total_grid_units / 2.0;
	for y in &mut pin_grid_y_vals {
		*y += shift;
	}

	let height = total_grid_units * GRID_SIZE;
	(height, pin_grid_y_vals)
}

/// Minimum chip height needed to fit both the input and output pin stacks
/// (the taller of the two wins). Mirrors
/// `SubChipHelper.MinChipHeightForPins(inputs, outputs)`.
pub fn min_chip_height_for_pins(inputs: &[PinBitCount], outputs: &[PinBitCount]) -> f32 {
	let h_in = if inputs.is_empty() { 0.0 } else { calculate_default_pin_layout(inputs).0 };
	let h_out = if outputs.is_empty() { 0.0 } else { calculate_default_pin_layout(outputs).0 };
	h_in.max(h_out)
}

/// Minimum footprint of a chip body given only its pins (name-based sizing
/// from `CalculateMinChipSize` is not reproduced here since it depends on
/// text layout / font metrics that belong to the UI framework, not the
/// simulation-facing layout; callers can widen the returned size to fit a
/// label using their own text measurement).
pub fn calculate_min_chip_size_for_pins(inputs: &[PinBitCount], outputs: &[PinBitCount]) -> Vec2 {
	let min_height = min_chip_height_for_pins(inputs, outputs);
	let has_pins = !inputs.is_empty() || !outputs.is_empty();
	let min_width = if has_pins { GRID_SIZE * 2.0 } else { 0.0 };

	Vec2::new(min_width.max(GRID_SIZE), min_height.max(GRID_SIZE))
}

/// Rough average glyph width, as a fraction of font size, for the
/// proportional sans-serif font used for chip names. This module has no
/// access to real font metrics (that lives in the text-rendering layer,
/// `render::gpu`'s glyphon integration) so this is a deliberate estimate,
/// not a measurement -- close enough to size a chip body so its name
/// isn't clipped, mirroring (approximately) what
/// `Draw.CalculateTextBoundsSize` would return in the original.
pub const AVG_CHAR_WIDTH_RATIO: f32 = 0.62;

/// Estimated world-space width needed to draw `text` at `font_size`. See
/// `AVG_CHAR_WIDTH_RATIO` for why this is an estimate rather than an
/// exact font-metrics measurement.
pub fn estimate_text_width(text: &str, font_size: f32) -> f32 {
	text.chars().count() as f32 * font_size * AVG_CHAR_WIDTH_RATIO
}

/// Minimum footprint of a chip body given its pins *and* its name label
/// (when the label is actually drawn on the body, i.e. `name_location !=
/// Hidden`). Mirrors `SubChipHelper.CalculateMinChipSize`'s union of pin
/// bounds and name-text bounds.
///
/// This is the fix for labels effectively not rendering: sizing a body
/// from its pins alone (`calculate_min_chip_size_for_pins`) very often
/// gives a body far narrower than its name -- e.g. a single-input,
/// single-output chip is only `GRID_SIZE * 2` (0.25 world units) wide,
/// nowhere near enough room for a name like "Full Adder". Since
/// `render::scene::build_scene` uses the chip body's width as the text
/// label's wrap/clip width, an under-sized body means the label's text is
/// clipped down to a sliver and is effectively invisible on screen even
/// though the geometry/label data is technically being produced. Callers
/// building subchip placements should use this (not the pins-only
/// variant) so labels always have room to actually draw.
pub fn calculate_min_chip_size(inputs: &[PinBitCount], outputs: &[PinBitCount], desc: &ChipDescription, font_size: f32) -> Vec2 {
	let name = &desc.name;
	// 7-segment/RGB/dot displays draw live pixel content that's illegible at their pin-count-only
	// minimum size, and have no name label to widen them the way an ordinary chip's does -- floor
	// them at a larger, readable footprint instead. See `display_min_size`'s own docs.
	if let Some(min) = display_min_size(desc.chip_type) {
		return desc.size.max(min);
	}

	let pins_size = calculate_min_chip_size_for_pins(inputs, outputs);
	if desc.name_location == NameLocation::Hidden || name.is_empty() {
		return pins_size;
	}
	let name_width = estimate_text_width(name, font_size);
	// Half a character of padding on each side of the name label, so the
	// text doesn't butt against the body edges.
	let padding = font_size * AVG_CHAR_WIDTH_RATIO;
	Vec2::new(pins_size.x.max(name_width + padding), pins_size.y)
}

/// World-space footprint enforced as a floor for `SevenSegmentDisplay`/`DisplayRgb`/`DisplayDot`
/// bodies, which otherwise size down to `calculate_min_chip_size_for_pins`'s pin-count-only result
/// (as small as `GRID_SIZE * 2` wide, since their name is `Hidden` and can't widen them the way an
/// ordinary chip's label does) -- far too small to read the pixel/segment content drawn on them.
/// `None` for chip types that aren't one of these three displays, so callers can widen a placed
/// subchip's already-computed size (component-wise `max` against this) without needing their own
/// type match.
pub fn display_min_size(chip_type: ChipType) -> Option<Vec2> {
	match chip_type {
		ChipType::SevenSegmentDisplay => Some(Vec2::splat(GRID_SIZE) * 10.0),
		ChipType::DisplayRgb => Some(Vec2::splat(GRID_SIZE) * 22.0),
		ChipType::DisplayDot => Some(Vec2::splat(GRID_SIZE) * 14.0),
		_ => None,
	}
}

/// Minimum on-screen thickness (in device pixels) grid lines should keep,
/// regardless of `GRID_THICKNESS` or camera zoom. Below this, a thin flat
/// quad -- this renderer draws lines as plain triangles, with no
/// line-antialiasing pass like the original's `Draw.LineThickAA` -- covers
/// less than a pixel and rasterizes inconsistently from line to line
/// depending on its exact sub-pixel offset: some lines land on a pixel,
/// their neighbours don't. That's what makes a zoomed-out grid look like
/// it's "falling apart" / uneven rather than a uniform mesh, since
/// `GRID_THICKNESS` alone (0.0035 world units) drops well under a pixel
/// as soon as the camera zooms out even moderately.
pub const GRID_MIN_PIXEL_THICKNESS: f32 = 1.5;

/// World-space thickness to actually draw grid lines at for a given
/// `zoom` (screen pixels per world unit): `GRID_THICKNESS`, widened if
/// needed so it never renders thinner than `GRID_MIN_PIXEL_THICKNESS` on
/// screen. See `GRID_MIN_PIXEL_THICKNESS` for why this matters.
pub fn grid_line_thickness(zoom: f32) -> f32 {
	if zoom <= 0.0 {
		return GRID_THICKNESS;
	}
	GRID_THICKNESS.max(GRID_MIN_PIXEL_THICKNESS / zoom)
}

/// Body footprint for a chip's own boundary dev-pin, drawn in the scene as
/// a tiny one-pin "component" (see `render::scene::build_scene`'s dev-pin
/// drawing). Reuses the ordinary pins-only sizing formula with a single
/// pin of `bit_count` on it, so a dev-pin's body -- like any other chip's
/// body -- grows with the bit width it carries (e.g. an 8-bit dev-pin is
/// visibly larger than a 1-bit one), rather than every dev-pin sharing one
/// fixed placeholder size regardless of width.
pub fn dev_pin_body_size(bit_count: PinBitCount) -> Vec2 {
	calculate_min_chip_size_for_pins(&[bit_count], &[])
}

/// Radius of the clickable circle drawn for a 1-bit *input* dev-pin's
/// body: twice the ordinary connection-pin radius (`PIN_RADIUS`), so the
/// thing a player actually has to click to toggle a switch is
/// comfortably bigger than a plain wire-attachment pin, not the same
/// tiny size. Also doubles as the side length of the square cell used
/// for each individual bit of a *wider* input (see
/// `INPUT_BIT_CELL_SIZE`) -- both trace back to this one constant so a
/// 4/8-bit input's per-bit cells read as the same scale as the 1-bit
/// case's circle, not an arbitrary unrelated size.
pub const INPUT_BIT_CIRCLE_RADIUS: f32 = PIN_RADIUS * 2.0;

/// Side length of the square clickable cell drawn for each individual
/// bit of a multi-bit input (4-bit, 8-bit, ...). Equal to
/// `INPUT_BIT_CIRCLE_RADIUS`, per that constant's docs.
pub const INPUT_BIT_CELL_SIZE: f32 = INPUT_BIT_CIRCLE_RADIUS;

/// Grid arrangement (columns, rows) of per-bit clickable cells for an
/// *input* dev-pin's body comes from `PinBitCount::input_bit_grid_dims`.
pub fn input_dev_pin_body_size(bit_count: PinBitCount) -> Vec2 {
	let (cols, rows) = bit_count.input_bit_grid_dims();
	Vec2::new(INPUT_BIT_CELL_SIZE * cols as f32, INPUT_BIT_CELL_SIZE * rows as f32)
}

/// World-space centre offsets
pub fn input_bit_cell_offsets(bit_count: PinBitCount) -> Vec<Vec2> {
	let (cols, rows) = bit_count.input_bit_grid_dims();
	let total = input_dev_pin_body_size(bit_count);
	let mut offsets = Vec::with_capacity((cols * rows) as usize);
	for row in 0..rows {
		for col in 0..cols {
			let x = -total.x / 2.0 + INPUT_BIT_CELL_SIZE * (col as f32 + 0.5);
			let y = total.y / 2.0 - INPUT_BIT_CELL_SIZE * (row as f32 + 0.5);
			// X, y = position, the offset of "PIN_RADIUS * 4" is to make it not instersect with the pin itself
			let offset = Vec2::new(x - PIN_RADIUS * 4.0, y);
			offsets.push(offset);
		}
	}
	offsets
}

/// World-space thickness of the grey-ish border drawn around a dev-pin's
/// body (see `render::scene::build_scene`'s dev-pin drawing). Kept as its
/// own constant, distinct from `CHIP_OUTLINE_WIDTH`, since a dev-pin body
/// is much smaller than an ordinary chip body and a full-width outline
/// would visually swallow it.
pub const DEV_PIN_BORDER_WIDTH: f32 = 0.02;

/// Corner radius used to round a dev-pin body's outward-facing corners
/// (see `SceneGeometry::add_rounded_rect`). Scales with the body's own
/// size (rather than a flat constant) so an 8-bit dev-pin's rounding
/// doesn't look disproportionately small next to its larger body.
pub fn dev_pin_corner_radius(size: Vec2) -> f32 {
	(size.x.min(size.y) * 0.35).max(0.0)
}

/// Arc resolution (points per rounded corner) used when drawing a dev-pin
/// body's rounded corners (`render::scene::draw_dev_pin_body`). A dev-pin
/// body is tiny on screen, so a coarser arc than `add_circle`'s typical
/// 16-24 segments is indistinguishable while costing fewer triangles.
pub const DEV_PIN_SEGMENTS: u32 = (PIN_SEGMENTS as f32 * 1.5) as u32;

/// Absolute world-space position of one pin, given the parent chip's centre
/// position + size and the pin's grid-space y-offset (from
/// `calculate_default_pin_layout`). `is_left_side` picks the left (input) or
/// right (output) edge of the chip body.
pub fn pin_world_position(chip_centre: Vec2, chip_size: Vec2, pin_grid_y: f32, is_left_side: bool) -> Vec2 {
	let half_w = chip_size.x / 2.0;
	let x_offset = if is_left_side { -half_w - SUB_CHIP_PIN_INSET } else { half_w + SUB_CHIP_PIN_INSET };
	Vec2::new(chip_centre.x + x_offset, chip_centre.y + pin_grid_y * GRID_SIZE)
}

// ---- Grid snapping (GridHelper.cs) -----------------------------------------

pub fn snap_to_grid_scalar(v: f32) -> f32 {
	(v / GRID_SIZE).round() * GRID_SIZE
}

pub fn snap_to_grid(v: Vec2) -> Vec2 {
	Vec2::new(snap_to_grid_scalar(v.x), snap_to_grid_scalar(v.y))
}

/// Snaps to grid lines *or* the centre of grid cells (whichever is
/// closer, per axis) -- mirrors `GridHelper.SnapToGrid(v, true, true)`,
/// the variant placement/wire editing actually uses.
pub fn snap_to_grid_centred(v: Vec2) -> Vec2 {
	Vec2::new(snap_to_grid_scalar(v.x * 2.0) / 2.0, snap_to_grid_scalar(v.y * 2.0) / 2.0)
}

/// Constrains `curr` to lie horizontally or vertically of `prev` --
/// whichever axis is already closer -- mirroring
/// `GridHelper.ForceStraightLine` (the "straight wires" pref / shift-hold).
pub fn force_straight_line(prev: Vec2, curr: Vec2) -> Vec2 {
	let mut offset = curr - prev;
	if offset.x.abs() > offset.y.abs() {
		offset.y = 0.0;
	} else {
		offset.x = 0.0;
	}
	prev + offset
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pin_stack_is_symmetric_about_zero_matching_centred_body_rect() {
		// For any pin list, since the chip body rect is drawn centred on (0,0) spanning
		// [-height/2, height/2], the topmost pin's outer edge and the bottommost pin's outer edge
		// should sit exactly on those bounds -- the stack shouldn't be shifted into one half of the body.
		let pins = [PinBitCount::Bit1, PinBitCount::Bit1, PinBitCount::Bit4];
		let (height, ys) = calculate_default_pin_layout(&pins);
		let half = height / GRID_SIZE / 2.0;
		let top_pin_outer_edge = ys[0] + pins[0].pin_grid_height() as f32 / 2.0;
		let bottom_pin_outer_edge = ys[ys.len() - 1] - pins.last().unwrap().pin_grid_height() as f32 / 2.0;
		assert_eq!(top_pin_outer_edge, half);
		assert_eq!(bottom_pin_outer_edge, -half);
	}

	/*
		#[test]
		fn min_chip_size_widens_for_a_name_longer_than_the_pin_bounds() {
			// A single 1-bit input/output pair alone only needs GRID_SIZE*2
			// (0.25 units) of width -- nowhere near enough for "Full Adder".
			let inputs = [PinBitCount::Bit1];
			let outputs = [PinBitCount::Bit1];
			let pins_only = calculate_min_chip_size_for_pins(&inputs, &outputs);
			let with_name = calculate_min_chip_size(&inputs, &outputs, "Full Adder", NameLocation::Centre, 0.25);
			assert!(with_name.x > pins_only.x, "body should widen to fit the name label, not stay pin-sized");
			assert_eq!(with_name.x, estimate_text_width("Full Adder", 0.25));
			// Height is unaffected by the name -- only pins drive it here.
			assert_eq!(with_name.y, pins_only.y);
		}

		#[test]
		fn min_chip_size_ignores_a_short_name_that_fits_within_pin_bounds() {
			let inputs = [PinBitCount::Bit8, PinBitCount::Bit8, PinBitCount::Bit8];
			let outputs = [PinBitCount::Bit8, PinBitCount::Bit8, PinBitCount::Bit8];
			let pins_only = calculate_min_chip_size_for_pins(&inputs, &outputs);
			let with_name = calculate_min_chip_size(&inputs, &outputs, "A", NameLocation::Centre, 0.25);
			assert_eq!(with_name.x, pins_only.x);
		}

		#[test]
		fn min_chip_size_ignores_name_when_hidden() {
			let inputs = [PinBitCount::Bit1];
			let outputs = [];
			let pins_only = calculate_min_chip_size_for_pins(&inputs, &outputs);
			let with_hidden_name = calculate_min_chip_size(&inputs, &outputs, "A Very Long Chip Name Indeed", NameLocation::Hidden, 0.25);
			assert_eq!(with_hidden_name, pins_only);
		}

		#[test]
		fn min_chip_size_ignores_empty_name() {
			let size = calculate_min_chip_size(&[], &[], "", NameLocation::Centre, 0.25);
			assert_eq!(size, calculate_min_chip_size_for_pins(&[], &[]));
		}
	*/

	#[test]
	fn snap_to_grid_centred_lands_on_lines_or_cell_centres() {
		// Centres are half-grid increments (0.0625 here): 0.05 rounds up to the
		// centre at 0.0625, while plain snapping would push it to the line at 0.
		assert_eq!(snap_to_grid_centred(Vec2::new(0.05, -0.06)), Vec2::new(0.0625, -0.0625));
		assert_eq!(snap_to_grid(Vec2::new(0.05, 0.07)), Vec2::new(0.0, 0.125));
	}

	#[test]
	fn force_straight_line_flattens_the_dominant_axis() {
		let prev = Vec2::new(1.0, 1.0);
		assert_eq!(force_straight_line(prev, Vec2::new(3.0, 1.4)), Vec2::new(3.0, 1.0)); // mostly horizontal -> keep x
		assert_eq!(force_straight_line(prev, Vec2::new(1.3, 4.0)), Vec2::new(1.0, 4.0)); // mostly vertical -> keep y
		assert_eq!(force_straight_line(prev, prev), prev); // already straight
	}
}
