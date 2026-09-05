//! Arbitrary-width bit-vector helpers behind [`super::eval::Native`]/[`super::eval::NativeList`]'s
//! formulas.
//!
//! Bits are packed low-word-first, low-bit-first within each word, and these free functions
//! operate on plain `&[Bits]`/`Vec<Bits>` slices rather than any fixed-size integer type -- a
//! gate of *any* width packs into `words_for(bits)` words and every formula below
//! (`and`/`add`/`field`/...) already works over however many words that turns out to be, so
//! there's no 64-bit ceiling to hit.
//!
//! `eval` (via `Native`/`NativeList`) runs one of these formulas per gate per simulation step,
//! so the boolean-result helpers (`all_ones_masked`/`any_set_masked`/`parity_masked`/
//! `fields_equal`) are written to return a plain `bool` computed by scanning `a` in place --
//! zero heap allocations -- rather than by building masked/compared `Vec<Bits>` copies the way
//! composing `eq`/`and`/`mask` would. `recognize`'s registry (`AND2`, `OR_N`, `XNOR_2N`, ...)
//! uses these directly, since they back the most common gates in any circuit.

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

/// Splits `n` into a full-word count and the bit-width of the partial word above them (0 if `n`
/// divides evenly) -- every masking/truncating helper below needs to know which word is the
/// last significant one and how much of it counts, so this is computed once and shared rather
/// than re-derived per helper.
fn full_words_and_remainder(n: u32) -> (usize, u32) {
	((n / WORD_BITS) as usize, n % WORD_BITS)
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

/// Bitwise NOT of the low `n` bits of `a`, one word per output word. Masks the partial top word
/// inline instead of building an inverted copy and a separate mask `Vec` to AND it against, so
/// this is one allocation (the output itself) instead of three.
pub fn not(a: &[Bits], n: u32) -> Vec<Bits> {
	if n == 0 {
		return Vec::new();
	}
	let (full_words, rem) = full_words_and_remainder(n);
	(0..words_for(n))
		.map(|i| {
			let w = !a.get(i).copied().unwrap_or(0);
			if i < full_words {
				w
			} else {
				w & (((1 as Bits) << rem) - 1)
			}
		})
		.collect()
}

/// True iff every one of the low `n` bits of `a` is set -- the allocation-free core of `AND`
/// gates (`AND2`/`AND_N`/`NAND2`/`NAND_N`), scanned word-by-word in place instead of composing
/// `eq(&and(a, &mask(n)), &mask(n))`, which would build three throwaway `Vec`s to answer what's
/// really a single pass over `a`.
pub fn all_ones_masked(a: &[Bits], n: u32) -> bool {
	if n == 0 {
		return true;
	}
	let (full_words, rem) = full_words_and_remainder(n);
	for i in 0..full_words {
		if a.get(i).copied().unwrap_or(0) != Bits::MAX {
			return false;
		}
	}
	if rem != 0 {
		let m = ((1 as Bits) << rem) - 1;
		if a.get(full_words).copied().unwrap_or(0) & m != m {
			return false;
		}
	}
	true
}

/// True iff any of the low `n` bits of `a` is set -- the `OR`/`NOR` counterpart of
/// [`all_ones_masked`], same in-place scan instead of `!is_zero(&and(a, &mask(n)))`.
pub fn any_set_masked(a: &[Bits], n: u32) -> bool {
	if n == 0 {
		return false;
	}
	let (full_words, rem) = full_words_and_remainder(n);
	for i in 0..full_words {
		if a.get(i).copied().unwrap_or(0) != 0 {
			return true;
		}
	}
	if rem != 0 {
		let m = ((1 as Bits) << rem) - 1;
		if a.get(full_words).copied().unwrap_or(0) & m != 0 {
			return true;
		}
	}
	false
}

/// Parity (odd popcount) of the low `n` bits of `a` -- the `XOR`/`XNOR` counterpart, replacing
/// `popcount(&and(a, &mask(n))) & 1 == 1` with a running count over `a` directly.
pub fn parity_masked(a: &[Bits], n: u32) -> bool {
	if n == 0 {
		return false;
	}
	let (full_words, rem) = full_words_and_remainder(n);
	let mut ones = 0u32;
	for i in 0..full_words {
		ones += a.get(i).copied().unwrap_or(0).count_ones();
	}
	if rem != 0 {
		let m = ((1 as Bits) << rem) - 1;
		ones += (a.get(full_words).copied().unwrap_or(0) & m).count_ones();
	}
	ones & 1 == 1
}

/// The `word_index`-th packed word (0-indexed from the low bit) of the `n`-bit field starting
/// at bit `start` -- `field`'s per-word core, split out so [`fields_equal`] can compare two
/// fields word-by-word without materializing either one first.
fn field_word(a: &[Bits], start: u32, word_index: usize) -> Bits {
	let word_shift = (start / WORD_BITS) as usize + word_index;
	let bit_shift = start % WORD_BITS;
	let lo = a.get(word_shift).copied().unwrap_or(0) >> bit_shift;
	let hi = if bit_shift == 0 { 0 } else { a.get(word_shift + 1).copied().unwrap_or(0).checked_shl(WORD_BITS - bit_shift).unwrap_or(0) };
	lo | hi
}

/// Extracts the `n`-bit field starting at bit `start`, i.e. `(a >> start) & mask(n)` -- the
/// building block every candidate below uses to split a packed word into named operands (`a`,
/// `c`, `cin`, ...) regardless of how many words wide the whole gate is. Masks the partial top
/// word inline rather than ANDing the whole result against a separately built mask `Vec`.
pub fn field(a: &[Bits], start: u32, n: u32) -> Vec<Bits> {
	if n == 0 {
		return Vec::new();
	}
	let (full_words, rem) = full_words_and_remainder(n);
	(0..words_for(n))
		.map(|w| {
			let word = field_word(a, start, w);
			if w < full_words {
				word
			} else {
				word & (((1 as Bits) << rem) - 1)
			}
		})
		.collect()
}

/// True iff the `n`-bit fields starting at `start_a` and `start_b` are equal -- the `XNOR_2N`
/// building block, compared word-by-word via [`field_word`] so neither field is ever
/// materialized as its own `Vec` just to be diffed and discarded.
pub fn fields_equal(a: &[Bits], start_a: u32, start_b: u32, n: u32) -> bool {
	if n == 0 {
		return true;
	}
	let (full_words, rem) = full_words_and_remainder(n);
	for w in 0..words_for(n) {
		let diff = field_word(a, start_a, w) ^ field_word(a, start_b, w);
		let m = if w < full_words { Bits::MAX } else { ((1 as Bits) << rem) - 1 };
		if diff & m != 0 {
			return false;
		}
	}
	true
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
/// the top remaining word). Masks the partial top word inline instead of ANDing against a
/// separately built mask `Vec`, same one-allocation shape as [`not`]/[`field`].
pub fn truncate(a: &[Bits], n: u32) -> Vec<Bits> {
	if n == 0 {
		return Vec::new();
	}
	let (full_words, rem) = full_words_and_remainder(n);
	(0..words_for(n))
		.map(|i| {
			let w = a.get(i).copied().unwrap_or(0);
			if i < full_words {
				w
			} else {
				w & (((1 as Bits) << rem) - 1)
			}
		})
		.collect()
}

/// Wraps a single-bit boolean result as the one-word `Vec<Bits>` every 1-output-bit candidate
/// (`AND_N`, `XOR_N`, ...) returns.
pub fn bit_result(b: bool) -> Vec<Bits> {
	vec![b as Bits]
}

/// Shifts `a` left by `amount` bits, growing the output by as many words as needed -- the
/// building block [`concat`] uses to make room for a low field it's placing below `a`. Unlike
/// [`field`]/[`truncate`], this never drops bits: shifting `n` bits left by `k` needs `n + k`
/// bits of output, so the result is sized accordingly rather than masked back down to `a`'s own
/// width.
pub fn shift_left(a: &[Bits], amount: u32) -> Vec<Bits> {
	if amount == 0 {
		return a.to_vec();
	}
	let word_shift = (amount / WORD_BITS) as usize;
	let bit_shift = amount % WORD_BITS;
	let mut out = vec![0 as Bits; a.len() + word_shift + 1];
	for (i, &word) in a.iter().enumerate() {
		if word == 0 {
			continue;
		}
		let idx = i + word_shift;
		if bit_shift == 0 {
			out[idx] |= word;
		} else {
			out[idx] |= word << bit_shift;
			out[idx + 1] |= word >> (WORD_BITS - bit_shift);
		}
	}
	out
}

/// Concatenates two bit-vectors into one, `low` occupying the bottom `low_bits` bits and `high`
/// stacked immediately above -- i.e. `low_masked | (high << low_bits)`. This is [`field`]'s
/// inverse: where `field` pulls a sub-range back out of a packed word, `concat` is how a
/// formula reassembles a result out of pieces that arrived in a different order than the table
/// row they're built from (e.g. a carry-out pin a chip designer put *before* the sum bits
/// instead of after).
pub fn concat(low: &[Bits], low_bits: u32, high: &[Bits]) -> Vec<Bits> {
	if low_bits == 0 {
		return high.to_vec();
	}
	let low_masked = truncate(low, low_bits);
	let high_shifted = shift_left(high, low_bits);
	or(&low_masked, &high_shifted)
}
