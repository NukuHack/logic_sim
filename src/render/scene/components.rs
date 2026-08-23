//! Layer 3 (top) of the chip scene: component bodies. Every subchip's
//! body rectangle is drawn last so it's never occluded by wires or pins,
//! with chip-type-specific renderers for the visualisation chips (7-
//! segment / RGB / dot / LED displays, key bindings).

use crate::description::{ChipType, Color, NameLocation};
use crate::pin_state::LogicState;
use crate::render::foundation::{point_in_rect, SceneGeometry, TextLabel};
use crate::render::layout;
use crate::render::scene::lookup::PinStateLookup;
use crate::render::scene::placed::PlacedSubChip;
use crate::render::theme::{self, Rgba};
use crate::structs::Vec2;

/// Layer 3 (top): every subchip's body rectangle, drawn last so a
/// component's body is never occluded by a wire or pin drawn earlier.
///
/// The name label is hover-gated: it's only added when `hover_world_pos`
/// lands on this subchip's body rect *and* `pin_already_hovered` is
/// false (a pin sitting on/near the component's edge takes precedence --
/// see `build_scene`'s doc comment), and even then only if the chip's own
/// `NameLocation` isn't `Hidden` (e.g. display/bus/pin chips, whose body
/// is the visualisation, never show a name). Mirrors
/// `DevSceneDrawer.DrawSubChip`'s "if (... desc.NameLocation !=
/// NameDisplayLocation.Hidden)" gate; the `isKeyChip` special case (which
/// shows a keybinding string even when hidden) isn't ported here since it
/// needs live key-binding state this module doesn't have.
pub(crate) fn draw_components(
	geo: &mut SceneGeometry,
	placed: &[PlacedSubChip],
	pin_state: &dyn PinStateLookup,
	hover_world_pos: Option<Vec2>,
	pin_already_hovered: bool,
) {
	for sub in placed {
		// Use this chip's saved body colour (alpha 0 means "not saved" --
		// fall back to the theme default) rather than always drawing every
		// chip with the same flat grey.
		let body_colour = if sub.desc.colour[3] > 0.0 { sub.desc.colour } else { theme::CHIP_BODY_COL };

		// 7-segment/RGB/dot displays draw their own live pixel/segment content in place of the plain
		// body rect (`NameLocation` is `Hidden` because the body is the visualisation). `DisplayLed`
		// needs no branch here: its "display" is just the tinted body rect already produced above.
		match sub.desc.chip_type {
			ChipType::SevenSegmentDisplay => draw_display_seven_segment(geo, sub, pin_state),
			ChipType::DisplayRgb => draw_display_pixel_grid(geo, sub, pin_state, true),
			ChipType::DisplayDot => draw_display_pixel_grid(geo, sub, pin_state, false),
			ChipType::DisplayLed => draw_display_led(geo, sub, pin_state, body_colour),
			ChipType::Key => draw_key_component(geo, sub, body_colour),
			_ => geo.add_rect(sub.centre, sub.size, body_colour),
		}

		let is_hovered = !pin_already_hovered && hover_world_pos.is_some_and(|p| point_in_rect(p, sub.centre, sub.size));
		// draw name if options allow
		if sub.desc.name_location != NameLocation::Hidden {
			let name_pos = match sub.desc.name_location {
				NameLocation::Top => {
					Vec2::new(sub.centre.x, sub.centre.y + sub.size.y / 2.0 - theme::FONT_SIZE_CHIP_NAME / 2.0 - layout::GRID_SIZE / 2.0)
				}
				_ => sub.centre,
			};
			geo.labels.push(TextLabel {
				pos: name_pos,
				text: sub.desc.name.clone(),
				colour: theme::text_colour_for_background(body_colour),
				font_size: theme::FONT_SIZE_CHIP_NAME,
				width: sub.size.x,
			});
		}
		// draw label if hovered
		if let Some(label) = &sub.label {
			if is_hovered {
				let label_pos = sub.centre - Vec2::new(0.0, sub.size.y / 2.0 + theme::FONT_SIZE_CHIP_NAME);
				geo.labels.push(TextLabel {
					pos: label_pos,
					text: label.into(),
					colour: theme::text_colour_for_background(body_colour),
					font_size: theme::FONT_SIZE_CHIP_NAME,
					width: sub.size.x,
				});
			}
		}
	}
}

/// Draws a `SevenSegmentDisplay` subchip's live segment pattern, reading
/// segment states straight from its own input pins (`A`..`G` = pin ids
/// `0`..`6`) via `pin_state`, plus the `COL` pin (id `7`) which swaps in
/// an alternate (blue) palette when high. Mirrors
/// `DevSceneDrawer.DrawDisplay_SevenSegment`/its `ChipType.SevenSegmentDisplay`
/// case, except segments are drawn as plain rectangles rather than the
/// original's pointed-end "diamond" shape -- a cosmetic simplification,
/// not a functional one: on/off state and colour per segment are exact.
fn draw_display_seven_segment(geo: &mut SceneGeometry, sub: &PlacedSubChip, pin_state: &dyn PinStateLookup) {
	const TARGET_HEIGHT_ASPECT: f32 = 1.75;
	const SEGMENT_THICKNESS_FRAC: f32 = 0.165;
	const SEGMENT_VERTICAL_SPACING_FRAC: f32 = 0.07;
	const DISPLAY_INSET_FRAC: f32 = 0.2;

	let centre = sub.centre;
	// The body is sized to fit this display, so derive the display's own "scale" from whichever body
	// dimension is the tighter fit for the fixed 1:1.75 aspect, rather than assuming a saved
	// `DisplayDescription::Scale` (which this port's builtin chip descriptions don't carry).
	let scale = sub.size.x.min(sub.size.y / TARGET_HEIGHT_ASPECT);

	let bounds_width = scale;
	let bounds_height = bounds_width * TARGET_HEIGHT_ASPECT;
	let segment_thickness = scale * SEGMENT_THICKNESS_FRAC;
	let segment_width = bounds_width - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_region_height = bounds_height - segment_thickness - scale * DISPLAY_INSET_FRAC;
	let segment_height = segment_region_height / 2.0 - scale * SEGMENT_VERTICAL_SPACING_FRAC;

	// Black backing behind the segments, same as the original's
	// `Draw.Quad(centre, boundsSize, Color.black)`.
	geo.add_rect(centre, Vec2::new(bounds_width, bounds_height), theme::STATE_DISCONNECTED_COL);

	let col_offset = if pin_state.logic_state(sub.id, 7) == Some(LogicState::High) { 3 } else { 0 };
	let seg_col = |pin_id: i32| {
		let on = pin_state.logic_state(sub.id, pin_id) == Some(LogicState::High);
		theme::SEVEN_SEG_COLS[(if on { 1 } else { 0 }) + col_offset]
	};

	let (a, b, c, d, e, f, g) = (seg_col(0), seg_col(1), seg_col(2), seg_col(3), seg_col(4), seg_col(5), seg_col(6));

	let offset_x = Vec2::new(segment_width / 2.0, 0.0);
	let offset_y = Vec2::new(0.0, segment_region_height / 4.0);
	let vertical_size = Vec2::new(segment_thickness, segment_height);
	let horizontal_size = Vec2::new(segment_width, segment_thickness);

	geo.add_rect(centre, horizontal_size, g); // middle
	geo.add_rect(centre + Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, a); // top
	geo.add_rect(centre - Vec2::new(0.0, segment_region_height / 2.0), horizontal_size, d); // bottom
	geo.add_rect(centre - offset_x + offset_y, vertical_size, f); // top-left
	geo.add_rect(centre - offset_x - offset_y, vertical_size, e); // bottom-left
	geo.add_rect(centre + offset_x + offset_y, vertical_size, b); // top-right
	geo.add_rect(centre + offset_x - offset_y, vertical_size, c); // bottom-right
}

fn draw_key_component(geo: &mut SceneGeometry, sub: &PlacedSubChip, body_colour: [f32; 4]) {
	// Draw this subchip's name label, unless explicitly hidden (e.g. display/bus/pin chips, whose
	// body is the visualisation) -- except the Key chip, which forces its label to show regardless:
	// the bound key's letter (from saved `InternalData[0]`, capitalised ASCII) is its only visualisation.
	let letter = sub.internal_data.first().map(|code| (*code as u8 as char).to_string()).expect("Should have a key");
	geo.labels.push(TextLabel {
		pos: sub.centre,
		text: letter,
		colour: theme::text_colour_for_background(body_colour),
		font_size: theme::FONT_SIZE_CHIP_NAME,
		width: sub.size.x,
	});

	geo.add_rect(sub.centre, sub.size, body_colour);
}

fn draw_display_led(geo: &mut SceneGeometry, sub: &PlacedSubChip, pin_state: &dyn PinStateLookup, body_colour: [f32; 4]) {
	// An LED's body is its indicator: tint it with the saved `InternalData[0]` colour, lit/dimmed/
	// disconnected like a wire of that colour, driven by the live state of its one input pin.
	// Falls back to the ordinary body-colour handling below if this instance has no saved colour.
	let led_colour = sub.internal_data.first().copied().map(|idx| {
		let colour = Color::from_int(idx as i32);
		let logic = sub.desc.input_pins.first().and_then(|p| pin_state.logic_state(sub.id, p.id)).unwrap_or(LogicState::Low);
		theme::state_colour(logic, colour)
	});

	// Use this chip's saved body colour (alpha 0 means "not saved" --
	// fall back to the theme default) rather than always drawing every
	// chip with the same flat grey.
	let color = led_colour.unwrap_or(body_colour);

	geo.add_rect(sub.centre, sub.size, color);
}

/// Draws a `DisplayRgb`/`DisplayDot` subchip's live 16x16 pixel buffer,
/// reading each pixel from `pin_state.internal_state(sub.id)` (the same
/// front-buffer layout `Simulator::process_display_rgb`/
/// `process_display_dot` write: address `y * 16 + x`, packed as
/// `R | G<<4 | B<<8` nibbles for RGB, or a plain 0/1 value for the dot
/// display). Mirrors `DevSceneDrawer.DrawDisplay_RGB`/`DrawDisplay_Dot`.
/// Falls back to a uniform dim grid (no live sim, e.g. in a chip-picker
/// preview) exactly like the original's `useSim == false` branch.
fn draw_display_pixel_grid(geo: &mut SceneGeometry, sub: &PlacedSubChip, pin_state: &dyn PinStateLookup, is_rgb: bool) {
	const PIXELS_PER_ROW: usize = 16;
	const BORDER_FRAC: f32 = 0.95;
	const PIXEL_SIZE_FRAC: f32 = 0.925;
	const OFF_PIXEL_COL: Rgba = [0.1, 0.1, 0.1, 1.0];

	let centre = sub.centre;
	let scale = sub.size.x.min(sub.size.y);

	// Black backing behind the pixel grid.
	geo.add_rect(centre, Vec2::new(scale, scale), theme::STATE_DISCONNECTED_COL);

	let size = scale * BORDER_FRAC;
	let pixel_size = size / PIXELS_PER_ROW as f32;
	let pixel_draw_size = Vec2::new(pixel_size, pixel_size) * PIXEL_SIZE_FRAC;
	let bottom_left = centre - Vec2::new(size, size) * 0.5;

	let internal_state = pin_state.internal_state(sub.id);

	fn unpack_4bit_channel(raw: u32) -> f32 {
		(raw & 0b1111) as f32 / 15.0
	}

	for y in 0..PIXELS_PER_ROW {
		for x in 0..PIXELS_PER_ROW {
			let address = y * PIXELS_PER_ROW + x;
			let col = match internal_state.and_then(|s| s.get(address)) {
				Some(&pixel_state) => {
					if is_rgb {
						[unpack_4bit_channel(pixel_state), unpack_4bit_channel(pixel_state >> 4), unpack_4bit_channel(pixel_state >> 8), 1.0]
					} else {
						let v = (pixel_state != 0) as u32 as f32;
						[v, v, v, 1.0]
					}
				}
				None => OFF_PIXEL_COL,
			};

			let pos = bottom_left + Vec2::new(pixel_size, pixel_size) * 0.5 + Vec2::new(pixel_size * x as f32, pixel_size * y as f32);
			geo.add_rect(pos, pixel_draw_size, col);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::ChipDescription;
	use crate::render::scene::test_support::nand_desc;

	/// Minimal `PinStateLookup` test double for exercising the display
	/// drawing functions directly: reports a fixed logic state per
	/// `(owner_id, pin_id)` (defaulting to `Low` for anything not listed,
	/// same as a genuinely unsimulated pin would read), plus an optional
	/// fixed `internal_state` buffer for the RGB/dot pixel grid.
	struct FixedDisplayState {
		pins: std::collections::HashMap<(i32, i32), LogicState>,
		internal: Option<Vec<u32>>,
	}

	impl PinStateLookup for FixedDisplayState {
		fn is_high(&self, owner_id: i32, pin_id: i32) -> Option<bool> {
			Some(self.logic_state(owner_id, pin_id) == Some(LogicState::High))
		}

		fn logic_state(&self, owner_id: i32, pin_id: i32) -> Option<LogicState> {
			Some(*self.pins.get(&(owner_id, pin_id)).unwrap_or(&LogicState::Low))
		}

		fn internal_state(&self, _owner_id: i32) -> Option<&[u32]> {
			self.internal.as_deref()
		}
	}

	/// A `PlacedSubChip` for exercising `draw_display_seven_segment`/
	/// `draw_display_pixel_grid` directly: `desc` is never read by either
	/// function (they only use `id`/`centre`/`size`), so any placeholder
	/// `ChipDescription` reference is fine.
	fn test_placed_sub_chip(desc: &ChipDescription, id: i32, centre: Vec2, size: Vec2) -> PlacedSubChip<'_> {
		PlacedSubChip {
			id,
			desc,
			centre,
			size,
			input_pin_y: vec![],
			output_pin_y: vec![],
			label: None,
			pin_colour_info: vec![],
			internal_data: vec![],
		}
	}

	/// One rect == 2 triangles == 6 vertices; a helper so the display
	/// tests below can assert vertex counts in terms of "how many rects
	/// got drawn" instead of a raw (and less legible) vertex count.
	fn rects_drawn(geo: &SceneGeometry) -> usize {
		assert_eq!(geo.triangles.len() % 6, 0, "every add_rect call produces exactly 6 vertices");
		geo.triangles.len() / 6
	}

	/// Every vertex colour actually present in `geo` -- lets a test assert
	/// "this colour shows up somewhere" without needing to know which
	/// rect (by draw order) corresponds to which segment/pixel.
	fn colours_present(geo: &SceneGeometry) -> std::collections::HashSet<[u32; 4]> {
		geo.triangles.iter().map(|v| v.colour.map(|c| c.to_bits())).collect()
	}

	/// A blank 7-segment display (every segment pin low, `COL` low) draws
	/// the black backing plus all 7 segments in their "off" colour --
	/// nothing in the "on" or alternate-palette colours should appear.
	#[test]
	fn seven_segment_all_low_draws_seven_off_segments_and_a_backing() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 42, Vec2::ZERO, Vec2::new(1.25, 2.5));
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		// 1 black backing rect + 7 segment rects.
		assert_eq!(rects_drawn(&geo), 8);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "expected the palette-A off colour to appear");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "no segment should be lit");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[3].map(f32::to_bits)), "COL is low, so palette B shouldn't appear at all");
	}

	/// Driving just the `A` segment (pin id 0) high lights only that
	/// segment's rect in the palette-A "on" colour, while every other
	/// segment stays in the palette-A "off" colour -- i.e. each segment's
	/// colour is read from its own pin independently.
	#[test]
	fn seven_segment_segment_a_high_lights_only_segment_a() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 7, Vec2::ZERO, Vec2::new(1.0, 2.0));
		let mut pins = std::collections::HashMap::new();
		pins.insert((7, 0), LogicState::High); // segment A
		let state = FixedDisplayState { pins, internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "segment A should be lit");
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "the other 6 segments should still be off");
	}

	/// The `COL` pin (id 7) going high swaps in the alternate (palette B)
	/// on/off colours entirely -- palette A's colours shouldn't appear at
	/// all once `COL` is high, matching the original's `colOffset` switch.
	#[test]
	fn seven_segment_col_pin_switches_to_alternate_palette() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 3, Vec2::ZERO, Vec2::new(1.0, 2.0));
		let mut pins = std::collections::HashMap::new();
		pins.insert((3, 7), LogicState::High); // COL
		pins.insert((3, 6), LogicState::High); // segment G, lit
		let state = FixedDisplayState { pins, internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_seven_segment(&mut geo, &sub, &state);

		let colours = colours_present(&geo);
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[4].map(f32::to_bits)), "lit segment should use palette B's on colour");
		assert!(colours.contains(&theme::SEVEN_SEG_COLS[3].map(f32::to_bits)), "unlit segments should use palette B's off colour");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[0].map(f32::to_bits)), "palette A's off colour shouldn't appear once COL is high");
		assert!(!colours.contains(&theme::SEVEN_SEG_COLS[1].map(f32::to_bits)), "palette A's on colour shouldn't appear once COL is high");
	}

	/// With no live sim (`internal_state` returns `None`), the RGB/dot
	/// pixel grid falls back to a uniform dim grey for every pixel --
	/// mirroring the original's `useSim == false` branch -- rather than
	/// panicking on a missing buffer or defaulting to some other colour.
	#[test]
	fn pixel_grid_with_no_sim_draws_uniform_off_pixels() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 1, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: None };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, true);

		// 1 black backing rect + 16*16 pixel rects.
		assert_eq!(rects_drawn(&geo), 1 + 16 * 16);

		let colours = colours_present(&geo);
		assert_eq!(colours.len(), 2, "backing (black) + one uniform off-pixel colour, nothing else");
		assert!(colours.contains(&[0.1f32, 0.1, 0.1, 1.0].map(f32::to_bits)));
	}

	/// The RGB display decodes each pixel's packed nibbles (`R | G<<4 |
	/// B<<8`, matching `Simulator::process_display_rgb`'s write path) into
	/// a full-brightness colour when a channel's nibble is maxed (`0xF`),
	/// and address `y*16+x` selects the right pixel out of the buffer.
	#[test]
	fn rgb_pixel_grid_decodes_packed_nibbles_at_the_right_address() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 5, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let mut internal = vec![0u32; 256];
		// Pixel at (x=3, y=2): full red, zero green, zero blue.
		internal[2 * 16 + 3] = 0xF;
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: Some(internal) };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, true);

		let colours = colours_present(&geo);
		assert!(colours.contains(&[1.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "the one written pixel should decode to full red");
		// Every other pixel is still address 0 (untouched, all-zero) => black.
		assert!(colours.contains(&[0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)));
	}

	/// The dot display treats its internal-state value as a plain
	/// on/off flag (any nonzero -> white), not the RGB nibble packing --
	/// same address scheme, different decode.
	#[test]
	fn dot_pixel_grid_treats_nonzero_as_on() {
		let desc = nand_desc();
		let sub = test_placed_sub_chip(&desc, 9, Vec2::ZERO, Vec2::new(2.0, 2.0));
		let mut internal = vec![0u32; 256];
		internal[0] = 1; // pixel (0,0) on
		let state = FixedDisplayState { pins: std::collections::HashMap::new(), internal: Some(internal) };

		let mut geo = SceneGeometry::default();
		draw_display_pixel_grid(&mut geo, &sub, &state, false);

		let colours = colours_present(&geo);
		assert!(colours.contains(&[1.0f32, 1.0, 1.0, 1.0].map(f32::to_bits)), "a nonzero dot pixel should render pure white");
		assert!(colours.contains(&[0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits)), "every other (zero) pixel should render black");
	}
}
