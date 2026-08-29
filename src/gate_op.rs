use std::collections::HashMap;
use std::hash::Hash;


// add a checkbox into the customization menu (default off) like "optimize"
// that would toggle this from None to a nice specially tailored version just for that circuit
// in some circuits the off can be the same as disabled - but obv. some aren't

// the input should be PinState probably, maybe even the output too



/// The packed word type a specific optimized gate uses internally -- chosen
/// per-gate to be exactly as wide as that gate's pin count needs, so a 4-bit
/// gate pays for a `u8`, not a `u128`.
pub trait WireWord: Copy + Eq + Hash + Send + Sync + 'static {
	fn pack(bits: &[LogicState]) -> Self;
	fn unpack(self, out: &mut [LogicState]);
}

macro_rules! impl_wire_word {
	($($t:ty),*) => {$(
		impl WireWord for $t {
			fn pack(bits: &[LogicState]) -> Self {
				bits.iter().enumerate()
					.fold(0, |acc, (i, b)| acc | ((b.is_high() as $t) << i))
			}
			fn unpack(self, out: &mut [LogicState]) {
				for (i, slot) in out.iter_mut().enumerate() {
					*slot = LogicState::from_bool((self >> i) & 1 == 1);
				}
			}
		}
	)*};
}
impl_wire_word!(u8, u16, u32, u64, u128);

/// Escape hatch for the rare bus wider than 128 wires -- everything else
/// stays on one of the inline int types above.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WideWord {
	bits: Box<[u64]>, // one bit per LogicState.is_high(); ceil(n_wires / 64) words
}

impl WireWord for WideWord {
	fn pack(bits: &[LogicState]) -> Self {
		let words = bits
			.chunks(64)
			.map(|chunk| chunk.iter().enumerate().fold(0u64, |acc, (i, b)| acc | ((b.is_high() as u64) << i)))
			.collect();
		WideWord { bits: words }
	}
	fn unpack(self, out: &mut [LogicState]) {
		for (i, slot) in out.iter_mut().enumerate() {
			let word = self.bits[i / 64];
			*slot = LogicState::from_bool((word >> (i % 64)) & 1 == 1);
		}
	}
}

/// Object-safe interface so `SimChip` can hold *any* width of optimized gate
/// behind a single pointer-sized field, instead of infecting `SimChip`,
/// `Vec<SimChip>`, `Simulator`, etc. with a generic parameter.
pub trait OptimizedGate: Send + Sync {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]);
}

pub trait OptimizedGate: Send + Sync {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]);
}

pub struct Lut<In: WireWord, Out: WireWord> {
	pub table: HashMap<In, Out>,
}
impl<In: WireWord, Out: WireWord> OptimizedGate for Lut<In, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		if let Some(&val) = self.table.get(&In::pack(input)) {
			val.unpack(output);
		}
	}
}

pub struct Native<In: WireWord, Out: WireWord> {
	pub f: fn(In) -> Out,
}
impl<In: WireWord, Out: WireWord> OptimizedGate for Native<In, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		(self.f)(In::pack(input)).unpack(output);
	}
}

/// A short pipeline: `In -> Mid` once, then `Mid -> Mid` for any remaining
/// steps, finally `Mid -> Out` to finalize to correct size. `Mid` should be picked wide enough to hold whatever the widest
/// intermediate value in the chain needs (e.g. a carry-out bit beyond the
/// nominal output width) -- the caller building this decides that width,
/// same as it decides `In`/`Out` for the other variants.
pub struct NativeList<In: WireWord, Mid: WireWord, Out: WireWord> {
	pub first: fn(In) -> Mid,
	pub rest: Vec<fn(Mid) -> Mid>,
    pub last: fn(Mid) -> Out,
}
impl<In: WireWord, Mid: WireWord, Out: WireWord> OptimizedGate for NativeList<In, Mid, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		let mid = self.rest.iter().fold((self.first)(In::pack(input)), |w, f| f(w));
		((self.last)(mid)).unpack(output);
	}
}




/*
// requirements for correct working :
ChipDescription/SimChip need a should_be_cached: bool field (currently nothing tracks the user's "Chip Caching: On/Off" choice from that customization menu at all)
SimChip needs to retain its name (today it only keeps chip_type + id, not the description name used as the cache key)
Simulator::step_chip/pin access is private/immutable-only from outside sim.rs, so this logic would ultimately want to move into sim.rs as real impl Simulator methods rather than live as a separate free-function module.

*/





//! Combinational-chip lookup-table caching, ported from `DLS.Simulation.Simulator`
//! (`RecalculateCachedLUTs`, `ProcessCachedChip`) and `DLS.Simulation.SimChip`
//! (`IsCombinational`, `CalculateNumberOfInputBits`, `ResetReceivedFlagsOnAllPins`).
//!
//! NONE of this exists yet in `sim.rs` / `builtins.rs` / `description.rs` -- the current
//! port always walks the full subchip graph every tick (`Simulator::step_chip`), even for
//! chips that are purely combinational (outputs depend only on current inputs, nothing
//! sequential/stateful going on). The original avoids re-walking those every frame by
//! building a lookup table once (every possible input combination -> output combination)
//! and then just indexing into it. This is pure pseudocode / a porting sketch: it names
//! the exact hookup points into the real arena types (`Simulator`, `SimChip`, `ChipIdx`)
//! but won't compile as-is (no borrow-checker-safe field access has been worked out, gaps
//! marked `todo!()`) -- consider it a blueprint for a real `impl Simulator` block later,
//! not a drop-in module.
//!
//! ---- What's missing, concretely ----
//! 1. `ChipDescription`/`SubChipDescription` needs a `should_be_cached: bool` field
//!    (the user-facing "Chip Caching: On/Off" wheel selector in `ChipCustomizationMenu.cs`
//!    has no Rust-side equivalent at all yet -- neither the data field nor the UI).
//! 2. `SimChip` (in `sim.rs`) needs a `should_be_cached: bool` mirrored from that
//!    description at build time, exactly like `is_builtin` already is.
//! 3. `Simulator` needs the two caches below plus the two size-limit constants.
//! 4. `Simulator::step_chip` needs a branch that tries the cache before recursing.

use crate::description::PinBitCount;
use crate::sim::{ChipIdx, Simulator};
use std::collections::{HashMap, HashSet};

/// Mirrors `Simulator.MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING`. A combinational chip with at
/// most this many input bits is *always* cached, no opt-in required -- 2^12 = 4096 table
/// rows is cheap enough to just always build.
pub const MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING: u32 = 12;

/// Mirrors `Simulator.MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`. Above the auto-cache limit but
/// at or below this one, caching is available but opt-in (`should_be_cached`) since memory
/// cost grows exponentially with input bit count (2^24 = 16M rows). Above this, a chip is
/// never cached regardless of what the user asked for.
pub const MAX_NUM_INPUT_BITS_WHEN_USER_CACHING: u32 = 24;

/// One row per possible input combination; each row is the resulting output pin states.
/// Keyed by chip name (== library lookup key), matching the original's
/// `Dictionary<string, uint[][]> combinationalChipCaches`.
///
/// NOTE: keying by name (rather than a per-instance id) means every instance of the same
/// custom chip shares one LUT, which is correct *only* because the LUT is a pure function
/// of the chip's own wiring/subchips -- it doesn't depend on anything about where a
/// particular instance sits in the outer graph.
pub type CombinationalChipCache = HashMap<String, Vec<Vec<u32>>>;

/// Names already proven non-combinational (or too big to cache), so `recalculate_cached_luts`
/// doesn't waste time re-deriving that answer for every instance every time it's called.
pub type NonCombinationalSet = HashSet<String>;

/// Extra fields `Simulator` would need alongside its existing `pins`/`chips`/etc. Sketched
/// as a standalone struct here rather than editing `sim.rs` directly, per the "only touch
/// this file" ask -- fold these into `Simulator` for real once this is fleshed out.
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

/// Mirrors `SimChip.CalculateNumberOfInputBits`: total width, in bits, across every input
/// pin on this chip (e.g. one 4-bit pin + two 1-bit pins == 6).
pub fn calculate_num_input_bits(sim: &Simulator, chip: ChipIdx) -> u32 {
	let c = sim.chip(chip);
	let mut total = 0u32;
	for &p in &c.input_pins {
		total += sim.pin(p).state.bit_count() as u32; // PinState needs to expose its own width; not yet wired up
	}
	total
}

/// Mirrors `SimChip.IsCombinational`: true iff this chip's outputs depend purely on its
/// current inputs -- no memory, no loops.
///
/// Three-part check, same order as the original:
///  1. Builtin chips are hardcoded combinational/not (NAND, tristate buffer, merge/split ==
///     yes; clock, pulse, RAM, ROM, displays, key, buzzer == no, since they carry state or
///     react to something other than their input pins).
///  2. For a custom chip: every subchip's input pins must have <=1 incoming wire (more than
///     one means a chosen-at-random race condition is possible => not deterministic => not
///     purely combinational), and every subchip must itself be recursively combinational.
///  3. The subchip wiring graph must be acyclic (topological sort over subchip-id ->
///     dependent-subchip-id edges; a chip that can't be fully visited has a feedback loop,
///     i.e. is sequential even though built from "combinational" pieces -- an SR latch made
///     of two NANDs, for instance).
pub fn is_combinational(sim: &Simulator, chip: ChipIdx) -> bool {
	use crate::description::ChipType as E;

	let chip_type = sim.chip(chip).chip_type;
	match chip_type {
		E::Nand | E::TriStateBuffer | E::Merge1To4Bit | E::Merge1To8Bit | E::Merge4To8Bit | E::Split4To1Bit | E::Split8To4Bit | E::Split8To1Bit => return true,
		E::Clock | E::Pulse | E::DevRam8Bit | E::Rom256x16 | E::SevenSegmentDisplay | E::DisplayRgb | E::DisplayDot | E::DisplayLed | E::Key | E::Buzzer => return false,
		_ => {} // Custom (or a bus/io pin type) -- fall through to structural check below.
	}

	// Part 2: every subchip input pin has at most one incoming connection, and every
	// subchip is itself combinational.
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

	// Part 3: cycle check via Kahn's algorithm over the subchip dependency graph.
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
					continue; // self-loop pin-to-pin within the same chip id shouldn't happen, but skip defensively
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

/// Mirrors `SimChip.ResetReceivedFlagsOnAllPins`. Needed before feeding a chip every
/// possible input combination during LUT construction (see `recalculate_cached_luts`
/// below) so stale "already received this frame" bookkeeping from a previous simulated
/// input doesn't bleed into the next one.
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

/// Mirrors `Simulator.RecalculateCachedLUTs`: recursively try to build (and register) a LUT
/// for `chip` and every one of its subchips, skipping anything already resolved either way.
///
/// A chip is cachable iff: it's a custom chip (builtins already get O(1) handling in
/// `process_builtin_chip`, no LUT needed), it's actually combinational (`is_combinational`),
/// and its input-bit count is within budget -- always within `MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING`,
/// or opted in via `should_be_cached` and within `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`.
pub fn recalculate_cached_luts(sim: &mut Simulator, caching: &mut CachingState, chip: ChipIdx) {
	let name = todo!("Simulator doesn't expose a chip's name today -- SimChip only stores chip_type + id, not the description's `name`. Would need to thread that through at build time, same as `is_builtin`.");

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
		let should_be_cached = false; // todo!() -- read from SimChip::should_be_cached once that field exists
		let within_budget = num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING
			|| (should_be_cached && num_input_bits <= MAX_NUM_INPUT_BITS_WHEN_USER_CACHING);

		let is_custom = sim.chip(chip).chip_type == crate::description::ChipType::Custom;
		if !is_custom || !within_budget || !is_combinational(sim, chip) {
			caching.chips_known_to_not_be_combinational.insert(name);
			return;
		}

		// Buffer current input pin states so the "real" simulation state isn't disturbed by
		// the exhaustive sweep below.
		let buffered_input: Vec<PinBitCount> = todo!("snapshot chip.input_pins[i].state for restoring afterwards");

		let num_possible_inputs: u64 = 1u64 << num_input_bits;
		let mut lut: Vec<Vec<u32>> = Vec::with_capacity(num_possible_inputs as usize);

		for input in 0..num_possible_inputs {
			reset_received_flags_on_all_pins(sim, chip);

			// Slice `input`'s bits out across each input pin, low pin first (matches the
			// original's `tempInput >>= numberOfBits` walk), then step the chip once with
			// that combination applied so its outputs settle.
			let mut remaining = input;
			let pins: Vec<_> = sim.chip(chip).input_pins.clone();
			for &_p in &pins {
				let _bits = remaining; // todo!(): mask + write into pin state, matching `chip.InputPins[i].State = tempInput & mask`
				remaining >>= 0; // todo!(): shift by this pin's actual bit width
			}

			todo!("step_chip(chip) equivalent -- Simulator::step_chip is private to sim.rs today; would need a `pub(crate)` seam or to move this whole module's logic into sim.rs itself");

			let outputs: Vec<u32> = todo!("collect chip.output_pins[i].state.bit_states() for each output pin");
			lut.push(outputs);
		}

		caching.combinational_chip_caches.insert(name, lut);

		// Restore the buffered "real" input and re-settle outputs so nothing downstream
		// observes the exhaustive sweep's leftover state.
		let _ = buffered_input;
		reset_received_flags_on_all_pins(sim, chip);
		todo!("re-apply buffered_input, then step_chip(chip) once more");
	}
}

/// Mirrors `Simulator.ProcessCachedChip`: sets `chip`'s output pins by indexing straight
/// into its LUT instead of simulating. Returns `false` (meaning "fall back to a real step")
/// if any input pin is currently in tri-state, since tri-state values were never enumerated
/// into the table.
pub fn process_cached_chip(sim: &mut Simulator, caching: &CachingState, chip: ChipIdx) -> bool {
	let name: String = todo!("same missing SimChip::name as above");

	#[allow(unreachable_code)]
	{
		let pins: Vec<_> = sim.chip(chip).input_pins.clone();
		let mut input: u64 = 0;
		for &p in pins.iter().rev() {
			let state = sim.pin(p).state;
			if state.tristate_flags() != 0 {
				return false; // not cached for tri-state combinations, must simulate for real
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

/// Sketch of the `step_chip` integration point (`Simulator::step_chip`'s subchip loop in
/// `sim.rs` today has no caching branch at all -- it always either calls
/// `process_builtin_chip` or recurses via `step_chip`). This shows where the three-way
/// branch from the original's `StepChip` would slot in:
///
/// ```ignore
/// if next_sub_chip.is_builtin {
///     self.process_builtin_chip(next_sub_chip, audio);
/// } else if caching.use_caching && caching.combinational_chip_caches.contains_key(&name_of(next_sub_chip)) {
///     if !gate_op::process_cached_chip(self, &caching, next_sub_chip) {
///         self.step_chip(next_sub_chip, audio); // cache lookup declined (tri-state input) -- simulate for real
///     }
/// } else if caching.use_caching && !caching.chips_known_to_not_be_combinational.contains(&name_of(next_sub_chip)) {
///     gate_op::recalculate_cached_luts(self, &mut caching, next_sub_chip); // try to build a cache for next time
///     self.step_chip(next_sub_chip, audio);
/// } else {
///     self.step_chip(next_sub_chip, audio);
/// }
/// ```
pub fn step_chip_with_caching_sketch() {}