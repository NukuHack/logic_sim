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
use super::eval::{Bits, CachedGate, Lut, Native};

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
		let mut v = vec![
			// NOT
			Candidate { name: "NOT", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| bitvec::not(w, 1) },
			// BUFFER
			Candidate { name: "BUFFER", config: 0, applicable: |i, o, _| i == 1 && o == 1, formula: |w, _, _, _| bitvec::truncate(w, 1) },
			// 2-input AND
			Candidate {
				name: "AND2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::all_ones_masked(w, 2)),
			},
			// 2-input OR
			Candidate {
				name: "OR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::any_set_masked(w, 2)),
			},
			// 2-input XOR
			Candidate {
				name: "XOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(bitvec::parity_masked(w, 2)),
			},
			// 2-input NAND
			Candidate {
				name: "NAND2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(!bitvec::all_ones_masked(w, 2)),
			},
			// 2-input NOR
			Candidate {
				name: "NOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(!bitvec::any_set_masked(w, 2)),
			},
			// 2-input XNOR
			Candidate {
				name: "XNOR2",
				config: 0,
				applicable: |i, o, _| i == 2 && o == 1,
				formula: |w, _, _, _| bit_result(!bitvec::parity_masked(w, 2)),
			},
			// N-input AND (fast: scan for all-ones, no intermediate Vecs)
			Candidate {
				name: "AND_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::all_ones_masked(w, i)),
			},
			// N-input OR (fast: scan for any-set, no intermediate Vecs)
			Candidate {
				name: "OR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::any_set_masked(w, i)),
			},
			// N-input XOR (fast: running-popcount parity, no intermediate Vecs)
			Candidate {
				name: "XOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(bitvec::parity_masked(w, i)),
			},
			// N-input NAND (fast: scan for all-ones, no intermediate Vecs)
			Candidate {
				name: "NAND_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(!bitvec::all_ones_masked(w, i)),
			},
			// N-input NOR (fast: scan for any-set, no intermediate Vecs)
			Candidate {
				name: "NOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(!bitvec::any_set_masked(w, i)),
			},
			// N-bit XNOR (2N inputs: equality comparison between two N-bit fields)
			// Note: this is placed before general XNOR_N to ensure 2N-equality matching takes priority
			Candidate {
				name: "XNOR_2N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 4 && i % 2 == 0,
				formula: |w, i, _, _| {
					let n = i >> 1; // divide by 2 faster than /
					bit_result(bitvec::fields_equal(w, 0, n, n))
				},
			},
			// N-input XNOR (true XNOR: odd parity of inputs)
			Candidate {
				name: "XNOR_N",
				config: 0,
				applicable: |i, o, _| o == 1 && i >= 2,
				formula: |w, i, _, _| bit_result(!bitvec::parity_masked(w, i)), // XNOR is inverse of XOR
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
		];
		v.extend(adder_variants());
		v
	})
}

/// Bit flags packed into a `Candidate`'s `config` for every entry [`adder_variants`] produces.
/// `CFG_HAS_CIN`/`CFG_HAS_COUT` say whether that pin exists at all; `CFG_CIN_FIRST`/
/// `CFG_COUT_FIRST` say, when it does, whether it sits at the front of its pin list (input
/// pins for cin, output pins for cout) instead of the back. All four are independent, so
/// `adder_applicable`/`adder_formula` below read them individually rather than switching on a
/// combined enum.
const CFG_HAS_CIN: Bits = 1 << 0;
const CFG_HAS_COUT: Bits = 1 << 1;
const CFG_CIN_FIRST: Bits = 1 << 2;
const CFG_COUT_FIRST: Bits = 1 << 3;

/// Shared shape check for every adder variant: works out how many operand bits remain once the
/// optional carry-in pin is set aside, and requires that remainder to split evenly into two
/// equal-width operands sized to explain `out_bits` (plus one more bit when a carry-out pin is
/// present). *Where* cin/cout sit doesn't change how many bits there are, so every variant --
/// cin/cout absent, first, or last -- shares this one predicate instead of each hardcoding its
/// own near-identical arithmetic.
fn adder_applicable(in_bits: u32, out_bits: u32, config: Bits) -> bool {
	let has_cin = config & CFG_HAS_CIN != 0;
	let has_cout = config & CFG_HAS_COUT != 0;
	let operand_bits = if has_cin { in_bits.wrapping_sub(1) } else { in_bits };
	if operand_bits < 2 || operand_bits % 2 != 0 {
		return false;
	}
	let n = operand_bits / 2;
	out_bits == if has_cout { n + 1 } else { n }
}

/// Shared formula for every adder variant. Removing a single cin pin from a pin list --
/// whichever position it's in -- never disturbs the relative order of what's left, so the two
/// operand fields are always "whatever remains, split into two equal halves in order"; only
/// cin's own position (`operand_start`/`cin_pos`) and, symmetrically, whether the carry-out
/// lands below or above the sum bits, actually depend on the `_FIRST` flags in `config`.
fn adder_formula(w: &[Bits], in_bits: u32, out_bits: u32, config: Bits) -> Vec<Bits> {
	let has_cin = config & CFG_HAS_CIN != 0;
	let has_cout = config & CFG_HAS_COUT != 0;
	let cin_first = config & CFG_CIN_FIRST != 0;
	let cout_first = config & CFG_COUT_FIRST != 0;

	let operand_bits = if has_cin { in_bits - 1 } else { in_bits };
	let n = operand_bits / 2;
	// Cin (if present and first) occupies bit 0, pushing both operand fields up by one bit.
	let operand_start = if has_cin && cin_first { 1 } else { 0 };

	let mut sum = bitvec::add(&bitvec::field(w, operand_start, n), &bitvec::field(w, operand_start + n, n));
	if has_cin {
		// First: cin is bit 0. Last: cin is the one input bit past both operands.
		let cin_pos = if cin_first { 0 } else { operand_start + 2 * n };
		sum = bitvec::add(&sum, &bitvec::field(w, cin_pos, 1));
	}

	if !has_cout {
		return bitvec::truncate(&sum, n);
	}
	if !cout_first {
		// `add` already leaves the carry sitting at bit `n`, exactly where a trailing
		// carry-out pin belongs -- truncating to `n + 1` bits is the whole job.
		return bitvec::truncate(&sum, out_bits);
	}
	// Carry-out pin comes *before* the sum bits instead: peel the carry off bit `n` and
	// reassemble it as the new bit 0, shifting the sum bits up by one to make room.
	let carry = bitvec::field(&sum, n, 1);
	let sum_bits = bitvec::field(&sum, 0, n);
	bitvec::concat(&carry, 1, &sum_bits)
}

/// Every cin/cout arrangement `adder_applicable`/`adder_formula` can recognize: carry-in absent,
/// first, or last among the input pins, crossed with carry-out absent, first, or last among the
/// output pins (skipping the nonsensical "positioned but absent" combinations) -- covering both
/// pin orderings real adder chips actually get built with ("a, b, cin" and "cin, a, b"; "sum,
/// cout" and "cout, sum"), on both sides, in every combination.
///
/// This is the "quickly identify it's somewhat adder-looking, then check the sub-options" shape:
/// `adder_applicable` alone (shape-only, no formula eval) already rules out a candidate whose
/// `in_bits`/`out_bits` can't possibly be an adder of *any* flavor, and `find_candidate`'s
/// anchor-row check rejects most of what's left in one row before ever running a full sweep --
/// so trying all nine of these costs barely more than trying one, and adding a tenth arrangement
/// later (say, a fixed-position "carry in the middle") is one more row here, not a new
/// hand-written candidate.
fn adder_variants() -> Vec<Candidate> {
	const VARIANTS: &[(&str, Bits)] = &[
		("ADDER_N", 0),
		("ADDER_N_COU", CFG_HAS_COUT),
		("ADDER_N_COU_COUTFIRST", CFG_HAS_COUT | CFG_COUT_FIRST),
		("ADDER_N_CIN", CFG_HAS_CIN),
		("ADDER_N_CIN_CINFIRST", CFG_HAS_CIN | CFG_CIN_FIRST),
		("ADDER_N_CIN_COU", CFG_HAS_CIN | CFG_HAS_COUT),
		("ADDER_N_CIN_COU_CINFIRST", CFG_HAS_CIN | CFG_HAS_COUT | CFG_CIN_FIRST),
		("ADDER_N_CIN_COU_COUTFIRST", CFG_HAS_CIN | CFG_HAS_COUT | CFG_COUT_FIRST),
		("ADDER_N_CIN_COU_CINFIRST_COUTFIRST", CFG_HAS_CIN | CFG_HAS_COUT | CFG_CIN_FIRST | CFG_COUT_FIRST),
	];
	VARIANTS.iter().map(|&(name, config)| Candidate { name, config, applicable: adder_applicable, formula: adder_formula }).collect()
}

/// Recognizes `lut` as a known gate pattern and, on a match, returns an equivalent [`Native`].
/// `lut` is expected to be a single-field table (one packed `u32` covering the gate's whole
/// output, as [`super::build::build_lut`] produces) rather than one field per output pin --
/// see `Lut`'s doc comment.
///
/// Streams each candidate's output against `lut`'s rows and bails at the first mismatch, so
/// this never allocates a candidate table (unlike building one up front and comparing `Vec`s)
/// and rejects most candidates in a handful of iterations.
///
/// Returns `None` (rather than panicking or truncating) when `in_bits`/`out_bits` is zero, when
/// `lut` doesn't actually have `2^in_bits` rows, or when no candidate in the registry matches
/// every row. Note this cap is about `Lut` itself, not the `Native` this returns: a materialized
/// table over more than ~64 input bits was never buildable in the first place (`2^65` rows), so
/// `recognize` naturally never sees one -- but the `Native` it hands back has no such limit and
/// is exactly as capable at 4000 bits as it is here (see `formula_from_candidate` +
/// `Native::new` for building one directly at a width no `Lut` could ever hold).
pub fn recognize(in_bits: u32, out_bits: u32, lut: &Lut) -> Option<Box<dyn CachedGate>> {
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
pub fn recognize_formula(in_bits: u32, out_bits: u32, lut: &Lut) -> Option<(Bits, RecFn)> {
	find_candidate(in_bits, out_bits, lut).map(|c| (c.config, c.formula))
}

/// Shared search behind [`recognize`]/[`recognize_formula`]: streams `lut`'s rows against
/// every applicable candidate's formula and returns the first exact match, bailing at the
/// first mismatching row per candidate (see the module doc comment for why this is cheap even
/// when nothing matches).
///
/// `lut` is indexed by a plain `usize`, so it can never hold more than `2^63`-ish rows to begin
/// with; comparing each row's single `u32` field against the candidate formula's low output
/// word is therefore lossless for every table this function will ever actually see (see
/// `build_lut`'s 32-bit output cap).
fn find_candidate(in_bits: u32, out_bits: u32, lut: &Lut) -> Option<&'static Candidate> {
	if in_bits == 0 || out_bits == 0 {
		return None;
	}
	if lut.len() as u64 != 1u64.checked_shl(in_bits).unwrap_or(0) {
		return None; // table doesn't match the claimed width; nothing sane to compare against
	}

	let row_value = |row: usize| -> u64 { lut.row(row as u64).and_then(|r| r.first()).copied().unwrap_or(0) as u64 };

	// Anchor row checked before the full sweep: the all-ones input is cheap to compute and,
	// in practice, is where most non-matching-but-`applicable` candidates (e.g. AND2 vs.
	// NAND2, which agree on every row except this one) first diverge from `lut`. Checking it
	// up front turns what would otherwise be a full O(2^in_bits) scan into O(1) for those
	// cases; a genuine match still falls through to the exhaustive check below, since one
	// matching row is never enough to prove equivalence on its own.
	let last_row = lut.len() - 1;
	let last_actual = row_value(last_row);
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
		for row in 0..=last_row {
			if row_value(row) != eval_row(candidate, row) {
				continue 'candidates;
			}
		}
		return Some(candidate);
	}
	None
}
