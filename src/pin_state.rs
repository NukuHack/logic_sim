//! Helpers for dealing with pin state.
//!
//! A pin (or bus of up to 16 pins) is represented by `PinState`: a packed u32 under
//! the hood (tristate flags in the high 16 bits, bit values in the low 16 bits), but
//! exposed as a real type with methods, so callers doing things like "give me bit 3
//! of this 8-bit bus" don't need to hand-roll masks/shifts outside this module.
//!
//! Each individual wire is a `LogicState`: `Low`, `High`, or `Disconnected` (tri-state).

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

	// output is = "is_low"
	#[inline(always)]
	const fn bit_value(self) -> bool {
		self.is_high()
	}

	// output is = "is_disonnected"
	#[inline(always)]
	const fn tristate_value(self) -> bool {
		!self.is_connected()
	}
}

// Kept for call sites that still want the raw discriminant rather than the enum.
pub const LOGIC_HIGH: u8 = LogicState::High.to_int();
pub const LOGIC_LOW: u8 = LogicState::Low.to_int();
pub const LOGIC_DISCONNECTED: u8 = LogicState::Disconnected.to_int();

/// Packed state of up to 16 pins/wires: bit values in the low 16 bits, tri-state
/// ("disconnected") flags in the high 16 bits. This is the thing `SimPin::state`
/// stores, and what flows along wires/buses during simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinState(u32);

impl Default for PinState {
	fn default() -> Self {
		Self::DISCONNECTED
	}
}

impl PinState {
	/// A single connected LOW bit at index 0 -- the all-zero state.
	pub const LOW: PinState = PinState::from_raw(LOGIC_LOW as u32);
	/// A single connected HIGH bit at index 0.
	pub const HIGH: PinState = PinState::from_raw(LOGIC_HIGH as u32);
	/// A single Disconnected bit at index 0 (its tristate flag set;
	pub const OFF: PinState = PinState::from_parts(0, 1);
	/// Every wire of the word disconnected -- e.g. a pin nothing has ever driven.
	pub const DISCONNECTED: PinState = PinState::from_parts(0, u16::MAX);

	// --- raw <-> PinState -----------------------------------------------------

	#[inline(always)]
	pub const fn from_raw(raw: u32) -> Self {
		Self(raw)
	}

	#[inline(always)]
	pub const fn raw(self) -> u32 {
		self.0
	}

	/// Builds directly from a bit-states word and a tristate-flags word (each bit
	/// `i` of `bit_states`/`tristate_flags` describes wire `i`).
	#[inline(always)]
	pub const fn from_parts(bit_states: u16, tristate_flags: u16) -> Self {
		Self((bit_states as u32) | ((tristate_flags as u32) << 16))
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

	/// A single wire at `index` set to `state`, all other wires low/connected.
	#[inline(always)]
	pub fn from_bit(index: u32, state: LogicState) -> Self {
		let mut s = Self::default();
		s.set_bit(index, state);
		s
	}

	// --- whole-word access -------------------------------------------------

	#[inline(always)]
	pub const fn bit_states(self) -> u16 {
		self.0 as u16
	}

	#[inline(always)]
	pub const fn tristate_flags(self) -> u16 {
		(self.0 >> 16) as u16
	}

	#[inline(always)]
	pub fn set(&mut self, bit_states: u16, tristate_flags: u16) {
		*self = Self::from_parts(bit_states, tristate_flags);
	}

	#[inline(always)]
	pub fn set_raw(&mut self, other: u32) {
		self.0 = other;
	}

	#[inline(always)]
	pub fn set_all_disconnected(&mut self) {
		self.set(0, u16::MAX);
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
		let mask = 1u16 << index;
		let bits = (self.bit_states() & !mask) | ((state.bit_value() as u16) << index);
		let tris = (self.tristate_flags() & !mask) | ((state.tristate_value() as u16) << index);
		self.set(bits, tris);
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
	/// `PinState` (wires above `width` are masked off). This replaces the old
	/// `set_4bit_from_8bit_source` / `set_8bit_from_16bit_source` / manual
	/// `(x >> offset) & MASK` patterns: `byte.extract(4, 4)` is "the upper nibble",
	/// `byte.extract(3, 1)` is "just bit 3", etc.
	pub const fn extract(self, offset: u32, width: u32) -> PinState {
		let mask = width_mask(width);
		let bits = (self.bit_states() >> offset) & mask;
		let tris = (self.tristate_flags() >> offset) & mask;
		PinState::from_parts(bits, tris)
	}

	/// Builds a fresh `PinState` by placing the low `width` wires of each `piece`
	/// at the given `dest_offset`, replacing the old `set_8bit_from_4bit_sources` /
	/// `set_16bit_from_8bit_sources` / `set_8bit_from_1bit_sources`-style helpers.
	/// e.g. an 8-bit bus from two nibbles: `PinState::combine(&[(low_nibble, 0, 4), (high_nibble, 4, 4)])`.
	pub fn combine(parts: &[(PinState, u32, u32)]) -> PinState {
		let mut bits = 0u16;
		let mut tris = 0u16;
		for &(piece, dest_offset, width) in parts {
			let mask = width_mask(width);
			bits |= (piece.bit_states() & mask) << dest_offset;
			tris |= (piece.tristate_flags() & mask) << dest_offset;
		}
		PinState::from_parts(bits, tris)
	}

	// --- editing ------------------------------------------------------------

	/// Flips wire `index`, clearing tri-state (can't be disconnected when toggling,
	/// as only input dev-pins are ever toggled directly).
	#[inline(always)]
	pub fn toggle_bit(&mut self, index: u32) {
		let bits = self.bit_states() ^ (1u16 << index);
		self.set(bits, 0);
	}
}

// --- conflict resolution (used when two sources drive the same pin) --------
impl PinState {
	/// Shared tri-state merge: `combined` is the already-computed bitwise result of
	/// the boolean op (OR/AND/NAND) applied to both value words. Per bit:
	/// - both sides driven  -> use `combined`
	/// - only one side driven -> the driven side wins outright (floating input
	///   doesn't get a vote)
	/// - neither side driven -> stays disconnected
	#[inline(always)]
	const fn merge_driven(self, other: Self, combined: u16) -> Self {
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

		Self::from_parts(value, tri)
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
	/// don't need to mask them off here -- just flip the whole value word.)
	#[inline(always)]
	pub const fn not(self) -> Self {
		Self::from_parts(!self.bit_states(), self.tristate_flags())
	}
}

#[inline(always)]
const fn width_mask(width: u32) -> u16 {
	if width >= 16 {
		u16::MAX
	} else {
		((1u32 << width) - 1) as u16
	}
}
