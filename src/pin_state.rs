//! Helpers for dealing with pin state.
//!
//! A pin (or bus of up to 8 pins) is represented by `PinState`: an enum tagged
//! by how many tristate wires it actually carries (1, 4, or 8 -- the same
//! widths `PinBitCount` supports), each variant packing its bits into a u16
//! under the hood (tri-state flags in the high byte, bit values in the low
//! byte). Exposing this as a real enum -- rather than a bare u16 -- means a
//! `PinState` can answer "how many wires am I?" for itself (`len`/`width`)
//! instead of callers having to track that separately, while every bit of
//! packing/unpacking logic still lives in this module so call sites doing
//! things like "give me bit 3 of this 8-bit bus" don't need to hand-roll
//! masks/shifts.
//!
//! Each individual wire is a `LogicState`: `Low`, `High`, or `Disconnected` (tri-state).

use crate::description::PinBitCount;
use num_enum::{IntoPrimitive, TryFromPrimitive};

/// Tri-state logic level for a single bit, used by the renderer to pick a
/// colour: `High`/`Low` map to the lit/dim variant of a pin's palette
/// colour, `Disconnected` always renders flat black regardless of palette
/// (mirrors `LOGIC_DISCONNECTED` / `DrawSettings.StateDisconnectedCol`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum LogicState {
	Low = 0,  // 00
	High = 1, // 01
	#[default]
	Disconnected = 2, // 10 - so off, with indicator bit
}

impl LogicState {
	/// Builds a `LogicState` from the raw tristated bit value (0/1/2, as packed by `PinState::bit`).
	#[inline(always)]
	pub fn from_int(a: u8) -> Self {
		Self::try_from(a).unwrap_or_default()
	}

	#[inline(always)]
	pub const fn to_int(self) -> u8 {
		self as u8 // a bit unsafe, but theoretically should work flawlessly
	}

	#[inline(always)]
	pub const fn from_bool(high: bool) -> Self {
		if high {
			LogicState::High
		} else {
			LogicState::Low
		}
	}

	#[inline(always)]
	pub const fn is_high(self) -> bool {
		matches!(self, LogicState::High)
	}

	#[inline(always)]
	pub const fn is_connected(self) -> bool {
		!matches!(self, LogicState::Disconnected)
	}
}

// Kept for call sites that still want the raw discriminant rather than the enum.
pub const LOGIC_HIGH: u8 = LogicState::High.to_int();
pub const LOGIC_LOW: u8 = LogicState::Low.to_int();
pub const LOGIC_DISCONNECTED: u8 = LogicState::Disconnected.to_int();

/// Packed state of a pin/bus, tagged by how many wires it carries. Bit values
/// live in the low byte of the packed u16, tri-state ("disconnected") flags
/// in the high byte -- same layout regardless of variant, so unused high
/// wires of a `Bit1`/`Bit4` value are simply left at zero. This is the thing
/// `SimPin::state` stores, and what flows along wires/buses during
/// simulation -- a 16-bit payload keeps every `SimPin` down to a size the
/// stepping hot loop keeps in cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinState {
	/// A single wire -- the width of a plain gate output or 1-bit dev-pin.
	Bit1(u16),
	/// A 4-wire bus/nibble.
	Bit4(u16),
	/// An 8-wire bus/byte -- the widest bus this sim supports.
	Bit8(u16),
}

impl PinState {
	/// A single connected LOW bit at index 0 -- the all-zero state.
	pub const LOW: PinState = PinState::from_raw_with_width(LOGIC_LOW as u16, PinBitCount::Bit1);
	/// A single connected HIGH bit at index 0.
	pub const HIGH: PinState = PinState::from_raw_with_width(LOGIC_HIGH as u16, PinBitCount::Bit1);
	/// A single Disconnected bit at index 0 (its tristate flag set).
	pub const OFF: PinState = PinState::from_raw_with_width((LOGIC_DISCONNECTED as u16) << 7, PinBitCount::Bit1);
	/// Every wire of the word disconnected -- e.g. a pin nothing has ever driven.
	pub const DISCONNECTED: PinState = PinState::from_raw((u8::MAX as u16) << 8);

	// --- width / length ---------------------------------------------------

	/// The real number of tristate wires this value carries: 1, 4, or 8.
	/// This is the whole point of `PinState` being an enum rather than a
	/// bare packed integer -- a value can report its own width.
	#[inline(always)]
	#[allow(clippy::len_without_is_empty)]
	pub const fn len(self) -> u32 {
		match self {
			PinState::Bit1(_) => 1,
			PinState::Bit4(_) => 4,
			PinState::Bit8(_) => 8,
		}
	}

	/// Same information as `len`, as the `PinBitCount` type used elsewhere
	/// (e.g. `PinDescription::bit_count`) for describing pin widths.
	#[inline(always)]
	pub const fn width(self) -> PinBitCount {
		match self {
			PinState::Bit1(_) => PinBitCount::Bit1,
			PinState::Bit4(_) => PinBitCount::Bit4,
			PinState::Bit8(_) => PinBitCount::Bit8,
		}
	}

	/// Re-tags this value to a different width, keeping the same packed
	/// bits as-is (bits outside the new width are neither cleared nor
	/// validated -- callers that care should `extract` first).
	#[inline(always)]
	pub const fn retagged(self, width: PinBitCount) -> Self {
		Self::tagged(width, self.raw())
	}

	/// Tags a raw packed word with an explicit `PinBitCount`. Unlike
	/// `from_raw_for_width` (which rounds an arbitrary wire count up to the
	/// nearest supported width), this maps `PinBitCount` directly -- no
	/// `to_int`/rounding involved, so it stays usable from `const fn`s.
	#[inline(always)]
	const fn tagged(width: PinBitCount, raw: u16) -> Self {
		match width {
			PinBitCount::Bit1 => PinState::Bit1(raw),
			PinBitCount::Bit4 => PinState::Bit4(raw),
			PinBitCount::Bit8 => PinState::Bit8(raw),
		}
	}

	/// Picks the narrowest supported width (1/4/8) that can hold `width`
	/// wires, and tags `raw` with it. Used internally wherever an operation
	/// (`extract`, `combine`, `from_bit`, ...) is given/implies a concrete
	/// wire count and needs to produce a correctly-tagged result.
	#[inline(always)]
	const fn from_raw_for_width(width: u32, raw: u16) -> Self {
		if width <= 1 {
			PinState::Bit1(raw)
		} else if width <= 4 {
			PinState::Bit4(raw)
		} else {
			PinState::Bit8(raw)
		}
	}

	/// Rebuilds `self` with a new packed payload, keeping the same width tag.
	#[inline(always)]
	const fn with_raw(self, raw: u16) -> Self {
		match self {
			PinState::Bit1(_) => PinState::Bit1(raw),
			PinState::Bit4(_) => PinState::Bit4(raw),
			PinState::Bit8(_) => PinState::Bit8(raw),
		}
	}

	#[inline(always)]
	const fn pack(bit_states: u8, tristate_flags: u8) -> u16 {
		(bit_states as u16) | ((tristate_flags as u16) << 8)
	}

	// --- raw <-> PinState -----------------------------------------------------

	/// Builds an 8-wide `PinState` from a raw packed word. Prefer
	/// `from_raw_with_width` when the real wire count is known.
	#[inline(always)]
	pub const fn from_raw(raw: u16) -> Self {
		PinState::Bit8(raw)
	}

	/// Builds a `PinState` of the given width from a raw packed word.
	#[inline(always)]
	pub const fn from_raw_with_width(raw: u16, width: PinBitCount) -> Self {
		Self::tagged(width, raw)
	}

	#[inline(always)]
	pub const fn raw(self) -> u16 {
		match self {
			PinState::Bit1(v) | PinState::Bit4(v) | PinState::Bit8(v) => v,
		}
	}

	/// Builds an 8-wide `PinState` directly from a bit-states byte and a
	/// tristate-flags byte (each bit `i` describes wire `i`). Prefer
	/// `from_parts_with_width` when the real wire count is known.
	#[inline(always)]
	pub const fn from_parts(bit_states: u8, tristate_flags: u8) -> Self {
		PinState::Bit8(Self::pack(bit_states, tristate_flags))
	}

	/// Builds a `PinState` of the given width from a bit-states byte and a
	/// tristate-flags byte (each bit `i` describes wire `i`).
	#[inline(always)]
	pub const fn from_parts_with_width(bit_states: u8, tristate_flags: u8, width: PinBitCount) -> Self {
		Self::tagged(width, Self::pack(bit_states, tristate_flags))
	}

	// --- constructors for common cases -----------------------------------------

	/// A single-wire state (bit index 0), e.g. the output of a gate.
	#[inline(always)]
	pub fn single(state: LogicState) -> Self {
		Self::from_bit(0, state)
	}

	#[inline(always)]
	pub fn from_bool(single: bool) -> Self {
		Self::single(LogicState::from_bool(single))
	}

	/// A single wire at `index` set to `state`, all other wires (up to
	/// `index`) low/connected. Tags the result with the narrowest width
	/// that can hold `index`.
	#[inline(always)]
	pub fn from_bit(index: u32, state: LogicState) -> Self {
		let mut s = Self::from_raw_for_width(index + 1, Self::pack(0, u8::MAX));
		s.set_bit(index, state);
		s
	}

	// --- whole-word access -------------------------------------------------

	#[inline(always)]
	pub const fn bit_states(self) -> u8 {
		self.raw() as u8
	}

	#[inline(always)]
	pub const fn tristate_flags(self) -> u8 {
		(self.raw() >> 8) as u8
	}

	#[inline(always)]
	pub fn set(&mut self, bit_states: u8, tristate_flags: u8) {
		*self = self.with_raw(Self::pack(bit_states, tristate_flags));
	}

	#[inline(always)]
	pub fn set_raw(&mut self, other: u16) {
		*self = self.with_raw(other);
	}

	#[inline(always)]
	pub fn set_all_disconnected(&mut self) {
		self.set(0, u8::MAX);
	}

	pub fn set_all_low(&mut self) {
		self.set(0, 0);
	}

	// --- single-bit access (the "no manual bitshift" API) ----------------------

	/// Reads the tristated value of wire `index` directly, e.g. `bus.bit(3)`.
	#[inline(always)]
	pub const fn bit(self, index: u32) -> LogicState {
		let bit = (self.bit_states() >> index) & 1;
		let tri = (self.tristate_flags() >> index) & 1;
		if tri != 0 {
			LogicState::Disconnected
		} else if bit != 0 {
			LogicState::High
		} else {
			LogicState::Low
		}
	}

	#[inline(always)]
	pub fn set_bit(&mut self, index: u32, state: LogicState) {
		// Edits the packed word in place -- clear/set exactly this wire's
		// value bit and tri-state flag, leaving the other seven untouched.
		let bit = 1u16 << index;
		let flag = bit << 8;
		let mut raw = self.raw();
		match state {
			LogicState::Low => raw &= !(bit | flag),
			LogicState::High => {
				raw |= bit;
				raw &= !flag;
			}
			LogicState::Disconnected => {
				raw &= !bit;
				raw |= flag;
			}
		}
		*self = self.with_raw(raw);
	}

	/// Builder-style variant of `set_bit`.
	#[inline(always)]
	pub fn with_bit(mut self, index: u32, state: LogicState) -> Self {
		self.set_bit(index, state);
		self
	}

	/// Whether wire 0 reads as `High` (ignores tri-state -- historically used for
	/// "is this control line asserted" checks).
	#[inline(always)]
	pub const fn first_bit_high(self) -> bool {
		matches!(self.bit(0), LogicState::High)
	}

	// --- sub-slices / composition ------------------------------------------

	/// Pulls out `width` wires starting at `offset`, right-aligned into a fresh
	/// `PinState` tagged with that same `width` (wires above `width` are masked
	/// off). This replaces the old `set_4bit_from_8bit_source` /
	/// `set_8bit_from_16bit_source` / manual `(x >> offset) & MASK` patterns:
	/// `byte.extract(4, 4)` is "the upper nibble", `byte.extract(3, 1)` is
	/// "just bit 3", etc.
	pub const fn extract(self, offset: u32, width: u32) -> PinState {
		let mask = width_mask(width);
		let bits = (self.bit_states() >> offset) & mask;
		let tris = (self.tristate_flags() >> offset) & mask;
		Self::from_raw_for_width(width, Self::pack(bits, tris))
	}

	/// Builds a fresh `PinState` by placing the low `width` wires of each `piece`
	/// at the given `dest_offset`, replacing the old `set_8bit_from_4bit_sources` /
	/// `set_16bit_from_8bit_sources` / `set_8bit_from_1bit_sources`-style helpers.
	/// e.g. an 8-bit bus from two nibbles: `PinState::combine(&[(low_nibble, 0, 4), (high_nibble, 4, 4)])`.
	/// The result is tagged with the widest span any `(dest_offset, width)` reaches.
	pub fn combine(parts: &[(PinState, u32, u32)]) -> PinState {
		let mut bits = 0u8;
		let mut tris = 0u8;
		let mut span = 1u32;
		for &(piece, dest_offset, width) in parts {
			let mask = width_mask(width);
			bits |= (piece.bit_states() & mask) << dest_offset;
			tris |= (piece.tristate_flags() & mask) << dest_offset;
			span = span.max(dest_offset + width);
		}
		Self::from_raw_for_width(span, Self::pack(bits, tris))
	}

	// --- editing ------------------------------------------------------------

	/// Flips wire `index`, clearing tri-state (can't be disconnected when toggling,
	/// as only input dev-pins are ever toggled directly).
	#[inline(always)]
	pub fn toggle_bit(&mut self, index: u32) {
		let raw = (self.raw() ^ (1u16 << index)) & 0x00FF;
		*self = self.with_raw(raw);
	}
}

// --- conflict resolution (used when two sources drive the same pin) --------
impl PinState {
	/// Shared tri-state merge: `combined` is the already-computed bitwise result of
	/// the boolean op (OR/AND/NAND) applied to both value bytes. Per bit:
	/// - both sides driven  -> use `combined`
	/// - only one side driven -> the driven side wins outright (floating input
	///   doesn't get a vote)
	/// - neither side driven -> stays disconnected
	///
	/// The result keeps `self`'s width tag (both sides are expected to carry
	/// the same width in practice, since they're driving the same pin).
	#[inline(always)]
	const fn merge_driven(self, other: Self, combined: u8) -> Self {
		let val_a = self.bit_states();
		let val_b = other.bit_states();
		let tri_a = self.tristate_flags();
		let tri_b = other.tristate_flags();

		let conn_a = !tri_a;
		let conn_b = !tri_b;
		let both_conn = conn_a & conn_b;
		let only_a = conn_a & !conn_b;
		let only_b = conn_b & !conn_a;

		let value = (combined & both_conn) | (val_a & only_a) | (val_b & only_b);
		let tri = tri_a & tri_b; // disconnected only if BOTH sides are disconnected

		self.with_raw(Self::pack(value, tri))
	}

	#[inline(always)]
	pub const fn or(self, other: Self) -> Self {
		let combined = self.bit_states() | other.bit_states();
		self.merge_driven(other, combined)
	}

	#[inline(always)]
	pub const fn and(self, other: Self) -> Self {
		let combined = self.bit_states() & other.bit_states();
		self.merge_driven(other, combined)
	}

	#[inline(always)]
	pub const fn nand(self, other: Self) -> Self {
		let combined = !(self.bit_states() & other.bit_states());
		self.merge_driven(other, combined)
	}

	/// Unary NOT: flips every driven bit, disconnected bits stay disconnected.
	/// (Value bits under a disconnected flag are never read by `bit()`, so we
	/// don't need to mask them off here -- just flip the whole value byte.)
	#[inline(always)]
	pub const fn not(self) -> Self {
		self.with_raw(Self::pack(!self.bit_states(), self.tristate_flags()))
	}
}

#[inline(always)]
const fn width_mask(width: u32) -> u8 {
	if width >= 8 {
		u8::MAX
	} else {
		((1u16 << width) - 1) as u8
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn len_matches_variant() {
		assert_eq!(PinState::LOW.len(), 1);
		assert_eq!(PinState::HIGH.len(), 1);
		assert_eq!(PinState::OFF.len(), 1);
		assert_eq!(PinState::DISCONNECTED.len(), 8);
		assert_eq!(PinState::from_raw_with_width(0, PinBitCount::Bit4).len(), 4);
	}

	#[test]
	fn width_round_trips_through_pin_bit_count() {
		assert_eq!(PinState::LOW.width(), PinBitCount::Bit1);
		assert_eq!(PinState::from_parts_with_width(0, 0, PinBitCount::Bit4).width(), PinBitCount::Bit4);
		assert_eq!(PinState::from_parts_with_width(0, 0, PinBitCount::Bit8).width(), PinBitCount::Bit8);
	}

	#[test]
	fn extract_tags_result_with_requested_width() {
		let byte = PinState::from_raw(0b1010_1010);
		assert_eq!(byte.extract(4, 4).len(), 4);
		assert_eq!(byte.extract(0, 1).len(), 1);
	}

	#[test]
	fn combine_tags_result_with_widest_span() {
		let nibble = PinState::from_raw_with_width(0b1111, PinBitCount::Bit4);
		let byte = PinState::combine(&[(nibble, 0, 4), (nibble, 4, 4)]);
		assert_eq!(byte.len(), 8);
	}

	#[test]
	fn retagged_keeps_bits_changes_width() {
		let v = PinState::from_raw(0x00FF).retagged(PinBitCount::Bit4);
		assert_eq!(v.len(), 4);
		assert_eq!(v.raw(), 0x00FF);
	}

	#[test]
	fn ops_preserve_and_pack_still_works() {
		let a = PinState::from_bit(0, LogicState::High);
		let b = PinState::from_bit(0, LogicState::Low);
		assert_eq!(a.len(), 1);
		assert_eq!(a.or(b).bit(0), LogicState::High);
		assert_eq!(a.and(b).bit(0), LogicState::Low);
	}
}
