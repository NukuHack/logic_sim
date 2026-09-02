//! Runtime simulation graph + stepping logic, ported from DLS.Simulation (SimPin.cs, SimChip.cs, Simulator.cs).
//! The original C# uses plain object references, forming a graph with cross-links between pins in
//! different chips. Rust doesn't like that kind of aliased mutable graph with owned references, so
//! instead of `Rc<RefCell<..>>` everywhere this uses a flat-arena design: all pins live in one `Vec<SimPin>`
//! and all chips in one `Vec<SimChip>`, indexed rather than referenced, keeping the hot loop allocation-free.

use crate::description::{CacheKind, ChipDescription, ChipLibrary, ChipType, PinAddress};
use crate::gate_op::CachingState;
use crate::pin_state::PinState;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Captured `internal_state` per chip id-path (see
/// [`Simulator::capture_internal_states`]).
pub type InternalStateMap = HashMap<Vec<i32>, Vec<u32>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinIdx(pub usize);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChipIdx(pub usize);

#[derive(Debug)]
pub struct SimPin {
	pub id: i32,
	pub parent_chip: ChipIdx,
	pub is_input: bool,
	pub state: PinState,

	pub connected_target_pins: Vec<PinIdx>,

	/// Simulation frame index on which this pin last received an input.
	pub last_updated_frame_index: u64,
	/// Address of the pin from which this pin last received its input.
	pub latest_source_id: i32,
	pub latest_source_parent_chip_id: i32,

	/// Number of wires that feed a signal into this pin.
	pub num_input_connections: i32,
	pub num_inputs_received_this_frame: i32,
}

impl SimPin {
	fn new(id: i32, is_input: bool, parent_chip: ChipIdx) -> Self {
		Self {
			id,
			parent_chip,
			is_input,
			state: PinState::DISCONNECTED,
			connected_target_pins: Vec::new(),
			last_updated_frame_index: 0,
			latest_source_id: -1,
			latest_source_parent_chip_id: -1,
			num_input_connections: 0,
			num_inputs_received_this_frame: 0,
		}
	}
}

#[derive(Debug)]
pub struct SimChip {
	pub chip_type: ChipType,
	pub id: i32,
	/// This chip's name, as declared by its `ChipDescription` (e.g. "NAND",
	/// "7-Segment Driver", a player's custom chip name). Shared -- an
	/// `Arc<str>` clone is a refcount bump, not a fresh allocation
	pub name: Arc<str>,
	/// Some builtin chips (RAM, ROM, displays...) need internal state for
	/// memory; also used for other arbitrary chip-specific data.
	pub internal_state: Vec<u32>,
	pub is_builtin: bool,
	/// Copied from `ChipDescription::cache_kind` (the "real chip", i.e. the
	/// library/description-level source of truth) each time this `SimChip`
	/// is (re)built by `build_recursive`. Kept as a plain copy here --
	/// rather than having `gate_op::caching` reach back into a
	/// `ChipDescription`/`ChipLibrary` on every lookup
	pub cache_kind: CacheKind,

	pub input_pins: Vec<PinIdx>,
	pub output_pins: Vec<PinIdx>,
	pub sub_chips: Vec<ChipIdx>,

	pub num_connected_inputs: i32,
	pub num_inputs_ready: i32,
}

impl SimChip {
	pub fn is_ready(&self) -> bool {
		self.num_inputs_ready == self.num_connected_inputs
	}
}

/// External stimulus: values the "player"/host program is driving into
/// input dev-pins on the root chip, addressed by (owner id, pin id).
#[derive(Debug, Clone, Copy)]
pub struct ExternalInput {
	pub address: PinAddress,
	pub state: PinState,
}

/// Owns the whole simulation graph (all pins/chips across every level of
/// nesting) and knows how to step it forward one simulation frame.
pub struct Simulator {
	pins: Vec<SimPin>,
	chips: Vec<SimChip>,
	root: ChipIdx,

	pub simulation_frame: u64,
	pub steps_per_clock_transition: u32,

	needs_order_pass: bool,
	can_dynamic_reorder_this_frame: bool,

	pcg_rng_state: u32,
	/// `StdRng` rather than `ThreadRng` so a whole built `Simulator` is
	/// `Send` and can be stepped on the background sim thread (see
	/// `viewer::sim_thread`); nothing about the simulation depends on
	/// which thread's entropy pool seeded it.
	rng: StdRng,

	start_time: Instant,
	elapsed_seconds_old: f64,

	/// Key-chip state: which key characters are currently held (set by host).
	pub held_keys: HashSet<char>,
	/// KeyMods-chip state: current modifier keys, as a `key_mods_bits` bitmask
	/// (set by host).
	pub key_modifiers: u32,
	/// Player-driven states for the root chip's own input dev-pins, keyed by pin id -- what
	/// clicking a switch's bit grid toggles.
	pub driven_inputs: HashMap<i32, PinState>,

	/// Combinational-chip LUT cache + the "use caching at all" toggle --
	/// see [`crate::gate_op`]. Public so the viewer's
	/// customization checkbox (`ProjectDescription::prefs_use_caching`,
	/// applied via `viewer::sim_thread::SimHandle::set_use_caching`) can
	/// flip `caching.use_caching` directly through the shared-simulator
	/// lock, same as every other prefs-driven field on this struct.
	pub caching: CachingState,
}

impl Default for Simulator {
	fn default() -> Self {
		Self {
			pins: Vec::new(),
			chips: Vec::new(),
			root: ChipIdx::default(),

			simulation_frame: 0,
			steps_per_clock_transition: 0,

			needs_order_pass: false,
			can_dynamic_reorder_this_frame: false,

			pcg_rng_state: 0,
			rng: StdRng::from_entropy(), // or StdRng::seed_from_u64(0) for deterministic

			start_time: Instant::now(),
			elapsed_seconds_old: 0.0,

			held_keys: HashSet::new(),
			key_modifiers: 0,
			driven_inputs: HashMap::new(),

			caching: CachingState::default(),
		}
	}
}

/// Bit layout for `Simulator::key_modifiers` / the `KeyMods` builtin chip's output pin.
pub mod key_mods_bits {
	pub const SHIFT: u32 = 1 << 0;
	pub const CONTROL: u32 = 1 << 1;
	pub const ALT: u32 = 1 << 2;
	pub const SUPER: u32 = 1 << 3;
}

impl Simulator {
	/// Build a simulator whose root chip is `root_desc`, resolving any
	/// subchips against `library`.
	pub fn build(root_desc: &ChipDescription, library: &ChipLibrary) -> Self {
		let mut pins = Vec::new();
		let mut chips = Vec::new();
		let mut interner: HashMap<String, Arc<str>> = HashMap::new();
		let root = build_recursive(root_desc, library, -1, None, &mut pins, &mut chips, &mut interner);

		Simulator {
			pins,
			chips,
			root,
			simulation_frame: 0,
			steps_per_clock_transition: 30,
			needs_order_pass: true,
			can_dynamic_reorder_this_frame: false,
			pcg_rng_state: 0,
			rng: StdRng::from_entropy(),
			start_time: Instant::now(),
			elapsed_seconds_old: 0.0,
			held_keys: HashSet::new(),
			key_modifiers: 0,
			driven_inputs: HashMap::new(),
			caching: CachingState::default(),
		}
	}

	pub fn root(&self) -> ChipIdx {
		self.root
	}

	pub fn chip(&self, idx: ChipIdx) -> &SimChip {
		&self.chips[idx.0]
	}

	pub fn pin(&self, idx: PinIdx) -> &SimPin {
		&self.pins[idx.0]
	}

	/// Mutable counterpart to [`Self::pin`] -- needed by
	/// [`crate::gate_op::caching`] to drive a chip's input pins through
	/// every possible combination while sweeping a truth table, and to
	/// write a cache hit's looked-up outputs straight onto a chip's
	/// output pins.
	pub(crate) fn pin_mut(&mut self, idx: PinIdx) -> &mut SimPin {
		&mut self.pins[idx.0]
	}

	/// Mutable counterpart to [`Self::chip`]. Currently only used by tests;
	/// kept alongside `pin_mut` for symmetry and because
	/// [`crate::gate_op::caching`] is the kind of code that tends to want it.
	#[allow(dead_code)]
	pub(crate) fn chip_mut(&mut self, idx: ChipIdx) -> &mut SimChip {
		&mut self.chips[idx.0]
	}

	/// Find a direct subchip of `chip` by its saved instance id. Used by
	/// the renderer to fetch a display subchip's `internal_state` (e.g.
	/// the pixel/segment data behind a 7-segment/RGB/dot display) given
	/// only the `PlacedSubChip::id` it already has on hand. Mirrors the
	/// lookup half of `SimChip.GetSimPinFromAddress`, but for the owning
	/// chip itself rather than one of its pins.
	pub fn find_sub_chip(&self, chip: ChipIdx, id: i32) -> Option<ChipIdx> {
		let c = &self.chips[chip.0];
		c.sub_chips.iter().copied().find(|&sub| self.chips[sub.0].id == id)
	}

	/// Find a pin anywhere within `chip` (its own dev-pins, or a direct
	/// subchip's pins) by address. Mirrors SimChip.GetSimPinFromAddress.
	pub fn find_pin(&self, chip: ChipIdx, address: PinAddress) -> Option<PinIdx> {
		let c = &self.chips[chip.0];

		for &sub in &c.sub_chips {
			let s = &self.chips[sub.0];
			if s.id == address.pin_owner_id {
				for &p in s.input_pins.iter().chain(s.output_pins.iter()) {
					if self.pins[p.0].id == address.pin_id {
						return Some(p);
					}
				}
			}
		}

		c.input_pins.iter().chain(c.output_pins.iter()).find(|&&p| self.pins[p.0].id == address.pin_owner_id).copied()
	}

	// ---- Player-driven input dev-pins (see `Self::driven_inputs`) ----

	/// Flips one bit of the player-driven state for the root chip's input
	/// dev-pin `pin_id` -- what clicking that pin's per-bit grid does. An
	/// untracked pin starts from LOW, and toggling leaves it actively
	/// driven (a clicked switch is never floating).
	pub fn toggle_driven_input_bit(&mut self, pin_id: i32, bit_index: u32) {
		self.driven_inputs.entry(pin_id).or_insert(PinState::LOW).toggle_bit(bit_index);
	}

	/// Overwrites the whole player-driven state for one input dev-pin.
	pub fn set_driven_input(&mut self, pin_id: i32, state: PinState) {
		self.driven_inputs.insert(pin_id, state);
	}

	/// Drops every player-driven input so all root inputs read as LOW
	/// again -- used when the viewer switches which chip it's simulating
	/// (a toggled switch's state shouldn't outlive the simulation run it
	/// was set in).
	pub fn reset_driven_inputs(&mut self) {
		self.driven_inputs.clear();
	}

	/// Run a single simulation step: apply externally-driven input states, then propagate
	/// signals through the whole chip graph.
	pub fn run_simulation_step(&mut self, external_inputs: &[ExternalInput], audio: &mut crate::audio::SimAudio) {
		audio.init_frame();

		self.pcg_rng_state = self.rng.gen::<u32>();
		self.can_dynamic_reorder_this_frame = self.simulation_frame.is_multiple_of(100);
		self.simulation_frame += 1;

		// Step 1) copy externally-driven (player-controlled) input states in:
		// every root input dev-pin is actively driven each frame -- the
		// player's toggle where one exists, a connected LOW otherwise --
		// then any host-supplied addresses on top of that. Iterated by
		// index: this runs every tick, and cloning the pin list (or any
		// list below) would malloc per chip per tick.
		let num_root_inputs = self.chips[self.root.0].input_pins.len();
		for i in 0..num_root_inputs {
			let p = self.chips[self.root.0].input_pins[i];
			let pin_id = self.pins[p.0].id;
			let state = self.driven_inputs.get(&pin_id).copied().unwrap_or(PinState::LOW);
			self.pins[p.0].state = state;
		}
		for input in external_inputs {
			if let Some(pin_idx) = self.find_pin(self.root, input.address) {
				self.pins[pin_idx.0].state = input.state;
			}
		}

		if self.needs_order_pass {
			self.step_chip_reorder(self.root, audio);
			self.needs_order_pass = false;
		} else {
			self.step_chip(self.root, audio);
		}

		self.notify_audio_state(audio);
	}

	/// Keeps the audio mix fading toward silence while the simulation is
	/// paused (`Simulator.UpdateInPausedState`): no notes register, but the
	/// smoothing pass still runs so a sounding buzzer decays away.
	pub fn update_in_paused_state(&mut self, audio: &mut crate::audio::SimAudio) {
		audio.init_frame();
		self.notify_audio_state(audio);
	}

	fn notify_audio_state(&mut self, audio: &mut crate::audio::SimAudio) {
		let elapsed_seconds = self.start_time.elapsed().as_secs_f64();
		let delta_time = if self.simulation_frame <= 1 { 0.0 } else { elapsed_seconds - self.elapsed_seconds_old };
		self.elapsed_seconds_old = elapsed_seconds;
		audio.notify_all_notes_registered(delta_time);
	}

	/// Recursively propagate signals through this chip and its subchips.
	pub(crate) fn step_chip(&mut self, chip_idx: ChipIdx, audio: &mut crate::audio::SimAudio) {
		self.propagate_inputs(chip_idx);

		let num_sub = self.chips[chip_idx.0].sub_chips.len();

		// NOTE: subchips are assumed to be sorted in reverse order of desired visitation.
		let mut i = num_sub as isize - 1;
		while i >= 0 {
			let idx = i as usize;
			let mut next_sub_chip = self.chips[chip_idx.0].sub_chips[idx];

			if self.can_dynamic_reorder_this_frame && idx > 0 && !self.chips[next_sub_chip.0].is_ready() && self.random_bool() {
				let potential_swap = self.chips[chip_idx.0].sub_chips[idx - 1];
				if !self.chips[potential_swap.0].chip_type.is_bus_origin_type() {
					next_sub_chip = potential_swap;
					self.chips[chip_idx.0].sub_chips.swap(idx, idx - 1);
				}
			}

			self.step_sub_chip(next_sub_chip, audio);
			self.propagate_outputs(next_sub_chip);

			i -= 1;
		}
	}

	/// Evaluates one direct subchip: builtins dispatch in O(1) as always;
	/// a combinational custom chip takes the cached-LUT fast path once
	/// [`Self::caching`] has one for it (mirrors
	/// `Simulator.ProcessCachedChip`/`RecalculateCachedLUTs`); anything
	/// else falls back to a full recursive [`Self::step_chip`].
	fn step_sub_chip(&mut self, chip_idx: ChipIdx, audio: &mut crate::audio::SimAudio) {
		if self.chips[chip_idx.0].is_builtin {
			self.process_builtin_chip(chip_idx, audio);
			return;
		}

		if self.caching.use_caching {
			let name = Arc::clone(&self.chips[chip_idx.0].name);

			if self.caching.combinational_chip_cache.contains_key(name.as_ref()) {
				if crate::gate_op::process_cached_chip(self, chip_idx) {
					return;
				}
				// A tri-state input declined the lookup (never enumerated
				// into the table) -- fall through to a real step below.
			} else if !self.caching.not_combinational_chip_cache.contains(name.as_ref()) {
				crate::gate_op::recalculate_chip_cache(self, chip_idx);

				if self.caching.combinational_chip_cache.contains_key(name.as_ref()) && crate::gate_op::process_cached_chip(self, chip_idx) {
					return;
				}
			}
		}

		self.step_chip(chip_idx, audio);
	}

	/// Like `step_chip`, but also determines (and records) a good traversal
	/// order for the subchips as it goes, swapping them into place. Needed
	/// once after any structural edit to the graph.
	fn step_chip_reorder(&mut self, chip_idx: ChipIdx, audio: &mut crate::audio::SimAudio) {
		self.propagate_inputs(chip_idx);

		let mut num_remaining = self.chips[chip_idx.0].sub_chips.len();

		while num_remaining > 0 {
			let next_idx = self.choose_next_sub_chip(chip_idx, num_remaining);
			let next_sub_chip = self.chips[chip_idx.0].sub_chips[next_idx];

			self.chips[chip_idx.0].sub_chips.swap(next_idx, num_remaining - 1);
			num_remaining -= 1;

			if self.chips[next_sub_chip.0].chip_type == ChipType::Custom {
				self.step_chip_reorder(next_sub_chip, audio);
			} else {
				self.process_builtin_chip(next_sub_chip, audio);
			}

			self.propagate_outputs(next_sub_chip);
		}
	}

	fn choose_next_sub_chip(&mut self, chip_idx: ChipIdx, num: usize) -> usize {
		let sub_chips = &self.chips[chip_idx.0].sub_chips;
		let mut no_sub_chips_ready = true;
		let mut is_non_bus_chip_remaining = false;
		let mut next_index = 0usize;

		for (i, sub) in sub_chips.iter().enumerate().take(num) {
			if self.chips[sub.0].is_ready() {
				no_sub_chips_ready = false;
				next_index = i;
				break;
			}
			is_non_bus_chip_remaining |= !self.chips[sub.0].chip_type.is_bus_origin_type();
		}

		if no_sub_chips_ready {
			next_index = (self.rng.gen::<u32>() as usize) % num;

			if is_non_bus_chip_remaining {
				for _ in 0..num {
					let sub_chips = &self.chips[chip_idx.0].sub_chips;
					if !self.chips[sub_chips[next_index].0].chip_type.is_bus_origin_type() {
						break;
					}
					next_index = (next_index + 1) % num;
				}
			}
		}

		next_index
	}

	fn propagate_inputs(&mut self, chip_idx: ChipIdx) {
		// Index iteration everywhere below: these run once per chip per
		// tick, and cloning each pin list to appease the borrow checker
		// used to make stepping malloc-bound (the single biggest reason
		// the port stepped slower than the original).
		let num_inputs = self.chips[chip_idx.0].input_pins.len();
		for i in 0..num_inputs {
			let p = self.chips[chip_idx.0].input_pins[i];
			self.propagate_signal(p);
		}
	}

	fn propagate_outputs(&mut self, chip_idx: ChipIdx) {
		let num_outputs = self.chips[chip_idx.0].output_pins.len();
		for i in 0..num_outputs {
			let p = self.chips[chip_idx.0].output_pins[i];
			self.propagate_signal(p);
		}
		self.chips[chip_idx.0].num_inputs_ready = 0;
	}

	fn propagate_signal(&mut self, source: PinIdx) {
		// The innermost hot loop of the whole simulation -- one pass per
		// propagated pin per tick. Cloning the target list here was one
		// heap alloc per wire crossing per chip per tick.
		let num_targets = self.pins[source.0].connected_target_pins.len();
		for i in 0..num_targets {
			let target = self.pins[source.0].connected_target_pins[i];
			self.receive_input(target, source);
		}
	}

	/// Called on sub-chip input pins, or chip dev-pins, when a connected
	/// source pin propagates its signal to them.
	fn receive_input(&mut self, target: PinIdx, source: PinIdx) {
		let frame = self.simulation_frame;
		// The source side is read-only here: snapshot it once up front so
		// the writes below never re-walk `pins[source]`/`chips[..]` (and
		// so a pin wired to itself still sees its pre-call state).
		let source_state = self.pins[source.0].state;
		let source_id = self.pins[source.0].id;
		let source_parent_chip_id = self.chips[self.pins[source.0].parent_chip.0].id;

		if self.pins[target.0].last_updated_frame_index != frame {
			self.pins[target.0].last_updated_frame_index = frame;
			self.pins[target.0].num_inputs_received_this_frame = 0;
		}

		let set;
		if self.pins[target.0].num_inputs_received_this_frame > 0 {
			// Already received input this frame: choose randomly whether to
			// accept the conflicting input (same choice for all bits).
			let cur_state = self.pins[target.0].state;
			let or = source_state.or(cur_state);
			let and = source_state.and(cur_state);
			let bits_new = if self.random_bool() { or.bit_states() } else { and.bit_states() };

			let mask = or.tristate_flags(); // any wire disconnected on either side
			let bits_new = (bits_new & !mask) | (or.bit_states() & mask);

			let state_new = PinState::from_parts(bits_new, and.tristate_flags());

			set = state_new != cur_state;
			self.pins[target.0].state = state_new;
		} else {
			self.pins[target.0].state = source_state;
			set = true;
		}

		if set {
			let t = &mut self.pins[target.0];
			t.latest_source_id = source_id;
			t.latest_source_parent_chip_id = source_parent_chip_id;
		}

		self.pins[target.0].num_inputs_received_this_frame += 1;

		let target_pin = &self.pins[target.0];
		if target_pin.is_input && target_pin.num_inputs_received_this_frame == target_pin.num_input_connections {
			let parent = target_pin.parent_chip;
			self.chips[parent.0].num_inputs_ready += 1;
		}
	}

	/// PCG-based pseudo-random bool, matching the original's algorithm so
	/// race-condition resolution has the same statistical behaviour.
	fn random_bool(&mut self) -> bool {
		self.pcg_rng_state = self.pcg_rng_state.wrapping_mul(747796405).wrapping_add(2891336453);
		let state = self.pcg_rng_state;
		let mut result = ((state >> ((state >> 28).wrapping_add(4))) ^ state).wrapping_mul(277803737);
		result = (result >> 22) ^ result;
		result < u32::MAX / 2
	}

	pub fn add_connection(&mut self, chip_idx: ChipIdx, source: PinAddress, target: PinAddress) {
		let (Some(source_pin), Some(target_pin)) = (self.find_pin(chip_idx, source), self.find_pin(chip_idx, target)) else {
			// Mirrors the original: silently ignore if a pin can't be found
			// (e.g. stale saved chip referencing a since-removed pin).
			return;
		};

		self.pins[source_pin.0].connected_target_pins.push(target_pin);
		self.pins[target_pin.0].num_input_connections += 1;

		if self.pins[target_pin.0].num_input_connections == 1 {
			// Find owning chip of target pin among subchips of chip_idx (if any)
			for &sub in &self.chips[chip_idx.0].sub_chips {
				if self.chips[sub.0].id == target.pin_owner_id {
					self.chips[sub.0].num_connected_inputs += 1;
					break;
				}
			}
		}

		self.needs_order_pass = true;
	}

	fn process_builtin_chip(&mut self, chip_idx: ChipIdx, audio: &mut crate::audio::SimAudio) {
		use ChipType as E;
		let chip_type = self.chips[chip_idx.0].chip_type;

		macro_rules! in_state {
			($i:expr) => {
				self.pins[self.chips[chip_idx.0].input_pins[$i].0].state
			};
		}
		macro_rules! set_out {
			($i:expr, $v:expr) => {{
				let p = self.chips[chip_idx.0].output_pins[$i];
				self.pins[p.0].state = $v;
			}};
		}

		match chip_type {
			E::Nand => {
				let a = in_state!(0).first_bit_high();
				let b = in_state!(1).first_bit_high();
				set_out!(0, PinState::from_bool(!(a && b)));
			}
			E::Clock => {
				let spct = self.steps_per_clock_transition;
				let high = spct != 0 && ((self.simulation_frame / spct as u64) & 1) == 0;
				set_out!(0, PinState::from_bool(high));
			}
			E::Pulse => {
				const DURATION: usize = 0;
				const TICKS_REMAINING: usize = 1;
				const INPUT_OLD: usize = 2;

				let input_state = in_state!(0);
				let pulse_input_high = input_state.first_bit_high();
				let mut ticks_remaining = self.chips[chip_idx.0].internal_state[TICKS_REMAINING];

				if ticks_remaining == 0 {
					let is_rising_edge = pulse_input_high && self.chips[chip_idx.0].internal_state[INPUT_OLD] == 0;
					if is_rising_edge {
						ticks_remaining = self.chips[chip_idx.0].internal_state[DURATION];
						self.chips[chip_idx.0].internal_state[TICKS_REMAINING] = ticks_remaining;
					}
				}

				let mut output_state = PinState::LOW;
				if ticks_remaining > 0 {
					self.chips[chip_idx.0].internal_state[TICKS_REMAINING] -= 1;
					output_state = PinState::HIGH;
				} else if input_state.tristate_flags() != 0 {
					output_state = PinState::OFF;
				}

				set_out!(0, output_state);
				self.chips[chip_idx.0].internal_state[INPUT_OLD] = pulse_input_high as u32;
			}
			E::Split4To1Bit => {
				let in4 = in_state!(0);
				set_out!(0, in4.extract(3, 1));
				set_out!(1, in4.extract(2, 1));
				set_out!(2, in4.extract(1, 1));
				set_out!(3, in4.extract(0, 1));
			}
			E::Merge1To4Bit => {
				set_out!(0, PinState::combine(&[(in_state!(3), 0, 1), (in_state!(2), 1, 1), (in_state!(1), 2, 1), (in_state!(0), 3, 1)]));
			}
			E::Merge1To8Bit => {
				set_out!(
					0,
					PinState::combine(&[
						(in_state!(7), 0, 1),
						(in_state!(6), 1, 1),
						(in_state!(5), 2, 1),
						(in_state!(4), 3, 1),
						(in_state!(3), 4, 1),
						(in_state!(2), 5, 1),
						(in_state!(1), 6, 1),
						(in_state!(0), 7, 1),
					])
				);
			}
			E::Merge4To8Bit => {
				let a4 = in_state!(0);
				let b4 = in_state!(1);
				set_out!(0, PinState::combine(&[(b4, 0, 4), (a4, 4, 4)]));
			}
			E::Split8To4Bit => {
				let in8 = in_state!(0);
				set_out!(0, in8.extract(4, 4)); // upper nibble
				set_out!(1, in8.extract(0, 4)); // lower nibble
			}
			E::Split8To1Bit => {
				let in8 = in_state!(0);
				for bit in 0..8 {
					set_out!(bit, in8.extract(7 - bit as u32, 1));
				}
			}
			E::TriStateBuffer => {
				let data = in_state!(0);
				let enable = in_state!(1).first_bit_high();
				set_out!(0, if enable { data } else { PinState::DISCONNECTED });
			}
			E::Key => {
				let key_char = self.chips[chip_idx.0].internal_state.first().copied().unwrap_or(0) as u8 as char;
				let is_held = self.held_keys.contains(&key_char);
				set_out!(0, PinState::from_bool(is_held));
			}
			E::KeyMods => {
				set_out!(0, PinState::from_raw((self.key_modifiers & 0xFF) as u16));
			}
			E::DisplayRgb => self.process_display_rgb(chip_idx),
			E::DisplayDot => self.process_display_dot(chip_idx),
			E::DevRam8Bit => self.process_ram_8bit(chip_idx),
			E::Rom256x16 => {
				const BYTE_MASK: u32 = 0b1111_1111;
				let address = in_state!(0).bit_states() as usize;
				let data = self.chips[chip_idx.0].internal_state[address];
				set_out!(0, PinState::from_raw(((data >> 8) & BYTE_MASK) as u16));
				set_out!(1, PinState::from_raw((data & BYTE_MASK) as u16));
			}
			E::Buzzer => {
				let freq_index = in_state!(0).bit_states() as i32;
				let volume_index = in_state!(1).bit_states() as u32;
				audio.register_note(freq_index, volume_index);
			}
			_ => {
				if chip_type.is_bus_origin_type() {
					let input = in_state!(0);
					set_out!(0, input);
				}
			}
		}
	}

	fn process_ram_8bit(&mut self, chip_idx: ChipIdx) {
		macro_rules! in_state {
			($i:expr) => {
				self.pins[self.chips[chip_idx.0].input_pins[$i].0].state
			};
		}
		let address_pin = in_state!(0);
		let data_pin = in_state!(1);
		let write_enable_pin = in_state!(2);
		let reset_pin = in_state!(3);
		let clock_pin = in_state!(4);

		let internal = &mut self.chips[chip_idx.0].internal_state;
		let last = internal.len() - 1;
		let clock_high = clock_pin.first_bit_high();
		let is_rising_edge = clock_high && internal[last] == 0;
		internal[last] = clock_high as u32;

		if is_rising_edge {
			if reset_pin.first_bit_high() {
				internal.iter_mut().take(256).for_each(|x| *x = 0);
			} else if write_enable_pin.first_bit_high() {
				let addr = address_pin.bit_states() as usize;
				internal[addr] = data_pin.bit_states() as u32;
			}
		}

		let addr = address_pin.bit_states() as usize;
		let out_pin = self.chips[chip_idx.0].output_pins[0];
		// RAM cells start as full random u32 words (see `build_internal_state`),
		// but the data pin is 8 bits wide: drive its low byte, fully connected.
		self.pins[out_pin.0].state = PinState::from_parts(self.chips[chip_idx.0].internal_state[addr] as u8, 0);
	}

	fn process_display_rgb(&mut self, chip_idx: ChipIdx) {
		const ADDRESS_SPACE: usize = 256;
		macro_rules! in_state {
			($i:expr) => {
				self.pins[self.chips[chip_idx.0].input_pins[$i].0].state
			};
		}
		let address_pin = in_state!(0);
		let red_pin = in_state!(1);
		let green_pin = in_state!(2);
		let blue_pin = in_state!(3);
		let reset_pin = in_state!(4);
		let write_pin = in_state!(5);
		let refresh_pin = in_state!(6);
		let clock_pin = in_state!(7);

		let internal = &mut self.chips[chip_idx.0].internal_state;
		let last = internal.len() - 1;
		let clock_high = clock_pin.first_bit_high();
		let is_rising_edge = clock_high && internal[last] == 0;
		internal[last] = clock_high as u32;

		if is_rising_edge {
			if reset_pin.first_bit_high() {
				for i in 0..ADDRESS_SPACE {
					internal[i + ADDRESS_SPACE] = 0;
				}
			} else if write_pin.first_bit_high() {
				let addr = address_pin.bit_states() as usize + ADDRESS_SPACE;
				let data = red_pin.bit_states() as u32 | ((green_pin.bit_states() as u32) << 4) | ((blue_pin.bit_states() as u32) << 8);
				internal[addr] = data;
			}

			if refresh_pin.first_bit_high() {
				for i in 0..ADDRESS_SPACE {
					internal[i] = internal[i + ADDRESS_SPACE];
				}
			}
		}

		let col_data = self.chips[chip_idx.0].internal_state[address_pin.bit_states() as usize];
		macro_rules! set_out {
			($i:expr, $v:expr) => {{
				let p = self.chips[chip_idx.0].output_pins[$i];
				self.pins[p.0].state = PinState::from_raw(($v) as u16);
			}};
		}
		set_out!(0, (col_data) & 0b1111);
		set_out!(1, (col_data >> 4) & 0b1111);
		set_out!(2, (col_data >> 8) & 0b1111);
	}

	fn process_display_dot(&mut self, chip_idx: ChipIdx) {
		const ADDRESS_SPACE: usize = 256;
		macro_rules! in_state {
			($i:expr) => {
				self.pins[self.chips[chip_idx.0].input_pins[$i].0].state
			};
		}
		let address_pin = in_state!(0);
		let pixel_input_pin = in_state!(1);
		let reset_pin = in_state!(2);
		let write_pin = in_state!(3);
		let refresh_pin = in_state!(4);
		let clock_pin = in_state!(5);

		let internal = &mut self.chips[chip_idx.0].internal_state;
		let last = internal.len() - 1;
		let clock_high = clock_pin.first_bit_high();
		let is_rising_edge = clock_high && internal[last] == 0;
		internal[last] = clock_high as u32;

		if is_rising_edge {
			if reset_pin.first_bit_high() {
				for i in 0..ADDRESS_SPACE {
					internal[i + ADDRESS_SPACE] = 0;
				}
			} else if write_pin.first_bit_high() {
				let addr = address_pin.bit_states() as usize + ADDRESS_SPACE;
				internal[addr] = pixel_input_pin.bit_states() as u32;
			}

			if refresh_pin.first_bit_high() {
				for i in 0..ADDRESS_SPACE {
					internal[i] = internal[i + ADDRESS_SPACE];
				}
			}
		}

		let pixel_state = self.chips[chip_idx.0].internal_state[address_pin.bit_states() as usize] as u8;
		let out_pin = self.chips[chip_idx.0].output_pins[0];
		self.pins[out_pin.0].state = PinState::from_parts(pixel_state, 0);
	}

	/// Captures every `SimChip`'s `internal_state`, keyed by its id-path from the root (root's
	/// own id first, then each nested subchip id). This is the volatile runtime memory --
	/// RAM/ROM contents, pulse countdowns, display buffers, clock phases -- that a rebuild would
	/// otherwise reset (`build_internal_state`'s defaults), and what `ViewerState::rebuild_sim`
	/// carries across so editing one wire no longer wipes unrelated chips' memory.
	pub fn capture_internal_states(&self) -> InternalStateMap {
		let mut map = InternalStateMap::default();
		self.capture_internal_states_at(self.root, &[], &mut map);
		map
	}

	fn capture_internal_states_at(&self, idx: ChipIdx, path: &[i32], map: &mut InternalStateMap) {
		let chip = &self.chips[idx.0];
		let mut key = Vec::with_capacity(path.len() + 1);
		key.extend_from_slice(path);
		key.push(chip.id);
		map.insert(key.clone(), chip.internal_state.clone());
		for &sub in &chip.sub_chips {
			self.capture_internal_states_at(sub, &key, map);
		}
	}

	/// Restores previously captured internal states onto the matching
	/// (same id-path) chips of this simulator. Element-wise up to the
	/// shorter length, so a state whose shape legitimately changed can
	/// never break a builtin's indexing invariants; unmatched chips keep
	/// their freshly-built defaults (a brand-new dev RAM still starts
	/// randomized, exactly like the original's fresh placements).
	pub fn restore_internal_states(&mut self, map: &InternalStateMap) {
		self.restore_internal_states_at(self.root, &[], map);
	}

	fn restore_internal_states_at(&mut self, idx: ChipIdx, path: &[i32], map: &InternalStateMap) {
		let mut key = Vec::with_capacity(path.len() + 1);
		key.extend_from_slice(path);
		key.push(self.chips[idx.0].id);
		let chip_type = self.chips[idx.0].chip_type;
		// How many leading slots of this chip's `internal_state` are pure
		// config (written only by the player through an editor popup, never
		// by `process_builtin_chip` at runtime) versus genuine volatile
		// memory a rebuild should carry over (RAM/ROM cell contents, pulse
		// countdowns, display buffers
		let skip_leading = match chip_type {
			// `[0]` is the bound key char (`E::Key` only ever reads it).
			ChipType::Key => usize::MAX,
			// `[0]` is the palette index rendering reads each frame;
			// nothing in `sim.rs` processes `DisplayLed` at all.
			ChipType::DisplayLed => usize::MAX,
			// All 256 words are ROM contents -- `E::Rom256x16` only ever
			// reads `internal_state`, it's never written back at runtime.
			ChipType::Rom256x16 => usize::MAX,
			// `[0]` is the linked partner's instance id, `[1]` is the
			// flip flag -- inert as far as the sim is concerned (bus
			// routing is baked into wire topology at `Simulator::build`
			// time, and nothing in `process_builtin_chip` reads either
			// slot for a bus chip), but excluded for the same reason as
			// the config-only types above rather than relying on that.
			t if t.is_bus_type() => usize::MAX,
			// `[0]` is the configured pulse length (ticks); `[1]`
			// (ticks remaining) and `[2]` (last sampled input edge) are
			// genuine runtime state `process_builtin_chip`'s `Pulse` arm
			// both reads and writes, so those still carry over normally.
			ChipType::Pulse => 1,
			_ => 0,
		};
		if skip_leading != usize::MAX {
			if let Some(saved) = map.get(&key) {
				let state = &mut self.chips[idx.0].internal_state;
				for (slot, &value) in state.iter_mut().zip(saved).skip(skip_leading) {
					*slot = value;
				}
			}
		}
		let mut i = 0;
		while i < self.chips[idx.0].sub_chips.len() {
			let sub = self.chips[idx.0].sub_chips[i];
			self.restore_internal_states_at(sub, &key, map);
			i += 1;
		}
	}

	/// Snapshots every pin's live state (`SimPin::state`) keyed by its
	/// owner-chip id-path + pin id + is_input flag. Paired with
	/// [`restore_pin_states`] to carry signal levels across a rebuild so
	/// the renderer doesn't see a frame of DISCONNECTED defaults.
	pub fn capture_pin_states(&self) -> PinStateMap {
		let mut map = PinStateMap::default();
		self.capture_pin_states_at(self.root, &[], &mut map);
		map
	}

	fn capture_pin_states_at(&self, idx: ChipIdx, path: &[i32], map: &mut PinStateMap) {
		let chip = &self.chips[idx.0];
		let mut key = Vec::with_capacity(path.len() + 1);
		key.extend_from_slice(path);
		key.push(chip.id);
		for &p in chip.input_pins.iter().chain(chip.output_pins.iter()) {
			let pin = &self.pins[p.0];
			let mut pin_key = key.clone();
			pin_key.push(pin.id);
			pin_key.push(pin.is_input as i32);
			map.insert(pin_key, pin.state);
		}
		for &sub in &chip.sub_chips {
			self.capture_pin_states_at(sub, &key, map);
		}
	}

	/// Restores previously captured pin states onto the matching pins of
	/// this simulator. Unmatched pins keep their freshly-built defaults.
	pub fn restore_pin_states(&mut self, map: &PinStateMap) {
		self.restore_pin_states_at(self.root, &[], map);
	}

	fn restore_pin_states_at(&mut self, idx: ChipIdx, path: &[i32], map: &PinStateMap) {
		let chip = &self.chips[idx.0];
		let mut key = Vec::with_capacity(path.len() + 1);
		key.extend_from_slice(path);
		key.push(chip.id);
		for &p in chip.input_pins.iter().chain(chip.output_pins.iter()) {
			let pin = &self.pins[p.0];
			// Only restore an input pin's state if it still has at least one
			// wire connection in the rebuilt simulation.  A wire deletion
			// removes the connection but the old state was captured before the
			// rebuild; blindly restoring it would leave the pin showing a
			// stale value instead of DISCONNECTED.
			if pin.is_input && pin.num_input_connections == 0 {
				continue;
			}
			let mut pin_key = key.clone();
			pin_key.push(pin.id);
			pin_key.push(pin.is_input as i32);
			if let Some(&saved) = map.get(&pin_key) {
				self.pins[p.0].state = saved;
			}
		}
		let sub_chips = self.chips[idx.0].sub_chips.clone();
		for sub in sub_chips {
			self.restore_pin_states_at(sub, &key, map);
		}
	}
}

/// Pin state snapshot keyed by (owner-chip id-path, pin id, is_input).
/// Used by `rebuild_sim` to carry live wire/signal states across rebuilds
/// so the renderer doesn't see a frame of DISCONNECTED defaults.
pub type PinStateMap = HashMap<Vec<i32>, PinState>;

/// Recursively build the flat pin/chip arenas from a ChipDescription tree.
fn build_recursive(
	desc: &ChipDescription,
	library: &ChipLibrary,
	sub_chip_id: i32,
	internal_state_override: Option<&[u32]>,
	pins: &mut Vec<SimPin>,
	chips: &mut Vec<SimChip>,
	interner: &mut HashMap<String, Arc<str>>,
) -> ChipIdx {
	// Recursively create subchips first.
	let mut sub_chip_indices = Vec::with_capacity(desc.sub_chips.len());
	for sub_desc in &desc.sub_chips {
		let full_desc = library.get(&sub_desc.name);
		let idx = build_recursive(full_desc, library, sub_desc.id, sub_desc.internal_data.as_deref(), pins, chips, interner);
		sub_chip_indices.push(idx);
	}

	let is_builtin = desc.chip_type != ChipType::Custom;

	let internal_state = build_internal_state(desc.chip_type, internal_state_override);
	let name = intern_name(interner, &desc.name);

	// Reserve the chip's own slot first so pins can point back to it.
	let chip_idx = ChipIdx(chips.len());
	chips.push(SimChip {
		chip_type: desc.chip_type,
		id: sub_chip_id,
		name,
		internal_state,
		is_builtin,
		input_pins: Vec::new(),
		output_pins: Vec::new(),
		cache_kind: desc.cache_kind,
		sub_chips: sub_chip_indices,
		num_connected_inputs: 0,
		num_inputs_ready: 0,
	});

	let mut input_pins = Vec::with_capacity(desc.input_pins.len());
	for p in &desc.input_pins {
		let idx = PinIdx(pins.len());
		pins.push(SimPin::new(p.id, true, chip_idx));
		input_pins.push(idx);
	}

	let mut output_pins = Vec::with_capacity(desc.output_pins.len());
	for p in &desc.output_pins {
		let idx = PinIdx(pins.len());
		pins.push(SimPin::new(p.id, false, chip_idx));
		output_pins.push(idx);
	}

	chips[chip_idx.0].input_pins = input_pins;
	chips[chip_idx.0].output_pins = output_pins;

	// Wire up connections declared on this (custom) chip.
	for wire in &desc.wires {
		connect(chip_idx, wire.source_pin_address, wire.target_pin_address, pins, chips);
	}

	chip_idx
}

/// Shares one `Arc<str>`
fn intern_name(interner: &mut HashMap<String, Arc<str>>, name: &str) -> Arc<str> {
	if let Some(existing) = interner.get(name) {
		return Arc::clone(existing);
	}
	let arc: Arc<str> = Arc::from(name);
	interner.insert(name.to_string(), Arc::clone(&arc));
	arc
}

fn connect(chip_idx: ChipIdx, source: PinAddress, target: PinAddress, pins: &mut [SimPin], chips: &mut [SimChip]) {
	let find = |addr: PinAddress, pins: &[SimPin], chips: &[SimChip]| -> Option<PinIdx> {
		let c = &chips[chip_idx.0];
		for &sub in &c.sub_chips {
			let s = &chips[sub.0];
			if s.id == addr.pin_owner_id {
				for &p in s.input_pins.iter().chain(s.output_pins.iter()) {
					if pins[p.0].id == addr.pin_id {
						return Some(p);
					}
				}
			}
		}
		c.input_pins.iter().chain(c.output_pins.iter()).find(|&&p| pins[p.0].id == addr.pin_owner_id).copied()
	};

	let (Some(source_pin), Some(target_pin)) = (find(source, pins, chips), find(target, pins, chips)) else {
		return; // stale wire referencing a removed pin; ignore, as original does
	};

	pins[source_pin.0].connected_target_pins.push(target_pin);
	pins[target_pin.0].num_input_connections += 1;

	if pins[target_pin.0].num_input_connections == 1 {
		for &sub in &chips[chip_idx.0].sub_chips {
			if chips[sub.0].id == target.pin_owner_id {
				chips[sub.0].num_connected_inputs += 1;
				break;
			}
		}
	}
}

fn build_internal_state(chip_type: ChipType, override_state: Option<&[u32]>) -> Vec<u32> {
	const ADDRESS_SIZE_8BIT: usize = 256;

	match chip_type {
		ChipType::DisplayRgb | ChipType::DisplayDot => vec![0u32; ADDRESS_SIZE_8BIT * 2 + 1],
		ChipType::DevRam8Bit => {
			let mut state = vec![0u32; ADDRESS_SIZE_8BIT + 1]; // +1 for clock edge state
			let mut rng = rand::thread_rng();
			use rand::RngCore;
			for slot in state.iter_mut().take(ADDRESS_SIZE_8BIT) {
				*slot = rng.next_u32();
			}
			state
		}
		// Indexed directly by the live 0..256 address bus with no bounds check (see
		// `process_builtin_chip`'s `Rom256x16` arm), so -- like the display/RAM types above -- it
		// always needs the full `ADDRESS_SIZE_8BIT` words, regardless of whether (or how much of)
		// `override_state` was actually saved. Missing words default to 0, same as an unwritten
		// RAM/display cell would if it started zeroed instead of random.
		ChipType::Rom256x16 => {
			let mut state = vec![0u32; ADDRESS_SIZE_8BIT];
			if let Some(saved) = override_state {
				for (slot, &v) in state.iter_mut().zip(saved) {
					*slot = v;
				}
			}
			state
		}
		// `[duration, ticks_remaining, input_old]`, all three indexed unconditionally by
		// `process_builtin_chip`'s `Pulse` arm -- a shorter (or absent) `override_state` needs
		// padding out to that length rather than being used as-is. 50 ticks matches the
		// original's fresh-placement default (`DescriptionCreator.CreateDefaultInstanceData`).
		ChipType::Pulse => {
			const DEFAULT: [u32; 3] = [50, 0, 0];
			match override_state {
				Some(saved) if saved.len() >= DEFAULT.len() => saved.to_vec(),
				Some(saved) => {
					let mut state = DEFAULT.to_vec();
					state[..saved.len()].copy_from_slice(saved);
					state
				}
				None => DEFAULT.to_vec(),
			}
		}
		_ => match override_state {
			Some(s) if !s.is_empty() => s.to_vec(),
			_ => Vec::new(),
		},
	}
}

#[cfg(test)]
mod internal_state_carry_tests {
	//! White-box: the capture/restore contract lives on `Simulator`'s
	//! private arena (`pins`/`chips`), so it can only be driven from
	//! beside the code -- an integration test would have no way in.

	use super::*;
	use crate::description::SubChipDescription;

	fn simulator_with_ram() -> Simulator {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		let mut root = ChipDescription::new("ROOT", ChipType::Custom);
		root.sub_chips.push(SubChipDescription {
			name: "dev.RAM-8".into(),
			id: 5,
			internal_data: None,
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});
		// A nested chip holding its own RAM, so path-keyed matching has a
		// two-level case to prove.
		let mut inner = ChipDescription::new("INNER", ChipType::Custom);
		inner.sub_chips.push(SubChipDescription {
			name: "dev.RAM-8".into(),
			id: 6,
			internal_data: None,
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});
		library.add(inner);
		root.sub_chips.push(SubChipDescription {
			name: "INNER".into(),
			id: 7,
			internal_data: None,
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});
		Simulator::build(&root, &library)
	}

	fn ram_word(sim: &Simulator, id: i32, address: usize) -> Option<u32> {
		for idx in 0..sim.chips.len() {
			if sim.chips[idx].id == id && sim.chips[idx].chip_type == ChipType::DevRam8Bit {
				return sim.chips[idx].internal_state.get(address).copied();
			}
		}
		None
	}

	fn set_ram_word(sim: &mut Simulator, id: i32, address: usize, value: u32) {
		for idx in 0..sim.chips.len() {
			if sim.chips[idx].id == id && sim.chips[idx].chip_type == ChipType::DevRam8Bit {
				sim.chips[idx].internal_state[address] = value;
				return;
			}
		}
		panic!("no dev RAM with id {id}");
	}

	#[test]
	fn captured_states_restore_onto_matching_id_paths_after_a_rebuild() {
		let mut first = simulator_with_ram();
		set_ram_word(&mut first, 5, 7, 0xBEEF);
		set_ram_word(&mut first, 6, 3, 0xF00D);

		let captured = first.capture_internal_states();

		// "Rebuild": same description, brand-new arena (fresh random RAM).
		let mut second = simulator_with_ram();
		assert_ne!(ram_word(&second, 5, 7), Some(0xBEEF), "precondition: the rebuild starts from scratch");
		second.restore_internal_states(&captured);

		assert_eq!(ram_word(&second, 5, 7), Some(0xBEEF), "the top-level RAM's contents carried over");
		assert_eq!(ram_word(&second, 6, 3), Some(0xF00D), "the nested RAM's contents carried over too");
	}

	fn key_chip_state<'a>(sim: &'a Simulator, id: i32) -> &'a [u32] {
		for idx in 0..sim.chips.len() {
			if sim.chips[idx].id == id && sim.chips[idx].chip_type == ChipType::Key {
				return &sim.chips[idx].internal_state;
			}
		}
		panic!("no Key chip with id {id}");
	}

	/// Regression test: rebinding a Key chip through the key-select popup (which edits
	/// `SubChipDescription::internal_data` then calls `ViewerState::rebuild_sim`, i.e. capture-
	/// old-states -> build-from-new- description -> restore-old-states) must not have that
	/// restore step silently undo the rebind by copying the *old* bound char back over the
	/// freshly-built chip's `internal_state`.
	#[test]
	fn rebuilding_after_a_key_chip_rebind_does_not_revert_to_the_old_key() {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		let mut root = ChipDescription::new("ROOT", ChipType::Custom);
		root.sub_chips.push(SubChipDescription {
			name: "Key".into(),
			id: 1,
			internal_data: Some(vec!['A' as u32]),
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});

		let first = Simulator::build(&root, &library);
		assert_eq!(key_chip_state(&first, 1), &['A' as u32], "precondition: built bound to 'A'");
		let captured = first.capture_internal_states();

		// The player rebinds it to '5' through the popup: the *description*
		// changes, then a rebuild carries the old capture across -- exactly
		// `ViewerState::rebuild_sim`'s sequence.
		root.sub_chips[0].internal_data = Some(vec!['5' as u32]);
		let mut second = Simulator::build(&root, &library);
		assert_eq!(key_chip_state(&second, 1), &['5' as u32], "precondition: rebuild picks up the new binding");
		second.restore_internal_states(&captured);

		assert_eq!(key_chip_state(&second, 1), &['5' as u32], "the rebind must survive the restore, not revert to 'A'");
	}

	/// A chip that only exists on one side (added/removed between builds)
	/// simply finds nothing to restore onto; restoring never panics or
	/// writes past a state buffer.
	#[test]
	fn restore_tolerates_structure_changes_and_length_mismatches() {
		let mut library = ChipLibrary::new();
		crate::register_all_builtins(&mut library);
		let mut root = ChipDescription::new("ROOT", ChipType::Custom);
		root.sub_chips.push(SubChipDescription {
			name: "dev.RAM-8".into(),
			id: 9,
			internal_data: None,
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});
		let mut first = Simulator::build(&root, &library);
		set_ram_word(&mut first, 9, 0, 42);
		let captured = first.capture_internal_states();

		// The rebuilt version wires something else entirely at id 9's slot.
		root.sub_chips.clear();
		root.sub_chips.push(SubChipDescription {
			name: "NAND".into(),
			id: 9,
			internal_data: None,
			position: Default::default(),
			label: None,
			pin_colour_info: Vec::new(),
		});
		let mut second = Simulator::build(&root, &library);
		second.restore_internal_states(&captured);
	}
}
