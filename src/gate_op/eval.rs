//! Evaluation strategies for optimized gates, exposed behind the object-safe `OptimizedGate`
//! trait so `SimChip` can store any gate width uniformly. Includes the precomputed-truth-table
//! `Lut` fast path, the closed-form `Native`/`NativeList` fast paths, and the `WireWord` trait
//! (with its impls for the built-in integer types) that packs gate pin states into fixed-width
//! integers for fast evaluation.

use super::bitvec::{pack_words, unpack_words};
use crate::pin_state::LogicState;
use std::marker::PhantomData;

/// Object-safe evaluator so `SimChip` can hold any width of optimized gate
/// behind one pointer-sized field, instead of making `SimChip` (and everything
/// that stores one) generic over `In`/`Out`.
pub trait OptimizedGate: Send + Sync {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]);
}

/// A precomputed truth table: every input combination mapped straight to its
/// output. `table` is indexed directly by the packed input word, so eval is a
/// single array read with no hashing -- this is the fast path and should be
/// preferred whenever the full table fits in memory (see the auto/user caching
/// budgets in `caching.rs`).
pub struct Lut<In: WireWord, Out: WireWord> {
	pub table: Box<[Out]>,
	_input: PhantomData<In>,
}

impl<In: WireWord, Out: WireWord> Lut<In, Out> {
	pub fn new(table: Box<[Out]>) -> Self {
		Self { table, _input: PhantomData }
	}
}

impl<In: WireWord, Out: WireWord> OptimizedGate for Lut<In, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		if let Some(val) = self.table.get(In::pack(input).as_index()) {
			val.clone().unpack(output);
		}
	}
}

/// Small per-instance parameter (carry-in/carry-out flags and the like) passed alongside a
/// formula. Deliberately still a plain `u64`, not a [`bitvec::Word`] vector: every use of
/// `config` across the registry is a handful of flag bits, never gate-width-dependent, so there
/// is nothing to gain from making it arbitrary-width too.
pub type Bits = u64;

/// A gate evaluated from a closed-form function over packed bits rather than a stored table,
/// e.g. `AND2` as `|w, ..| (w & 0b11) == 0b11`. Cheaper to build than a `Lut` (no table to
/// fill) and just as fast to run, at the cost of a few ALU ops per eval instead of one load --
/// and unlike a `Lut`, its cost doesn't grow with gate width at all, so this is what
/// [`super::recognize::recognize`] hands back for any pattern it matches (AND, adder, ...)
/// regardless of how wide the gate is.
///
/// Unified from what used to be two separate designs: a `Lut`-style generic type parameter
/// (which only ever covered widths a concrete `WireWord` impl could hold, capping out at 128
/// bits and leaving `WideWord` unable to serve as a `Native` word at all) and an earlier
/// `Native` hardcoded to a single `u64` word (capping every formula at 64 input/output bits).
/// `Native` now takes neither: it packs its input into as many [`bitvec::Word`]s as `in_bits`
/// needs and hands that slice straight to `f`, so a formula written against the `bitvec` helpers
/// (`and`/`add`/`field`/...) is correct for a 4-bit gate and a 4000-bit gate alike -- genuinely
/// unlimited input width, with the same small, readable closed-form `f` as before instead of an
/// opaque stored table.
///
/// `in_bits`/`out_bits` are carried alongside `config` (rather than baked into a concrete type
/// parameter) so one `fn` pointer can serve an entire parametric family -- e.g. `AND_N` for any
/// width, or the four carry-in/carry-out adder variants sharing one formula shape -- instead of
/// needing a hand-written monomorphized function per width.
pub struct Native {
	in_bits: u32,
	out_bits: u32,
	config: Bits,
	f: fn(&[Bits], u32, u32, Bits) -> Vec<Bits>,
}

impl Native {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, f: fn(&[Bits], u32, u32, Bits) -> Vec<Bits>) -> Self {
		Self { in_bits, out_bits, config, f }
	}
}

impl OptimizedGate for Native {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		debug_assert_eq!(input.len() as u32, self.in_bits);
		debug_assert_eq!(output.len() as u32, self.out_bits);

		let words = pack_words(input);
		let result = (self.f)(&words, self.in_bits, self.out_bits, self.config);
		unpack_words(&result, output);
	}
}

// TODO: rework NativeList to match Native's current arbitrary-width design; it still assumes
// the old fixed-width `WireWord` shape at each step.

/// A short chain of native steps: `In -> Mid` once, then `Mid -> Mid` for each
/// remaining step, then `Mid -> Out` to land on the final width. Pick `Mid`
/// wide enough for the widest intermediate value in the chain (e.g. a carry
/// bit beyond the nominal output width).
pub struct NativeList<In: WireWord, Mid: WireWord, Out: WireWord> {
	pub first: fn(In) -> Mid,
	pub rest: Vec<fn(Mid) -> Mid>,
	pub last: fn(Mid) -> Out,
}

impl<In: WireWord, Mid: WireWord, Out: WireWord> OptimizedGate for NativeList<In, Mid, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		let mid = self.rest.iter().fold((self.first)(In::pack(input)), |w, f| f(w));
		(self.last)(mid).unpack(output);
	}
}

/// A gate's pins packed into one integer, sized to fit that gate exactly (a 4-bit
/// gate uses `u8`, not `u128`). Packing lets evaluation work on a cheap integer
/// instead of walking a `&[LogicState]` slice every time.
///
/// `Clone` rather than `Copy`: every inline int type is `Copy` for free, but
/// `WideWord` owns a heap-allocated `Box<[u64]>` and can't be.
pub trait WireWord: Clone + Eq + Send + Sync + 'static {
	/// Packs each `LogicState` in `bits` into one bit, low pin first.
	fn pack(bits: &[LogicState]) -> Self;
	/// Unpacks back into per-pin states, inverse of `pack`.
	fn unpack(self, out: &mut [LogicState]);
	/// This word's value as a `Vec` index. Only meaningful for word sizes small
	/// enough to serve as a dense lookup-table index (see `WideWord`).
	fn as_index(&self) -> usize;

	/// This word's bits widened to `u64`, same low-bit-first layout as
	/// `pack`/`unpack`. Lets code that only cares about the bit pattern (e.g.
	/// `recognize`) work uniformly across `u8..u128` without being generic
	/// over which one it got. Only meaningful for widths <= 64; wider words
	/// are truncated to their low 64 bits (see `WideWord`'s impl).
	fn to_u64(&self) -> u64;
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
			fn as_index(&self) -> usize {
				*self as usize
			}
			fn to_u64(&self) -> u64 {
				*self as u64 // truncating for u128, which is fine: to_u64's contract is low-64-bits-only
			}
		}
	)*};
}
impl_wire_word!(u8, u16, u32, u64, u128);

/// Escape hatch for buses wider than 128 wires; everything narrower uses one of
/// the inline int types above instead. Not usable as a `Lut` index -- a dense
/// table over a >128-bit input space isn't buildable, so `as_index` panics.
#[derive(Clone, PartialEq, Eq)]
pub struct WideWord {
	bits: Box<[u64]>, // one bit per LogicState.is_high(); ceil(n_wires / 64) words
}

impl WireWord for WideWord {
	fn pack(bits: &[LogicState]) -> Self {
		let words = bits.chunks(64).map(|chunk| chunk.iter().enumerate().fold(0u64, |acc, (i, b)| acc | ((b.is_high() as u64) << i))).collect();
		WideWord { bits: words }
	}
	fn unpack(self, out: &mut [LogicState]) {
		for (i, slot) in out.iter_mut().enumerate() {
			let word = self.bits[i / 64];
			*slot = LogicState::from_bool((word >> (i % 64)) & 1 == 1);
		}
	}
	fn as_index(&self) -> usize {
		unreachable!("WideWord inputs are too wide for a dense Lut; use Native/NativeList instead")
	}
	fn to_u64(&self) -> u64 {
		self.bits.first().copied().unwrap_or(0)
	}
}
