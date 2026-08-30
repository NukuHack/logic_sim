//! Combinational-chip caching, ported from `DLS.Simulation.Simulator`
//! (`RecalculateCachedLUTs`, `ProcessCachedChip`) and `DLS.Simulation.SimChip`
//! (`IsCombinational`, `CalculateNumberOfInputBits`, `ResetReceivedFlagsOnAllPins`).
//!
//! Without this, `Simulator::step_chip` always walks the full subchip graph
//! every tick, even for chips that are purely combinational (output depends
//! only on the current input, nothing sequential). The idea is to build each
//! such chip's truth table once and index into it afterwards instead of
//! re-walking its subchip graph every single simulation frame.
//!
//! The actual hook-in lives in `Simulator::step_sub_chip` (in `sim.rs`):
//! for each non-builtin subchip it checks `caching.use_caching`, then either
//! looks the chip up in the LUT cache (`process_cached_chip`), tries to
//! build one for it (`recalculate_cached_luts`), or -- for anything already
//! known not to qualify -- falls straight through to a real recursive
//! `step_chip`.

use crate::pin_state::PinState;
use crate::sim::{ChipIdx, PinIdx, Simulator};
use std::collections::{HashMap, HashSet};

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING`: a combinational chip at or
/// under this many input bits is always cached (2^12 rows is cheap enough to
/// just always build).
pub const MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING: u32 = 12;

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`: above the auto-cache limit,
/// caching is opt-in (`SimChip::should_be_cached`, mirroring
/// `ChipDescription::should_be_cached`) up to this many bits (2^24 rows);
/// beyond it, a chip is never cached.
pub const MAX_NUM_INPUT_BITS_WHEN_USER_CACHING: u32 = 24;

/// One row per input combination, keyed by chip name. Matches the original's
/// `Dictionary<string, uint[][]> combinationalChipCaches`. Each row is the
/// chip's raw packed `PinState` word (bits + tri-state flags) per output
/// pin, in output-pin order -- not just the bit values -- so a cache hit can
/// reproduce a tri-state output exactly, not just a settled 0/1.
///
/// Keying by name means every instance of the same custom chip shares one LUT
/// -- correct only because the LUT is a pure function of the chip's own wiring,
/// independent of where an instance sits in the outer graph.
pub type CombinationalChipCache = HashMap<String, Vec<Vec<u32>>>;

/// Chip names already proven non-combinational (or too big to cache), so
/// `recalculate_cached_luts` doesn't re-derive the same answer every call.
pub type NonCombinationalSet = HashSet<String>;

/// Extra state `Simulator` needs alongside `pins`/`chips`/etc -- lives as
/// `Simulator::caching`.
#[derive(Debug)]
pub struct CachingState {
	pub combinational_chip_caches: CombinationalChipCache,
	pub chips_known_to_not_be_combinational: NonCombinationalSet,
	pub use_caching: bool,
}

impl Default for CachingState {
	fn default() -> Self {
		Self { combinational_chip_caches: HashMap::new(), chips_known_to_not_be_combinational: HashSet::new(), use_caching: true }
	}
}

/// Mirrors `SimChip.CalculateNumberOfInputBits`: total width in bits across
/// every input pin (e.g. one 4-bit pin + two 1-bit pins == 6).
pub fn calculate_num_input_bits(sim: &Simulator, chip: ChipIdx) -> u32 {
	let c = sim.chip(chip);
	let mut total = 0u32;
	for &p in &c.input_pins {
		total += sim.pin(p).state.len();
	}
	total
}

/// Mirrors `SimChip.IsCombinational`: true iff this chip's outputs depend only
/// on its current inputs (no memory, no feedback loop). Same three checks as
/// the original, in order:
///  1. Builtins are hardcoded (NAND/tristate/merge/split == yes; clock, pulse,
///     RAM, ROM, displays, key, buzzer == no -- they carry state or react to
///     something other than their input pins).
///  2. Every subchip input pin has at most one incoming wire (more would mean
///     a race condition, i.e. non-deterministic), and every subchip is itself
///     recursively combinational.
///  3. The subchip dependency graph is acyclic (topological sort) -- catches
///     things like an SR latch built from two "combinational" NANDs.
pub fn is_combinational(sim: &Simulator, chip: ChipIdx) -> bool {
	use crate::description::ChipType as E;

	let chip_type = sim.chip(chip).chip_type;
	match chip_type {
		E::Nand | E::TriStateBuffer | E::Merge1To4Bit | E::Merge1To8Bit | E::Merge4To8Bit | E::Split4To1Bit | E::Split8To4Bit | E::Split8To1Bit => {
			return true
		}
		E::Clock
		| E::Pulse
		| E::DevRam8Bit
		| E::Rom256x16
		| E::SevenSegmentDisplay
		| E::DisplayRgb
		| E::DisplayDot
		| E::DisplayLed
		| E::Key
		| E::Buzzer => return false,
		_ => {} // Custom (or a bus/io pin type) -- fall through to the structural check below.
	}

	for &sub in &sim.chip(chip).sub_chips {
		for &p in &sim.chip(sub).input_pins {
			if sim.pin(p).num_input_connections > 1 {
				return false;
			}
		}
	}
	for &sub in &sim.chip(chip).sub_chips {
		if !is_combinational(sim, sub) {
			return false;
		}
	}

	// Cycle check via Kahn's algorithm over the subchip dependency graph.
	let sub_chips: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	let mut graph: HashMap<i32, Vec<i32>> = HashMap::new();
	let mut in_degree: HashMap<i32, i32> = HashMap::new();

	for &sub in &sub_chips {
		let sub_id = sim.chip(sub).id;
		graph.entry(sub_id).or_default();
		in_degree.entry(sub_id).or_insert(0);

		for &out_pin in &sim.chip(sub).output_pins {
			for &target_pin in &sim.pin(out_pin).connected_target_pins {
				let target_chip = sim.pin(target_pin).parent_chip;
				let target_id = sim.chip(target_chip).id;
				if target_id == sub_id {
					continue; // self-loop within the same chip id shouldn't happen, but skip defensively
				}
				let edges = graph.entry(sub_id).or_default();
				if !edges.contains(&target_id) {
					edges.push(target_id);
					*in_degree.entry(target_id).or_insert(0) += 1;
				}
			}
		}
	}

	let mut queue: Vec<i32> = in_degree.iter().filter(|(_, &deg)| deg == 0).map(|(&id, _)| id).collect();
	let mut visited = 0usize;
	while let Some(id) = queue.pop() {
		visited += 1;
		if let Some(neighbors) = graph.get(&id) {
			for &n in neighbors {
				let deg = in_degree.get_mut(&n).unwrap();
				*deg -= 1;
				if *deg == 0 {
					queue.push(n);
				}
			}
		}
	}

	visited == in_degree.len()
}

/// Mirrors `SimChip.ResetReceivedFlagsOnAllPins`: clears "already received
/// this frame" bookkeeping before feeding a chip every possible input during
/// LUT construction, so state from a previous sweep step doesn't bleed in.
pub fn reset_received_flags_on_all_pins(sim: &mut Simulator, chip: ChipIdx) {
	let c_pins: Vec<PinIdx> = sim.chip(chip).input_pins.iter().chain(sim.chip(chip).output_pins.iter()).copied().collect();
	for p in c_pins {
		sim.pin_mut(p).num_inputs_received_this_frame = 0;
	}
	let subs: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	for sub in subs {
		reset_received_flags_on_all_pins(sim, sub);
	}
}

/// Mirrors `Simulator.RecalculateCachedLUTs`: recursively tries to build a LUT
/// for `chip` and every subchip, skipping anything already resolved either
/// way. A chip qualifies iff it's custom (builtins already run at O(1) via
/// `process_builtin_chip`), it's combinational, and its input width fits the
/// auto-cache budget, or the user opted in (`SimChip::should_be_cached`) and
/// it fits the larger budget.
///
/// `log` collects a short human-readable line per LUT actually built --
/// consumed once a frame by `viewer::sim_thread::SimHandle::drain_cache_log`
/// and shown as the transient status toast; every decision (including
/// "won't be cached") is also always printed straight to the terminal.
pub fn recalculate_cached_luts(sim: &mut Simulator, caching: &mut CachingState, chip: ChipIdx, log: &mut Vec<String>) {
	let name = sim.chip(chip).name.clone();

	if caching.combinational_chip_caches.contains_key(name.as_ref()) || caching.chips_known_to_not_be_combinational.contains(name.as_ref()) {
		return;
	}

	let subs: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	for sub in subs {
		recalculate_cached_luts(sim, caching, sub, log);
	}

	let num_input_bits = calculate_num_input_bits(sim, chip);
	let should_be_cached = sim.chip(chip).should_be_cached;
	let within_budget =
		num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING || (should_be_cached && num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING);

	let is_custom = sim.chip(chip).chip_type == crate::description::ChipType::Custom;
	if !is_custom || !within_budget || !is_combinational(sim, chip) {
		println!("[cache] '{name}' will not be cached (custom={is_custom}, within_budget={within_budget}, input_bits={num_input_bits})");
		caching.chips_known_to_not_be_combinational.insert(name.to_string());
		return;
	}

	// Snapshot current inputs so the exhaustive sweep below doesn't disturb real sim state.
	let input_pins: Vec<PinIdx> = sim.chip(chip).input_pins.clone();
	let output_pins: Vec<PinIdx> = sim.chip(chip).output_pins.clone();
	let buffered_input: Vec<PinState> = input_pins.iter().map(|&p| sim.pin(p).state).collect();

	let num_possible_inputs: u64 = 1u64 << num_input_bits;
	let mut lut: Vec<Vec<u32>> = Vec::with_capacity(num_possible_inputs as usize);

	// A purely combinational chip (checked above) can never contain a
	// Buzzer -- `is_combinational` hard-fails on one -- so this sweep can't
	// produce any real audio; a scratch, throwaway `SimAudio` is all a
	// `step_chip` call needs syntactically.
	let mut scratch_audio = crate::audio::SimAudio::new();

	for input in 0..num_possible_inputs {
		reset_received_flags_on_all_pins(sim, chip);

		// Slice `input`'s bits across each input pin, low pin first, then step the
		// chip once with that combination applied so its outputs settle.
		let mut remaining = input;
		for &p in &input_pins {
			let width = sim.pin(p).state.width();
			let bit_width = sim.pin(p).state.len();
			let mask: u64 = if bit_width >= 64 { u64::MAX } else { (1u64 << bit_width) - 1 };
			let bits = (remaining & mask) as u8;
			sim.pin_mut(p).state = PinState::from_parts_with_width(bits, 0, width);
			remaining >>= bit_width;
		}

		sim.step_chip(chip, &mut scratch_audio);

		let outputs: Vec<u32> = output_pins.iter().map(|&p| sim.pin(p).state.raw() as u32).collect();
		lut.push(outputs);
	}

	let message = format!("Cached chip '{name}': {num_possible_inputs} row(s), {num_input_bits} input bit(s)");
	println!("[cache] {message}");
	log.push(message);
	caching.combinational_chip_caches.insert(name.to_string(), lut);

	// Restore the buffered "real" input and re-settle outputs.
	reset_received_flags_on_all_pins(sim, chip);
	for (&p, &state) in input_pins.iter().zip(buffered_input.iter()) {
		sim.pin_mut(p).state = state;
	}
	sim.step_chip(chip, &mut scratch_audio);
}

/// Mirrors `Simulator.ProcessCachedChip`: sets `chip`'s outputs by indexing
/// into its LUT instead of simulating. Returns `false` ("fall back to a real
/// step") if any input pin is tri-state, since those combinations were never
/// enumerated into the table, or if there's no LUT for this chip at all yet.
pub fn process_cached_chip(sim: &mut Simulator, caching: &CachingState, chip: ChipIdx) -> bool {
	let name = sim.chip(chip).name.clone();

	let input_pins: Vec<PinIdx> = sim.chip(chip).input_pins.clone();
	let mut input: u64 = 0;
	let mut shift = 0u32;
	for &p in &input_pins {
		let state = sim.pin(p).state;
		if state.tristate_flags() != 0 {
			return false;
		}
		input |= (state.bit_states() as u64) << shift;
		shift += state.len();
	}

	let Some(lut) = caching.combinational_chip_caches.get(name.as_ref()) else {
		return false;
	};
	let Some(outputs) = lut.get(input as usize) else {
		// Defensive: shouldn't happen if the LUT's row count matches this
		// chip's current input width, but a stale/mismatched cache entry
		// (e.g. a live edit widened a pin) should degrade to a real step
		// rather than panic.
		return false;
	};

	let output_pins: Vec<PinIdx> = sim.chip(chip).output_pins.clone();
	for (i, &p) in output_pins.iter().enumerate() {
		let width = sim.pin(p).state.width();
		sim.pin_mut(p).state = PinState::from_raw_with_width(outputs[i] as u16, width);
	}
	true
}
