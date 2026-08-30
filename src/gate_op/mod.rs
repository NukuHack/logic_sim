//! Optimized gate evaluation: pack a gate's pins into a small integer and
//! evaluate via table lookup or a native function instead of walking the
//! subchip graph.
//!
//! - `word`: `WireWord`, the packed-integer representation pins are converted to/from.
//! - `eval`: `OptimizedGate` and its implementations (`Lut`, `Native`, `NativeList`) --
//!   the actual fast path, usable today.
//! - `caching`: a not-yet-compiling sketch of *when* to build/use a `Lut` per
//!   combinational chip. Kept separate from `eval` since it's pseudocode, not
//!   working code -- see its module doc for what's missing.

mod caching;
mod eval;
mod word;

pub use caching::{
	calculate_num_input_bits, is_combinational, process_cached_chip, recalculate_cached_luts, reset_received_flags_on_all_pins, CachingState,
	CombinationalChipCache, NonCombinationalSet, MAX_NUM_INPUT_BITS_WHEN_AUTO_CACHING, MAX_NUM_INPUT_BITS_WHEN_USER_CACHING,
};
pub use eval::{Lut, Native, NativeList, OptimizedGate};
pub use word::{WideWord, WireWord};

// TODO: add a "Chip Caching: On/Off" checkbox to the customization menu (default off)
// that toggles a chip between the plain subchip walk and the `Lut`-backed path above.
// For some circuits "off" and "disabled" behave the same; for others (see caching.rs)
// they don't, since caching changes *when* outputs settle, not just how fast.
