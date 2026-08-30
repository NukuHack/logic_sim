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

/// A hand-written closure standing in for a gate, e.g. `AND` as `|w: u8| w == 0b11`.
/// Cheaper to build than a `Lut` (no table to fill) and just as fast to run.
pub struct Native<In: WireWord, Out: WireWord> {
	pub f: fn(In) -> Out,
}

impl<In: WireWord, Out: WireWord> OptimizedGate for Native<In, Out> {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		(self.f)(In::pack(input)).unpack(output);
	}
}

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
