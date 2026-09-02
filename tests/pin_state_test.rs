//! Regression test for a freshly-built pin's initial width: it must reflect the pin's
//! declared `bit_count`, not the widest supported width. Exercised through the public
//! `Simulator::build`/`Simulator::pin` API, since the sim's internal arena is private.

use logic_sim::{ChipDescription, ChipLibrary, ChipType, PinBitCount, PinDescription, Simulator};

#[test]
fn freshly_built_pins_start_at_their_declared_width_not_the_widest_one() {
	let mut chip = ChipDescription::new("ROOT", ChipType::Custom);
	chip.input_pins.push(PinDescription::new("IN1", 1, PinBitCount::Bit1));
	chip.input_pins.push(PinDescription::new("IN4", 2, PinBitCount::Bit4));
	chip.output_pins.push(PinDescription::new("OUT1", 3, PinBitCount::Bit1));

	let library = ChipLibrary::new();
	let sim = Simulator::build(&chip, &library);
	let root = sim.chip(sim.root());

	assert_eq!(sim.pin(root.input_pins[0]).state.width(), PinBitCount::Bit1, "a 1-bit input shouldn't start out tagged 8-bit");
	assert_eq!(sim.pin(root.input_pins[1]).state.width(), PinBitCount::Bit4, "a 4-bit input shouldn't start out tagged 8-bit");
	assert_eq!(sim.pin(root.output_pins[0]).state.width(), PinBitCount::Bit1, "a 1-bit output shouldn't start out tagged 8-bit");
}
