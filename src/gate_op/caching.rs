//! Combinational-chip caching, ported from `DLS.Simulation.Simulator`
//! (`RecalculateCachedLUTs`, `ProcessCachedChip`) and `DLS.Simulation.SimChip`
//! (`IsCombinational`, `CalculateNumberOfInputBits`, `ResetReceivedFlagsOnAllPins`). The idea
//! is to build each such chip's truth table once and index into it afterwards instead of re-
//! walking its subchip graph every single simulation frame.

use crate::pin_state::PinState;
use crate::sim::{ChipIdx, PinIdx, Simulator};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING`: a combinational chip at or
/// under this many input bits is cheap enough (2^12 rows) that Save turns
/// caching on for it automatically, as long as the user hasn't manually set
/// the checkbox themselves (see `viewer::save_flow::resolve_should_cache`).
/// Purely a Save-time decision -- nothing at runtime auto-caches anymore.
pub const MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING: u32 = 12;

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`
/// will make it inf when i implement Native and NativeList
pub const MAX_NUM_INPUT_BITS_WHEN_USER_CACHING: u32 = 24;

/// One row per input combination, keyed by chip name. Matches the original's
/// `Dictionary<string, uint[][]> combinationalChipCaches`. Each row is the
/// chip's raw packed `PinState` word (bits + tri-state flags) per output
/// pin, in output-pin order -- not just the bit values -- so a cache hit can
/// reproduce a tri-state output exactly, not just a settled 0/1.
pub type CombinationalChipCache = HashMap<Arc<str>, Vec<Vec<u32>>>;

/// Chip names already proven non-combinational (or too big to cache), so
/// `recalculate_chip_cache` doesn't re-derive the same answer every call.
/// Same interned-`Arc<str>` sharing as [`CombinationalChipCache`].
pub type NonCombinationalSet = HashSet<Arc<str>>;

/// Extra state `Simulator` needs alongside `pins`/`chips`/etc -- lives as
/// `Simulator::caching`.
#[derive(Debug)]
pub struct CachingState {
	pub combinational_chip_cache: CombinationalChipCache,
	pub not_combinational_chip_cache: NonCombinationalSet,
	pub use_caching: bool,
}

impl Default for CachingState {
	fn default() -> Self {
		Self { combinational_chip_cache: HashMap::new(), not_combinational_chip_cache: HashSet::new(), use_caching: true }
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
/// on its current inputs (no memory, no feedback loop). Same three checks as the original.
///
/// Delegates to [`is_combinational_memoized`] with a fresh, call-local memo table: a chip's
/// subchips can reference the same underlying chip definition more than once (e.g. two AND
/// gates in one schematic), and without memoization each occurrence re-walks that shared
/// subtree from scratch.
pub fn is_combinational(sim: &Simulator, chip: ChipIdx) -> bool {
	let mut memo = HashMap::new();
	is_combinational_memoized(sim, chip, &mut memo)
}

/// Does the real work behind [`is_combinational`], keyed by chip id so repeated subchips
/// within one call are only walked once. `memo` is local to a single top-level call rather
/// than shared across calls: a chip's combinational-ness can only depend on its own subtree,
/// so a fresh table per call is both correct and enough to eliminate the redundant re-walks
/// that matter (a chip used many times inside one parent).
fn is_combinational_memoized(sim: &Simulator, chip: ChipIdx, memo: &mut HashMap<i32, bool>) -> bool {
	use crate::description::ChipType as E;

	let chip_id = sim.chip(chip).id;
	if let Some(&result) = memo.get(&chip_id) {
		return result;
	}

	let chip_type = sim.chip(chip).chip_type;
	if chip_type.is_merge_type() || chip_type.is_bus_type() || chip_type.is_io_type() {
		memo.insert(chip_id, true);
		return true;
	}
	match chip_type {
		E::Nand | E::TriStateBuffer => {
			memo.insert(chip_id, true);
			return true;
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
		| E::Buzzer
		| E::KeyMods => {
			memo.insert(chip_id, false);
			return false;
		}
		_ => {}
	}

	for &sub in &sim.chip(chip).sub_chips {
		for &p in &sim.chip(sub).input_pins {
			if sim.pin(p).num_input_connections > 1 {
				memo.insert(chip_id, false);
				return false;
			}
		}
	}
	for &sub in &sim.chip(chip).sub_chips {
		if !is_combinational_memoized(sim, sub, memo) {
			memo.insert(chip_id, false);
			return false;
		}
	}

	// Cycle check via Kahn's algorithm over the subchip dependency graph. Adjacency uses a
	// `HashSet` rather than a `Vec` so the "already recorded this edge" dedup check below is
	// O(1) instead of O(edges-so-far) -- chips with many interconnected subchips would
	// otherwise pay for that scan on every wire.
	let sub_chips: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	let mut graph: HashMap<i32, HashSet<i32>> = HashMap::new();
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
				if graph.entry(sub_id).or_default().insert(target_id) {
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
				let deg = in_degree.get_mut(&n).expect("every id in `graph` was inserted into `in_degree` above");
				*deg -= 1;
				if *deg == 0 {
					queue.push(n);
				}
			}
		}
	}

	let result = visited == in_degree.len();
	memo.insert(chip_id, result);
	result
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

/// Mirrors `Simulator.RecalculateCachedLUTs`: recursively tries to build a Cache
/// for `chip` and every subchip, skipping anything already resolved either way.
/// A chip qualifies if it's custom (builtins already run at O(1) via `process_builtin_chip`)
pub fn recalculate_chip_cache(sim: &mut Simulator, caching: &mut CachingState, chip: ChipIdx) {
	// Clone the name immediately so we don't hold a reference to sim
	let name = sim.chip(chip).name.clone();

	if caching.combinational_chip_cache.contains_key(name.as_ref()) || caching.not_combinational_chip_cache.contains(name.as_ref()) {
		return;
	}

	let sub_count = sim.chip(chip).sub_chips.len();
	for i in 0..sub_count {
		// Get the sub chip each iteration, no clone needed!
		let sub = sim.chip(chip).sub_chips[i];
		recalculate_chip_cache(sim, caching, sub);
	}

	let num_input_bits = calculate_num_input_bits(sim, chip);
	let should_be_cached = !sim.chip(chip).cache_kind.is_off();
	let within_budget = should_be_cached && num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING;

	let is_custom = sim.chip(chip).chip_type == crate::description::ChipType::Custom;
	if !is_custom || !within_budget || !is_combinational(sim, chip) {
		println!("[cache] '{name}' will not be cached (custom={is_custom}, within_budget={within_budget}, input_bits={num_input_bits})");
		caching.not_combinational_chip_cache.insert(name);
		return;
	}

	// Snapshot current inputs so the exhaustive sweep below doesn't disturb real sim state.
	let input_pins = sim.chip(chip).input_pins.clone();
	let output_pins = sim.chip(chip).output_pins.clone();

	let mut buffered_input = Vec::with_capacity(input_pins.len());
	for &p in &input_pins {
		buffered_input.push(sim.pin(p).state);
	}

	let num_possible_inputs = 1u64 << num_input_bits;
	let mut cache_rows = Vec::with_capacity(num_possible_inputs as usize);

	for input in 0..num_possible_inputs {
		reset_received_flags_on_all_pins(sim, chip);

		let mut remaining = input;
		for &p in &input_pins {
			let state = sim.pin(p).state;
			let bit_width = state.len();
			let mask: u64 = if bit_width >= 64 { u64::MAX } else { (1u64 << bit_width) - 1 };
			let bits = (remaining & mask) as u8;
			sim.pin_mut(p).state = PinState::from_parts_with_width(bits, 0, state.width());
			remaining >>= bit_width;
		}

		let outputs: Vec<u32> = output_pins.iter().map(|&p| sim.pin(p).state.raw() as u32).collect();
		cache_rows.push(outputs);
	}

	println!("[cache] Cached chip '{name}': {num_possible_inputs} row(s), {num_input_bits} input bit(s)");
	caching.combinational_chip_cache.insert(name, cache_rows);

	// Restore state
	reset_received_flags_on_all_pins(sim, chip);
	for (&p, &state) in input_pins.iter().zip(buffered_input.iter()) {
		sim.pin_mut(p).state = state;
	}
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

	let Some(cache_rows) = caching.combinational_chip_cache.get(name.as_ref()) else {
		return false;
	};
	let Some(outputs) = cache_rows.get(input as usize) else {
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
