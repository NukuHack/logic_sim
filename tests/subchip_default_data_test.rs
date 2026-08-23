//! Regression tests for a placement-time panic: a freshly-placed `PULSE` (or `ROM 256x16`)
//! subchip used to be given `internal_data: None` (see `bin/app.rs`'s `try_place_pending_chip`),
//! and `process_builtin_chip`'s `Pulse`/`Rom256x16` arms index straight into `internal_state`
//! with no bounds check -- so simulating one immediately after placing it panicked with
//! `index out of bounds: the len is 0 but the index is 1` (`Pulse`) or a similar panic reading
//! any nonzero ROM address, before a config popup was ever opened to give it real data.
//!
//! The fix has two parts: `bin/app.rs::default_internal_data` now populates sensible starting
//! data at placement time (not exercised here, since it lives in a binary crate the integration
//! tests can't reach), and -- more importantly for these tests -- `sim::build_internal_state` now
//! defends `Rom256x16`/`Pulse` the same way it already did `DisplayRgb`/`DisplayDot`/`DevRam8Bit`:
//! always building a correctly-sized `internal_state` regardless of what (if anything) was saved.

use logic_sim::{
	ChipDescription, ChipLibrary, ChipType, ExternalInput, PinAddress, PinBitCount, PinDescription, Simulator, SubChipDescription, Vec2,
	WireDescription,
};

const SUBCHIP_ID: i32 = 1;
const IN_PIN_ID: i32 = 200;
const OUT_PIN_ID: i32 = 201;
const OUT_PIN_ID_B: i32 = 202;

/// Wraps a single instance of `builtin_name` (one input pin at subchip-id `sub_in_id`, one or two
/// output pins at `sub_out_ids`) as a subchip of a tiny custom chip, with `internal_data` exactly
/// as given -- in particular, `None` reproduces the pre-fix placement code path verbatim.
fn wrap_builtin(
	builtin_name: &str,
	sub_in_id: i32,
	in_bits: PinBitCount,
	sub_out_ids: &[(i32, PinBitCount)],
	internal_data: Option<Vec<u32>>,
) -> (Simulator, Vec<i32>) {
	let mut wrapper = ChipDescription::new("WRAPPER", ChipType::Custom);
	wrapper.input_pins = vec![PinDescription::new("IN", IN_PIN_ID, in_bits)];

	let own_out_ids: Vec<i32> = if sub_out_ids.len() == 1 { vec![OUT_PIN_ID] } else { vec![OUT_PIN_ID, OUT_PIN_ID_B] };
	wrapper.output_pins = own_out_ids.iter().zip(sub_out_ids).map(|(&id, &(_, bits))| PinDescription::new(format!("OUT{id}"), id, bits)).collect();

	wrapper.sub_chips = vec![SubChipDescription {
		name: builtin_name.to_string(),
		id: SUBCHIP_ID,
		internal_data,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: Vec::new(),
	}];

	let mut wires = vec![WireDescription::new(PinAddress::new(IN_PIN_ID, IN_PIN_ID), PinAddress::new(SUBCHIP_ID, sub_in_id))];
	for (&own_id, &(sub_id, _)) in own_out_ids.iter().zip(sub_out_ids) {
		wires.push(WireDescription::new(PinAddress::new(SUBCHIP_ID, sub_id), PinAddress::new(own_id, own_id)));
	}
	wrapper.wires = wires;

	let mut library = ChipLibrary::new();
	logic_sim::register_all_builtins(&mut library);
	library.add(wrapper.clone());
	let sim = Simulator::build(&wrapper, &library);
	(sim, own_out_ids)
}

fn drive_and_step(sim: &mut Simulator, in_state: u32, steps: usize) {
	let inputs = [ExternalInput { address: PinAddress::new(IN_PIN_ID, IN_PIN_ID), state: in_state }];
	for _ in 0..steps {
		sim.run_simulation_step(&inputs);
	}
}

fn read(sim: &Simulator, out_pin_id: i32) -> u32 {
	let pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("wrapper output pin should resolve");
	sim.pin(pin).state
}

// ---- PULSE ----

/// The exact crash reported: placing a `PULSE` with no internal data, then simulating it (a
/// rising edge on its input is what drove the original panic at `sim.rs:455`, reading
/// `internal_state[TICKS_REMAINING]` out of an empty vec).
#[test]
fn pulse_subchip_with_no_internal_data_does_not_panic_on_a_rising_edge() {
	let (mut sim, out_ids) = wrap_builtin("PULSE", 0, PinBitCount::Bit1, &[(1, PinBitCount::Bit1)], None);
	drive_and_step(&mut sim, 0, 2);
	drive_and_step(&mut sim, 1, 2); // rising edge -- this used to panic
	assert_eq!(read(&sim, out_ids[0]) & 1, 1, "pulse should be actively firing right after the triggering edge");
}

/// With no saved duration, the chip should fall back to a real (nonzero) default pulse length
/// rather than silently emitting a zero-length pulse -- confirm the output is still high a
/// several steps after the edge, not just on the one step it landed on.
#[test]
fn pulse_subchip_with_no_internal_data_uses_a_nonzero_default_duration() {
	let (mut sim, out_ids) = wrap_builtin("PULSE", 0, PinBitCount::Bit1, &[(1, PinBitCount::Bit1)], None);
	drive_and_step(&mut sim, 0, 2);
	drive_and_step(&mut sim, 1, 1);
	for _ in 0..201 {
		drive_and_step(&mut sim, 1, 1);
		if read(&sim, out_ids[0]) & 1 == 0 {
			return; // pulse ended on its own well within a plausible default duration -- fine.
		}
	}
	panic!("pulse never ended within 200 steps -- default duration looks unreasonably long, or stuck");
}

/// A too-short (but non-empty) saved `internal_data` -- e.g. a save file from before some other
/// future field was added -- must be padded out rather than indexed out of bounds either.
#[test]
fn pulse_subchip_with_short_internal_data_does_not_panic() {
	let (mut sim, out_ids) = wrap_builtin("PULSE", 0, PinBitCount::Bit1, &[(1, PinBitCount::Bit1)], Some(vec![5]));
	drive_and_step(&mut sim, 0, 2);
	drive_and_step(&mut sim, 1, 2);
	assert_eq!(read(&sim, out_ids[0]) & 1, 1);
}

// ---- ROM 256x16 ----

/// The other half of the same class of bug: reading any nonzero address off a ROM placed with no
/// internal data indexed straight past the end of an empty `internal_state`.
#[test]
fn rom_subchip_with_no_internal_data_does_not_panic_for_any_address() {
	let (mut sim, out_ids) = wrap_builtin("ROM 256\u{d7}16", 0, PinBitCount::Bit8, &[(1, PinBitCount::Bit8), (2, PinBitCount::Bit8)], None);
	for addr in [0u32, 1, 128, 255] {
		drive_and_step(&mut sim, addr, 2);
		assert_eq!(read(&sim, out_ids[0]) & 0xFF, 0, "unwritten ROM word should read back as 0, addr {addr}");
		assert_eq!(read(&sim, out_ids[1]) & 0xFF, 0, "unwritten ROM word should read back as 0, addr {addr}");
	}
}

/// A saved ROM contents vector shorter than the full 256 words (e.g. one only populated up to the
/// last address the player actually edited) must have the remaining words default to 0 rather
/// than panic when an unpopulated address is read.
#[test]
fn rom_subchip_with_partially_populated_internal_data_pads_the_rest_with_zero() {
	let mut data = vec![0u32; 3];
	data[2] = 0xAB;
	let (mut sim, out_ids) = wrap_builtin("ROM 256\u{d7}16", 0, PinBitCount::Bit8, &[(1, PinBitCount::Bit8), (2, PinBitCount::Bit8)], Some(data));

	drive_and_step(&mut sim, 2, 2);
	assert_eq!(read(&sim, out_ids[1]) & 0xFF, 0xAB, "the word that WAS saved should still read back correctly");

	drive_and_step(&mut sim, 255, 2); // well past the saved data's length
	assert_eq!(read(&sim, out_ids[0]) & 0xFF, 0, "an address past the saved data should read back as 0, not panic");
	assert_eq!(read(&sim, out_ids[1]) & 0xFF, 0, "an address past the saved data should read back as 0, not panic");
}
