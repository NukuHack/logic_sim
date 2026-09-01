use logic_sim::gate_op::{recognize, registry, Lut, Native, OptimizedGate};
use logic_sim::pin_state::LogicState;

fn eval_bools(gate: &dyn OptimizedGate, in_bits: u32, out_bits: u32, input: &[bool]) -> Vec<bool> {
	let states: Vec<LogicState> = input.iter().map(|&b| LogicState::from_bool(b)).collect();
	assert_eq!(states.len(), in_bits as usize);
	let mut out = vec![LogicState::Low; out_bits as usize];
	gate.eval(&states, &mut out);
	out.iter().map(|s| s.is_high()).collect()
}

/// Builds an actual `Lut<u64, u64>` (the "real" wide type, not a toy stand-in) by evaluating
/// a reference bool-slice function over every row -- used only to set up test fixtures.
fn lut_for(in_bits: u32, out_bits: u32, f: impl Fn(&[bool]) -> Vec<bool>) -> Lut<u64, u64> {
	let rows = 1u64 << in_bits;
	let mut table = Vec::with_capacity(rows as usize);
	for i in 0..rows {
		let bits: Vec<bool> = (0..in_bits).map(|b| (i >> b) & 1 == 1).collect();
		let out = f(&bits);
		assert_eq!(out.len() as u32, out_bits);
		table.push(out.iter().enumerate().fold(0u64, |acc, (idx, &b)| acc | ((b as u64) << idx)));
	}
	Lut::new(table.into_boxed_slice())
}

/// Helper to create a Native from a registry candidate by name
fn formula_from_candidate(name: &str, in_bits: u32, out_bits: u32) -> Native {
	let candidate = registry().iter().find(|c| c.name() == name).expect(name);
	Native::new(in_bits, out_bits, candidate.config(), candidate.formula())
}

#[test]
fn recognizes_and2() {
	let lut = lut_for(2, 1, |b| vec![b[0] & b[1]]);
	let gate = recognize(2, 1, &lut).expect("AND2 should be recognized");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![false]);
}

#[test]
fn recognizes_xor3_as_xor_n() {
	let lut = lut_for(3, 1, |b| vec![b[0] ^ b[1] ^ b[2]]);
	let gate = recognize(3, 1, &lut).expect("XOR3 should be recognized");
	assert_eq!(eval_bools(&*gate, 3, 1, &[true, true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 3, 1, &[true, true, false]), vec![false]);
}

#[test]
fn recognizes_equals4() {
	let lut = lut_for(4, 1, |b| vec![b[..2] == b[2..]]);
	let gate = recognize(4, 1, &lut).expect("EQUALS4 should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, false, true, false]), vec![true]);
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, false, false, false]), vec![false]);
}

#[test]
fn recognizes_adder2() {
	let lut = lut_for(4, 3, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let sum = a + c;
		(0..3).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 3, &lut).expect("ADDER2 should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 3, &[true, true, true, false]), vec![false, false, true]);
}

#[test]
fn unrecognized_table_returns_none() {
	let lut = Lut::<u64, u64>::new(vec![7u64; 4].into_boxed_slice());
	assert!(recognize(2, 1, &lut).is_none());
}

/// The actual motivating case: a 40-in/21-out adder is far too wide to ever materialize as a
/// `Lut` (2^40 rows), so this exercises `Native` eval directly instead of round-tripping
/// through `recognize`, matching how a wide gate would need to be recognized structurally
/// rather than by an exhaustive table diff in real use.
#[test]
fn formula_handles_wide_adder_without_a_table() {
	let in_bits = 40;
	let out_bits = 21;
	let gate = formula_from_candidate("ADDER_N", in_bits, out_bits);
	let n = (in_bits / 2) as usize;
	let mut input = vec![false; in_bits as usize];
	for i in 0..n {
		input[i] = i % 3 == 0; // arbitrary a
	}
	for i in 0..n {
		input[n + i] = i % 5 == 0; // arbitrary c
	}
	let a: u64 = (0..n).fold(0, |acc, i| acc | ((input[i] as u64) << i));
	let c: u64 = (0..n).fold(0, |acc, i| acc | ((input[n + i] as u64) << i));
	let expected = a + c;

	let out = eval_bools(&gate, in_bits, out_bits, &input);
	let actual: u64 = out.iter().enumerate().fold(0, |acc, (i, &b)| acc | ((b as u64) << i));
	assert_eq!(actual, expected);
}

/// Same 2-bit-operand adder, checked against all four carry-in/carry-out combinations to
/// make sure `adder_variants` actually produced four distinct, individually-correct gates
/// (not e.g. four copies of the same config by a lookup bug).
#[test]
fn recognizes_all_four_adder_variants() {
	// No cin, with cout: in=4 (a1a0 c1c0), out=3 (sum + carry) -- same case as recognizes_adder2.
	let lut = lut_for(4, 3, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let sum = a + c;
		(0..3).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	assert!(recognize(4, 3, &lut).is_some());

	// No cin, no cout: in=4, out=2 (sum truncated, carry silently dropped).
	let lut = lut_for(4, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let sum = (a + c) & 0b11;
		(0..2).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 2, &lut).expect("adder without carry-out should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, true, true, false]), vec![false, false]); // 3+1=4 -> 0b100, truncated to 00

	// With cin, with cout: in=5 (a1a0 c1c0 cin), out=3.
	let lut = lut_for(5, 3, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let cin = b[4] as u64;
		let sum = a + c + cin;
		(0..3).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	let gate = recognize(5, 3, &lut).expect("adder with carry-in should be recognized");
	assert_eq!(eval_bools(&*gate, 5, 3, &[true, false, false, false, true]), vec![false, true, false]); // 1+0+1=2

	// With cin, no cout: in=5, out=2.
	let lut = lut_for(5, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let cin = b[4] as u64;
		let sum = (a + c + cin) & 0b11;
		(0..2).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	let gate = recognize(5, 2, &lut).expect("adder with carry-in, no carry-out should be recognized");
	assert_eq!(eval_bools(&*gate, 5, 2, &[true, true, true, true, true]), vec![true, true]);
	// 3+3+1=7 -> truncated to 0b11
}

#[test]
fn recognizes_or2() {
	let lut = lut_for(2, 1, |b| vec![b[0] | b[1]]);
	let gate = recognize(2, 1, &lut).expect("OR2 should be recognized");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[false, false]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![true]);
}

#[test]
fn recognizes_nand2() {
	let lut = lut_for(2, 1, |b| vec![!(b[0] & b[1])]);
	let gate = recognize(2, 1, &lut).expect("NAND2 should be recognized");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![true]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[false, false]), vec![true]);
}

#[test]
fn recognizes_nor2() {
	let lut = lut_for(2, 1, |b| vec![!(b[0] | b[1])]);
	let gate = recognize(2, 1, &lut).expect("NOR2 should be recognized");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[false, false]), vec![true]);
}

#[test]
fn recognizes_xnor2() {
	let lut = lut_for(2, 1, |b| vec![!(b[0] ^ b[1])]);
	let gate = recognize(2, 1, &lut).expect("XNOR2 should be recognized");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[false, false]), vec![true]);
}

#[test]
fn recognizes_not() {
	let lut = lut_for(1, 1, |b| vec![!b[0]]);
	let gate = recognize(1, 1, &lut).expect("NOT should be recognized");
	assert_eq!(eval_bools(&*gate, 1, 1, &[true]), vec![false]);
	assert_eq!(eval_bools(&*gate, 1, 1, &[false]), vec![true]);
}

#[test]
fn recognizes_buffer() {
	let lut = lut_for(1, 1, |b| vec![b[0]]);
	let gate = recognize(1, 1, &lut).expect("BUFFER should be recognized");
	assert_eq!(eval_bools(&*gate, 1, 1, &[true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 1, 1, &[false]), vec![false]);
}

#[test]
fn recognizes_and4_as_and_n() {
	let lut = lut_for(4, 1, |b| vec![b[0] & b[1] & b[2] & b[3]]);
	let gate = recognize(4, 1, &lut).expect("AND4 should be recognized as AND_N");
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, true, true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, true, true, false]), vec![false]);
}

#[test]
fn recognizes_or5_as_or_n() {
	let lut = lut_for(5, 1, |b| vec![b[0] | b[1] | b[2] | b[3] | b[4]]);
	let gate = recognize(5, 1, &lut).expect("OR5 should be recognized as OR_N");
	assert_eq!(eval_bools(&*gate, 5, 1, &[false, false, false, false, false]), vec![false]);
	assert_eq!(eval_bools(&*gate, 5, 1, &[false, false, false, false, true]), vec![true]);
}

#[test]
fn recognizes_xor6_as_xor_n() {
	let lut = lut_for(6, 1, |b| vec![b[0] ^ b[1] ^ b[2] ^ b[3] ^ b[4] ^ b[5]]);
	let gate = recognize(6, 1, &lut).expect("XOR6 should be recognized as XOR_N");
	assert_eq!(eval_bools(&*gate, 6, 1, &[true, false, true, false, true, false]), vec![true]);
	assert_eq!(eval_bools(&*gate, 6, 1, &[true, true, true, false, true, false]), vec![false]);
}

#[test]
fn recognizes_nand4_as_nand_n() {
	let lut = lut_for(4, 1, |b| vec![!(b[0] & b[1] & b[2] & b[3])]);
	let gate = recognize(4, 1, &lut).expect("NAND4 should be recognized as NAND_N");
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, true, true, true]), vec![false]);
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, true, true, false]), vec![true]);
}

#[test]
fn recognizes_nor4_as_nor_n() {
	let lut = lut_for(4, 1, |b| vec![!(b[0] | b[1] | b[2] | b[3])]);
	let gate = recognize(4, 1, &lut).expect("NOR4 should be recognized as NOR_N");
	assert_eq!(eval_bools(&*gate, 4, 1, &[false, false, false, false]), vec![true]);
	assert_eq!(eval_bools(&*gate, 4, 1, &[false, false, false, true]), vec![false]);
}

#[test]
fn recognizes_xnor4_as_xnor_n() {
	let lut = lut_for(4, 1, |b| vec![!(b[0] ^ b[1] ^ b[2] ^ b[3])]);
	let gate = recognize(4, 1, &lut).expect("XNOR4 should be recognized as XNOR_N");
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, false, true, false]), vec![true]);
	assert_eq!(eval_bools(&*gate, 4, 1, &[true, true, true, false]), vec![false]);
}

#[test]
fn recognizes_equals6_as_xnor_2n() {
	let lut = lut_for(6, 1, |b| vec![b[..3] == b[3..]]);
	let gate = recognize(6, 1, &lut).expect("EQUALS6 should be recognized as XNOR_2N");
	assert_eq!(eval_bools(&*gate, 6, 1, &[true, false, true, true, false, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 6, 1, &[true, false, true, true, false, false]), vec![false]);
}
#[test]
fn recognizes_bitwise_and_wide() {
	let lut = lut_for(4, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let result = a & c;
		(0..2).map(|idx| (result >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 2, &lut).expect("Bitwise AND should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, false, true, true]), vec![true, false]); // 01 & 11 = 01
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, true, false, true]), vec![false, true]); // 11 & 10 = 10
}

#[test]
fn recognizes_bitwise_or_wide() {
	let lut = lut_for(4, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let result = a | c;
		(0..2).map(|idx| (result >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 2, &lut).expect("Bitwise OR should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, false, true, true]), vec![true, true]); // 01 | 11 = 11
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, true, false, true]), vec![true, true]);
	// 11 | 10 = 11
}

#[test]
fn recognizes_bitwise_xor_wide() {
	let lut = lut_for(4, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let result = a ^ c;
		(0..2).map(|idx| (result >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 2, &lut).expect("Bitwise XOR should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, false, true, true]), vec![false, true]); // 01 ^ 11 = 10
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, true, false, true]), vec![true, false]);
	// 11 ^ 10 = 01
}

#[test]
fn recognizes_bitwise_xnor_wide() {
	let lut = lut_for(4, 2, |b| {
		let a = b[0] as u64 | ((b[1] as u64) << 1);
		let c = b[2] as u64 | ((b[3] as u64) << 1);
		let result = !(a ^ c) & 0b11;
		(0..2).map(|idx| (result >> idx) & 1 == 1).collect()
	});
	let gate = recognize(4, 2, &lut).expect("Bitwise XNOR should be recognized");
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, false, true, true]), vec![true, false]); // ~(01 ^ 11) = ~10 = 01
	assert_eq!(eval_bools(&*gate, 4, 2, &[true, true, false, true]), vec![false, true]);
	// ~(11 ^ 10) = ~01 = 10
}

#[test]
fn recognizes_not_n_wide() {
	let lut = lut_for(3, 3, |b| (0..3).map(|idx| !b[idx]).collect());
	let gate = recognize(3, 3, &lut).expect("NOT_N should be recognized");
	assert_eq!(eval_bools(&*gate, 3, 3, &[true, false, true]), vec![false, true, false]);
}

#[test]
fn recognizes_buffer_n_wide() {
	let lut = lut_for(3, 3, |b| (0..3).map(|idx| b[idx]).collect());
	let gate = recognize(3, 3, &lut).expect("BUFFER_N should be recognized");
	assert_eq!(eval_bools(&*gate, 3, 3, &[true, false, true]), vec![true, false, true]);
}

#[test]
fn recognizes_adder3_with_carry() {
	let lut = lut_for(6, 4, |b| {
		let a = (0..3).fold(0u64, |acc, idx| acc | ((b[idx] as u64) << idx));
		let c = (0..3).fold(0u64, |acc, idx| acc | ((b[3 + idx] as u64) << idx));
		let sum = a + c;
		(0..4).map(|idx| (sum >> idx) & 1 == 1).collect()
	});
	let gate = recognize(6, 4, &lut).expect("3-bit adder with carry should be recognized");
	// 5 + 7 = 12 (101 + 111 = 1100)
	assert_eq!(eval_bools(&*gate, 6, 4, &[true, false, true, true, true, true]), vec![false, false, true, true]);
}

#[test]
fn rejects_wrong_table_size() {
	// Table size doesn't match in_bits
	let lut = Lut::<u64, u64>::new(vec![0u64; 3].into_boxed_slice());
	assert!(recognize(2, 1, &lut).is_none());
}

#[test]
fn rejects_zero_bits() {
	let lut = Lut::<u64, u64>::new(vec![0u64; 1].into_boxed_slice());
	assert!(recognize(0, 1, &lut).is_none());
	assert!(recognize(1, 0, &lut).is_none());
}

#[test]
fn rejects_too_wide() {
	let lut = Lut::<u64, u64>::new(vec![0u64; 1].into_boxed_slice());
	assert!(recognize(65, 1, &lut).is_none());
	assert!(recognize(1, 65, &lut).is_none());
}

#[test]
fn formula_handles_64bit_operations() {
	// Test 64-bit buffer
	let gate = formula_from_candidate("BUFFER_N", 64, 64);
	let mut input = vec![false; 64];
	input[63] = true; // Set highest bit
	let expected = input.clone();
	let out = eval_bools(&gate, 64, 64, &input);
	assert_eq!(out, expected);

	// Test 64-bit NOT
	let gate = formula_from_candidate("NOT_N", 64, 64);
	let out = eval_bools(&gate, 64, 64, &input);
	let expected: Vec<bool> = input.iter().map(|&b| !b).collect();
	assert_eq!(out, expected);
}

#[test]
fn adder_variants_formula_correctness() {
	// Direct formula testing for adder variants
	let candidates = [
		("ADDER_N", 8, 4),         // 8-bit input, 4-bit output (no carry)
		("ADDER_N_COU", 8, 5),     // 8-bit input, 5-bit output (with carry out)
		("ADDER_N_CIN", 9, 4),     // 9-bit input (with carry in), 4-bit output
		("ADDER_N_CIN_COU", 9, 5), // 9-bit input (with carry in), 5-bit output
	];

	for (name, in_bits, out_bits) in candidates {
		let gate = formula_from_candidate(name, in_bits, out_bits);

		let n: usize = 4; // 4-bit operands
		let a: u64 = 5; // 0101
		let c: u64 = 3; // 0011
		let cin: u64 = if in_bits > 2 * n as u32 { 1 } else { 0 }; // carry in for 9-bit variants

		let mut input = vec![false; in_bits as usize];
		for i in 0..n {
			input[i] = (a >> i) & 1 == 1;
			input[n + i] = (c >> i) & 1 == 1;
		}
		if cin > 0 {
			input[2 * n] = true;
		}

		let out = eval_bools(&gate, in_bits, out_bits, &input);
		let actual: u64 = out.iter().enumerate().fold(0, |acc, (i, &b)| acc | ((b as u64) << i));
		let expected = a + c + cin;

		// If no carry out expected, mask to output bits
		let expected = if name.contains("COU") || out_bits > n as u32 { expected } else { expected & ((1 << n) - 1) };

		assert_eq!(actual, expected, "Failed for {}", name);
	}
}

#[test]
fn rejects_non_matching_candidate_quickly() {
	// Create a table that looks like AND for first few rows but differs later
	let lut = lut_for(3, 1, |b| {
		// Custom function that matches AND2 for first 2 inputs but ignores third
		vec![b[0] & b[1]]
	});
	// This should be recognized as AND_N (3-input AND would need all 3)
	// But our function ignores the 3rd input, so AND_N won't match
	// It should try AND_N first, fail, then not match anything else
	let result = recognize(3, 1, &lut);
	assert!(result.is_none());
}

#[test]
fn recognizes_wide_and() {
	// Test wide AND with 8 inputs
	let lut = lut_for(8, 1, |b| vec![b.iter().all(|&x| x)]);
	let gate = recognize(8, 1, &lut).expect("8-input AND should be recognized");
	let all_true = vec![true; 8];
	let one_false = {
		let mut v = vec![true; 8];
		v[3] = false;
		v
	};
	assert_eq!(eval_bools(&*gate, 8, 1, &all_true), vec![true]);
	assert_eq!(eval_bools(&*gate, 8, 1, &one_false), vec![false]);
}

#[test]
fn recognizes_wide_equality() {
	// Test 16-bit equality (8+8 inputs, 1 output)
	let lut = lut_for(16, 1, |b| vec![b[..8] == b[8..]]);
	let gate = recognize(16, 1, &lut).expect("16-bit equality should be recognized");

	// Test matching halves
	let mut input = vec![false; 16];
	input[0] = true;
	input[1] = false;
	input[2] = true;
	input[8] = true;
	input[9] = false;
	input[10] = true;
	assert_eq!(eval_bools(&*gate, 16, 1, &input), vec![true]);

	// Test non-matching halves
	input[10] = false;
	assert_eq!(eval_bools(&*gate, 16, 1, &input), vec![false]);
}

/// `find_candidate` checks the all-ones ("last") row before doing the full sweep as a fast
/// rejection anchor. AND2 and OR2 agree on every row except the all-ones one (AND2: 1, OR2:
/// 1 too -- so use AND2 vs. NAND2, which disagree exactly there: NAND2(1,1)=0, AND2(1,1)=1).
/// A genuine AND2 table must therefore still be recognized as AND2, not incorrectly bounced
/// by the anchor check.
#[test]
fn anchor_row_check_does_not_reject_a_true_match() {
	let lut = lut_for(2, 1, |b| vec![b[0] & b[1]]);
	let gate = recognize(2, 1, &lut).expect("AND2 should still be recognized with the anchor pre-check in place");
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![true]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![false]);
	assert_eq!(eval_bools(&*gate, 2, 1, &[false, false]), vec![false]);
}

/// A table that matches AND2 everywhere except the very last (all-ones) row: the anchor
/// check should reject this immediately, and no other candidate in the registry should
/// accidentally match it either.
#[test]
fn table_mismatching_only_at_last_row_is_rejected() {
	let lut = lut_for(2, 1, |b| {
		if b[0] && b[1] {
			vec![false] // AND2 says `true` here; this table disagrees only on this row
		} else {
			vec![b[0] & b[1]]
		}
	});
	assert!(recognize(2, 1, &lut).is_none());
}

/// Mirror of the previous case at a wider width, so the anchor check's "last row" is
/// `2^in_bits - 1` for a non-trivial `in_bits` rather than the trivial 2-bit case above.
#[test]
fn wide_table_mismatching_only_at_last_row_is_rejected() {
	let lut = lut_for(6, 1, |b| {
		if b.iter().all(|&x| x) {
			vec![false] // AND_N/AND6 says `true` on the all-ones row; this table disagrees only there
		} else {
			vec![b.iter().all(|&x| x)]
		}
	});
	assert!(recognize(6, 1, &lut).is_none());
}

/// A table that matches AND2's formula only at the last row but disagrees everywhere else
/// must still be rejected: passing the anchor check is necessary but not sufficient, so the
/// full sweep after it has to catch this.
#[test]
fn table_matching_only_at_last_row_is_rejected() {
	let lut = lut_for(2, 1, |b| {
		if b[0] && b[1] {
			vec![true] // agrees with AND2 here
		} else {
			vec![!(b[0] & b[1])] // disagrees with AND2 (and everything else) elsewhere
		}
	});
	assert!(recognize(2, 1, &lut).is_none());
}

/// Sanity check at a size (2^20 rows) large enough that an accidental O(candidates *
/// 2^in_bits) blow-up, rather than the intended fast anchor-based rejection, would make this
/// test noticeably slow -- correctness is what's asserted, the point is that it still
/// finishes quickly.
#[test]
fn recognizes_wide_and_with_many_input_bits() {
	let in_bits = 20;
	let lut = lut_for(in_bits, 1, |b| vec![b.iter().all(|&x| x)]);
	let gate = recognize(in_bits, 1, &lut).expect("20-input AND should be recognized");
	assert_eq!(eval_bools(&*gate, in_bits, 1, &vec![true; in_bits as usize]), vec![true]);
	let mut one_false = vec![true; in_bits as usize];
	one_false[19] = false;
	assert_eq!(eval_bools(&*gate, in_bits, 1, &one_false), vec![false]);
}
