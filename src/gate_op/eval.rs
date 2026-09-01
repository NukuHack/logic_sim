//! Evaluation strategies for optimized gates, exposed behind the object-safe
//! `OptimizedGate` trait so `SimChip` can store any gate width uniformly.
//! Includes the precomputed-truth-table `Lut` fast path.

use super::word::WireWord;
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

/// Packed-bits currency `Native` and [`super::recognize`]'s candidate registry both deal in:
/// gate pins packed low-bit-first into one register-sized integer. A plain `u64` (rather than
/// going through `WireWord`/`In::pack`) means every candidate check and every `Native::eval` is
/// pure register arithmetic -- no monomorphization per word type, and no mismatch between the
/// width `recognize` reasons about (`in_bits`/`out_bits`, checked against the real `Lut`) and
/// the width baked into some concrete `In`/`Out` pair.
pub type Bits = u64;

/// A gate evaluated from a closed-form function over packed bits rather than a stored table,
/// e.g. `AND2` as `|w, ..| (w & 0b11) == 0b11`. Cheaper to build than a `Lut` (no table to
/// fill) and just as fast to run, at the cost of a few ALU ops per eval instead of one load --
/// and unlike a `Lut`, its cost doesn't grow with gate width at all, so this is what
/// [`super::recognize::recognize`] hands back for any pattern it matches (AND, adder, ...)
/// regardless of how wide the gate is.
///
/// `in_bits`/`out_bits` are carried alongside `config` (rather than baked into a concrete
/// `In`/`Out` type pair) so one `fn` pointer can serve an entire parametric family -- e.g.
/// `AND_N` for any width, or the four carry-in/carry-out adder variants sharing one formula
/// shape -- instead of needing a hand-written monomorphized function per width.
pub struct Native {
	in_bits: u32,
	out_bits: u32,
	config: Bits,
	f: fn(Bits, u32, u32, Bits) -> Bits,
}

impl Native {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, f: fn(Bits, u32, u32, Bits) -> Bits) -> Self {
		Self { in_bits, out_bits, config, f }
	}
}

impl OptimizedGate for Native {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		debug_assert_eq!(input.len() as u32, self.in_bits);
		debug_assert_eq!(output.len() as u32, self.out_bits);

		let mut word: Bits = 0;
		for (i, s) in input.iter().enumerate() {
			word |= (s.is_high() as Bits) << i;
		}

		let result = (self.f)(word, self.in_bits, self.out_bits, self.config);

		for (i, slot) in output.iter_mut().enumerate() {
			*slot = LogicState::from_bool((result >> i) & 1 == 1);
		}
	}
}

// TODO : rework NativeList, since Native has changed a lot

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
