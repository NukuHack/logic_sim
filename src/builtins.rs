//! Builtin chip descriptions -- the pin layouts for every non-custom chip
//! type (NAND, Clock, RAM, displays, bus, merge/split, I/O pins, ...).
//!
//! Ported from DLS.Game.BuiltinChipCreator. Unlike the original, this drops
//! most things related to editor rendering/layout (colour, grid snapping,
//! name display location) since the simulation core has no use for it --
//! only pin names, IDs, and bit-widths matter for building and running the
//! simulation graph. The one exception is `size`: the display chips
//! (7-segment/RGB/dot/LED) need their body sized to fit their fixed-aspect
//! visualisation (see `render::scene`'s `draw_display_*` functions), so
//! their sizes are computed here exactly as `BuiltinChipCreator` does, via
//! the same `layout::min_chip_height_for_pins` this port's renderer already
//! uses for custom chips. Every other builtin chip leaves `size` as its
//! default `Vec2::default()` (zero), which `render::scene::place_sub_chips`
//! treats as "not saved" and falls back to computing a size from pins alone.

use crate::description::{ChipDescription, ChipLibrary, ChipType, NameLocation, PinBitCount, PinDescription};
use crate::render::layout;
use crate::structs::Vec2;

/// Build every builtin chip description and register them all in `library`.
/// Mirrors `BuiltinChipCreator.CreateAllBuiltinChipDescriptions`, followed by
/// adding each to the library (as `ChipLibrary::new(customChips, builtinChips)`
/// does in the original `Game/Project/ChipLibrary.cs`).
pub fn register_all(library: &mut ChipLibrary) {
	for desc in create_all() {
		library.add(desc);
	}
}

pub fn create_all() -> Vec<ChipDescription> {
	let mut chips = vec![
		// ---- I/O Pins ----
		create_input_or_output_pin(ChipType::In1Bit),
		create_input_or_output_pin(ChipType::Out1Bit),
		create_input_or_output_pin(ChipType::In4Bit),
		create_input_or_output_pin(ChipType::Out4Bit),
		create_input_or_output_pin(ChipType::In8Bit),
		create_input_or_output_pin(ChipType::Out8Bit),
		create_input_key_chip(),
		create_key_mods_chip(),
		// ---- Basic Chips ----
		create_nand(),
		create_tristate_buffer(),
		create_clock(),
		create_pulse(),
		// ---- Memory ----
		dev_create_ram_8(),
		create_rom_8(),
		// ---- Merge / Split ----
		create_bit_conversion_chip(ChipType::Split4To1Bit, PinBitCount::Bit4, PinBitCount::Bit1, 1, 4),
		create_bit_conversion_chip(ChipType::Split8To4Bit, PinBitCount::Bit8, PinBitCount::Bit4, 1, 2),
		create_bit_conversion_chip(ChipType::Split8To1Bit, PinBitCount::Bit8, PinBitCount::Bit1, 1, 8),
		create_bit_conversion_chip(ChipType::Merge1To8Bit, PinBitCount::Bit1, PinBitCount::Bit8, 8, 1),
		create_bit_conversion_chip(ChipType::Merge1To4Bit, PinBitCount::Bit1, PinBitCount::Bit4, 4, 1),
		create_bit_conversion_chip(ChipType::Merge4To8Bit, PinBitCount::Bit4, PinBitCount::Bit8, 2, 1),
		// ---- Displays ----
		create_display_7seg(),
		create_display_rgb(),
		create_display_dot(),
		create_display_led(),
	];

	// ---- Bus ----
	for bit_count in [PinBitCount::Bit1, PinBitCount::Bit4, PinBitCount::Bit8] {
		chips.push(create_bus(bit_count));
		chips.push(create_bus_terminus(bit_count));
	}

	// ---- Audio ----
	chips.push(create_buzzer());

	validate_all_pin_ids(&chips);
	chips
}

fn pin(name: &str, id: i32, bit_count: PinBitCount) -> PinDescription {
	PinDescription::new(name, id, bit_count)
}

fn pin1(name: &str, id: i32) -> PinDescription {
	pin(name, id, PinBitCount::Bit1)
}

fn builtin(chip_type: ChipType, inputs: Vec<PinDescription>, outputs: Vec<PinDescription>) -> ChipDescription {
	let mut desc = ChipDescription::new(name_for(chip_type), chip_type);
	desc.input_pins = inputs;
	desc.output_pins = outputs;
	desc
}

/// Like `builtin`, but with `NameLocation = Hidden`. Mirrors
/// `BuiltinChipCreator.CreateBuiltinChipDescription`'s `nameLoc` argument
/// for chip types whose body *is* the visualisation (displays, buses, the
/// key chip) -- drawing "SEVEN SEGMENT DISPLAY" etc. across the chip body
/// would just be visual noise on top of the thing it's meant to show.
fn builtin_hidden_name(chip_type: ChipType, inputs: Vec<PinDescription>, outputs: Vec<PinDescription>) -> ChipDescription {
	let mut desc = builtin(chip_type, inputs, outputs);
	desc.name_location = NameLocation::Hidden;
	desc
}

fn create_nand() -> ChipDescription {
	builtin(ChipType::Nand, vec![pin1("IN B", 0), pin1("IN A", 1)], vec![pin1("OUT", 2)])
}

fn create_buzzer() -> ChipDescription {
	builtin(ChipType::Buzzer, vec![pin("PITCH", 1, PinBitCount::Bit8), pin("VOLUME", 0, PinBitCount::Bit4)], vec![])
}

fn dev_create_ram_8() -> ChipDescription {
	builtin(
		ChipType::DevRam8Bit,
		vec![pin("ADDRESS", 0, PinBitCount::Bit8), pin("DATA", 1, PinBitCount::Bit8), pin1("WRITE", 2), pin1("RESET", 3), pin1("CLOCK", 4)],
		vec![pin("OUT", 5, PinBitCount::Bit8)],
	)
}

fn create_rom_8() -> ChipDescription {
	builtin(
		ChipType::Rom256x16,
		vec![pin("ADDRESS", 0, PinBitCount::Bit8)],
		vec![pin("OUT B", 1, PinBitCount::Bit8), pin("OUT A", 2, PinBitCount::Bit8)],
	)
}

fn create_input_key_chip() -> ChipDescription {
	builtin_hidden_name(ChipType::Key, vec![], vec![pin1("OUT", 0)])
}

/// Outputs the host's current keyboard modifier keys (shift/ctrl/alt/super)
/// as a bitmask on a single 8-bit pin -- see `sim::key_mods_bits` for which
/// bit is which. No per-instance configuration (unlike `Key`), so the name
/// stays visible on the chip body.
fn create_key_mods_chip() -> ChipDescription {
	builtin(ChipType::KeyMods, vec![], vec![pin("OUT", 0, PinBitCount::Bit8)])
}

fn create_tristate_buffer() -> ChipDescription {
	builtin(ChipType::TriStateBuffer, vec![pin1("IN", 0), pin1("ENABLE", 1)], vec![pin1("OUT", 2)])
}

fn create_clock() -> ChipDescription {
	builtin(ChipType::Clock, vec![], vec![pin1("CLK", 0)])
}

fn create_pulse() -> ChipDescription {
	builtin(ChipType::Pulse, vec![pin1("IN", 0)], vec![pin1("PULSE", 1)])
}

fn create_bit_conversion_chip(
	chip_type: ChipType,
	bit_count_in: PinBitCount,
	bit_count_out: PinBitCount,
	num_in: i32,
	num_out: i32,
) -> ChipDescription {
	let inputs = (0..num_in).map(|i| pin(&get_pin_name(i, num_in, true), i, bit_count_in)).collect();
	let outputs = (0..num_out).map(|i| pin(&get_pin_name(i, num_out, false), num_in + i, bit_count_out)).collect();

	builtin(chip_type, inputs, outputs)
}

fn get_pin_name(pin_index: i32, pin_count: i32, is_input: bool) -> String {
	let mut letter = format!(" {}", (b'A' + (pin_count - pin_index - 1) as u8) as char);
	if pin_count == 1 {
		letter = String::new();
	}
	format!("{}{}", if is_input { "IN" } else { "OUT" }, letter)
}

fn create_display_7seg() -> ChipDescription {
	let inputs = vec![pin1("A", 0), pin1("B", 1), pin1("C", 2), pin1("D", 3), pin1("E", 4), pin1("F", 5), pin1("G", 6), pin1("COL", 7)];
	// Mirrors `BuiltinChipCreator.CreateDisplay7Seg`: height fits the 8
	// 1-bit input pins, width is a fixed 10 grid units.
	let height = layout::min_chip_height_for_pins(&input_bit_counts(&inputs), &[]);
	let mut desc = builtin_hidden_name(ChipType::SevenSegmentDisplay, inputs, vec![]);
	desc.size = Vec2::new(layout::GRID_SIZE * 10.0, height);
	desc
}

fn create_display_rgb() -> ChipDescription {
	let inputs = vec![
		pin("ADDRESS", 0, PinBitCount::Bit8),
		pin("RED", 1, PinBitCount::Bit4),
		pin("GREEN", 2, PinBitCount::Bit4),
		pin("BLUE", 3, PinBitCount::Bit4),
		pin1("RESET", 4),
		pin1("WRITE", 5),
		pin1("REFRESH", 6),
		pin1("CLOCK", 7),
	];
	let outputs = vec![pin("R OUT", 8, PinBitCount::Bit4), pin("G OUT", 9, PinBitCount::Bit4), pin("B OUT", 10, PinBitCount::Bit4)];
	// Mirrors `BuiltinChipCreator.CreateDisplayRGB`: a fixed 21x21 grid
	// square, independent of the pin layout (the 16x16 pixel grid needs
	// more room than the pins alone would require).
	let mut desc = builtin_hidden_name(ChipType::DisplayRgb, inputs, outputs);
	let side = layout::GRID_SIZE * 21.0;
	desc.size = Vec2::new(side, side);
	desc
}

fn create_display_dot() -> ChipDescription {
	let inputs =
		vec![pin("ADDRESS", 0, PinBitCount::Bit8), pin1("PIXEL IN", 1), pin1("RESET", 2), pin1("WRITE", 3), pin1("REFRESH", 4), pin1("CLOCK", 5)];
	let outputs = vec![pin1("PIXEL OUT", 6)];
	// Mirrors `BuiltinChipCreator.CreateDisplayDot`: a square sized to fit
	// the pins (unlike RGB's fixed 21x21 -- the dot display has fewer
	// pins, so `MinChipHeightForPins` alone already gives it enough room).
	let height = layout::min_chip_height_for_pins(&input_bit_counts(&inputs), &output_bit_counts(&outputs));
	let mut desc = builtin_hidden_name(ChipType::DisplayDot, inputs, outputs);
	desc.size = Vec2::new(height, height);
	desc
}

fn create_display_led() -> ChipDescription {
	let inputs = vec![pin1("IN", 0)];
	// Mirrors `BuiltinChipCreator.CreateDisplayLED`: a square sized to fit
	// its single 1-bit input pin.
	let height = layout::min_chip_height_for_pins(&input_bit_counts(&inputs), &[]);
	let mut desc = builtin_hidden_name(ChipType::DisplayLed, inputs, vec![]);
	desc.size = Vec2::new(height, height);
	desc
}

/// `PinDescription::bit_count` for each pin in `pins`, in order -- the
/// shape `layout::min_chip_height_for_pins` wants. A tiny local helper
/// since these builtin display chips are the only place in this module
/// that needs to feed pins into the layout module.
fn input_bit_counts(pins: &[PinDescription]) -> Vec<PinBitCount> {
	pins.iter().map(|p| p.bit_count).collect()
}
fn output_bit_counts(pins: &[PinDescription]) -> Vec<PinBitCount> {
	pins.iter().map(|p| p.bit_count).collect()
}

/// (Not really a "chip", but convenient to treat it as one -- these back
/// the dev-pins on a custom chip's boundary.)
pub fn create_input_or_output_pin(chip_type: ChipType) -> ChipDescription {
	let (is_input, is_output, num_bits) = is_input_or_output_pin(chip_type);
	let name = if is_input { "IN" } else { "OUT" };
	let p = pin(name, 0, num_bits);

	let inputs = if is_input { vec![p.clone()] } else { vec![] };
	let outputs = if is_output { vec![p] } else { vec![] };

	builtin(chip_type, inputs, outputs)
}

fn is_input_or_output_pin(chip_type: ChipType) -> (bool, bool, PinBitCount) {
	use ChipType::*;
	match chip_type {
		In1Bit => (true, false, PinBitCount::Bit1),
		Out1Bit => (false, true, PinBitCount::Bit1),
		In4Bit => (true, false, PinBitCount::Bit4),
		Out4Bit => (false, true, PinBitCount::Bit4),
		In8Bit => (true, false, PinBitCount::Bit8),
		Out8Bit => (false, true, PinBitCount::Bit8),
		_ => (false, false, PinBitCount::Bit1),
	}
}

fn create_bus(bit_count: PinBitCount) -> ChipDescription {
	let chip_type = match bit_count {
		PinBitCount::Bit1 => ChipType::Bus1Bit,
		PinBitCount::Bit4 => ChipType::Bus4Bit,
		PinBitCount::Bit8 => ChipType::Bus8Bit,
	};
	let name = name_for(chip_type);

	builtin_hidden_name(chip_type, vec![pin(&format!("{name} (Hidden)"), 0, bit_count)], vec![pin(&name, 1, bit_count)])
}

fn create_bus_terminus(bit_count: PinBitCount) -> ChipDescription {
	let chip_type = match bit_count {
		PinBitCount::Bit1 => ChipType::BusTerminus1Bit,
		PinBitCount::Bit4 => ChipType::BusTerminus4Bit,
		PinBitCount::Bit8 => ChipType::BusTerminus8Bit,
	};
	let bus_origin = create_bus(bit_count);

	builtin_hidden_name(chip_type, vec![pin(&bus_origin.name, 0, bit_count)], vec![])
}

/// Mirrors DLS.Description.ChipTypeHelper.GetName -- the display name for a
/// chip type, also used as its `ChipDescription.name` / library lookup key
/// for builtins.
pub fn name_for(chip_type: ChipType) -> String {
	use ChipType::*;
	let s = match chip_type {
		Custom => "Custom",
		Nand => "NAND",
		Clock => "CLOCK",
		Pulse => "PULSE",
		TriStateBuffer => "3-STATE BUFFER",
		DevRam8Bit => "dev.RAM-8",
		Rom256x16 => "ROM 256\u{d7}16",
		Split4To1Bit => "4-1BIT",
		Split8To1Bit => "8-1BIT",
		Split8To4Bit => "8-4BIT",
		Merge4To8Bit => "4-8BIT",
		Merge1To8Bit => "1-8BIT",
		Merge1To4Bit => "1-4BIT",
		DisplayRgb => "RGB DISPLAY",
		DisplayDot => "DOT DISPLAY",
		SevenSegmentDisplay => "7-SEGMENT",
		DisplayLed => "LED",
		Buzzer => "BUZZER",
		In1Bit => "IN-1",
		In4Bit => "IN-4",
		In8Bit => "IN-8",
		Out1Bit => "OUT-1",
		Out4Bit => "OUT-4",
		Out8Bit => "OUT-8",
		Key => "KEY",
		KeyMods => "MOD KEYS",
		Bus1Bit => "BUS-1",
		Bus4Bit => "BUS-4",
		Bus8Bit => "BUS-8",
		BusTerminus1Bit => "BUS-TERMINUS-1",
		BusTerminus4Bit => "BUS-TERMINUS-4",
		BusTerminus8Bit => "BUS-TERMINUS-8",
	};
	s.to_string()
}

fn validate_all_pin_ids(chips: &[ChipDescription]) {
	use std::collections::HashSet;
	for chip in chips {
		let mut ids = HashSet::new();
		for p in chip.input_pins.iter().chain(chip.output_pins.iter()) {
			if !ids.insert(p.id) {
				panic!("Pin has duplicate ID ({}) in builtin chip: {}", p.id, chip.name);
			}
		}
	}
}
