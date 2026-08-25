//! Helpers for dealing with pin state.
//! Pin state is stored as a u32, with format:
//! Tristate flags (most significant 16 bits) | Bit states (least significant 16 bits)

use num_enum::{IntoPrimitive, TryFromPrimitive};

/// Mask for a single bit value (bit state, and tristate flag)
pub const SINGLE_BIT_MASK: u32 = 1 | (1 << 16);

#[inline(always)]
pub fn bit_states(state: u32) -> u16 {
	state as u16
}

#[inline(always)]
pub fn tristate_flags(state: u32) -> u16 {
	(state >> 16) as u16
}

#[inline(always)]
pub fn set(state: &mut u32, bit_states: u16, tristate_flags: u16) {
	*state = (bit_states as u32) | ((tristate_flags as u32) << 16);
}

#[inline(always)]
pub fn set_raw(state: &mut u32, other: u32) {
	*state = other;
}

#[inline(always)]
pub fn get_bit_tristated_value(state: u32, bit_index: u32) -> u16 {
	let bit_state = (bit_states(state) >> bit_index) & 1;
	let tri = (tristate_flags(state) >> bit_index) & 1;
	bit_state | (tri << 1) // 0 = LOW, 1 = HIGH, 2 = DISCONNECTED
}

#[inline(always)]
pub fn first_bit_high(state: u32) -> bool {
	(state & 1) as u8 == LOGIC_HIGH
}

#[inline(always)]
pub fn set_all_disconnected(state: &mut u32) {
	set(state, 0, u16::MAX);
}

const LOW_MASK: u16 = 0b1111;
const HIGH_MASK: u16 = 0b1111_0000;
pub fn set_4bit_from_8bit_source(state: &mut u32, source_8bit: u32, first_nibble: bool) {
	let source_bit_states = bit_states(source_8bit);
	let source_tristate_flags = tristate_flags(source_8bit);

	if first_nibble {
		set(state, source_bit_states & LOW_MASK, source_tristate_flags & LOW_MASK);
	} else {
		set(state, (source_bit_states & HIGH_MASK) >> 4, (source_tristate_flags & HIGH_MASK) >> 4);
	}
}

pub fn set_8bit_from_4bit_sources(state: &mut u32, a: u32, b: u32) {
	let bit_states_val = bit_states(a) | (bit_states(b) << 4);
	let tristate_val = (tristate_flags(a) & 0b1111) | ((tristate_flags(b) & 0b1111) << 4);
	set(state, bit_states_val, tristate_val);
}

/// Tri-state logic level for a single bit, used by the renderer to pick a
/// colour: `High`/`Low` map to the lit/dim variant of a pin's palette
/// colour, `Disconnected` always renders flat black regardless of palette
/// (mirrors `LOGIC_DISCONNECTED` / `DrawSettings.StateDisconnectedCol`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum LogicState {
	Low = 0,
	High = 1,
	#[default]
	Disconnected = 2,
}
pub const LOGIC_HIGH: u8 = LogicState::High.to_int();
pub const LOGIC_LOW: u8 = LogicState::Low.to_int();
pub const LOGIC_DISCONNECTED: u8 = LogicState::Disconnected.to_int();
impl LogicState {
	/// Builds a `LogicState` from the raw tristated bit value
	pub fn from_int(a: u8) -> Self {
		Self::try_from(a).unwrap_or_default()
	}

	pub const fn to_int(&self) -> u8 {
		*self as u8 // a bit unsafe, but theoretically should work flawlessly
	}
}

pub fn toggle(state: &mut u32, bit_index: u32) {
	let mut bits = bit_states(*state);
	bits ^= 1u16 << bit_index;
	// Clear tristate flags (can't be disconnected if toggling, as only input dev pins are allowed)
	set(state, bits, 0);
}
