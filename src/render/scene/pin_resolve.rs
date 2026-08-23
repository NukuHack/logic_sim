//! Pin lookup helpers shared by wire endpoint resolution and hit-testing:
//! given a wire address's `(owner_id, pin_id)`, resolve which pin it refers
//! to and where that pin sits right now (a subchip's laid-out pin row, or
//! one of the current chip's own boundary dev-pins at its saved position).

use crate::description::{ChipDescription, Color, PinBitCount};
use crate::render::layout;
use crate::render::scene::placed::PlacedSubChip;
use crate::structs::Vec2;
use std::collections::HashMap;

/// Resolves a wire's colour palette index from its source pin, mirroring
/// the same owner-id resolution `resolve_pin_position` uses: a subchip's
/// output pin (respecting any per-instance `OutputPinColourInfo` override)
/// or one of this chip's own boundary dev-pins. Falls back to palette index
/// 0 if the pin can't be resolved.
pub(crate) fn resolve_pin_colour(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	owner_id: i32,
	pin_id: i32,
) -> Color {
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
			return sub.output_pin_colour(pin.id, pin.colour);
		}
		if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
			return pin.colour;
		}
		return Color::default();
	}

	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return p.colour;
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return p.colour;
	}

	Color::default()
}

/// Resolves a wire's bit count from its source pin, using the same
/// owner-id resolution as `resolve_pin_position`/`resolve_pin_colour`.
/// Falls back to `Bit1` if the pin can't be resolved.
pub(crate) fn resolve_pin_bit_count(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	owner_id: i32,
	pin_id: i32,
) -> PinBitCount {
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		if let Some(pin) = sub.desc.output_pins.iter().find(|p| p.id == pin_id) {
			return pin.bit_count;
		}
		if let Some(pin) = sub.desc.input_pins.iter().find(|p| p.id == pin_id) {
			return pin.bit_count;
		}
		return PinBitCount::Bit1;
	}

	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return p.bit_count;
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return p.bit_count;
	}

	PinBitCount::Bit1
}

pub(crate) fn resolve_pin_position(
	chip: &ChipDescription,
	placed: &[PlacedSubChip],
	owner_to_placed: &HashMap<i32, usize>,
	owner_id: i32,
	pin_id: i32,
	is_input_side: bool,
) -> Option<Vec2> {
	// Case 1: owner refers to a subchip in this scene.
	if let Some(&idx) = owner_to_placed.get(&owner_id) {
		let sub = &placed[idx];
		// Bus origin/terminus chips draw their one visible pin on a fixed
		// default side (bus -> right, terminus -> left) unless flipped via
		// saved `InternalData[1]` ("is flip"); see `PlacedSubChip::internal_data`.
		let is_flipped = sub.desc.chip_type.is_bus_type() && sub.internal_data.get(1).copied().unwrap_or(0) != 0;
		if let Some((i, pin)) = sub.desc.input_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
			let y = sub.input_pin_y.get(i).copied().unwrap_or(0.0);
			let _ = pin;
			return Some(layout::pin_world_position(sub.centre, sub.size, y, true ^ is_flipped));
		}
		if let Some((i, pin)) = sub.desc.output_pins.iter().enumerate().find(|(_, p)| p.id == pin_id) {
			let y = sub.output_pin_y.get(i).copied().unwrap_or(0.0);
			let _ = pin;
			return Some(layout::pin_world_position(sub.centre, sub.size, y, false ^ is_flipped));
		}
		return None;
	}

	// Case 2: owner refers to one of this chip's own boundary dev-pins (owner id == the pin's own
	// global id, single local pin id 0). Unlike a subchip's pins (derived from body + default pin
	// layout), a dev-pin's position is authoritative and saved directly on `PinDescription` -- use as-is.
	let _ = pin_id;
	let _ = is_input_side;
	if let Some(p) = chip.input_pins.iter().find(|p| p.id == owner_id) {
		return Some(p.position);
	}
	if let Some(p) = chip.output_pins.iter().find(|p| p.id == owner_id) {
		return Some(p.position);
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::description::{ChipType, PinDescription};
	use crate::render::scene::test_support::nand_desc;

	/// A chip's own boundary dev-pins (`ChipDescription::input_pins`/
	/// `output_pins`) must resolve to their saved, authoritative
	/// `PinDescription::position` -- not a fabricated stacked-Y placeholder.
	#[test]
	fn resolve_pin_position_uses_dev_pins_saved_position() {
		let mut chip = ChipDescription::new("DEV_PIN_TEST", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 10, PinBitCount::Bit1);
		in0.position = Vec2::new(-3.5, 1.25);
		let mut in1 = PinDescription::new("IN1", 11, PinBitCount::Bit1);
		in1.position = Vec2::new(-3.5, -0.75);
		chip.input_pins.push(in0);
		chip.input_pins.push(in1);

		let mut out0 = PinDescription::new("OUT0", 20, PinBitCount::Bit1);
		out0.position = Vec2::new(5.0, 0.0);
		chip.output_pins.push(out0);

		let placed: Vec<PlacedSubChip> = Vec::new();
		let owner_to_placed: HashMap<i32, usize> = HashMap::new();

		let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 10, 0, true).unwrap();
		assert_eq!(in0_pos, Vec2::new(-3.5, 1.25));

		let in1_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 11, 0, true).unwrap();
		assert_eq!(in1_pos, Vec2::new(-3.5, -0.75));

		let out0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 20, 0, false).unwrap();
		assert_eq!(out0_pos, Vec2::new(5.0, 0.0));
	}

	/// Dev-pins placed at unevenly-spaced, non-grid-multiple positions must
	/// each resolve independently to their own saved position -- guards
	/// against any reintroduction of an index-based stacking placeholder
	/// (which would space pins evenly regardless of where they were
	/// actually placed).
	#[test]
	fn resolve_pin_position_does_not_stack_dev_pins_by_index() {
		let mut chip = ChipDescription::new("DEV_PIN_TEST_2", ChipType::Custom);
		let mut in0 = PinDescription::new("IN0", 1, PinBitCount::Bit1);
		in0.position = Vec2::new(-2.0, 10.0);
		let mut in1 = PinDescription::new("IN1", 2, PinBitCount::Bit1);
		in1.position = Vec2::new(-2.0, 10.5); // deliberately close to in0, not evenly stacked
		chip.input_pins.push(in0);
		chip.input_pins.push(in1);

		let placed: Vec<PlacedSubChip> = Vec::new();
		let owner_to_placed: HashMap<i32, usize> = HashMap::new();

		let pos0 = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 0, true).unwrap();
		let pos1 = resolve_pin_position(&chip, &placed, &owner_to_placed, 2, 0, true).unwrap();

		assert_eq!(pos0, Vec2::new(-2.0, 10.0));
		assert_eq!(pos1, Vec2::new(-2.0, 10.5));
	}

	/// A subchip's pins are still *derived* from the subchip's body + pin
	/// layout via `layout::pin_world_position` (unlike dev-pins, whose
	/// position is authoritative) -- this fix must not have broken that
	/// path.
	#[test]
	fn resolve_pin_position_still_derives_subchip_pin_position() {
		let chip = nand_desc();
		let mut sub_desc = ChipDescription::new("SUBCHIP", ChipType::Nand);
		sub_desc.input_pins.push(PinDescription::new("A", 10, PinBitCount::Bit1));
		sub_desc.input_pins.push(PinDescription::new("B", 11, PinBitCount::Bit1));
		sub_desc.output_pins.push(PinDescription::new("OUT", 20, PinBitCount::Bit1));
		let sub = PlacedSubChip {
			id: 1,
			desc: &sub_desc,
			centre: Vec2::new(2.0, 0.0),
			size: Vec2::new(1.0, 1.0),
			input_pin_y: vec![0.25, -0.25],
			label: None,
			output_pin_y: vec![0.0],
			pin_colour_info: Vec::new(),
			internal_data: Vec::new(),
		};
		let placed = vec![sub];
		let mut owner_to_placed = HashMap::new();
		owner_to_placed.insert(1, 0);

		let expected_out = layout::pin_world_position(placed[0].centre, placed[0].size, 0.0, false);
		let out_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 20, false).unwrap();
		assert_eq!(out_pos, expected_out);

		let expected_in0 = layout::pin_world_position(placed[0].centre, placed[0].size, 0.25, true);
		let in0_pos = resolve_pin_position(&chip, &placed, &owner_to_placed, 1, 10, true).unwrap();
		assert_eq!(in0_pos, expected_in0);
	}
}
