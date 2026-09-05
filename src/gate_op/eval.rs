//! The fast-path gate evaluators: the precomputed-truth-table [`Lut`], and the closed-form
//! [`Native`]/[`NativeList`] (with the `Bits`-word helpers in `bitvec` that back their
//! formulas). All three implement [`CachedGate`], the one evaluator trait `Simulator` needs --
//! see `sim::process_cached_chip`, the only real caller today, for the shape this is built
//! around: a chip's input pins packed low-bit-first into a single `u64`, and one raw packed
//! word written back per output pin.

use super::bitvec::{pack_words, unpack_words};
use crate::pin_state::LogicState;
use std::fmt;

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
pub trait CachedGate: std::fmt::Debug + std::fmt::Display + Send + Sync {
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

impl fmt::Display for Lut {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let rows = self.rows.len();
		let cols = self.rows.first().map_or(0, |row| row.len());
		let total_elements = rows * cols;

		// Get min/max values (optional but nice)
		let (min, max) = if rows > 0 && cols > 0 {
			let all_values = self.rows.iter().flat_map(|row| row.iter());
			let min = all_values.clone().min().unwrap_or(&0);
			let max = all_values.max().unwrap_or(&0);
			(*min, *max)
		} else {
			(0, 0)
		};

		write!(f, "LUT [{} rows x {} cols] - {} total entries, range: {}..={}", rows, cols, total_elements, min, max)
	}
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

impl fmt::Display for Native {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Get function pointer info (optional, if DuoSizedFn has some debug info)
		let fn_name = if self.f as usize == 0 {
			"unknown".to_string()
		} else {
			// If DuoSizedFn stores any metadata or you can get a name
			// You might need to adjust this based on what DuoSizedFn actually is
			format!("fn@{:p}", self.f)
		};

		write!(f, "Native [{} inputs → {} outputs] (config: {:?}, fn: {})", self.in_bits, self.out_bits, self.config, fn_name)
	}
}

impl Native {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, f: fn(&[Bits], u32, u32, Bits) -> Vec<Bits>) -> Self {
		Self { in_bits, out_bits, config, f }
	}

	/// Runs this `Native`'s formula against a packed `u64` input and hands back the raw
	/// combined output word (low `out_bits` bits meaningful, no per-pin splitting and no
	/// truncation to a single `u32`) -- the building block [`NativeSplit`] needs to get at
	/// every output pin's bits before slicing them back apart. Unlike [`CachedGate::eval`]
	/// (this type's narrower `u64`-in/single-`u32`-out bridge), this doesn't require or care
	/// about `out_bits <= 32`; it's `NativeSplit::eval` that caps the *combined* width at 32
	/// (see its doc comment), not this.
	fn eval_combined(&self, input: u64) -> u64 {
		(self.f)(&[input], self.in_bits, self.out_bits, self.config).first().copied().unwrap_or(0)
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

impl fmt::Display for NativeList {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let total_steps = 1 + self.rest.len() + 1; // first + rest + last

		write!(f, "NativeList [{}b → {}b] ({} steps, {} config)", self.in_bits, self.out_bits, total_steps, self.config)?;

		// Optionally show step breakdown if not too verbose
		if f.alternate() {
			// With {:#} format, show more detail
			write!(f, " [first + {} middle + last]", self.rest.len())?;
		}

		Ok(())
	}
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

/// A chip with more than one output *pin*, each pin independently recognized as its own
/// closed-form [`Native`] against the same shared input -- e.g. a chip with `OUT1 = NOT(IN)`
/// and `OUT2 = BUFFER(IN)` as two separate 1-bit pins becomes two `Native`s (`NOT`, `BUFFER`)
/// bundled here, rather than one `Lut` row per input combination.
///
/// `Native::eval` itself only ever fills a single `out` word (see its `CachedGate` impl), which
/// is exactly why a plain `Native` can't represent a multi-pin chip on its own -- `NativeMulti`
/// is the thin fan-out on top: run every entry's `eval` against the same `input`, one call per
/// output pin, writing into that pin's own slot of the caller's `out` slice. Still O(1) per
/// pin and stores no table, so a wide multi-pin chip (bus splitters composed with recognizable
/// per-pin logic, etc) gets the same win a single-output `Native` already does.
///
/// Building one only makes sense when *every* output pin is individually recognizable --
/// [`super::caching::recalculate_chip_cache`] falls back to a plain multi-column `Lut` the
/// moment any single pin doesn't match a known pattern, since a partial `NativeMulti` (some
/// pins closed-form, one pin only representable as a table) isn't a shape this type can hold.
#[derive(Debug)]
pub struct NativeMulti {
	/// One entry per output pin, in the same order as the chip's `output_pins`.
	entries: Vec<Native>,
}

impl fmt::Display for NativeMulti {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let pin_count = self.entries.len();

		// Show summary of what each pin does (if we can extract it)
		write!(f, "NativeMulti [{} pins", pin_count)?;

		// Show first few pins with their input/output widths
		let max_show = 3;
		let show_count = pin_count.min(max_show);

		if pin_count > 0 {
			write!(f, ": ")?;
			for (i, native) in self.entries.iter().take(show_count).enumerate() {
				if i > 0 {
					write!(f, ", ")?;
				}
				write!(f, "pin{}: {}b→{}b", i, native.in_bits, native.out_bits)?;
			}
			if pin_count > max_show {
				write!(f, ", … (+{} more)", pin_count - max_show)?;
			}
		}

		write!(f, "]")?;

		Ok(())
	}
}

impl NativeMulti {
	pub fn new(entries: Vec<Native>) -> Self {
		Self { entries }
	}
}

impl CachedGate for NativeMulti {
	fn eval(&self, input: u64, out: &mut [u32]) -> bool {
		if out.len() != self.entries.len() {
			return false;
		}
		for (slot, native) in out.iter_mut().zip(self.entries.iter()) {
			// Each `Native` only ever wants/fills a single-element `out` slice -- see its own
			// `CachedGate::eval` -- so hand it a length-1 scratch slot rather than a sub-slice
			// of the caller's buffer.
			let mut single = [0u32];
			if !native.eval(input, &mut single) {
				return false;
			}
			*slot = single[0];
		}
		true
	}
}

/// A single [`Native`] recognized against every output pin's bits packed together into one
/// combined word -- pin 0's bits at the low end, then each subsequent pin's bits stacked above
/// it, the same low-bit-first convention [`super::caching::recalculate_chip_cache`] already
/// uses to pack multiple *input* pins into one word (see its `input |= ... << shift` loop).
/// This is what lets something like an adder's N-bit sum pin plus a separate 1-bit carry-out
/// pin still collapse to one `ADDER_N_COU` `Native`: read as two independent pins neither the
/// sum alone (no carry info) nor the carry alone (no single-candidate shape) matches anything
/// in the registry, but read as one `(n+1)`-bit combined word, it's exactly `ADDER_N_COU`'s
/// output shape.
///
/// `eval` runs the wrapped `Native` once and slices the result back apart per `out_layout`, so
/// this is exactly as O(1) as a plain `Native` -- one formula call regardless of how many
/// output pins it's feeding.
#[derive(Debug)]
pub struct NativeSplit {
	native: Native,
	/// Each output pin's `(bit offset into the combined word, width)`, in output-pin order.
	out_layout: Vec<(u32, u32)>,
}

impl fmt::Display for NativeSplit {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let pin_count = self.out_layout.len();
		let total_out_bits: u32 = self.out_layout.iter().map(|(_, width)| width).sum();

		write!(f, "NativeSplit [{} pins, {}b total out", pin_count, total_out_bits)?;

		// Show the layout if not too many pins
		if pin_count <= 4 {
			write!(f, ": ")?;
			for (i, (offset, width)) in self.out_layout.iter().enumerate() {
				if i > 0 {
					write!(f, ", ")?;
				}
				write!(f, "p{}: bits {}-{}", i, offset, offset + width - 1)?;
			}
		} else {
			// Show summary of layout
			write!(f, ", layout: ")?;
			for (i, (offset, width)) in self.out_layout.iter().take(2).enumerate() {
				if i > 0 {
					write!(f, ", ")?;
				}
				write!(f, "p{}: {}b@{}", i, width, offset)?;
			}
			if pin_count > 2 {
				write!(f, ", … (+{} more)", pin_count - 2)?;
			}
		}

		write!(f, "]")?;

		// Show underlying native info
		write!(f, " (native: {}b→{}b, {} config)", self.native.in_bits, self.native.out_bits, self.native.config)?;

		Ok(())
	}
}

impl NativeSplit {
	pub fn new(native: Native, out_layout: Vec<(u32, u32)>) -> Self {
		Self { native, out_layout }
	}
}

impl CachedGate for NativeSplit {
	fn eval(&self, input: u64, out: &mut [u32]) -> bool {
		if out.len() != self.out_layout.len() {
			return false;
		}
		let combined = self.native.eval_combined(input);
		for (slot, &(offset, width)) in out.iter_mut().zip(self.out_layout.iter()) {
			let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
			*slot = ((combined >> offset) & mask) as u32;
		}
		true
	}
}
