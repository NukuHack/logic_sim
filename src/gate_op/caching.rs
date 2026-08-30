//! Sketch for combinational-chip caching, ported from `DLS.Simulation.Simulator`
//! (`RecalculateCachedLUTs`, `ProcessCachedChip`) and `DLS.Simulation.SimChip`
//! (`IsCombinational`, `CalculateNumberOfInputBits`, `ResetReceivedFlagsOnAllPins`).
//!
//! Today `Simulator::step_chip` always walks the full subchip graph every tick,
//! even for chips that are purely combinational (output depends only on the
//! current input, nothing sequential). The idea here is to build each such
//! chip's truth table once and index into it afterwards instead of re-walking.
//!
//! This is pseudocode, not a working module -- several `todo!()`s mark gaps
//! that need real `Simulator`/`SimChip` support first:
//! 1. `ChipDescription` needs a `should_be_cached: bool` field (the "Chip
//!    Caching: On/Off" menu option has no Rust-side equivalent yet).
//! 2. `SimChip` needs that same flag mirrored at build time, like `is_builtin`.
//! 3. `SimChip` needs to retain its `name` (today it only keeps `chip_type` + `id`).
//! 4. `Simulator::step_chip`/pin access is private to `sim.rs`; this logic should
//!    ultimately move there as real `impl Simulator` methods rather than live
//!    as free functions.

use crate::description::PinBitCount;
use crate::sim::{ChipIdx, Simulator};
use std::collections::{HashMap, HashSet};

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING`: a combinational chip at or
/// under this many input bits is always cached (2^12 rows is cheap enough to
/// just always build).
pub const MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING: u32 = 12;

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`: above the auto-cache limit,
/// caching is opt-in (`should_be_cached`) up to this many bits (2^24 rows);
/// beyond it, a chip is never cached.
pub const MAX_NUM_INPUT_BITS_WHEN_USER_CACHING: u32 = 24;

/// One row per input combination, keyed by chip name. Matches the original's
/// `Dictionary<string, uint[][]> combinationalChipCaches`.
///
/// Keying by name means every instance of the same custom chip shares one LUT
/// -- correct only because the LUT is a pure function of the chip's own wiring,
/// independent of where an instance sits in the outer graph.
pub type CombinationalChipCache = HashMap<String, Vec<Vec<u32>>>;

/// Chip names already proven non-combinational (or too big to cache), so
/// `recalculate_cached_luts` doesn't re-derive the same answer every call.
pub type NonCombinationalSet = HashSet<String>;

/// Extra state `Simulator` would need alongside `pins`/`chips`/etc. Kept as a
/// standalone struct for now; fold into `Simulator` once this is fleshed out.
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
		E::Nand | E::TriStateBuffer | E::Merge1To4Bit | E::Merge1To8Bit | E::Merge4To8Bit | E::Split4To1Bit | E::Split8To4Bit | E::Split8To1Bit => return true,
		E::Clock | E::Pulse | E::DevRam8Bit | E::Rom256x16 | E::SevenSegmentDisplay | E::DisplayRgb | E::DisplayDot | E::DisplayLed | E::Key | E::Buzzer => return false,
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
	let c_pins: Vec<_> = sim.chip(chip).input_pins.iter().chain(sim.chip(chip).output_pins.iter()).copied().collect();
	for _p in c_pins {
		todo!("needs a Simulator::pin_mut(idx) accessor -- SimPin::num_inputs_received_this_frame = 0");
	}
	let subs: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	for sub in subs {
		reset_received_flags_on_all_pins(sim, sub);
	}
}

/// Mirrors `Simulator.RecalculateCachedLUTs`: recursively try to build a LUT
/// for `chip` and every subchip, skipping anything already resolved either
/// way. A chip qualifies iff it's custom (builtins already run at O(1) via
/// `process_builtin_chip`), it's combinational, and its input width fits the
/// auto-cache budget, or the user opted in and it fits the larger budget.
pub fn recalculate_cached_luts(sim: &mut Simulator, caching: &mut CachingState, chip: ChipIdx) {
	let name = todo!("SimChip only stores chip_type + id today, not the description's `name`; needs threading through at build time, same as `is_builtin`.");

	#[allow(unreachable_code)]
	{
		let name: String = name;
		if caching.combinational_chip_caches.contains_key(&name) || caching.chips_known_to_not_be_combinational.contains(&name) {
			return;
		}

		let subs: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
		for sub in subs {
			recalculate_cached_luts(sim, caching, sub);
		}

		let num_input_bits = calculate_num_input_bits(sim, chip);
		let should_be_cached = false; // todo!(): read from SimChip::should_be_cached once that field exists
		let within_budget = num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING
			|| (should_be_cached && num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING);

		let is_custom = sim.chip(chip).chip_type == crate::description::ChipType::Custom;
		if !is_custom || !within_budget || !is_combinational(sim, chip) {
			caching.chips_known_to_not_be_combinational.insert(name);
			return;
		}

		// Snapshot current inputs so the exhaustive sweep below doesn't disturb real sim state.
		let buffered_input: Vec<PinBitCount> = todo!("snapshot chip.input_pins[i].state for restoring afterwards");

		let num_possible_inputs: u64 = 1u64 << num_input_bits;
		let mut lut: Vec<Vec<u32>> = Vec::with_capacity(num_possible_inputs as usize);

		for input in 0..num_possible_inputs {
			reset_received_flags_on_all_pins(sim, chip);

			// Slice `input`'s bits across each input pin, low pin first, then step the
			// chip once with that combination applied so its outputs settle.
			let mut remaining = input;
			let pins: Vec<_> = sim.chip(chip).input_pins.clone();
			for &_p in &pins {
				let _bits = remaining; // todo!(): mask + write into pin state
				remaining >>= 0; // todo!(): shift by this pin's actual bit width
			}

			todo!("step_chip(chip) equivalent -- Simulator::step_chip is private to sim.rs; needs a pub(crate) seam, or this module should move into sim.rs");

			let outputs: Vec<u32> = todo!("collect chip.output_pins[i].state.bit_states() for each output pin");
			lut.push(outputs);
		}

		caching.combinational_chip_caches.insert(name, lut);

		// Restore the buffered "real" input and re-settle outputs.
		let _ = buffered_input;
		reset_received_flags_on_all_pins(sim, chip);
		todo!("re-apply buffered_input, then step_chip(chip) once more");
	}
}

/// Mirrors `Simulator.ProcessCachedChip`: sets `chip`'s outputs by indexing
/// into its LUT instead of simulating. Returns `false` ("fall back to a real
/// step") if any input pin is tri-state, since those combinations were never
/// enumerated into the table.
pub fn process_cached_chip(sim: &mut Simulator, caching: &CachingState, chip: ChipIdx) -> bool {
	let name: String = todo!("same missing SimChip::name as above");

	#[allow(unreachable_code)]
	{
		let pins: Vec<_> = sim.chip(chip).input_pins.clone();
		let mut input: u64 = 0;
		for &p in pins.iter().rev() {
			let state = sim.pin(p).state;
			if state.tristate_flags() != 0 {
				return false;
			}
			input <<= 0; // todo!(): shift by this pin's bit width
			input |= state.bit_states() as u64;
		}

		let Some(lut) = caching.combinational_chip_caches.get(&name) else {
			return false;
		};
		let outputs = &lut[input as usize];

		let out_pins: Vec<_> = sim.chip(chip).output_pins.clone();
		for (i, &p) in out_pins.iter().enumerate() {
			let _ = (p, outputs[i]); // todo!(): sim.pin_mut(p).state = PinState::from_raw(outputs[i] as u16)
		}
		true
	}
}

/// Sketch of where this would hook into `Simulator::step_chip`'s subchip loop,
/// which today only ever calls `process_builtin_chip` or recurses:
///
/// ```ignore
/// if next_sub_chip.is_builtin {
///     self.process_builtin_chip(next_sub_chip, audio);
/// } else if caching.use_caching && caching.combinational_chip_caches.contains_key(&name_of(next_sub_chip)) {
///     if !caching::process_cached_chip(self, &caching, next_sub_chip) {
///         self.step_chip(next_sub_chip, audio); // cache lookup declined (tri-state input)
///     }
/// } else if caching.use_caching && !caching.chips_known_to_not_be_combinational.contains(&name_of(next_sub_chip)) {
///     caching::recalculate_cached_luts(self, &mut caching, next_sub_chip);
///     self.step_chip(next_sub_chip, audio);
/// } else {
///     self.step_chip(next_sub_chip, audio);
/// }
/// ```
pub fn step_chip_with_caching_sketch() {}
