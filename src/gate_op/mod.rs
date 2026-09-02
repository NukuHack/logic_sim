//! Optimized gate evaluation: pack a gate's pins into a small integer and evaluate via table
//! lookup or a native function instead of walking the subchip graph.
//!
//! - `bitvec`: arbitrary-width bit-vector helpers (`WireWord`'s packed-integer representation
//!   that pins are converted to/from).
//! - `eval`: `OptimizedGate` and its implementations (`Lut`, `Native`, `NativeList`) -- the
//!   actual fast path, usable today.
//! - `recognize`: matches a materialized `Lut` against known gate patterns and hands back an
//!   equivalent `Native`.
//! - `caching`: *when* to build/use a `Lut` per combinational chip -- `Simulator::step_sub_chip`
//!   (in `sim.rs`) calls straight into this module's `recalculate_chip_cache` for every
//!   non-builtin subchip it steps.

mod bitvec;
mod build;
mod caching;
mod eval;
mod recognize;

pub use build::{build_lut, MAX_LUT_INPUT_BITS, MAX_LUT_OUTPUT_BITS};

pub use caching::{
	calculate_num_input_bits, is_combinational, recalculate_chip_cache, reset_received_flags_on_all_pins, CachedGate, CachingState, LutGate,
	MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING,
};
pub use eval::{Bits, Lut, Native, NativeList, OptimizedGate, WideWord, WireWord};
pub use recognize::{recognize, registry, Candidate};
