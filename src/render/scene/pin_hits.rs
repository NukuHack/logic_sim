//! Pin hit-testing: point-in-shape tests mirroring the drawn pin
//! geometry exactly, the `PinHit` result type wire placement is built on,
//! and the lookups resolving a click to a connectable pin (a subchip's
//! own pin or one of the current chip's boundary dev-pins).

use crate::description::{ChipDescription, PinBitCount};
use crate::render::foundation::{point_in_circle, point_in_rect, point_in_rounded_rect};
use crate::render::layout;
use crate::render::scene::placed::PlacedSubChip;
use crate::structs::Vec2;

/// One real pin (never a wire tap -- see `WireTapHit` for that) hit by
/// `hit_test_sub_chip_pin`/`hit_test_any_pin`: either a placed subchip's
/// own pin or one of the current chip's own boundary dev-pins.
/// `owner_id`/`pin_id` are exactly what `PinAddress::new` needs to
/// reference this pin -- for a boundary dev-pin both are the pin's own
/// id, the same self-owned convention `resolve_pin_colour`/`find_pin`
/// already rely on. `is_input` is this pin's literal kind (matches
/// `hit_test_dev_pin`'s existing convention); `is_boundary` distinguishes
/// a chip's own dev-pin from a subchip's, since the two need opposite
/// treatment when deciding which end of a new wire a pin can be (see
/// `is_wire_source`).
#[derive(Debug, Clone, Copy)]
pub struct PinHit {
	pub owner_id: i32,
	pub pin_id: i32,
	pub is_input: bool,
	pub is_boundary: bool,
	pub position: Vec2,
	pub bit_count: PinBitCount,
}

impl PinHit {
	/// Whether this pin can be a new wire's *source* end. A subchip's
	/// output pin drives a wire, same as one of the owning chip's own
	/// boundary *input* dev-pins does from the inside (it's an input
	/// from outside the chip, but the thing that actually feeds the
	/// internal circuit) -- so `is_boundary` flips which literal kind
	/// counts as the source side. Exactly the opposite of
	/// `is_wire_target`.
	pub fn is_wire_source(&self) -> bool {
		self.is_input == self.is_boundary
	}

	/// Whether this pin can be a new wire's *target* end -- see
	/// `is_wire_source`.
	pub fn is_wire_target(&self) -> bool {
		!self.is_wire_source()
	}
}

/// A point-in-shape test mirroring `draw_pin_shape`'s exact branching: a
/// plain circle for a 1-bit pin, or the same "pill" `add_rounded_rect`
/// call (round on both sides) for a wider pin -- see
/// `point_in_rounded_rect`/`point_in_circle`.
pub(crate) fn point_in_pin_shape(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> bool {
	match bit_count {
		PinBitCount::Bit1 => point_in_circle(point, pos, bit_count.pin_radius()),
		PinBitCount::Bit4 | PinBitCount::Bit8 => {
			let size = bit_count.pin_visual_shape_size();
			point_in_rounded_rect(point, pos, size, size.y / 2.0, true, true)
		}
	}
}

/// A point-in-shape test mirroring `draw_dev_pin_body`'s exact geometry
/// (its outer, full-size border shape -- the fill is strictly smaller, so
/// testing against the border is the more generous/correct hit area).
pub(crate) fn point_in_dev_pin_body(point: Vec2, pos: Vec2, bit_count: PinBitCount, round_left: bool) -> bool {
	let size = layout::dev_pin_body_size(bit_count);
	let radius = layout::dev_pin_corner_radius(size);
	point_in_rounded_rect(point, pos, size, radius, round_left, !round_left)
}

/// A point-in-shape test covering an *input* dev-pin's whole clickable
/// body (the union of every individual bit cell) -- used for the
/// coarse "is the cursor anywhere on this pin" hover check. For
/// per-bit toggle handling (which exact bit a click landed on), use
/// `hit_test_input_dev_pin_bit` instead.
#[allow(dead_code)]
fn point_in_input_dev_pin_body(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> bool {
	let size = layout::input_dev_pin_body_size(bit_count);
	point_in_rect(point, pos, size)
}

/// Returns the bit index (0-based) of whichever of an input dev-pin's
/// individual clickable cells `point` landed on, or `None` if it missed
/// every cell. `pos` is the dev-pin's own saved position, the same value
/// passed to `draw_input_dev_pin_body`. Bit-0's cell is the top-left of
/// the grid (see `layout::input_bit_cell_offsets`), matching the same
/// bit-index convention `PinState::bit` uses --
/// callers wiring up an actual click-to-toggle handler can flip bit
/// `bit_index` of the pin's state directly.
pub fn hit_test_input_dev_pin_bit(point: Vec2, pos: Vec2, bit_count: PinBitCount) -> Option<u32> {
	let cell_size = Vec2::new(layout::INPUT_BIT_CELL_SIZE, layout::INPUT_BIT_CELL_SIZE);
	for (bit_index, offset) in layout::input_bit_cell_offsets(bit_count).into_iter().enumerate() {
		let cell_pos = pos + offset;
		let hit = match bit_count {
			PinBitCount::Bit1 => point_in_circle(point, cell_pos, layout::INPUT_BIT_CIRCLE_RADIUS),
			PinBitCount::Bit4 | PinBitCount::Bit8 => point_in_rect(point, cell_pos, cell_size),
		};
		if hit {
			return Some(bit_index as u32);
		}
	}
	None
}

/// Finds whichever of `chip`'s *own* boundary dev-pins (never a
/// subchip's pins) has its body under `world_pos`, if any -- used to
/// resolve a right-click to "Label this pin" (see `PlacedSubChip`'s and
/// `point_in_dev_pin_body`'s docs for the input/output `round_left`
/// distinction this mirrors). Returns `(is_input, pin_id)`.
pub fn hit_test_dev_pin(chip: &ChipDescription, world_pos: Vec2) -> Option<(bool, i32)> {
	for pin in &chip.input_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, true) {
			return Some((true, pin.id));
		}
	}
	for pin in &chip.output_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, false) {
			return Some((false, pin.id));
		}
	}
	None
}

/// Finds whichever *subchip's own* pin (never one of `chip`'s boundary
/// dev-pins -- see `hit_test_any_pin` for that) has its exact drawn
/// shape (`point_in_pin_shape`, matching `draw_pins`) under `world_pos`,
/// if any. Iterates subchips back-to-front (last-placed first), the
/// same draw-order precedence `hit_test_sub_chip` uses.
pub fn hit_test_sub_chip_pin(placed: &[PlacedSubChip], world_pos: Vec2) -> Option<PinHit> {
	for sub in placed.iter().rev() {
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;

		for (i, pin) in sub.desc.input_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped);
			if point_in_pin_shape(world_pos, pos, pin.bit_count) {
				return Some(PinHit {
					owner_id: sub.id,
					pin_id: pin.id,
					is_input: true,
					is_boundary: false,
					position: pos,
					bit_count: pin.bit_count,
				});
			}
		}
		for (i, pin) in sub.desc.output_pins.iter().filter(|p| !p.name.contains("(Hidden)")).enumerate() {
			let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
			let pos = layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped);
			if point_in_pin_shape(world_pos, pos, pin.bit_count) {
				return Some(PinHit {
					owner_id: sub.id,
					pin_id: pin.id,
					is_input: false,
					is_boundary: false,
					position: pos,
					bit_count: pin.bit_count,
				});
			}
		}
	}
	None
}

/// Finds whichever pin -- a subchip's own pin or one of `chip`'s own
/// boundary dev-pins -- has its exact drawn shape under `world_pos`, if
/// any. Used to resolve a wire-placement click to a connectable
/// endpoint; subchip pins are tried first since `draw_pins` draws them
/// first, so a dev-pin overlapping one (unlikely in practice) still
/// loses to whichever is actually on top.
pub fn hit_test_any_pin(chip: &ChipDescription, placed: &[PlacedSubChip], world_pos: Vec2) -> Option<PinHit> {
	if let Some(hit) = hit_test_sub_chip_pin(placed, world_pos) {
		return Some(hit);
	}
	for pin in &chip.input_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, true) {
			return Some(PinHit { owner_id: pin.id, pin_id: 0, is_input: true, is_boundary: true, position: pin.position, bit_count: pin.bit_count });
		}
	}
	for pin in &chip.output_pins {
		if point_in_dev_pin_body(world_pos, pin.position, pin.bit_count, false) {
			return Some(PinHit {
				owner_id: pin.id,
				pin_id: 0,
				is_input: false,
				is_boundary: true,
				position: pin.position,
				bit_count: pin.bit_count,
			});
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	/// `point_in_pin_shape` for `Bit8` must use `Bit8`'s own (wider) pill
	/// size, not silently reuse `Bit4`'s -- a point past `Bit4`'s pill
	/// width but still within `Bit8`'s must hit for `Bit8` and miss for
	/// `Bit4`.
	#[test]
	fn point_in_pin_shape_bit8_uses_its_own_wider_size_not_bit4s() {
		let pos = Vec2::ZERO;
		let size4 = PinBitCount::Bit4.pin_visual_shape_size();
		let size8 = PinBitCount::Bit8.pin_visual_shape_size();
		assert!(size8.x > size4.x, "sanity: 8-bit pill must be wider than 4-bit's");

		let point = Vec2::new((size4.x + size8.x) / 4.0, 0.0); // strictly between the two half-widths
		assert!(!point_in_pin_shape(point, pos, PinBitCount::Bit4), "should miss the narrower 4-bit pill");
		assert!(point_in_pin_shape(point, pos, PinBitCount::Bit8), "should hit the wider 8-bit pill");
	}

	/// `point_in_dev_pin_body`'s `round_left` flag must actually flip
	/// which side is rounded (input dev-pins round left/outward, output
	/// dev-pins round right/outward -- see `draw_dev_pin_body`'s docs), not
	/// just be accepted and ignored. Pick a corner point that's a
	/// rounded-corner miss on one side but a square-corner hit on the
	/// other, and check both orientations disagree on it as expected.
	#[test]
	fn point_in_dev_pin_body_round_left_flag_actually_flips_the_rounded_side() {
		let pos = Vec2::ZERO;
		let bit_count = PinBitCount::Bit1;
		let size = layout::dev_pin_body_size(bit_count);
		let radius = layout::dev_pin_corner_radius(size);
		// Just past the (rounded) corner's arc, on the right side, still
		// inside the plain bounding box.
		let right_corner = Vec2::new(size.x / 2.0 - 1e-4, size.y / 2.0 - 1e-4);

		// Sanity: at this size/radius, the exact corner point is actually
		// excluded by a rounded corner but included by a square one.
		assert!(!point_in_rounded_rect(right_corner, pos, size, radius, false, true));
		assert!(point_in_rounded_rect(right_corner, pos, size, radius, false, false));

		// round_left = false -> input-style, but here we're checking the
		// *output* convention (round_right, i.e. round_left = false):
		// right side rounded -> this corner should miss.
		assert!(!point_in_dev_pin_body(right_corner, pos, bit_count, false));
		// round_left = true -> input-style: right side is the square one
		// -> this same corner point should hit.
		assert!(point_in_dev_pin_body(right_corner, pos, bit_count, true));
	}

	/// `point_in_dev_pin_body` must scale with `bit_count` the same way
	/// `draw_dev_pin_body` draws it (`layout::dev_pin_body_size`) -- an
	/// 8-bit dev-pin's body is taller than a 1-bit one's, so a point at
	/// 1-bit's edge but still within 8-bit's must hit only for 8-bit.
	#[test]
	fn point_in_dev_pin_body_scales_with_bit_count() {
		let pos = Vec2::ZERO;
		let size1 = layout::dev_pin_body_size(PinBitCount::Bit1);
		let size8 = layout::dev_pin_body_size(PinBitCount::Bit8);
		assert!(size8.y > size1.y, "sanity: 8-bit dev-pin body must be taller than 1-bit's");

		let point = Vec2::new(0.0, (size1.y + size8.y) / 4.0); // strictly between the two half-heights
		assert!(!point_in_dev_pin_body(point, pos, PinBitCount::Bit1, true));
		assert!(point_in_dev_pin_body(point, pos, PinBitCount::Bit8, true));
	}
}
