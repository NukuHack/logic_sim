//! Demo: builds a 1-bit half adder out of NAND gates (NAND is the only
//! builtin logic primitive DLS actually simulates directly), drives its
//! inputs across all 4 combinations, and prints the outputs. This proves
//! out the ported Simulator end-to-end: description -> build -> step.

use logic_sim::{
    ChipDescription, ChipLibrary, ChipType, ExternalInput, PinAddress, PinBitCount,
    PinDescription, SubChipDescription, Simulator, WireDescription, Vec2
};

fn nand_desc() -> ChipDescription {
    let mut d = ChipDescription::new("NAND", ChipType::Nand);
    d.input_pins.push(PinDescription::new("A", 0, PinBitCount::Bit1));
    d.input_pins.push(PinDescription::new("B", 1, PinBitCount::Bit1));
    d.output_pins.push(PinDescription::new("OUT", 2, PinBitCount::Bit1)); // ids must be unique across in+out
    d
}

/// Build a custom "AND" chip: NAND -> NAND(x,x), two subchips wired together.
fn and_desc() -> ChipDescription {
    let mut d = ChipDescription::new("AND", ChipType::Custom);
    d.input_pins.push(PinDescription::new("A", 100, PinBitCount::Bit1));
    d.input_pins.push(PinDescription::new("B", 101, PinBitCount::Bit1));
    d.output_pins.push(PinDescription::new("OUT", 200, PinBitCount::Bit1));

    // subchip 1: first NAND, subchip 2: inverter NAND (both inputs tied together)
    d.sub_chips.push(SubChipDescription { name: "NAND".into(), id: 1, internal_data: None, label: None, position: Vec2::new(-1.0, 0.0), pin_colour_info: Vec::new() });
    d.sub_chips.push(SubChipDescription { name: "NAND".into(), id: 2, internal_data: None, label: None, position: Vec2::new(1.0, 0.0), pin_colour_info: Vec::new() });

    // dev-pin A/B (owner id = -1 like original convention for chip's own IO... using 0 here
    // since our PinAddress just needs owner ids that resolve; the chip's own pins are matched
    // by pin_owner_id == pin_id on the *chip's own* input/output pin list)
    d.wires.push(WireDescription::new(PinAddress::new(100, 100), PinAddress::new(1, 0)));
    d.wires.push(WireDescription::new(PinAddress::new(101, 101), PinAddress::new(1, 1)));
    d.wires.push(WireDescription::new(PinAddress::new(1, 2), PinAddress::new(2, 0)));
    d.wires.push(WireDescription::new(PinAddress::new(1, 2), PinAddress::new(2, 1)));
    d.wires.push(WireDescription::new(PinAddress::new(2, 2), PinAddress::new(200, 200)));

    d
}

fn main() {
    let mut library = ChipLibrary::new();
    library.add(nand_desc());
    let and_chip = and_desc();

    let mut sim = Simulator::build(&and_chip, &library);

    println!("A B | AND");
    for &a in &[0u32, 1] {
        for &b in &[0u32, 1] {
            let inputs = vec![
                ExternalInput { address: PinAddress::new(100, 100), state: a },
                ExternalInput { address: PinAddress::new(101, 101), state: b },
            ];
            // run a few frames to let the signal settle/propagate
            for _ in 0..3 {
                sim.run_simulation_step(&inputs);
            }
            let out_pin = sim.chip(sim.root()).output_pins[0];
            let out_state = sim.pin(out_pin).state & 1;
            println!("{a} {b} | {out_state}");
        }
    }
}
