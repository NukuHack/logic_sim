//! Recognizes a materialized [`Lut`] as a known gate pattern (AND, adder, etc.) and, on a
//! match, hands back a [`Native`] that computes the same function from a tiny closed-form
//! expression over packed bits instead of a stored table.
//!
//! Why this matters: a `Lut<In, Out>` is only as cheap as its table is small. A 20-input gate
//! is already a million-row table; a handful of bits more and it stops being buildable at all.
//! `Native` is O(1) to evaluate and O(1) to store regardless of width, so recognizing a wide
//! `Lut` as e.g. `ADDER_N` turns an expensive (or outright impossible) table into a couple of
//! shifts, masks and an add. Recognition itself streams the candidate's output row-by-row
//! against the real table and bails at the first mismatch, so a non-matching candidate is
//! usually rejected in a handful of comparisons rather than a full table diff.

use super::bitvec::{self, bit_result};
use super::eval::{Bits, Lut, Native, OptimizedGate, WireWord};

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
	formula: fn(&[Bits], u32, u32, Bits) -> Vec<Bits>,
}
impl Candidate {
	pub fn name(&self) -> &'static str {
		self.name
	}
	pub fn config(&self) -> Bits {
		self.config
	}
	pub fn formula(&self) -> fn(&[Bits], u32, u32, Bits) -> Vec<Bits> {
		self.formula
	}
}

#[allow(clippy::manual_range_contains)]
pub fn registry() -> &'static [Candidate] {
	static REGISTRY: std::sync::OnceLock<Vec<Candidate>> = std::sync::OnceLock::new();
	REGISTRY.get_or_init(|| {
		vec![
			// NOT
			Candidate { name: "NOT", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| bitvec::not(w, 1) },
			// BUFFER
			Candidate { name: "BUFFER", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| bitvec::truncate(w, 1) },
			// 2-input AND
			Candidate {
				name: "AND2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::eq(&bitvec::and(w, &bitvec::mask(2)), &bitvec::mask(2))),
			},
			// 2-input OR
			Candidate {
				name: "OR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(!bitvec::is_zero(&bitvec::and(w, &bitvec::mask(2)))),
			},
			// 2-input XOR
			Candidate {
				name: "XOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::popcount(&bitvec::and(w, &bitvec::mask(2))) & 1 == 1),
			},
			// 2-input NAND
			Candidate {
				name: "NAND2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(!bitvec::eq(&bitvec::and(w, &bitvec::mask(2)), &bitvec::mask(2))),
			},
			// 2-input NOR
			Candidate {
				name: "NOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::is_zero(&bitvec::and(w, &bitvec::mask(2)))),
			},
			// 2-input XNOR
			Candidate {
				name: "XNOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::popcount(&bitvec::and(w, &bitvec::mask(2))) & 1 == 0),
			},
			// N-input AND (fast: compare against all-ones)
			Candidate {
				name: "AND_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::eq(&bitvec::and(w, &bitvec::mask(i)), &bitvec::mask(i))),
			},
			// N-input OR (fast: test nonzero)
			Candidate {
				name: "OR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(!bitvec::is_zero(&bitvec::and(w, &bitvec::mask(i)))),
			},
			// N-input XOR (fast: parity via popcount)
			Candidate {
				name: "XOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::popcount(&bitvec::and(w, &bitvec::mask(i))) & 1 == 1),
			},
			// N-input NAND (fast: compare against all-ones)
			Candidate {
				name: "NAND_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(!bitvec::eq(&bitvec::and(w, &bitvec::mask(i)), &bitvec::mask(i))),
			},
			// N-input NOR (fast: test zero)
			Candidate {
				name: "NOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::is_zero(&bitvec::and(w, &bitvec::mask(i)))),
			},
			// N-bit XNOR (2N inputs: equality comparison between two N-bit fields)
			// Note: this is placed before general XNOR_N to ensure 2N-equality matching takes priority
			Candidate {
				name: "XNOR_2N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 4 && i % 2 == 0,
				formula: |w, i, _, _| {
					let n = i >> 1; // divide by 2 faster than /
					bit_result(bitvec::eq(&bitvec::field(w, 0, n), &bitvec::field(w, n, n)))
				},
			},
			// N-input XNOR (true XNOR: odd parity of inputs)
			Candidate {
				name: "XNOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::popcount(&bitvec::and(w, &bitvec::mask(i))) & 1 == 0), // XNOR is inverse of XOR
			},
			// N-bit NOT (fast: invert then mask) -- no width cap: `bitvec::not` works for any `i`
			Candidate { name: "NOT_N", config: 0, applicable: |i, o, _| i == o && i >= 2, formula: |w, i, _, _| bitvec::not(w, i) },
			// N-bit BUFFER (fast: mask only) -- no width cap: `bitvec::truncate` works for any `i`
			Candidate { name: "BUFFER_N", config: 0, applicable: |i, o, _| i == o && i >= 2, formula: |w, i, _, _| bitvec::truncate(w, i) },
			// N-bit AND (fast: two fields and AND)
			Candidate {
				name: "AND_N_WIDE",
				config: 0,
				applicable: |i, o, _| {
					o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2 // bit ops faster
				},
				formula: |w, _i, o, _| bitvec::and(&bitvec::field(w, 0, o), &bitvec::field(w, o, o)),
			},
			// N-bit OR (fast: two fields and OR)
			Candidate {
				name: "OR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| bitvec::or(&bitvec::field(w, 0, o), &bitvec::field(w, o, o)),
			},
			// N-bit XOR (fast: two fields and XOR)
			Candidate {
				name: "XOR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| bitvec::xor(&bitvec::field(w, 0, o), &bitvec::field(w, o, o)),
			},
			// N-bit XNOR (true bitwise XNOR: inverse of XOR for each bit position)
			Candidate {
				name: "XNOR_N_WIDE",
				config: 0,
				applicable: |i, o, _| o == (i >> 1) && i >= 4 && (i & 1) == 0 && o >= 2,
				formula: |w, _i, o, _| bitvec::not(&bitvec::xor(&bitvec::field(w, 0, o), &bitvec::field(w, o, o)), o),
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
					bitvec::truncate(&bitvec::add(&bitvec::field(w, 0, n), &bitvec::field(w, n, n)), n)
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
					bitvec::truncate(&bitvec::add(&bitvec::field(w, 0, n), &bitvec::field(w, n, n)), o)
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
					let sum = bitvec::add(&bitvec::add(&bitvec::field(w, 0, n), &bitvec::field(w, n, n)), &bitvec::field(w, 2 * n, 1));
					bitvec::truncate(&sum, n)
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
					let sum = bitvec::add(&bitvec::add(&bitvec::field(w, 0, n), &bitvec::field(w, n, n)), &bitvec::field(w, 2 * n, 1));
					bitvec::truncate(&sum, o)
				},
			},
		]
	})
}

/// Recognizes `lut` as a known gate pattern and, on a match, returns an equivalent [`Native`]
/// -- generic over `In`/`Out` so any real [`Lut<In, Out>`] can be handed in directly, not just
/// the `u32`-packed shape a caller happens to have lying around.
///
/// Streams each candidate's output against `lut.table` row by row and bails at the first
/// mismatch, so this never allocates a candidate table (unlike building one up front and
/// comparing `Vec`s) and rejects most candidates in a handful of iterations. `in_bits`/`out_bits`
/// are taken explicitly rather than derived from `In`/`Out`'s storage type, since a gate's real
/// width (say 5 bits) is usually narrower than the container type chosen to hold it (`u8`).
///
/// Returns `None` (rather than panicking or truncating) when `in_bits`/`out_bits` is zero, when
/// `lut`'s table doesn't actually have `2^in_bits` rows, or when no candidate in the registry
/// matches every row. Note this cap is about `Lut` itself, not the `Native` this returns: a
/// materialized table over more than ~64 input bits was never buildable in the first place
/// (`2^65` rows), so `recognize` naturally never sees one -- but the `Native` it hands back has
/// no such limit and is exactly as capable at 4000 bits as it is here (see
/// `formula_from_candidate` + `Native::new` for building one directly at a width no `Lut` could
/// ever hold).
pub fn recognize<In: WireWord, Out: WireWord>(in_bits: u32, out_bits: u32, lut: &Lut<In, Out>) -> Option<Box<dyn OptimizedGate>> {
	let candidate = find_candidate(in_bits, out_bits, lut)?;
	Some(Box::new(Native::new(in_bits, out_bits, candidate.config, candidate.formula)))
}

pub type RecFn = fn(&[Bits], u32, u32, Bits) -> Vec<Bits>;
#[allow(unused)]
/// Like [`recognize`], but hands back the matched candidate's raw `(config, formula)` pair
/// instead of a boxed `Native`, for callers that already track their own `in_bits`/`out_bits`
/// (e.g. [`super::caching`], which wants to store just the two words needed to call `formula`
/// directly against a chip's cache entry, without an extra allocation or vtable indirection
/// per cached chip).
pub fn recognize_formula<In: WireWord, Out: WireWord>(in_bits: u32, out_bits: u32, lut: &Lut<In, Out>) -> Option<(Bits, RecFn)> {
	find_candidate(in_bits, out_bits, lut).map(|c| (c.config, c.formula))
}

/// Shared search behind [`recognize`]/[`recognize_formula`]: streams `lut`'s table against
/// every applicable candidate's formula and returns the first exact match, bailing at the
/// first mismatching row per candidate (see the module doc comment for why this is cheap even
/// when nothing matches).
///
/// `lut.table` is indexed by a plain `usize`, so it can never hold more than `2^63`-ish rows to
/// begin with; comparing each row's low word (`to_u64`) against the candidate formula's low
/// output word is therefore lossless for every table this function will ever actually see.
fn find_candidate<In: WireWord, Out: WireWord>(in_bits: u32, out_bits: u32, lut: &Lut<In, Out>) -> Option<&'static Candidate> {
	if in_bits == 0 || out_bits == 0 {
		return None;
	}
	if lut.table.len() as u64 != 1u64.checked_shl(in_bits).unwrap_or(0) {
		return None; // table doesn't match the claimed width; nothing sane to compare against
	}

	// Anchor row checked before the full sweep: the all-ones input is cheap to compute and,
	// in practice, is where most non-matching-but-`applicable` candidates (e.g. AND2 vs.
	// NAND2, which agree on every row except this one) first diverge from `lut`. Checking it
	// up front turns what would otherwise be a full O(2^in_bits) scan into O(1) for those
	// cases; a genuine match still falls through to the exhaustive check below, since one
	// matching row is never enough to prove equivalence on its own.
	let last_row = lut.table.len() - 1;
	let last_actual = lut.table[last_row].to_u64();
	let eval_row = |candidate: &Candidate, row: usize| -> u64 {
		(candidate.formula)(&[row as Bits], in_bits, out_bits, candidate.config).first().copied().unwrap_or(0)
	};

	'candidates: for candidate in registry() {
		if !(candidate.applicable)(in_bits, out_bits, candidate.config) {
			continue;
		}
		if eval_row(candidate, last_row) != last_actual {
			continue;
		}
		for (row, actual) in lut.table.iter().enumerate() {
			if actual.to_u64() != eval_row(candidate, row) {
				continue 'candidates;
			}
		}
		return Some(candidate);
	}
	None
}
