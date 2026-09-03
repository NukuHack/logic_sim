//! Covers the public [`logic_sim::gate_op`] fast-path evaluators: [`Lut`]/[`build_lut`] (the
//! deduplicated truth-table representation shared by `recalculate_chip_cache` and
//! `recognize`), and the [`CachedGate`] bridge on [`Native`]/[`NativeList`] that lets those two
//! closed-form evaluators slot into the same `u64`-in/`u32`-out entry point `Lut` uses.

use logic_sim::gate_op::{build_lut, registry, CachedGate, Lut, Native, NativeList};

fn eval_row(gate: &dyn CachedGate, out_len: usize, input: u64) -> Vec<u32> {
	let mut out = vec![0u32; out_len];
	assert!(gate.eval(input, &mut out), "eval should succeed for an in-range input");
	out
}

#[test]
fn lut_looks_up_the_row_matching_input() {
	// Two output pins per row: pin 0 echoes the input, pin 1 is always 7.
	let lut = Lut::new(vec![vec![0, 7], vec![1, 7], vec![2, 7], vec![3, 7]]);
	assert_eq!(eval_row(&lut, 2, 0), vec![0, 7]);
	assert_eq!(eval_row(&lut, 2, 2), vec![2, 7]);
}

#[test]
fn lut_eval_fails_gracefully_out_of_range() {
	let lut = Lut::new(vec![vec![0], vec![1]]);
	let mut out = [0u32];
	// Row 5 doesn't exist (only rows 0 and 1 do): `out` must be left untouched, matching
	// `process_cached_chip`'s "fall back to a real step" contract for a stale cache entry.
	out[0] = 42;
	assert!(!lut.eval(5, &mut out));
	assert_eq!(out[0], 42);
}

#[test]
fn lut_eval_copies_only_the_overlapping_length_on_mismatch() {
	let lut = Lut::new(vec![vec![10, 20]]);
	// Caller asks for 3 output pins but the row only has 2: `eval` copies just the
	// overlapping 2 and leaves the rest of `out` untouched, rather than panicking on the
	// length mismatch -- a defensive fallback for a stale cache entry, same as the
	// out-of-range-row case above.
	let mut out = [9u32; 3];
	assert!(lut.eval(0, &mut out));
	assert_eq!(out, [10, 20, 9]);
}

#[test]
fn build_lut_matches_a_hand_built_lut() {
	// AND3: output pin 0 is 1 only when every input bit is set.
	let lut = build_lut(3, 1, |row| (row == 0b111) as u64).unwrap();
	assert_eq!(eval_row(&lut, 1, 0b111), vec![1]);
	assert_eq!(eval_row(&lut, 1, 0b110), vec![0]);
	assert_eq!(lut.len(), 8);
}

#[test]
fn build_lut_rejects_widths_it_cannot_represent() {
	assert!(build_lut(0, 1, |_| 0).is_none());
	assert!(build_lut(1, 0, |_| 0).is_none());
	assert!(build_lut(64, 1, |_| 0).is_none()); // 1u64 << 64 would overflow
	assert!(build_lut(4, 33, |_| 0).is_none()); // wider than a single packed u32 field
}

/// `Native`'s `CachedGate` bridge is the piece that actually lets `recognize`'s output reach
/// `process_cached_chip` some day: this exercises that bridge the same way that real caller
/// would, not `eval_wide` (see `recognize_test.rs` for `eval_wide` coverage on wide gates).
#[test]
fn native_cached_gate_bridge_matches_its_formula() {
	let and2 = registry().iter().find(|c| c.name() == "AND2").expect("AND2 in registry");
	let gate = Native::new(2, 1, and2.config(), and2.formula());

	assert_eq!(eval_row(&gate, 1, 0b11), vec![1]);
	assert_eq!(eval_row(&gate, 1, 0b01), vec![0]);
}

#[test]
fn native_cached_gate_bridge_declines_gates_it_cannot_represent() {
	let buffer_n = registry().iter().find(|c| c.name() == "BUFFER_N").expect("BUFFER_N in registry");

	// 64 output bits doesn't fit a single packed u32 field -- the bridge should say so
	// rather than silently truncating the result.
	let wide_out = Native::new(4, 64, buffer_n.config(), buffer_n.formula());
	let mut out = [0u32; 1];
	assert!(!wide_out.eval(0, &mut out));

	// Asking for two output words when this gate only has one field is likewise a mismatch.
	let narrow = Native::new(4, 4, buffer_n.config(), buffer_n.formula());
	let mut out = [0u32; 2];
	assert!(!narrow.eval(0, &mut out));
}

/// `NativeList` chains `first` -> `rest` -> `last`; this just checks the chain actually runs
/// in order and its `CachedGate` bridge matches a single-step equivalent.
#[test]
fn native_list_runs_its_steps_in_order() {
	// Doubles the input, then adds one, landing on a 5-bit result.
	let list = NativeList::new(
		4,
		5,
		0,
		|words, _in_bits, _cfg| vec![words[0] << 1],
		vec![|words, _cfg| vec![words[0] + 1]],
		|words, out_bits, _cfg| vec![words[0] & ((1u64 << out_bits) - 1)],
	);

	// input 5 -> doubled to 10 -> plus one is 11 -> masked to 5 bits stays 11.
	assert_eq!(eval_row(&list, 1, 5), vec![11]);
}
