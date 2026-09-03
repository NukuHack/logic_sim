//! The fast-path gate evaluators: the precomputed-truth-table [`Lut`], and the closed-form
//! [`Native`]/[`NativeList`] (with the `Bits`-word helpers in `bitvec` that back their
//! formulas). All three implement [`CachedGate`], the one evaluator trait `Simulator` needs --
//! see `sim::process_cached_chip`, the only real caller today, for the shape this is built
//! around: a chip's input pins packed low-bit-first into a single `u64`, and one raw packed
//! word written back per output pin.

use super::bitvec::{pack_words, unpack_words};
use crate::pin_state::LogicState;

/// Small per-instance parameter (carry-in/carry-out flags and the like) passed alongside a
/// formula, and the word size [`Native`]/[`NativeList`] formulas operate on. Deliberately a
/// plain `u64`, not some arbitrary-width vector type: a formula already takes a `&[Bits]` slice
/// when it needs more than one word (see `bitvec`), so `Bits` itself only has to be "one word",
/// not "big enough for any gate".
pub type Bits = u64;

/// The one evaluator trait every fast-path gate representation
/// implements, so `CachingState`/`SimChip` can store any of them uniformly behind one
/// pointer-sized field.
///
/// Shaped directly around `process_cached_chip`, its only real caller: `input` is a chip's
/// input pins packed low-bit-first into one `u64` (see `caching::calculate_num_input_bits` and
/// `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING` for why 64 bits is enough for every chip that can
/// actually reach this path), and `out` gets one raw packed word per output pin, matching
/// `PinState::from_raw_with_width`. This is *not* one bit per output wire -- an output pin can itself be multiple wires (a nibble/byte bus), and
/// `out`'s job is to hand each such pin back its own raw value, not a single bit-vector spanning
/// every pin.
pub trait CachedGate: std::fmt::Debug + Send + Sync {
	/// Evaluates `input` into `out`. Returns `false` if this evaluator has nothing for `input`
	/// (e.g. an out-of-range `Lut` row from a stale cache entry, or an `out` slice the wrong
	/// length), in which case `out` is left untouched and the caller should fall back to a real
	/// step.
	fn eval(&self, input: u64, out: &mut [u32]) -> bool;
}

/// A combinational chip's full truth table, one row per possible input combination, one packed
/// `u32` per output pin. Indexed directly by `input`, so eval is a single array read with no
/// hashing -- this is the fast path and should be preferred whenever the full table fits in
/// memory (see the auto/user caching budgets in `caching.rs`).
#[derive(Debug)]
pub struct Lut {
	rows: Box<[Box<[u32]>]>,
}

impl Lut {
	pub fn new(rows: Vec<Vec<u32>>) -> Self {
		Self { rows: rows.into_iter().map(Vec::into_boxed_slice).collect() }
	}

	/// Row count, i.e. `2^in_bits` for a fully-built table.
	pub fn len(&self) -> usize {
		self.rows.len()
	}

	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}

	/// The packed fields for input row `input`, or `None` if out of range.
	pub fn row(&self, input: u64) -> Option<&[u32]> {
		self.rows.get(input as usize).map(|r| &**r)
	}
}

impl CachedGate for Lut {
	fn eval(&self, input: u64, out: &mut [u32]) -> bool {
		let Some(row) = self.row(input) else {
			// Defensive: shouldn't happen if the LUT's row count matches this chip's
			// current input width, but a stale/mismatched cache entry (e.g. a live edit
			// widened a pin) should degrade to a real step rather than panic.
			return false;
		};
		let n = out.len().min(row.len());
		out[..n].copy_from_slice(&row[..n]);
		true
	}
}

type SizedFn = fn(&[Bits], u32, Bits) -> Vec<Bits>;
type NativeFn = fn(&[Bits], Bits) -> Vec<Bits>;
type DuoSizedFn = fn(&[Bits], u32, u32, Bits) -> Vec<Bits>;

/// A gate evaluated from a closed-form function over packed bits rather than a stored table,
/// e.g. `AND2` as `|w, ..| (w & 0b11) == 0b11`. Cheaper to build than a `Lut` (no table to
/// fill) and just as fast to run, at the cost of a few ALU ops per eval instead of one load --
/// and unlike a `Lut`, its cost doesn't grow with gate width at all, so this is what
/// [`super::recognize::recognize`] hands back for any pattern it matches (AND, adder, ...)
/// regardless of how wide the gate is.
///
/// Packs its input into as many `Bits` words as `in_bits` needs and hands that slice straight
/// to `f`, so a formula written against the `bitvec` helpers is
/// correct for every gate -- genuinely unlimited input width, with
/// the same small, readable closed-form `f` regardless of size. [`Self::eval_wide`] is where
/// that unlimited width is actually exercised (`recognize` and its tests use it directly); the
/// [`CachedGate`] impl below is the narrower `u64`-in/single-`u32`-out bridge
/// `process_cached_chip` can call today
///
/// `in_bits`/`out_bits` are carried alongside `config`
/// so one `fn` pointer can serve an entire parametric family
#[derive(Debug)]
pub struct Native {
	in_bits: u32,
	out_bits: u32,
	config: Bits,
	f: DuoSizedFn,
}

impl Native {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, f: fn(&[Bits], u32, u32, Bits) -> Vec<Bits>) -> Self {
		Self { in_bits, out_bits, config, f }
	}

	/// Arbitrary-width eval: packs/unpacks through `LogicState` slices directly instead of
	/// going through [`CachedGate::eval`]'s `u64`/single-`u32` bridge, so a gate wider than 64
	/// input bits or 32 output bits (impossible for any `Lut`, and past what today's only
	/// `CachedGate` caller can hand in) still evaluates correctly. This is what `recognize` and
	/// its wide-gate tests call.
	pub fn eval_wide(&self, input: &[LogicState], output: &mut [LogicState]) {
		debug_assert_eq!(input.len() as u32, self.in_bits);
		debug_assert_eq!(output.len() as u32, self.out_bits);

		let words = pack_words(input);
		let result = (self.f)(&words, self.in_bits, self.out_bits, self.config);
		unpack_words(&result, output);
	}
}

impl CachedGate for Native {
	fn eval(&self, input: u64, out: &mut [u32]) -> bool {
		if self.in_bits > 64 || self.out_bits > 32 || out.len() != 1 {
			return false;
		}
		let result = (self.f)(&[input], self.in_bits, self.out_bits, self.config);
		out[0] = result.first().copied().unwrap_or(0) as u32;
		true
	}
}

/// A chain of native steps: `first` applied once to the packed input, then each of
/// `rest` in order, then `last` to land on the final output width. Pick a step's return width
/// wider than the nominal output width when it needs headroom (e.g. a carry bit beyond the
/// final sum), and mask back down in a later step.
///
/// Reworked onto the same arbitrary-width `&[Bits]` shape [`Native`] uses instead of the old
/// fixed-width `WireWord`-bounded design -- so a chain is exactly as unlimited-width as a
/// single-step `Native` formula is, which is what actually lets it back the cache path in the
/// future.
#[derive(Debug)]
pub struct NativeList {
	in_bits: u32,
	out_bits: u32,
	config: Bits,
	first: SizedFn,
	rest: Vec<NativeFn>,
	last: SizedFn,
}

impl NativeList {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, first: SizedFn, rest: Vec<NativeFn>, last: SizedFn) -> Self {
		Self { in_bits, out_bits, config, first, rest, last }
	}

	fn run(&self, words: &[Bits]) -> Vec<Bits> {
		let mid = self.rest.iter().fold((self.first)(words, self.in_bits, self.config), |w, step| step(&w, self.config));
		(self.last)(&mid, self.out_bits, self.config)
	}

	/// Arbitrary-width eval, the `NativeList` counterpart of [`Native::eval_wide`].
	pub fn eval_wide(&self, input: &[LogicState], output: &mut [LogicState]) {
		debug_assert_eq!(input.len() as u32, self.in_bits);
		debug_assert_eq!(output.len() as u32, self.out_bits);

		let result = self.run(&pack_words(input));
		unpack_words(&result, output);
	}
}

impl CachedGate for NativeList {
	fn eval(&self, input: u64, out: &mut [u32]) -> bool {
		if self.in_bits > 64 || self.out_bits > 32 || out.len() != 1 {
			return false;
		}
		let result = self.run(&[input]);
		out[0] = result.first().copied().unwrap_or(0) as u32;
		true
	}
}
