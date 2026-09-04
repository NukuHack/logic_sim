//! Combinational-chip caching, ported from `DLS.Simulation.Simulator`
//! (`RecalculateCachedLUTs`, `ProcessCachedChip`) and `DLS.Simulation.SimChip`
//! (`IsCombinational`, `CalculateNumberOfInputBits`, `ResetReceivedFlagsOnAllPins`). The idea
//! is to build each such chip's truth table once and index into it afterwards instead of re-
//! walking its subchip graph every single simulation frame.

use super::eval::{CachedGate, Lut};
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

/// Mirrors `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`: the widest input a user-requested cache is
/// allowed to build a `Lut` for. TODO: lift this once `Native`/`NativeList` back the cache path
/// too, since they have no such width ceiling.
pub const MAX_NUM_INPUT_BITS_WHEN_USER_CACHING: u32 = 24;

/// Extra state `Simulator` needs alongside `pins`/`chips`/etc -- lives as
/// `Simulator::caching`.
#[derive(Debug)]
pub struct CachingState {
	/// Keyed by chip name. Replaces the original's `Dictionary<string, uint[][]> combinationalChipCaches`
	pub combinational_chip_cache: HashMap<Arc<str>, Box<dyn CachedGate>>,
	/// Chip names already proven non-combinational, so
	/// `recalculate_chip_cache` doesn't re-derive the same answer every call.
	/// Same interned-`Arc<str>` sharing as [`combinational_chip_cache`].
	pub not_combinational_chip_cache: HashSet<Arc<str>>,
	/// same as any preference would be, just moved here for nicer access
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

/// Flattens exactly the set of pins that [`reset_received_flags_on_all_pins`] would visit
/// for `chip` (its own input/output pins, plus every pin of every subchip, recursively) into
/// a single list, computed once.
///
/// The LUT sweep in [`recalculate_chip_cache`] calls the reset between *every* row.
/// two heap-allocated `Vec`s per chip in the tree, per row
/// turned what should be a cheap flag clear into the dominant cost of building a LUT for anything but a tiny chip.
/// Collecting the pin list once before the loop and then just iterating over it each row keeps the exact same set of pins reset,
/// in an allocation-free O(1)-per-row pass, instead of re-deriving the list from scratch every time.
fn collect_all_pins_recursive(sim: &Simulator, chip: ChipIdx, out: &mut Vec<PinIdx>) {
	out.extend(sim.chip(chip).input_pins.iter().copied());
	out.extend(sim.chip(chip).output_pins.iter().copied());
	let subs: Vec<ChipIdx> = sim.chip(chip).sub_chips.clone();
	for sub in subs {
		collect_all_pins_recursive(sim, sub, out);
	}
}

/// Fast path used inside the LUT sweep: resets a pre-flattened pin list (see
/// [`collect_all_pins_recursive`]) instead of re-walking the subchip tree.
fn reset_received_flags_fast(sim: &mut Simulator, pins: &[PinIdx]) {
	for &p in pins {
		sim.pin_mut(p).num_inputs_received_this_frame = 0;
	}
}

/// Mirrors `Simulator.RecalculateCachedLUTs`: recursively tries to build a Cache
/// for `chip` and every subchip, skipping anything already resolved either way.
/// A chip qualifies if it's custom (builtins already run at O(1) via `process_builtin_chip`)
pub fn recalculate_chip_cache(sim: &mut Simulator, chip: ChipIdx) {
	// Cloned up front so the borrow doesn't outlive the mutable `sim` accesses below.
	let name = sim.chip(chip).name.clone();

	if sim.caching.combinational_chip_cache.contains_key(name.as_ref()) || sim.caching.not_combinational_chip_cache.contains(name.as_ref()) {
		return;
	}

	let sub_count = sim.chip(chip).sub_chips.len();
	for i in 0..sub_count {
		// Indexed by position rather than iterating a cloned `Vec`: the recursive call below
		// takes `&mut Simulator`, so this can't hold a borrow of `sub_chips` across it.
		let sub = sim.chip(chip).sub_chips[i];
		recalculate_chip_cache(sim, sub);
	}

	let num_input_bits = calculate_num_input_bits(sim, chip);
	let should_be_cached = sim.chip(chip).cache_kind.is_none();
	let within_budget = should_be_cached && num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING;

	let is_custom = sim.chip(chip).chip_type == crate::description::ChipType::Custom;
	if !is_custom || !within_budget || !is_combinational(sim, chip) {
		log::debug!("[cache] '{name}' will not be cached (custom={is_custom}, within_budget={within_budget}, input_bits={num_input_bits})");
		sim.caching.not_combinational_chip_cache.insert(name);
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
	log::debug!("[cache] sweeping '{name}': {num_input_bits} input bits, {num_possible_inputs} rows");
	let mut cache_rows = Vec::with_capacity(num_possible_inputs as usize);

	// Flattened once, up front -- see `collect_all_pins_recursive` -- so the reset below is a
	// tight loop instead of a full, allocation-heavy subchip-tree walk on every single row.
	let mut all_pins = Vec::new();
	collect_all_pins_recursive(sim, chip, &mut all_pins);

	// A purely combinational chip (checked above) can never contain a
	// Buzzer -- `is_combinational` hard-fails on one -- so this sweep can't
	// produce any real audio; a scratch, throwaway `SimAudio` is all a
	// `step_chip` call needs syntactically.
	let mut scratch_audio = crate::audio::SimAudio::new();

	// `step_chip`'s single reverse pass over `sub_chips` only produces a correct result once
	// the subchips are sorted into "reverse of desired visitation" (i.e. topological) order --
	// normally established lazily, the first time `Simulator` runs a real
	// `run_simulation_step` (`needs_order_pass` / `step_chip_reorder`). This sweep can run
	// before that's ever happened (e.g. right after `Simulator::build`, as these tests do),
	// against subchips still sitting in raw load order -- so a lone `step_chip` call can
	// evaluate a subchip before the subchip(s) feeding it have propagated their outputs.
	// `step_chip_reorder` performs one real, dependency-driven evaluation pass (a subchip only
	// becomes eligible once every one of its inputs has actually been received) and permanently
	// swaps `sub_chips` into the order that produced, exactly like real simulation's first
	// frame -- so running it once, up front, makes every subsequent plain `step_chip` call
	// below correct in a single pass, recursively through any nested custom subchips too (see
	// its own doc comment). Current input values don't matter here: the order this settles on
	// depends only on the wiring graph, not on what bits happen to be flowing through it yet.
	sim.step_chip_reorder(chip, &mut scratch_audio);

	for input in 0..num_possible_inputs {
		reset_received_flags_fast(sim, &all_pins);

		let mut remaining = input;
		for &p in &input_pins {
			let state = sim.pin(p).state;
			let bit_width = state.len();
			let mask: u64 = if bit_width >= 64 { u64::MAX } else { (1u64 << bit_width) - 1 };
			let bits = (remaining & mask) as u8;
			sim.pin_mut(p).state = PinState::from_parts_with_width(bits, 0, state.width());
			remaining >>= bit_width;
		}
		sim.step_chip(chip, &mut scratch_audio);

		// `bit_states()`, not `raw()`: `raw()` also carries the tristate-flags byte, whose
		// bits for wires past this pin's actual width are left over from that pin's initial
		// `disconnected_with_width` state (all-ones) rather than meaningful data. `recognize`
		// (and `Native`'s formulas) compare/produce plain 0/1-per-wire values, so folding that
		// stale garbage into the row here would make an otherwise-matching candidate (e.g.
		// `OR2`) fail every comparison and force a fallback to a stored `Lut` full of the same
		// polluted values. `process_cached_chip` already mirrors this on the input side (see
		// its `state.bit_states()` build of `input`), so this keeps row capture consistent
		// with both how rows get compared and how they get replayed.
		let outputs: Vec<u32> = output_pins.iter().map(|&p| sim.pin(p).state.bit_states() as u32).collect();
		cache_rows.push(outputs);
	}

	// A plain `Native`'s `eval` only ever writes a single output word (see `CachedGate::eval`
	// on `Native`), so a chip with more than one output pin can't be a bare `Native`. Two
	// richer shapes get tried first, in order, before falling all the way back to a stored
	// `Lut`:
	//
	//  1. `NativeSplit` -- every output pin's bits packed together into one combined word
	//     (same low-bit-first convention the `input` loop above uses for input pins) and
	//     recognized as a *single* pattern, e.g. an adder's N-bit sum pin plus a separate
	//     1-bit carry-out pin as one `ADDER_N_COU`. Tried first since it catches cross-pin
	//     patterns `NativeMulti` fundamentally can't (no candidate produces a lone carry bit
	//     on its own).
	//  2. `NativeMulti` -- each output pin recognized independently, e.g. `OUT1 = NOT(IN)` /
	//     `OUT2 = BUFFER(IN)` as two separate pins. Catches combinations `NativeSplit` can't
	//     (unrelated per-pin patterns with no shared combined-word shape), so it's worth
	//     trying even after `NativeSplit` fails.
	//
	// Both need `cache_rows` itself, so they're computed before `cache_rows` gets consumed
	// into the whole-chip `Lut` below (the ultimate fallback if neither matches).
	let flattened_native: Option<super::eval::NativeSplit> = 'flatten: {
		if output_pins.len() <= 1 {
			break 'flatten None;
		}
		let mut out_layout = Vec::with_capacity(output_pins.len());
		let mut total_out_bits = 0u32;
		for &p in &output_pins {
			let w = sim.pin(p).state.len();
			out_layout.push((total_out_bits, w));
			total_out_bits += w;
		}
		// `Lut`/`recognize` only ever compare a single packed `u32` field per row (see
		// `MAX_LUT_OUTPUT_BITS`), so a combined word wider than that can't go through the
		// same recognition path -- not worth trying to flatten.
		if total_out_bits == 0 || total_out_bits > super::MAX_LUT_OUTPUT_BITS {
			break 'flatten None;
		}
		let combined_rows: Vec<Vec<u32>> = cache_rows
			.iter()
			.map(|row| {
				let mut combined: u64 = 0;
				for (i, &(offset, _)) in out_layout.iter().enumerate() {
					combined |= (row[i] as u64) << offset;
				}
				vec![combined as u32]
			})
			.collect();
		let combined_lut = Lut::new(combined_rows);
		match super::recognize::recognize_formula(num_input_bits, total_out_bits, &combined_lut) {
			Some((config, formula)) => {
				let native = super::eval::Native::new(num_input_bits, total_out_bits, config, formula);
				Some(super::eval::NativeSplit::new(native, out_layout))
			}
			None => None,
		}
	};

	let multi_pin_natives: Option<Vec<super::eval::Native>> = if flattened_native.is_some() || output_pins.len() <= 1 {
		// Either already matched above, or there's at most one pin (handled by the plain
		// `Native` path below) -- no need to also try the per-pin route.
		None
	} else {
		let mut natives = Vec::with_capacity(output_pins.len());
		'pins: for (pin_idx, &out_pin) in output_pins.iter().enumerate() {
			let out_bits = sim.pin(out_pin).state.len();
			let sub_rows: Vec<Vec<u32>> = cache_rows.iter().map(|row| vec![row[pin_idx]]).collect();
			let sub_lut = Lut::new(sub_rows);
			match super::recognize::recognize_formula(num_input_bits, out_bits, &sub_lut) {
				Some((config, formula)) => natives.push(super::eval::Native::new(num_input_bits, out_bits, config, formula)),
				None => {
					natives.clear();
					break 'pins;
				}
			}
		}
		if natives.len() == output_pins.len() {
			Some(natives)
		} else {
			None
		}
	};

	let lut = Lut::new(cache_rows);

	let cached_gate: Box<dyn CachedGate> = match output_pins.len() {
		1 => {
			let out_pin = output_pins[0];
			let out_bits = sim.pin(out_pin).state.len();
			match super::recognize::recognize(num_input_bits, out_bits, &lut) {
				Some(native) => {
					log::debug!("[cache] '{name}' recognized as a known gate pattern -- using Native instead of a {num_possible_inputs}-row Lut");
					native
				}
				None => {
					log::debug!("[cache] '{name}' matched no known gate pattern -- storing the {num_possible_inputs}-row Lut as-is");
					Box::new(lut)
				}
			}
		}
		n if n > 1 => {
			if let Some(split) = flattened_native {
				log::debug!(
					"[cache] '{name}' has {n} output pins, recognized as a single pattern across their combined bits -- using NativeSplit instead of a {num_possible_inputs}-row Lut"
				);
				Box::new(split)
			} else if let Some(natives) = multi_pin_natives {
				log::debug!(
					"[cache] '{name}' has {n} output pins, every one individually recognized as a known gate pattern -- using NativeMulti instead of a {num_possible_inputs}-row Lut"
				);
				Box::new(super::eval::NativeMulti::new(natives))
			} else {
				log::debug!(
					"[cache] '{name}' has {n} output pins and matched neither a combined nor a per-pin gate pattern -- storing the {num_possible_inputs}-row Lut as-is"
				);
				Box::new(lut)
			}
		}
		_ => {
			log::debug!("[cache] '{name}' has no output pins -- storing the (degenerate) {num_possible_inputs}-row Lut as-is");
			Box::new(lut)
		}
	};

	log::debug!("[cache] cached chip '{name}': {num_possible_inputs} row(s), {num_input_bits} input bit(s)");
	sim.caching.combinational_chip_cache.insert(name, cached_gate);

	// Restore the real input state the sweep overwrote, so the caller sees no side effects.
	reset_received_flags_fast(sim, &all_pins);
	for (&p, &state) in input_pins.iter().zip(buffered_input.iter()) {
		sim.pin_mut(p).state = state;
	}
}
