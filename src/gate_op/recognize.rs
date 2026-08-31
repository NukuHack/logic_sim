//! Recognizes a materialized [`Lut`] as a known gate pattern (AND, adder, etc.) and, on a
//! match, hands back a [`Formula`] that computes the same function from a tiny closed-form
//! expression over packed bits instead of a stored table.
//!
//! Why this matters: a `Lut<In, Out>` is only as cheap as its table is small. A 20-input gate
//! is already a million-row table; a handful of bits more and it stops being buildable at all.
//! `Formula` is O(1) to evaluate and O(1) to store regardless of width, so recognizing a wide
//! `Lut` as e.g. `ADDER_N` turns an expensive (or outright impossible) table into a couple of
//! shifts, masks and an add. Recognition itself streams the candidate's output row-by-row
//! against the real table and bails at the first mismatch, so a non-matching candidate is
//! usually rejected in a handful of comparisons rather than a full table diff.

use super::eval::{Lut, OptimizedGate};
use super::word::WireWord;
use crate::pin_state::LogicState;

/// Packed bits, up to 64 of them, low bit first -- the common currency both `Formula` and the
/// candidate registry deal in. Using a plain `u64` (rather than `Vec<bool>`/generic `WireWord`)
/// means every candidate check and every `Formula::eval` is pure register arithmetic: no heap
/// allocation, no per-bit branches, no monomorphization per word type.
type Bits = u64;

/// `1` bits in positions `[0, n)`, saturating at `u64::MAX` for `n >= 64` instead of overflowing
/// the shift. Every candidate formula below uses this to isolate an N-bit field.
#[inline(always)]
fn mask(n: u32) -> Bits {
	if n >= 64 {
		Bits::MAX
	} else {
		(1u64 << n) - 1
	}
}

/// A gate evaluated from a closed-form function over packed bits rather than a stored table.
/// This is what a matched candidate becomes: no table, so no memory cost and no cache-miss risk
/// no matter how wide the gate is, at the cost of a few ALU ops per eval instead of one load.
pub struct Formula {
	in_bits: u32,
	out_bits: u32,
	config: Bits,
	f: fn(Bits, u32, u32, Bits) -> Bits,
}
impl Formula {
	pub fn new(in_bits: u32, out_bits: u32, config: Bits, f: fn(Bits, u32, u32, Bits) -> Bits) -> Self {
		Self { in_bits, out_bits, config, f }
	}
}

impl OptimizedGate for Formula {
	fn eval(&self, input: &[LogicState], output: &mut [LogicState]) {
		debug_assert_eq!(input.len() as u32, self.in_bits);
		debug_assert_eq!(output.len() as u32, self.out_bits);

		let mut word: Bits = 0;
		for (i, s) in input.iter().enumerate() {
			word |= (s.is_high() as Bits) << i;
		}

		let result = (self.f)(word, self.in_bits, self.out_bits, self.config);

		for (i, slot) in output.iter_mut().enumerate() {
			*slot = LogicState::from_bool((result >> i) & 1 == 1);
		}
	}
}

/// One known gate pattern. `applicable` covers both fixed-shape gates (e.g. `AND2` wants
/// exactly `i==2, o==1`) and parametric families (e.g. `AND_N` wants any `i>=2, o==1`) with the
/// same predicate, so there's no separate "shape" enum to keep in sync with `formula`.
///
/// `config` is a per-instance parameter passed through to `applicable`/`formula` alongside
/// `in_bits`/`out_bits`. Most candidates ignore it, but it's what lets an entire *family* of
/// related gates (e.g. adder with/without carry-in, with/without carry-out) share one pair of
/// plain `fn` pointers instead of hand-writing a near-duplicate candidate per combination --
/// see `adder_variants` below. Still just an integer + two function-pointer calls per row, so
/// there's no closure/`Box<dyn Fn>` overhead versus a hardcoded variant.
pub struct Candidate {
	name: &'static str,
	config: Bits,
	applicable: fn(in_bits: u32, out_bits: u32, config: Bits) -> bool,
	formula: fn(Bits, u32, u32, Bits) -> Bits,
}
impl Candidate {
	pub fn name(&self) -> &'static str {
		self.name
	}
	pub fn config(&self) -> Bits {
		self.config
	}
	pub fn formula(&self) -> fn(Bits, u32, u32, Bits) -> Bits {
		self.formula
	}
}

#[allow(clippy::manual_range_contains)]
pub fn registry() -> &'static [Candidate] {
	static REGISTRY: std::sync::OnceLock<Vec<Candidate>> = std::sync::OnceLock::new();
	REGISTRY.get_or_init(|| {
		vec![
			// NOT
			Candidate { name: "NOT", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| (!w) & 1 },
			// BUFFER
			Candidate { name: "BUFFER", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| w & 1 },
			// 2-input AND
			Candidate { name: "AND2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| ((w & 0b11) == 0b11) as Bits },
			// 2-input OR
			Candidate { name: "OR2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| ((w & 0b11) != 0) as Bits },
			// 2-input XOR
			Candidate { name: "XOR2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| (w & 1) ^ ((w >> 1) & 1) },
			// 2-input NAND
			Candidate { name: "NAND2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| ((w & 0b11) != 0b11) as Bits },
			// 2-input NOR
			Candidate { name: "NOR2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| ((w & 0b11) == 0) as Bits },
			// 2-input XNOR
			Candidate { name: "XNOR2", config: 0, applicable: |i, o, _| i == 2 && o == 1, formula: |w, _, _, _| 1 ^ ((w & 1) ^ ((w >> 1) & 1)) },
			// N-input AND (fast: compare against all-ones)
			Candidate { name: "AND_N", config: 0, applicable: |i, o, _| o == 1 && i >= 2, formula: |w, i, _, _| ((w & mask(i)) == mask(i)) as Bits },
			// N-input OR (fast: test nonzero)
			Candidate { name: "OR_N", config: 0, applicable: |i, o, _| o == 1 && i >= 2, formula: |w, i, _, _| ((w & mask(i)) != 0) as Bits },
			// N-input XOR (fast: parity via popcount)
			Candidate {
				name: "XOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| (w & mask(i)).count_ones() as Bits & 1,
			},
			// N-input NAND (fast: compare against all-ones)
			Candidate { name: "NAND_N", config: 0, applicable: |i, o, _| o == 1 && i >= 2, formula: |w, i, _, _| ((w & mask(i)) != mask(i)) as Bits },
			// N-input NOR (fast: test zero)
			Candidate { name: "NOR_N", config: 0, applicable: |i, o, _| o == 1 && i >= 2, formula: |w, i, _, _| ((w & mask(i)) == 0) as Bits },
			// N-bit XNOR (2N inputs: equality comparison between two N-bit fields)
			// Note: this is placed before general XNOR_N to ensure 2N-equality matching takes priority
			Candidate {
				name: "XNOR_2N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 4 && i % 2 == 0,
				formula: |w, i, _, _| {
					let n = i >> 1; // divide by 2 faster than /
					let m = mask(n);
					(((w & m) ^ ((w >> n) & m)) == 0) as Bits
				},
			},
			// N-input XNOR (true XNOR: odd parity of inputs)
			Candidate {
				name: "XNOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| ((w & mask(i)).count_ones() & 1) as Bits ^ 1, // XNOR is inverse of XOR
			},
			// N-bit NOT (fast: invert then mask)
			Candidate { name: "NOT_N", config: 0, applicable: |i, o, _| i == o && i >= 2 && i <= 64, formula: |w, i, _, _| (!w) & mask(i) },
			// N-bit BUFFER (fast: mask only)
			Candidate { name: "BUFFER_N", config: 0, applicable: |i, o, _| i == o && i >= 2 && i <= 64, formula: |w, i, _, _| w & mask(i) },
			// N-bit AND (fast: two masks and AND)
			Candidate {
				name: "AND_N_WIDE",
				config: 0,
				applicable: |i, o, _| {
					o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2 // bit ops faster
				},
				formula: |w, _i, o, _| {
					let m = mask(o);
					(w & m) & ((w >> o) & m)
				},
			},
			// N-bit OR (fast: two masks and OR)
			Candidate {
				name: "OR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| {
					let m = mask(o);
					(w & m) | ((w >> o) & m)
				},
			},
			// N-bit XOR (fast: two masks and XOR)
			Candidate {
				name: "XOR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| {
					let m = mask(o);
					(w & m) ^ ((w >> o) & m)
				},
			},
			// N-bit XNOR (true bitwise XNOR: inverse of XOR for each bit position)
			Candidate {
				name: "XNOR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| {
					let m = mask(o);
					!((w & m) ^ ((w >> o) & m)) & m // XNOR is inverse of XOR
				},
			},
			// ADDER_N (no carry-in, no carry-out)
			Candidate {
				name: "ADDER_N",
				config: 0,
				applicable: |i, o, _| {
					if i < 2 || i % 2 != 0 {
						return false;
					}
					let n = i / 2;
					o == n
				},
				formula: |w, i, _, _| {
					let n = i / 2;
					let m = mask(n);
					let a = w & m;
					let c = (w >> n) & m;
					(a + c) & m
				},
			},
			// ADDER_N (without carry-in, with carry-out)
			Candidate {
				name: "ADDER_N_COU",
				config: 1 << 1, // CFG_HAS_COUT
				applicable: |i, o, _| {
					if i < 2 || i % 2 != 0 {
						return false;
					}
					let n = i / 2;
					o == n + 1
				},
				formula: |w, i, o, _| {
					let n = i / 2;
					let m = mask(n);
					let a = w & m;
					let c = (w >> n) & m;
					(a + c) & mask(o)
				},
			},
			// ADDER_N (with carry-in, without carry-out)
			Candidate {
				name: "ADDER_N_CIN",
				config: 1 << 0, // CFG_HAS_CIN
				applicable: |i, o, _| {
					let operand_bits = i.wrapping_sub(1);
					if operand_bits < 2 || operand_bits % 2 != 0 {
						return false;
					}
					let n = operand_bits / 2;
					o == n
				},
				formula: |w, i, _, _| {
					let operand_bits = i - 1;
					let n = operand_bits / 2;
					let m = mask(n);
					let a = w & m;
					let c = (w >> n) & m;
					let cin = (w >> (2 * n)) & 1;
					(a + c + cin) & m
				},
			},
			// ADDER_N (with carry-in and carry-out)
			Candidate {
				name: "ADDER_N_CIN_COU",
				config: (1 << 0) | (1 << 1), // CFG_HAS_CIN | CFG_HAS_COUT
				applicable: |i, o, _| {
					let operand_bits = i.wrapping_sub(1);
					if operand_bits < 2 || operand_bits % 2 != 0 {
						return false;
					}
					let n = operand_bits / 2;
					o == n + 1
				},
				formula: |w, i, o, _| {
					let operand_bits = i - 1;
					let n = operand_bits / 2;
					let m = mask(n);
					let a = w & m;
					let c = (w >> n) & m;
					let cin = (w >> (2 * n)) & 1;
					(a + c + cin) & mask(o)
				},
			},
		]
	})
}

/// Recognizes `lut` as a known gate pattern and, on a match, returns an equivalent [`Formula`]
/// -- generic over `In`/`Out` so any real [`Lut<In, Out>`] can be handed in directly, not just
/// the `u32`-packed shape a caller happens to have lying around.
///
/// Streams each candidate's output against `lut.table` row by row and bails at the first
/// mismatch, so this never allocates a candidate table (unlike building one up front and
/// comparing `Vec`s) and rejects most candidates in a handful of iterations. `in_bits`/`out_bits`
/// are taken explicitly rather than derived from `In`/`Out`'s storage type, since a gate's real
/// width (say 5 bits) is usually narrower than the container type chosen to hold it (`u8`).
///
/// Returns `None` (rather than panicking or truncating) when the gate is too wide for this to
/// even attempt: >64 bits either way is outside what a packed `u64` word, and therefore
/// `Formula`, can represent.
pub fn recognize<In: WireWord, Out: WireWord>(in_bits: u32, out_bits: u32, lut: &Lut<In, Out>) -> Option<Box<dyn OptimizedGate>> {
	if in_bits == 0 || in_bits > 64 || out_bits == 0 || out_bits > 64 {
		return None;
	}
	if lut.table.len() as u64 != 1u64 << in_bits {
		return None; // table doesn't match the claimed width; nothing sane to compare against
	}

	'candidates: for candidate in registry() {
		if !(candidate.applicable)(in_bits, out_bits, candidate.config) {
			continue;
		}
		for (row, actual) in lut.table.iter().enumerate() {
			let expected = (candidate.formula)(row as Bits, in_bits, out_bits, candidate.config);
			if actual.to_u64() != expected {
				continue 'candidates;
			}
		}
		let _ = candidate.name; // available for logging/debugging at the call site if desired
		return Some(Box::new(Formula { in_bits, out_bits, config: candidate.config, f: candidate.formula }));
	}
	None
}
