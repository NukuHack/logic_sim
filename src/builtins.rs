//! Builtin chip descriptions -- the pin layouts for every non-custom chip
//! type (NAND, Clock, RAM, displays, bus, merge/split, I/O pins, ...).
//!
//! Ported from DLS.Game.BuiltinChipCreator. Unlike the original, this drops
//! everything related to editor rendering/layout (size, colour, grid
//! snapping, name display location) since the simulation core has no use
//! for it -- only pin names, IDs, and bit-widths matter for building and
//! running the simulation graph. If a UI layer is added later, that visual
//! metadata can be reintroduced alongside it without touching this module.

use crate::description::{ChipDescription, ChipLibrary, ChipType, NameLocation, PinBitCount, PinDescription};

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
    builtin(
        ChipType::Nand,
        vec![pin1("IN B", 0), pin1("IN A", 1)],
        vec![pin1("OUT", 2)],
    )
}

fn create_buzzer() -> ChipDescription {
    builtin(
        ChipType::Buzzer,
        vec![pin("PITCH", 1, PinBitCount::Bit8), pin("VOLUME", 0, PinBitCount::Bit4)],
        vec![],
    )
}

fn dev_create_ram_8() -> ChipDescription {
    builtin(
        ChipType::DevRam8Bit,
        vec![
            pin("ADDRESS", 0, PinBitCount::Bit8),
            pin("DATA", 1, PinBitCount::Bit8),
            pin1("WRITE", 2),
            pin1("RESET", 3),
            pin1("CLOCK", 4),
        ],
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

fn create_tristate_buffer() -> ChipDescription {
    builtin(
        ChipType::TriStateBuffer,
        vec![pin1("IN", 0), pin1("ENABLE", 1)],
        vec![pin1("OUT", 2)],
    )
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
    let inputs = (0..num_in)
        .map(|i| pin(&get_pin_name(i, num_in, true), i, bit_count_in))
        .collect();
    let outputs = (0..num_out)
        .map(|i| pin(&get_pin_name(i, num_out, false), num_in + i, bit_count_out))
        .collect();

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
    builtin_hidden_name(
        ChipType::SevenSegmentDisplay,
        vec![
            pin1("A", 0),
            pin1("B", 1),
            pin1("C", 2),
            pin1("D", 3),
            pin1("E", 4),
            pin1("F", 5),
            pin1("G", 6),
            pin1("COL", 7),
        ],
        vec![],
    )
}

fn create_display_rgb() -> ChipDescription {
    builtin_hidden_name(
        ChipType::DisplayRgb,
        vec![
            pin("ADDRESS", 0, PinBitCount::Bit8),
            pin("RED", 1, PinBitCount::Bit4),
            pin("GREEN", 2, PinBitCount::Bit4),
            pin("BLUE", 3, PinBitCount::Bit4),
            pin1("RESET", 4),
            pin1("WRITE", 5),
            pin1("REFRESH", 6),
            pin1("CLOCK", 7),
        ],
        vec![
            pin("R OUT", 8, PinBitCount::Bit4),
            pin("G OUT", 9, PinBitCount::Bit4),
            pin("B OUT", 10, PinBitCount::Bit4),
        ],
    )
}

fn create_display_dot() -> ChipDescription {
    builtin_hidden_name(
        ChipType::DisplayDot,
        vec![
            pin("ADDRESS", 0, PinBitCount::Bit8),
            pin1("PIXEL IN", 1),
            pin1("RESET", 2),
            pin1("WRITE", 3),
            pin1("REFRESH", 4),
            pin1("CLOCK", 5),
        ],
        vec![pin1("PIXEL OUT", 6)],
    )
}

fn create_display_led() -> ChipDescription {
    builtin_hidden_name(ChipType::DisplayLed, vec![pin1("IN", 0)], vec![])
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

    builtin_hidden_name(
        chip_type,
        vec![pin(&format!("{name} (Hidden)"), 0, bit_count)],
        vec![pin(&name, 1, bit_count)],
    )
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
