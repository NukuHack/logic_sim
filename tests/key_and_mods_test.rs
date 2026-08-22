//! Tests for the `Key` builtin (turns on while a given letter is held) and
//! the new `KeyMods` builtin (outputs the current shift/ctrl/alt/super
//! state as a bitmask). Both chips are driven purely through
//! `Simulator::held_keys` / `Simulator::key_modifiers` here -- the actual
//! winit event -> `Simulator` field plumbing lives in `bin/app.rs` /
//! `bin/viewer.rs`, which (being GPU-window binaries) aren't exercised by
//! `cargo test`; see their module docs.

use logic_sim::description::{ChipDescription, ChipType, PinAddress, PinBitCount, SubChipDescription, WireDescription};
use logic_sim::sim::key_mods_bits;
use logic_sim::structs::Vec2;
use logic_sim::{ChipLibrary, Simulator};

/// Builds a tiny custom `ChipDescription` that places a single instance of
/// `builtin_name` and wires its (only) output pin straight out to the
/// wrapper's own output pin -- just enough scaffolding to observe a builtin
/// chip's output, since (per `Simulator::build`) a builtin chip only gets
/// simulated when it's a *subchip* of something, never as the root itself.
fn wrap_builtin_single_output(builtin_name: &str, out_bit_count: PinBitCount, internal_data: Option<Vec<u32>>) -> ChipDescription {
	const SUBCHIP_ID: i32 = 1;
	const OUT_PIN_ID: i32 = 100;

	let mut wrapper = ChipDescription::new("WRAPPER", ChipType::Custom);
	wrapper.output_pins = vec![logic_sim::PinDescription::new("OUT", OUT_PIN_ID, out_bit_count)];
	wrapper.sub_chips = vec![SubChipDescription {
		name: builtin_name.to_string(),
		id: SUBCHIP_ID,
		internal_data,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: Vec::new(),
	}];
	// Builtin's own output pin is always id 0 (see `builtins::create_input_key_chip` /
	// `create_key_mods_chip`). Own (wrapper-level) pins are addressed by
	// `PinAddress::new(<the pin's own id>, <anything>)` -- see `Simulator::find_pin`.
	wrapper.wires = vec![WireDescription::new(PinAddress::new(SUBCHIP_ID, 0), PinAddress::new(OUT_PIN_ID, OUT_PIN_ID))];
	wrapper
}

fn build_sim_around(builtin_name: &str, out_bit_count: PinBitCount, internal_data: Option<Vec<u32>>) -> (Simulator, i32) {
	let wrapper = wrap_builtin_single_output(builtin_name, out_bit_count, internal_data);
	let mut library = ChipLibrary::new();
	logic_sim::register_all_builtins(&mut library);
	library.add(wrapper.clone());
	let sim = Simulator::build(&wrapper, &library);
	(sim, 100) // 100 == OUT_PIN_ID above
}

fn read_output(sim: &Simulator, out_pin_id: i32) -> u32 {
	let pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("wrapper output pin should resolve");
	sim.pin(pin).state
}

// ---- Key chip ----

#[test]
fn key_chip_is_low_when_letter_not_held() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, Some(vec![b'A' as u32]));
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 0);
}

#[test]
fn key_chip_is_high_when_matching_letter_held() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, Some(vec![b'A' as u32]));
	sim.held_keys.insert('A');
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 1);
}

#[test]
fn key_chip_ignores_other_held_letters() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, Some(vec![b'A' as u32]));
	sim.held_keys.insert('B');
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 0);
}

/// The chip itself compares the *raw* char in `held_keys` against its
/// (always-uppercase) stored letter -- it does no case-folding on its own.
/// Lower-casing a basic 'a' keypress into 'A' before it reaches
/// `held_keys` is the host's job (done in `bin/app.rs`/`bin/viewer.rs`'s
/// `handle_key_event`), not the simulator's. This test documents that
/// contract: a stray lowercase char sitting in `held_keys` must NOT match.
#[test]
fn key_chip_does_not_match_lowercase_in_held_keys() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, Some(vec![b'A' as u32]));
	sim.held_keys.insert('a');
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 0);
}

#[test]
fn key_chip_releasing_the_key_turns_output_back_off() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, Some(vec![b'A' as u32]));
	sim.held_keys.insert('A');
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 1);

	sim.held_keys.remove(&'A');
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 0);
}

/// A `KEY` subchip with no saved `InternalData` (as can happen for a
/// freshly-placed chip before a letter's been assigned) shouldn't panic --
/// it should just behave as if bound to the null character, which nothing
/// can ever hold.
#[test]
fn key_chip_with_no_internal_data_does_not_panic() {
	let (mut sim, out_id) = build_sim_around("KEY", PinBitCount::Bit1, None);
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id) & 1, 0);
}

// ---- KeyMods chip ----

#[test]
fn key_mods_chip_outputs_zero_by_default() {
	let (mut sim, out_id) = build_sim_around("MOD KEYS", PinBitCount::Bit8, None);
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}
	assert_eq!(read_output(&sim, out_id), 0);
}

#[test]
fn key_mods_chip_outputs_current_modifier_bitmask() {
	let (mut sim, out_id) = build_sim_around("MOD KEYS", PinBitCount::Bit8, None);
	sim.key_modifiers = key_mods_bits::SHIFT | key_mods_bits::ALT;
	for _ in 0..2 {
		sim.run_simulation_step(&[]);
	}

	let state = read_output(&sim, out_id);
	// Low 16 bits are the driven bit-states...
	assert_eq!(state & 0xFFFF, key_mods_bits::SHIFT | key_mods_bits::ALT);
	// ...and the pin should be fully driven (no tristated bits), so the
	// high 16 bits (the tristate-flag half of the pin state word) are 0.
	assert_eq!(state >> 16, 0);
}

#[test]
fn key_mods_bits_are_all_distinct_single_bits() {
	let all = [key_mods_bits::SHIFT, key_mods_bits::CONTROL, key_mods_bits::ALT, key_mods_bits::SUPER];
	for &b in &all {
		assert_eq!(b.count_ones(), 1, "each modifier should be exactly one bit");
	}
	let mut seen = 0u32;
	for &b in &all {
		assert_eq!(seen & b, 0, "modifier bits must not overlap");
		seen |= b;
	}
}

// ---- Builtin description shape ----

#[test]
fn key_mods_builtin_is_registered_with_expected_pin_layout() {
	let chips = logic_sim::create_all_builtins();
	let key_mods = chips.iter().find(|c| c.name == "MOD KEYS").expect("MOD KEYS builtin should be registered");
	assert_eq!(key_mods.chip_type, ChipType::KeyMods);
	assert!(key_mods.input_pins.is_empty());
	assert_eq!(key_mods.output_pins.len(), 1);
	assert_eq!(key_mods.output_pins[0].bit_count, PinBitCount::Bit8);
}

#[test]
fn chip_type_key_mods_round_trips_through_int() {
	assert_eq!(ChipType::KeyMods.to_int(), 31);
	assert_eq!(ChipType::from_int(31), ChipType::KeyMods);
}
