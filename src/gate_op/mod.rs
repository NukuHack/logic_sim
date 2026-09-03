//! Optimized gate evaluation: pack a gate's pins into a small integer and evaluate via table
//! lookup or a native function instead of walking the subchip graph.
//!
//! - `bitvec`: arbitrary-width bit-vector helpers (the packed `Bits`-word representation
//!   `Native`/`NativeList` formulas operate on).
//! - `eval`: `CachedGate` and its implementations (`Lut`, `Native`, `NativeList`) -- the
//!   actual fast path, usable today.
//! - `recognize`: matches a materialized `Lut` against known gate patterns and hands back an
//!   equivalent `Native`.
//! - `caching`: *when* to build/use a `Lut` per combinational chip -- `Simulator::step_sub_chip`
//!   (in `sim.rs`) calls straight into this module's `recalculate_chip_cache` for every
//!   non-builtin subchip it steps.
//! - build.rs -- moved inside mod.rs
//!   Runtime constructor for a single-field [`Lut`]: builds a full `2^in_bits`-row table from a
//!   per-row fill function, for callers (currently `recognize`'s tests) that want a table
//!   representing one gate's whole packed output rather than one field per output pin -- see
//!   `Lut`'s doc comment for why one row shape covers both.

mod bitvec;
mod caching;
mod eval;
mod recognize;

pub use caching::{
	calculate_num_input_bits, is_combinational, recalculate_chip_cache, reset_received_flags_on_all_pins, CachingState,
	MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING,
};
pub use eval::{Bits, CachedGate, Lut, Native, NativeList};
pub use recognize::{recognize, registry, Candidate};

/// Largest input width a [`Lut`] table can be built at all: `1u64 << in_bits` (the row count)
/// would overflow above this. Not a memory budget -- a real table is unbuildable long before
/// this (see `MAX_NUM_INPUT_BITS_WHEN_USER_CACHING`, which caps caching at 24) -- just the
/// hard type-safety ceiling. Anything wider isn't a "bigger Lut", it's `Native`'s job.
pub const MAX_LUT_INPUT_BITS: u32 = 63;
/// Largest output width a single packed `u32` field can hold.
pub const MAX_LUT_OUTPUT_BITS: u32 = 32;

/// Builds a single-field [`Lut`] (one `u32` per row, the gate's whole packed output) by calling
/// `fill(row)` once for every input row (`0..2^in_bits`), same low-bit-first convention as
/// `bitvec`'s helpers.
///
/// Returns `None` when `in_bits`/`out_bits` is zero, `out_bits > 32`, or `in_bits` is wide
/// enough that `1u64 << in_bits` itself would overflow (nothing above ~63 bits is a real,
/// buildable `Lut` table -- use `Native` instead once a gate is that wide, which is exactly
/// what [`super::recognize::recognize`] already does).
pub fn build_lut(in_bits: u32, out_bits: u32, fill: impl Fn(u64) -> u64) -> Option<Lut> {
	if in_bits == 0 || in_bits > MAX_LUT_INPUT_BITS || out_bits == 0 || out_bits > MAX_LUT_OUTPUT_BITS {
		return None;
	}

	let rows = 1u64 << in_bits;
	let table: Vec<Vec<u32>> = (0..rows).map(|r| vec![fill(r) as u32]).collect();
	Some(Lut::new(table))
}
