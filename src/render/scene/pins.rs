//! Pin drawing: each subchip's connection shapes (a plain circle for
//! 1-bit pins, a "pill" for wider ones), a chip's own boundary dev-pin
//! bodies, and the clickable per-bit grid on input dev-pins -- layer 2 of
//! the chip scene.

use crate::description::{Color, PinBitCount};
use crate::pin_state::LogicState;
use crate::render::foundation::{RoundCorners, SceneGeometry};
use crate::render::layout;
use crate::render::scene::lookup::PinStateLookup;
use crate::render::scene::pin_hits::{point_in_dev_pin_body, point_in_pin_shape};
use crate::render::scene::placed::PlacedSubChip;
use crate::render::theme::{self, Rgba};
use crate::structs::Vec2;

/// Draws a single subchip pin's connection shape at `pos`, coloured
/// `colour`, scaled by `bit_count`: a plain circle for a 1-bit pin, or a
/// "pill" (a rectangular body with a half-circle cap on each end) for a
/// wider pin -- so a 4/8-bit pin reads as visibly carrying more than a
/// 1-bit pin's single wire, rather than every pin drawing at the same
/// fixed size. See `layout::pin_radius_for_bit_count`/
/// `pin_visual_shape_size` for the exact sizing rule.
///
/// The pill's rounded corners become true semicircle caps (not just
/// quarter-round corners) because `pin_visual_shape_size` always returns a
/// shape whose height already equals twice the intended cap radius, and
/// that radius is what's passed to `add_rounded_rect` below (see
/// `add_rounded_rect`'s own docs on how corner arcs merge into a full
/// semicircle when `radius == height / 2`).
fn draw_pin_shape(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Rgba) {
	match bit_count {
		PinBitCount::Bit1 => {
			geo.add_circle(pos, layout::pin_radius_for_bit_count(bit_count), colour, layout::PIN_SEGMENTS);
		}
		PinBitCount::Bit4 | PinBitCount::Bit8 => {
			let size = layout::pin_visual_shape_size(bit_count);
			let radius = size.y / 2.0;
			geo.add_rounded_rect(pos, size, colour, radius, RoundCorners::BOTH, layout::PIN_SEGMENTS / 4);
		}
	}
}

/// Draws one of a chip's own boundary dev-pins as a small "component"
/// body at `pos` (its real saved position): a partially rounded
/// rectangle, rounded on whichever side faces outward (`round_left` for
/// an input pin, sitting on the chip's left edge with wires approaching
/// from further left; the mirror for an output pin) and square on the
/// side facing in, filled with the pin's live state colour and outlined
/// in a grey-ish border. See `layout::dev_pin_body_size`/
/// `dev_pin_corner_radius` for the sizing this follows.
fn draw_dev_pin_body(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Color, logic: Option<LogicState>, round_left: bool) {
	let size = layout::dev_pin_body_size(bit_count);
	let radius = layout::dev_pin_corner_radius(size);
	let border = layout::DEV_PIN_BORDER_WIDTH.min(size.x / 2.0).min(size.y / 2.0);
	let fill_colour = theme::state_colour(logic.unwrap_or(LogicState::Low), colour);

	// Border first (drawn full-size, in the grey-ish outline colour)...
	geo.add_rounded_rect(
		pos,
		size,
		theme::CHIP_OUTLINE_COL,
		radius,
		RoundCorners { left: round_left, right: !round_left },
		layout::DEV_PIN_SEGMENTS / 4,
	);

	// ...then the pin-coloured fill on top, inset by the border width so
	// the border reads as an outline rather than being fully covered.
	let inner_size = Vec2::new((size.x - border * 2.0).max(0.0), (size.y - border * 2.0).max(0.0));
	let inner_radius = (radius - border).max(0.0);
	geo.add_rounded_rect(
		pos,
		inner_size,
		fill_colour,
		inner_radius,
		RoundCorners { left: round_left, right: !round_left },
		layout::DEV_PIN_SEGMENTS / 4,
	);
}

/// Draws one of a chip's own boundary *input* dev-pins as a grid of
/// individually-clickable bit cells, its drawn
/// footprint scales with how many bits it carries: one circle (twice a
/// plain pin's radius) for a 1-bit input, a 2x2 grid of squares for a
/// 4-bit input, 2x4 for 8-bit. See `layout::input_bit_grid_dims`/
/// `input_bit_cell_offsets` for the exact grid geometry, and
/// `hit_test_input_dev_pin_bit` for the matching per-cell hit test. Each
/// cell is coloured by that individual bit's own live state
/// (`PinStateLookup::bit_logic_state`), so e.g. an 8-bit input shows all
/// eight of its bits' states at a glance, not one averaged colour.
fn draw_input_dev_pin_body(geo: &mut SceneGeometry, pos: Vec2, bit_count: PinBitCount, colour: Color, pin_id: i32, pin_state: &dyn PinStateLookup) {
	for (bit_index, offset) in layout::input_bit_cell_offsets(bit_count).into_iter().enumerate() {
		let cell_pos = pos + offset;
		let logic = pin_state.bit_logic_state(pin_id, 0, bit_index as u32).unwrap_or(LogicState::Low);
		let fill_colour = theme::state_colour(logic, colour);

		match bit_count {
			PinBitCount::Bit1 => {
				geo.add_outlined_circle(
					cell_pos,
					layout::INPUT_BIT_CIRCLE_RADIUS,
					layout::DEV_PIN_BORDER_WIDTH.min(layout::INPUT_BIT_CIRCLE_RADIUS),
					fill_colour,
					theme::CHIP_OUTLINE_COL,
					layout::DEV_PIN_SEGMENTS * 2,
				);
			}
			PinBitCount::Bit4 | PinBitCount::Bit8 => {
				let size = Vec2::new(layout::INPUT_BIT_CELL_SIZE, layout::INPUT_BIT_CELL_SIZE);
				let border = layout::DEV_PIN_BORDER_WIDTH.min(size.x / 2.0).min(size.y / 2.0);
				geo.add_outlined_rect(cell_pos, size, border, fill_colour, theme::CHIP_OUTLINE_COL);
			}
		}
	}
}

/// Layer 2 (middle): every pin -- each subchip's input/output pins
/// (`draw_pin_shape` -- a plain circle for 1-bit, a "pill" for wider
/// pins) plus this chip's own boundary dev-pins (small rounded-rect
/// bodies, drawn via `draw_dev_pin_body`) -- so pins always sit visibly on
/// top of the wires that connect to them, and underneath the component
/// bodies that own them.
///
/// Also hit-tests every pin against `hover_world_pos` (if given) using its
/// *exact* drawn shape (`point_in_pin_shape`/`point_in_rounded_rect`,
/// mirroring `draw_pin_shape`/`draw_dev_pin_body`'s own geometry) and
/// returns the first hit's `(label anchor position, pin name)`, for
/// `build_scene` to turn into a hover label. Pins are hit-tested in the
/// same order they're drawn, so if two overlap the topmost (drawn last)
/// wins, matching what's visibly on top.
pub(crate) fn draw_pins(
	geo: &mut SceneGeometry,
	chip: &crate::description::ChipDescription,
	placed: &[PlacedSubChip],
	pin_state: &dyn PinStateLookup,
	hover_world_pos: Option<Vec2>,
) -> Option<(Vec2, String)> {
	let mut hovered: Option<(Vec2, String)> = None;

	for sub in placed {
		// Bus origin/terminus chips draw their one visible pin on a fixed
		// default side (bus -> right, terminus -> left) unless flipped via
		// saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;

		for (i, pin) in sub.desc.input_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped);
			let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
			draw_pin_shape(geo, pos, pin.bit_count, theme::state_colour(logic, pin.colour));
			if hover_world_pos.is_some_and(|p| point_in_pin_shape(p, pos, pin.bit_count)) {
				hovered = Some((pos, pin.name.clone()));
			}
		}
		for (i, pin) in sub.desc.output_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped);
			let logic = pin_state.logic_state(sub.id, pin.id).unwrap_or(LogicState::Low);
			// A specific placed instance can override its output pin's
			// colour (saved `OutputPinColourInfo`); fall back to the
			// chip-level pin colour when there's no override for this pin.
			let colour_idx = sub.output_pin_colour(pin.id, pin.colour);
			draw_pin_shape(geo, pos, pin.bit_count, theme::state_colour(logic, colour_idx));
			if hover_world_pos.is_some_and(|p| point_in_pin_shape(p, pos, pin.bit_count)) {
				hovered = Some((pos, pin.name.clone()));
			}
		}
	}

	// This chip's own boundary dev-pins, at their real saved position -- a partially rounded rectangle
	// (rounded outward, square where a wire attaches), filled with the pin's live colour and outlined,
	// so they read as visually distinct from a regular subchip pin's plain circle.
	for pin in &chip.input_pins {
		draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), true);
		if hover_world_pos.is_some_and(|p| point_in_dev_pin_body(p, pin.position, pin.bit_count, true)) {
			hovered = Some((pin.position, pin.name.clone()));
		}
		// the clickable part
		draw_input_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin.id, pin_state);
	}
	for pin in &chip.output_pins {
		draw_dev_pin_body(geo, pin.position, pin.bit_count, pin.colour, pin_state.logic_state(pin.id, 0), false);
		if hover_world_pos.is_some_and(|p| point_in_dev_pin_body(p, pin.position, pin.bit_count, false)) {
			hovered = Some((pin.position, pin.name.clone()));
		}
	}

	hovered
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::render::layout::PIN_SEGMENTS;

	#[test]
	fn draw_pin_shape_uses_a_circle_for_1bit_and_a_pill_for_wider_pins() {
		let mut geo_1bit = SceneGeometry::default();
		draw_pin_shape(&mut geo_1bit, Vec2::ZERO, PinBitCount::Bit1, theme::PIN_COL);
		assert_eq!(geo_1bit.triangles.len(), PIN_SEGMENTS as usize * 3);
	}
}
