//! Optimized gate evaluation: pack a gate's pins into a small integer and evaluate via table
//! lookup or a native function instead of walking the subchip graph. - `word`: `WireWord`,
//! the packed-integer representation pins are converted to/from. - `eval`: `OptimizedGate`
//! and its implementations (`Lut`, `Native`, `NativeList`) -- the actual fast path, usable
//! today. - `caching`: *when* to build/use a `Lut` per combinational chip --
//! `Simulator::step_sub_chip` (in `sim.rs`) calls straight into this module's
//! `process_cached_chip`/`recalculate_chip_cache` for every non-builtin subchip it steps.

mod caching;
mod eval;
mod recognize;
mod word;

pub use caching::{
	calculate_num_input_bits, is_combinational, process_cached_chip, recalculate_chip_cache, reset_received_flags_on_all_pins, CachingState,
	MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING,
};
pub use eval::{Bits, Lut, Native, NativeList, OptimizedGate};
pub use recognize::{recognize, registry, Candidate};
pub use word::{WideWord, WireWord};
