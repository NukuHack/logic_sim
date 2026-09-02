//! Runtime constructor for [`Lut`] that picks the *smallest* `WireWord` able to hold a gate's
//! actual width, instead of a caller having to pick a concrete `In`/`Out` type up front (and,
//! not knowing the real width at compile time, reaching for the biggest one "just in case").
//!
//! `Native`/`NativeList` don't need this: `Native` already packs into exactly
//! `words_for(bits)` `u64`s (see `bitvec`), so it's automatically as small as the gate needs
//! and has genuinely no upper bound -- that's what actually covers the "works past 128 input
//! bits" requirement for the *system* as a whole. `Lut` can't follow it there: a real table has
//! `2^in_bits` rows, so a `u128`-indexed `Lut` isn't "128 input bits, smallest necessary type",
//! it's a table with more rows than atoms in the observable universe. This dispatcher picks
//! the smallest `In`/`Out` among the widths a `Lut` table can *actually* be built at (up to 63
//! input bits, so `1u64 << in_bits` never overflows -- in practice `recalculate_chip_cache`
//! never asks for anywhere near that many, see `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`), and
//! `recognize()`/`Native` are exactly what picks up everything wider.
use super::eval::{Lut, OptimizedGate};

/// Largest input width a [`Lut`] table can be built at all: `1u64 << in_bits` (the row count)
/// would overflow above this. Not a memory budget -- a real table is unbuildable long before
/// this (see `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`, which caps caching at 24) -- just the
/// hard type-safety ceiling. Anything wider isn't a "bigger Lut", it's `Native`'s job.
pub const MAX_LUT_INPUT_BITS: u32 = 63;
/// Largest output width: widest `WireWord` impl used on the output side
pub const MAX_LUT_OUTPUT_BITS: u32 = 64;

/// Builds a `Box<dyn OptimizedGate>` around a [`Lut`] whose `In`/`Out` are the smallest of
/// `u8/u16/u32/u64/u128` (`In`) and `u8/u16/u32/u64` (`Out`) that actually fit `in_bits`/
/// `out_bits` -- a 3-bit gate is packed into a `Lut<u8, _>`, a 16-bit one into `Lut<u16, _>`,
/// and so on, so nothing pays for `u128` storage/indexing it never needed.
///
/// `fill(row)` computes the packed output word for input row `row` (`0..2^in_bits`); it's
/// called once per row to build the table, same convention as `In::pack`/`Out::unpack` (low
/// bit first).
///
/// Returns `None` when `in_bits`/`out_bits` is zero, `in_bits > 128`, `out_bits > 64`, or
/// `in_bits` is wide enough that `1u64 << in_bits` itself would overflow (nothing above ~63
/// bits is a real, buildable `Lut` table regardless of what `In` could theoretically index --
/// use `Native` instead once a gate is that wide, which is exactly what
/// [`super::recognize::recognize`] already does).
pub fn build_lut(in_bits: u32, out_bits: u32, fill: impl Fn(u64) -> u64) -> Option<Box<dyn OptimizedGate>> {
	if in_bits == 0 || in_bits > MAX_LUT_INPUT_BITS || out_bits == 0 || out_bits > MAX_LUT_OUTPUT_BITS {
		return None;
	}

	macro_rules! build {
		($In:ty, $Out:ty) => {{
			let rows = 1u64 << in_bits;
			let table: Box<[$Out]> = (0..rows).map(|r| fill(r) as $Out).collect();
			Box::new(Lut::<$In, $Out>::new(table)) as Box<dyn OptimizedGate>
		}};
	}

	// `In`/`Out` are each picked independently as the smallest integer type whose bit width
	// covers the gate's real width -- a 20-input/3-output gate becomes `Lut<u32, u8>`, not
	// `Lut<u32, u32>` and definitely not `Lut<u128, u64>`.
	Some(match (in_bits, out_bits) {
		(1..=8, 1..=8) => build!(u8, u8),
		(1..=8, 9..=16) => build!(u8, u16),
		(1..=8, 17..=32) => build!(u8, u32),
		(1..=8, 33..=64) => build!(u8, u64),

		(9..=16, 1..=8) => build!(u16, u8),
		(9..=16, 9..=16) => build!(u16, u16),
		(9..=16, 17..=32) => build!(u16, u32),
		(9..=16, 33..=64) => build!(u16, u64),

		(17..=32, 1..=8) => build!(u32, u8),
		(17..=32, 9..=16) => build!(u32, u16),
		(17..=32, 17..=32) => build!(u32, u32),
		(17..=32, 33..=64) => build!(u32, u64),

		(33..=63, 1..=8) => build!(u64, u8),
		(33..=63, 9..=16) => build!(u64, u16),
		(33..=63, 17..=32) => build!(u64, u32),
		(33..=63, 33..=64) => build!(u64, u64),

		_ => unreachable!("in_bits/out_bits already range-checked above against the MAX"),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pin_state::LogicState;

	fn eval_bools(gate: &dyn OptimizedGate, out_bits: u32, input: &[bool]) -> Vec<bool> {
		let states: Vec<LogicState> = input.iter().map(|&b| LogicState::from_bool(b)).collect();
		let mut out = vec![LogicState::Low; out_bits as usize];
		gate.eval(&states, &mut out);
		out.iter().map(|s| s.is_high()).collect()
	}

	#[test]
	fn three_wide_gate_uses_u8_not_u128() {
		// AND3: in_bits=3 falls in the (1..=8, 1..=8) bucket -> Lut<u8, u8>.
		let gate = build_lut(3, 1, |row| (row == 0b111) as u64).unwrap();
		assert_eq!(eval_bools(&*gate, 1, &[true, true, true]), vec![true]);
		assert_eq!(eval_bools(&*gate, 1, &[true, true, false]), vec![false]);
	}

	#[test]
	fn sixteen_wide_gate_uses_u16_not_u128() {
		let gate = build_lut(16, 1, |row| (row == 0xFFFF) as u64).unwrap();
		assert_eq!(eval_bools(&*gate, 1, &vec![true; 16]), vec![true]);
	}

	#[test]
	fn rejects_gates_wider_than_a_lut_table_can_index() {
		assert!(build_lut(64, 1, |_| 0).is_none()); // 1u64 << 64 would overflow
		assert!(build_lut(129, 1, |_| 0).is_none());
	}

	#[test]
	fn rejects_over_64_output_bits() {
		assert!(build_lut(4, 65, |_| 0).is_none());
	}
}
