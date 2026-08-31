//! Packs gate pin states into fixed-width integers for fast evaluation.
//! Defines the `WireWord` trait and its implementations for the built-in
//! integer types.

use crate::pin_state::LogicState;

/// A gate's pins packed into one integer, sized to fit that gate exactly (a 4-bit
/// gate uses `u8`, not `u128`). Packing lets evaluation work on a cheap integer
/// instead of walking a `&[LogicState]` slice every time.
///
/// `Clone` rather than `Copy`: every inline int type is `Copy` for free, but
/// `WideWord` owns a heap-allocated `Box<[u64]>` and can't be.
pub trait WireWord: Clone + Eq + Send + Sync + 'static {
	/// Packs each `LogicState` in `bits` into one bit, low pin first.
	fn pack(bits: &[LogicState]) -> Self;
	/// Unpacks back into per-pin states, inverse of `pack`.
	fn unpack(self, out: &mut [LogicState]);
	/// This word's value as a `Vec` index. Only meaningful for word sizes small
	/// enough to serve as a dense lookup-table index (see `WideWord`).
	fn as_index(&self) -> usize;

	/// This word's bits widened to `u64`, same low-bit-first layout as
	/// `pack`/`unpack`. Lets code that only cares about the bit pattern (e.g.
	/// `recognize`) work uniformly across `u8..u128` without being generic
	/// over which one it got. Only meaningful for widths <= 64; wider words
	/// are truncated to their low 64 bits (see `WideWord`'s impl).
	fn to_u64(&self) -> u64;
}

macro_rules! impl_wire_word {
	($($t:ty),*) => {$(
		impl WireWord for $t {
			fn pack(bits: &[LogicState]) -> Self {
				bits.iter().enumerate()
					.fold(0, |acc, (i, b)| acc | ((b.is_high() as $t) << i))
			}
			fn unpack(self, out: &mut [LogicState]) {
				for (i, slot) in out.iter_mut().enumerate() {
					*slot = LogicState::from_bool((self >> i) & 1 == 1);
				}
			}
			fn as_index(&self) -> usize {
				*self as usize
			}
			fn to_u64(&self) -> u64 {
				*self as u64 // truncating for u128, which is fine: to_u64's contract is low-64-bits-only
			}
		}
	)*};
}
impl_wire_word!(u8, u16, u32, u64, u128);

/// Escape hatch for buses wider than 128 wires; everything narrower uses one of
/// the inline int types above instead. Not usable as a `Lut` index -- a dense
/// table over a >128-bit input space isn't buildable, so `as_index` panics.
#[derive(Clone, PartialEq, Eq)]
pub struct WideWord {
	bits: Box<[u64]>, // one bit per LogicState.is_high(); ceil(n_wires / 64) words
}

impl WireWord for WideWord {
	fn pack(bits: &[LogicState]) -> Self {
		let words = bits.chunks(64).map(|chunk| chunk.iter().enumerate().fold(0u64, |acc, (i, b)| acc | ((b.is_high() as u64) << i))).collect();
		WideWord { bits: words }
	}
	fn unpack(self, out: &mut [LogicState]) {
		for (i, slot) in out.iter_mut().enumerate() {
			let word = self.bits[i / 64];
			*slot = LogicState::from_bool((word >> (i % 64)) & 1 == 1);
		}
	}
	fn as_index(&self) -> usize {
		unreachable!("WideWord inputs are too wide for a dense Lut; use Native/NativeList instead")
	}
	fn to_u64(&self) -> u64 {
		self.bits.first().copied().unwrap_or(0)
	}
}
