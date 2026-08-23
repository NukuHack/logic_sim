//! Resolved world-space placement of a chip's subchip instances: the body
//! rectangles and pin rows every scene layer (wires, pins, components) and
//! the interaction hit-tests share.

use crate::description::{ChipDescription, ChipLibrary, Color, PinBitCount};
use crate::render::layout;
use crate::render::theme;
use crate::structs::Vec2;

/// Resolved placement of one subchip instance within the scene, in world
/// space.
#[derive(Debug, Clone)]
pub struct PlacedSubChip<'a> {
	pub id: i32,
	pub desc: &'a ChipDescription,
	pub centre: Vec2,
	pub size: Vec2,
	pub input_pin_y: Vec<f32>,
	pub output_pin_y: Vec<f32>,
	/// Label
	pub label: Option<String>,
	/// Per-instance output pin colour overrides, copied from this placed
	/// instance's `SubChipDescription::pin_colour_info`.
	pub pin_colour_info: Vec<(i32, Color)>,
	/// Copied verbatim from this placed instance's
	/// `SubChipDescription::internal_data` (empty if the subchip has none).
	/// Interpretation is chip-type specific:
	///  - `Key`: `[0]` is the ASCII code (capitalised, e.g. `A` = 65) of the
	///    key this instance listens to.
	///  - `Rom256x16`: all 256 words of ROM contents, indexed by address.
	///  - `DisplayLed`: `[0]` is a `Color` palette index (same encoding as
	///    a pin's `Colour` field), used to tint the LED body.
	///  - Bus origin/terminus (`Bus1Bit`/`Bus4Bit`/`Bus8Bit`/
	///    `BusTerminus1Bit`/`BusTerminus4Bit`/`BusTerminus8Bit`): `[0]` is
	///    the id of the paired bus chip at the other end of the link,
	///    `[1]` is "is flipped" (`1` = draw this instance's visible pin on
	///    the opposite side from its type default).
	pub internal_data: Vec<u32>,
}

impl<'a> PlacedSubChip<'a> {
	/// Effective palette index for this instance's output pin `pin_id`,
	/// falling back to `default_colour` (the chip-level pin colour) if this
	/// instance has no override for it.
	pub fn output_pin_colour(&self, pin_id: i32, default_colour: Color) -> Color {
		self.pin_colour_info.iter().find(|(id, _)| *id == pin_id).map(|(_, colour)| *colour).unwrap_or(default_colour)
	}
}

/// Computes the world-space placement (body rect + pin y-offsets) of every
/// subchip in `chip`, resolving each subchip's own pin layout against
/// `library`. Subchips referencing an unknown chip name are skipped.
pub fn place_sub_chips<'a>(chip: &ChipDescription, library: &'a ChipLibrary) -> Vec<PlacedSubChip<'a>> {
	let mut placed = Vec::with_capacity(chip.sub_chips.len());

	for sub in &chip.sub_chips {
		let Some(desc) = library.try_get(&sub.name) else { continue };

		let input_bits: Vec<PinBitCount> = desc.input_pins.iter().map(|p| p.bit_count).collect();
		let output_bits: Vec<PinBitCount> = desc.output_pins.iter().map(|p| p.bit_count).collect();

		// Prefer the size actually saved on disk (`ChipDescription::size`) -- computed by the original
		// via `CalculateMinChipSize` with real font metrics, more accurate than anything derivable here.
		// Fall back to the pins+name-estimate heuristic only when nothing is saved (size == (0,0)).
		let size = if desc.size != Vec2::ZERO {
			Vec2::new(desc.size.x, desc.size.y)
		} else {
			layout::calculate_min_chip_size(&input_bits, &output_bits, desc, theme::FONT_SIZE_CHIP_NAME)
		};
		let (_, input_pin_y) = layout::calculate_default_pin_layout(&input_bits);
		let (_, output_pin_y) = layout::calculate_default_pin_layout(&output_bits);

		placed.push(PlacedSubChip {
			id: sub.id,
			desc,
			centre: sub.position,
			size,
			label: sub.label.clone(),
			input_pin_y,
			output_pin_y,
			pin_colour_info: sub.pin_colour_info.clone(),
			internal_data: sub.internal_data.clone().unwrap_or_default(),
		});
	}

	placed
}

#[cfg(test)]
mod tests {}
