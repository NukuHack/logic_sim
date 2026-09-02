//! Arbitrary-width bit-vector helpers behind [`super::eval::Native`]'s formulas.
//!
//! Bits are packed low-word-first, low-bit-first within each word -- the same layout
//! `WireWord` uses for its fixed-size integers -- except these free functions operate on plain
//! `&[Bits]`/`Vec<Bits>` slices instead of a concrete `u8..u128`/`WideWord` type. That's what
//! lets `Native` drop its old `In: WireWord` bound entirely: a gate of *any* width packs into
//! `words_for(bits)` words and every formula below (`and`/`add`/`field`/...) already works over
//! however many words that turns out to be, so there's no 64-bit ceiling left to hit.

use crate::{gate_op::Bits, pin_state::LogicState};

const WORD_BITS: u32 = Bits::BITS;

/// Number of `Word`s needed to hold `bits` bits.
pub fn words_for(bits: u32) -> usize {
	if bits == 0 {
		0
	} else {
		((bits - 1) / WORD_BITS + 1) as usize
	}
}

/// Packs a `LogicState` slice into words, low pin first -- same convention as `WireWord::pack`,
/// just not capped at 128 bits.
pub fn pack_words(bits: &[LogicState]) -> Vec<Bits> {
	let mut out = vec![0 as Bits; words_for(bits.len() as u32)];
	for (i, s) in bits.iter().enumerate() {
		if s.is_high() {
			out[i / WORD_BITS as usize] |= 1 << (i as u32 % WORD_BITS);
		}
	}
	out
}

/// Inverse of [`pack_words`]: unpacks words back into per-pin states. Missing high words (a
/// formula that returned fewer words than `out` needs) read as zero.
pub fn unpack_words(words: &[Bits], out: &mut [LogicState]) {
	for (i, slot) in out.iter_mut().enumerate() {
		let w = words.get(i / WORD_BITS as usize).copied().unwrap_or(0);
		*slot = LogicState::from_bool((w >> (i as u32 % WORD_BITS)) & 1 == 1);
	}
}

/// All-ones for the low `n` bits, zero above -- the arbitrary-width analogue of `recognize`'s
/// old `mask(n) -> u64`.
pub fn mask(n: u32) -> Vec<Bits> {
	let mut out = vec![0 as Bits; words_for(n)];
	let full_words = (n / WORD_BITS) as usize;
	for w in out.iter_mut().take(full_words) {
		*w = Bits::MAX;
	}
	let rem = n % WORD_BITS;
	if rem != 0 {
		out[full_words] = ((1 as Bits) << rem) - 1;
	}
	out
}

fn zip_map(a: &[Bits], b: &[Bits], f: impl Fn(Bits, Bits) -> Bits) -> Vec<Bits> {
	let len = a.len().max(b.len());
	(0..len).map(|i| f(a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0))).collect()
}

pub fn and(a: &[Bits], b: &[Bits]) -> Vec<Bits> {
	zip_map(a, b, |x, y| x & y)
}
pub fn or(a: &[Bits], b: &[Bits]) -> Vec<Bits> {
	zip_map(a, b, |x, y| x | y)
}
pub fn xor(a: &[Bits], b: &[Bits]) -> Vec<Bits> {
	zip_map(a, b, |x, y| x ^ y)
}
/// Bitwise NOT of `a`, masked down to `n` bits (there's no fixed-width type to bound the
/// inversion otherwise).
pub fn not(a: &[Bits], n: u32) -> Vec<Bits> {
	and(&a.iter().map(|&x| !x).collect::<Vec<_>>(), &mask(n))
}

pub fn is_zero(a: &[Bits]) -> bool {
	a.iter().all(|&w| w == 0)
}
pub fn eq(a: &[Bits], b: &[Bits]) -> bool {
	is_zero(&xor(a, b))
}
pub fn popcount(a: &[Bits]) -> u32 {
	a.iter().map(|w| w.count_ones()).sum()
}

/// Extracts the `n`-bit field starting at bit `start`, i.e. `(a >> start) & mask(n)` -- the
/// building block every candidate below uses to split a packed word into named operands (`a`,
/// `c`, `cin`, ...) regardless of how many words wide the whole gate is.
pub fn field(a: &[Bits], start: u32, n: u32) -> Vec<Bits> {
	if n == 0 {
		return Vec::new();
	}
	let word_shift = (start / WORD_BITS) as usize;
	let bit_shift = start % WORD_BITS;
	let out_len = words_for(n);
	let mut out = vec![0 as Bits; out_len];
	for (i, slot) in out.iter_mut().enumerate() {
		let lo = a.get(word_shift + i).copied().unwrap_or(0) >> bit_shift;
		let hi = if bit_shift == 0 { 0 } else { a.get(word_shift + i + 1).copied().unwrap_or(0).checked_shl(WORD_BITS - bit_shift).unwrap_or(0) };
		*slot = lo | hi;
	}
	and(&out, &mask(n))
}

/// Ripple-carry add across as many words as needed, growing the result by a word if the final
/// carry overflows the wider operand's width (callers mask back down with [`truncate`] when a
/// carry-out isn't wanted).
pub fn add(a: &[Bits], b: &[Bits]) -> Vec<Bits> {
	let len = a.len().max(b.len());
	let mut out = Vec::with_capacity(len + 1);
	let mut carry: u128 = 0;
	for i in 0..len {
		let sum = carry + a.get(i).copied().unwrap_or(0) as u128 + b.get(i).copied().unwrap_or(0) as u128;
		out.push(sum as Bits);
		carry = sum >> WORD_BITS;
	}
	if carry != 0 {
		out.push(carry as Bits);
	}
	out
}

/// Masks `a` down to exactly `n` bits (drops any excess words, zeroes any excess high bits in
/// the top remaining word).
pub fn truncate(a: &[Bits], n: u32) -> Vec<Bits> {
	and(a, &mask(n))
}

/// Wraps a single-bit boolean result as the one-word `Vec<Bits>` every 1-output-bit candidate
/// (`AND_N`, `XOR_N`, ...) returns.
pub fn bit_result(b: bool) -> Vec<Bits> {
	vec![b as Bits]
}
