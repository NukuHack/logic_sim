//! Address-decoding tests for the "big" displays -- `RGB DISPLAY` and `DOT DISPLAY` -- the two
//! display builtins driven by an 8-bit `ADDRESS` bus (`process_display_rgb` / `process_display_dot`
//! in `sim.rs`), as opposed to `7-SEGMENT`/`LED` which have no addressing at all. Both share the
//! same double-buffered shape: a write on a clock rising edge lands in a back buffer at
//! `addr + ADDRESS_SPACE`, and a `REFRESH` pulse (also clock-edge-gated) copies the whole back
//! buffer over the front buffer, which is what `ADDRESS` actually reads from combinationally.
//! These tests exist to pin down that exactly the addressed cell changes on write/read, that
//! individual address bits aren't swapped or dropped, and that the two-phase write/refresh
//! sequencing behaves as the source implies.

use logic_sim::{
	pin_state::PinState, ChipDescription, ChipLibrary, ChipType, ExternalInput, PinAddress, PinBitCount, PinDescription, Simulator,
	SubChipDescription, Vec2, WireDescription,
};

const SUBCHIP_ID: i32 = 1;
/// Base id for the wrapper's own input dev-pins, offset well clear of the subchip's own
/// pin ids (0..=7) so the two id spaces can never collide.
const OWN_INPUT_BASE: i32 = 1000;
/// Base id for the wrapper's own output dev-pins.
const OWN_OUTPUT_BASE: i32 = 2000;

/// Builds a tiny custom `ChipDescription` that places a single instance of `builtin_name` and
/// wires every one of its input/output pins straight through to a matching own boundary pin --
/// the same "wrap a builtin as a subchip so it actually gets simulated" trick used in
/// `key_and_mods_test.rs`, generalised here to the multi-pin displays. `sub_inputs`/`sub_outputs`
/// are `(pin_id, bit_count)` pairs, in the order the builtin declares them in `builtins.rs`.
/// Returns the built `Simulator` plus the wrapper-level pin ids, in the same order, so callers
/// can address them with `ExternalInput`/`find_pin`.
fn build_display_sim(builtin_name: &str, sub_inputs: &[(i32, PinBitCount)], sub_outputs: &[(i32, PinBitCount)]) -> (Simulator, Vec<i32>, Vec<i32>) {
	let own_input_ids: Vec<i32> = (0..sub_inputs.len() as i32).map(|i| OWN_INPUT_BASE + i).collect();
	let own_output_ids: Vec<i32> = (0..sub_outputs.len() as i32).map(|i| OWN_OUTPUT_BASE + i).collect();

	let mut wrapper = ChipDescription::new("WRAPPER", ChipType::Custom);
	wrapper.input_pins = own_input_ids.iter().zip(sub_inputs).map(|(&id, &(_, bits))| PinDescription::new(format!("IN{id}"), id, bits)).collect();
	wrapper.output_pins = own_output_ids.iter().zip(sub_outputs).map(|(&id, &(_, bits))| PinDescription::new(format!("OUT{id}"), id, bits)).collect();
	wrapper.sub_chips = vec![SubChipDescription {
		name: builtin_name.to_string(),
		id: SUBCHIP_ID,
		internal_data: None,
		position: Vec2::ZERO,
		label: None,
		pin_colour_info: Vec::new(),
	}];

	let mut wires = Vec::new();
	for (&own_id, &(sub_id, _)) in own_input_ids.iter().zip(sub_inputs) {
		wires.push(WireDescription::new(PinAddress::new(own_id, own_id), PinAddress::new(SUBCHIP_ID, sub_id)));
	}
	for (&own_id, &(sub_id, _)) in own_output_ids.iter().zip(sub_outputs) {
		wires.push(WireDescription::new(PinAddress::new(SUBCHIP_ID, sub_id), PinAddress::new(own_id, own_id)));
	}
	wrapper.wires = wires;

	let mut library = ChipLibrary::new();
	logic_sim::register_all_builtins(&mut library);
	library.add(wrapper.clone());
	let sim = Simulator::build(&wrapper, &library);
	(sim, own_input_ids, own_output_ids)
}

/// Drives `values` (wrapper-level pin id -> state) for `steps` simulation steps in a row, letting
/// combinational propagation (and, for the last step of a phase, a clock edge) settle.
fn drive(sim: &mut Simulator, values: &[(i32, u32)], steps: usize) {
	let inputs: Vec<ExternalInput> =
		values.iter().map(|&(id, state)| ExternalInput { address: PinAddress::new(id, id), state: PinState::from_raw(state) }).collect();
	for _ in 0..steps {
		sim.run_simulation_step(&inputs, &mut logic_sim::audio::SimAudio::new());
	}
}

fn read(sim: &Simulator, out_pin_id: i32) -> u32 {
	let pin = sim.find_pin(sim.root(), PinAddress::new(out_pin_id, out_pin_id)).expect("wrapper output pin should resolve");
	sim.pin(pin).state.raw()
}

// ---- DOT DISPLAY ----

/// Pin ids for `DOT DISPLAY`'s inputs/outputs, per `builtins::create_display_dot`.
mod dot {
	pub const ADDRESS: i32 = 0;
	pub const PIXEL_IN: i32 = 1;
	pub const RESET: i32 = 2;
	pub const WRITE: i32 = 3;
	pub const REFRESH: i32 = 4;
	pub const CLOCK: i32 = 5;
	pub const PIXEL_OUT: i32 = 6;
}

struct DotSim {
	sim: Simulator,
	addr: i32,
	pixel_in: i32,
	reset: i32,
	write: i32,
	refresh: i32,
	clock: i32,
	pixel_out: i32,
}

fn build_dot_sim() -> DotSim {
	let inputs = [
		(dot::ADDRESS, PinBitCount::Bit8),
		(dot::PIXEL_IN, PinBitCount::Bit1),
		(dot::RESET, PinBitCount::Bit1),
		(dot::WRITE, PinBitCount::Bit1),
		(dot::REFRESH, PinBitCount::Bit1),
		(dot::CLOCK, PinBitCount::Bit1),
	];
	let outputs = [(dot::PIXEL_OUT, PinBitCount::Bit1)];
	let (sim, own_in, own_out) = build_display_sim("DOT DISPLAY", &inputs, &outputs);
	DotSim {
		sim,
		addr: own_in[0],
		pixel_in: own_in[1],
		reset: own_in[2],
		write: own_in[3],
		refresh: own_in[4],
		clock: own_in[5],
		pixel_out: own_out[0],
	}
}

impl DotSim {
	/// Writes `pixel` to `addr` and refreshes the front buffer in one clock pulse: settle low
	/// (so the chip's own last-clock-state tracking reads low), then pulse the clock high (which
	/// both commits the write to the back buffer and, since REFRESH is held high throughout,
	/// copies it into the front buffer the same edge -- see `process_display_dot`).
	fn write_and_refresh(&mut self, addr: u32, pixel: u32) {
		let lines = [(self.addr, addr), (self.pixel_in, pixel), (self.reset, 0), (self.write, 1), (self.refresh, 1), (self.clock, 0)];
		drive(&mut self.sim, &lines, 3);
		let lines = [(self.addr, addr), (self.pixel_in, pixel), (self.reset, 0), (self.write, 1), (self.refresh, 1), (self.clock, 1)];
		drive(&mut self.sim, &lines, 3);
	}

	/// Pulses RESET (clearing the back buffer) and REFRESH (copying it forward) together, the
	/// same way a real reset would be wired: RESET high, WRITE low, REFRESH high, one clock edge.
	fn reset_and_refresh(&mut self) {
		let lines = [(self.addr, 0), (self.pixel_in, 0), (self.reset, 1), (self.write, 0), (self.refresh, 1), (self.clock, 0)];
		drive(&mut self.sim, &lines, 3);
		let lines = [(self.addr, 0), (self.pixel_in, 0), (self.reset, 1), (self.write, 0), (self.refresh, 1), (self.clock, 1)];
		drive(&mut self.sim, &lines, 3);
	}

	/// Reads back whatever's currently at `addr` in the front buffer, with WRITE/REFRESH/RESET
	/// all held low so nothing mutates as a side effect of reading.
	fn read_pixel(&mut self, addr: u32) -> u32 {
		let lines = [(self.addr, addr), (self.pixel_in, 0), (self.reset, 0), (self.write, 0), (self.refresh, 0), (self.clock, 0)];
		drive(&mut self.sim, &lines, 3);
		read(&self.sim, self.pixel_out) & 1
	}
}

#[test]
fn dot_display_reads_back_written_pixel_at_its_own_address() {
	let mut d = build_dot_sim();
	d.write_and_refresh(42, 1);
	assert_eq!(d.read_pixel(42), 1);
}

#[test]
fn dot_display_leaves_every_other_address_untouched_by_a_single_write() {
	let mut d = build_dot_sim();
	d.write_and_refresh(42, 1);
	for addr in [0u32, 1, 41, 43, 100, 255] {
		assert_eq!(d.read_pixel(addr), 0, "address {addr} should be unaffected by a write to 42");
	}
}

/// Walks a single set bit through every position of the 8-bit address (1, 2, 4, .. 128), writing
/// a distinct pixel to each and confirming it lands at *exactly* that address and nowhere else.
/// This is the most direct test against address bits being swapped, dropped, or misaligned --
/// a bit-reversed or off-by-one decoder would fail this even though single low addresses (like
/// the `42` above) might accidentally still pass.
#[test]
fn dot_display_address_bits_are_independently_and_correctly_decoded() {
	let mut d = build_dot_sim();
	let bit_addresses: Vec<u32> = (0..8).map(|bit| 1u32 << bit).collect();
	for &addr in &bit_addresses {
		d.write_and_refresh(addr, 1);
	}
	for &addr in &bit_addresses {
		assert_eq!(d.read_pixel(addr), 1, "bit-position address {addr} (0b{addr:08b}) should read back set");
	}
	// Every address that ISN'T an exact power of two (i.e. every combination of two or more of
	// the bits above) must still read back unset -- catching decoders that OR/alias bits together.
	for addr in 0u32..=255 {
		if bit_addresses.contains(&addr) {
			continue;
		}
		assert_eq!(d.read_pixel(addr), 0, "address {addr} (0b{addr:08b}) should not have been touched");
	}
}

/// Complementary checkerboard bit patterns (`0b10101010` / `0b01010101`) are a classic way to
/// catch an address decoder that's silently reading the bus reversed or shifted.
#[test]
fn dot_display_handles_complementary_checkerboard_addresses() {
	let mut d = build_dot_sim();
	d.write_and_refresh(0b1010_1010, 1);
	d.write_and_refresh(0b0101_0101, 1);
	assert_eq!(d.read_pixel(0b1010_1010), 1);
	assert_eq!(d.read_pixel(0b0101_0101), 1);
	// Neither write should have bled a bit into the other's address.
	assert_eq!(d.read_pixel(0b1010_1011), 0);
	assert_eq!(d.read_pixel(0b0101_0100), 0);
}

#[test]
fn dot_display_address_zero_and_max_are_both_individually_addressable() {
	let mut d = build_dot_sim();
	d.write_and_refresh(0, 1);
	d.write_and_refresh(255, 1);
	assert_eq!(d.read_pixel(0), 1);
	assert_eq!(d.read_pixel(255), 1);
	assert_eq!(d.read_pixel(1), 0);
	assert_eq!(d.read_pixel(254), 0);
}

/// `ADDRESS` reads the *front* buffer, which only updates on a REFRESH-gated clock edge (see
/// `process_display_dot`). A write with REFRESH held low should update the back buffer but must
/// not be visible on `PIXEL OUT` until a refresh actually happens.
#[test]
fn dot_display_write_without_refresh_is_not_visible_until_refreshed() {
	let mut d = build_dot_sim();
	// Settle low, then pulse the clock high with WRITE set but REFRESH held low.
	let lines_low = [(d.addr, 10), (d.pixel_in, 1), (d.reset, 0), (d.write, 1), (d.refresh, 0), (d.clock, 0)];
	drive(&mut d.sim, &lines_low, 3);
	let lines_high = [(d.addr, 10), (d.pixel_in, 1), (d.reset, 0), (d.write, 1), (d.refresh, 0), (d.clock, 1)];
	drive(&mut d.sim, &lines_high, 3);

	assert_eq!(d.read_pixel(10), 0, "unrefreshed write must not be visible on the front buffer yet");

	// Now refresh (with WRITE low, so this only copies the buffer, it doesn't re-write).
	let refresh_low = [(d.addr, 10), (d.pixel_in, 0), (d.reset, 0), (d.write, 0), (d.refresh, 1), (d.clock, 0)];
	drive(&mut d.sim, &refresh_low, 3);
	let refresh_high = [(d.addr, 10), (d.pixel_in, 0), (d.reset, 0), (d.write, 0), (d.refresh, 1), (d.clock, 1)];
	drive(&mut d.sim, &refresh_high, 3);

	assert_eq!(d.read_pixel(10), 1, "the previously-buffered write should now be visible after a refresh");
}

/// Holding the clock steady (never actually transitioning low-to-high) must never trigger a
/// write, regardless of how many steps run with WRITE held high -- writes are edge-triggered,
/// not level-triggered.
#[test]
fn dot_display_write_requires_an_actual_clock_rising_edge() {
	let mut d = build_dot_sim();
	let lines = [(d.addr, 7), (d.pixel_in, 1), (d.reset, 0), (d.write, 1), (d.refresh, 1), (d.clock, 0)];
	drive(&mut d.sim, &lines, 6);
	assert_eq!(d.read_pixel(7), 0, "no clock edge ever occurred, so nothing should have been written");
}

#[test]
fn dot_display_reset_clears_the_buffer_after_a_refresh() {
	let mut d = build_dot_sim();
	d.write_and_refresh(5, 1);
	assert_eq!(d.read_pixel(5), 1);

	d.reset_and_refresh();
	assert_eq!(d.read_pixel(5), 0, "reset (followed by a refresh) should clear a previously-set pixel");
}

// ---- RGB DISPLAY ----

/// Pin ids for `RGB DISPLAY`'s inputs/outputs, per `builtins::create_display_rgb`.
mod rgb {
	pub const ADDRESS: i32 = 0;
	pub const RED: i32 = 1;
	pub const GREEN: i32 = 2;
	pub const BLUE: i32 = 3;
	pub const RESET: i32 = 4;
	pub const WRITE: i32 = 5;
	pub const REFRESH: i32 = 6;
	pub const CLOCK: i32 = 7;
	pub const R_OUT: i32 = 8;
	pub const G_OUT: i32 = 9;
	pub const B_OUT: i32 = 10;
}

struct RgbSim {
	sim: Simulator,
	addr: i32,
	red: i32,
	green: i32,
	blue: i32,
	reset: i32,
	write: i32,
	refresh: i32,
	clock: i32,
	r_out: i32,
	g_out: i32,
	b_out: i32,
}

fn build_rgb_sim() -> RgbSim {
	let inputs = [
		(rgb::ADDRESS, PinBitCount::Bit8),
		(rgb::RED, PinBitCount::Bit4),
		(rgb::GREEN, PinBitCount::Bit4),
		(rgb::BLUE, PinBitCount::Bit4),
		(rgb::RESET, PinBitCount::Bit1),
		(rgb::WRITE, PinBitCount::Bit1),
		(rgb::REFRESH, PinBitCount::Bit1),
		(rgb::CLOCK, PinBitCount::Bit1),
	];
	let outputs = [(rgb::R_OUT, PinBitCount::Bit4), (rgb::G_OUT, PinBitCount::Bit4), (rgb::B_OUT, PinBitCount::Bit4)];
	let (sim, own_in, own_out) = build_display_sim("RGB DISPLAY", &inputs, &outputs);
	RgbSim {
		sim,
		addr: own_in[0],
		red: own_in[1],
		green: own_in[2],
		blue: own_in[3],
		reset: own_in[4],
		write: own_in[5],
		refresh: own_in[6],
		clock: own_in[7],
		r_out: own_out[0],
		g_out: own_out[1],
		b_out: own_out[2],
	}
}

impl RgbSim {
	fn write_and_refresh(&mut self, addr: u32, r: u32, g: u32, b: u32) {
		let lines =
			[(self.addr, addr), (self.red, r), (self.green, g), (self.blue, b), (self.reset, 0), (self.write, 1), (self.refresh, 1), (self.clock, 0)];
		drive(&mut self.sim, &lines, 3);
		let lines =
			[(self.addr, addr), (self.red, r), (self.green, g), (self.blue, b), (self.reset, 0), (self.write, 1), (self.refresh, 1), (self.clock, 1)];
		drive(&mut self.sim, &lines, 3);
	}

	fn read_rgb(&mut self, addr: u32) -> (u32, u32, u32) {
		let lines =
			[(self.addr, addr), (self.red, 0), (self.green, 0), (self.blue, 0), (self.reset, 0), (self.write, 0), (self.refresh, 0), (self.clock, 0)];
		drive(&mut self.sim, &lines, 3);
		(read(&self.sim, self.r_out) & 0b1111, read(&self.sim, self.g_out) & 0b1111, read(&self.sim, self.b_out) & 0b1111)
	}
}

#[test]
fn rgb_display_reads_back_written_colour_at_its_own_address() {
	let mut d = build_rgb_sim();
	d.write_and_refresh(99, 0xA, 0x5, 0x3);
	assert_eq!(d.read_rgb(99), (0xA, 0x5, 0x3));
}

#[test]
fn rgb_display_leaves_every_other_address_untouched_by_a_single_write() {
	let mut d = build_rgb_sim();
	d.write_and_refresh(99, 0xF, 0xF, 0xF);
	for addr in [0u32, 1, 98, 100, 255] {
		assert_eq!(d.read_rgb(addr), (0, 0, 0), "address {addr} should be unaffected by a write to 99");
	}
}

/// Same walking-single-address-bit strategy as `dot_display_address_bits_are_independently_and_correctly_decoded`,
/// adapted for RGB: each address gets a distinct, easily-distinguished colour so a swapped
/// address bit would show up as the wrong colour at the wrong location rather than just "on".
#[test]
fn rgb_display_address_bits_are_independently_and_correctly_decoded() {
	let mut d = build_rgb_sim();
	let bit_addresses: Vec<u32> = (0..8).map(|bit| 1u32 << bit).collect();
	for (i, &addr) in bit_addresses.iter().enumerate() {
		// Colour derived from the bit index so every address's expected colour is unique.
		let r = (i as u32 + 1) & 0b1111;
		d.write_and_refresh(addr, r, 0, 0);
	}
	for (i, &addr) in bit_addresses.iter().enumerate() {
		let expected_r = (i as u32 + 1) & 0b1111;
		assert_eq!(d.read_rgb(addr), (expected_r, 0, 0), "address {addr} (0b{addr:08b}) has the wrong colour");
	}
}

/// Each colour channel is packed into its own nibble of the same `u32` cell (see
/// `process_display_rgb`'s `data = red | (green << 4) | (blue << 8)`). Writing only one channel
/// must not leak into the others when read back.
#[test]
fn rgb_display_colour_channels_do_not_bleed_into_each_other() {
	let mut d = build_rgb_sim();
	d.write_and_refresh(1, 0xF, 0x0, 0x0);
	d.write_and_refresh(2, 0x0, 0xF, 0x0);
	d.write_and_refresh(3, 0x0, 0x0, 0xF);
	assert_eq!(d.read_rgb(1), (0xF, 0, 0));
	assert_eq!(d.read_rgb(2), (0, 0xF, 0));
	assert_eq!(d.read_rgb(3), (0, 0, 0xF));
}

#[test]
fn rgb_display_address_zero_and_max_are_both_individually_addressable() {
	let mut d = build_rgb_sim();
	d.write_and_refresh(0, 0x1, 0x2, 0x3);
	d.write_and_refresh(255, 0x4, 0x5, 0x6);
	assert_eq!(d.read_rgb(0), (0x1, 0x2, 0x3));
	assert_eq!(d.read_rgb(255), (0x4, 0x5, 0x6));
}

// ---- Builtin description shape ----
//
// Guards the `ADDRESS` pin's bit width specifically: `sim.rs`'s address decoding assumes an
// 8-bit (256-entry) address space (`ADDRESS_SPACE`/`ADDRESS_SIZE_8BIT` = 256 in `sim.rs`), so a
// pin layout change that narrows or widens `ADDRESS` without updating that constant would corrupt
// addressing silently rather than fail loudly -- these tests make that assumption explicit.

#[test]
fn dot_display_builtin_has_an_8bit_address_pin() {
	let chips = logic_sim::create_all_builtins();
	let dot = chips.iter().find(|c| c.chip_type == ChipType::DisplayDot).expect("DOT DISPLAY builtin should be registered");
	assert_eq!(dot.input_pins[0].name, "ADDRESS");
	assert_eq!(dot.input_pins[0].bit_count, PinBitCount::Bit8);
}

#[test]
fn rgb_display_builtin_has_an_8bit_address_pin() {
	let chips = logic_sim::create_all_builtins();
	let rgb = chips.iter().find(|c| c.chip_type == ChipType::DisplayRgb).expect("RGB DISPLAY builtin should be registered");
	assert_eq!(rgb.input_pins[0].name, "ADDRESS");
	assert_eq!(rgb.input_pins[0].bit_count, PinBitCount::Bit8);
}
