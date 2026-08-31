use super::eval::{Lut, OptimizedGate};

pub enum CandidateShape {
	Fixed { in_bits: u32, out_bits: u32 },
	Parametric,
}

pub struct Candidate {
	pub name: &'static str,
	pub shape: CandidateShape,
	pub build: fn(in_bits: u32, out_bits: u32) -> Option<(Vec<u32>, Box<dyn OptimizedGate>)>,
}

fn bits_of(mut word: u32, count: u32) -> Vec<bool> {
	let mut out = Vec::with_capacity(count as usize);
	for _ in 0..count {
		out.push(word & 1 == 1);
		word >>= 1;
	}
	out
}

fn pack_bits(bits: &[bool]) -> u32 {
	bits.iter().enumerate().fold(0u32, |acc, (i, &b)| acc | ((b as u32) << i))
}

fn build_table(in_bits: u32, out_bits: u32, f: impl Fn(&[bool]) -> Vec<bool>) -> Option<(Vec<u32>, Box<dyn OptimizedGate>)> {
	if in_bits == 0 || in_bits > 32 || out_bits == 0 || out_bits > 32 {
		return None;
	}
	let rows = 1u64 << in_bits;
	let mut table = Vec::with_capacity(rows as usize);
	for i in 0..rows {
		let input = bits_of(i as u32, in_bits);
		let output = f(&input);
		if output.len() as u32 != out_bits {
			return None;
		}
		table.push(pack_bits(&output));
	}
	let gate: Box<dyn OptimizedGate> = Box::new(Lut::<u32, u32>::new(table.clone().into_boxed_slice()));
	Some((table, gate))
}

fn fixed(in_bits: u32, out_bits: u32) -> CandidateShape {
	CandidateShape::Fixed { in_bits, out_bits }
}

fn registry() -> &'static [Candidate] {
	static REGISTRY: std::sync::OnceLock<Vec<Candidate>> = std::sync::OnceLock::new();
	REGISTRY.get_or_init(|| {
		vec![
			Candidate { name: "NOT", shape: fixed(1, 1), build: |i, o| build_table(i, o, |b| vec![!b[0]]) },
			Candidate { name: "BUFFER", shape: fixed(1, 1), build: |i, o| build_table(i, o, |b| vec![b[0]]) },
			Candidate { name: "AND2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![b[0] & b[1]]) },
			Candidate { name: "OR2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![b[0] | b[1]]) },
			Candidate { name: "XOR2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![b[0] ^ b[1]]) },
			Candidate { name: "NAND2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![!(b[0] & b[1])]) },
			Candidate { name: "NOR2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![!(b[0] | b[1])]) },
			Candidate { name: "XNOR2", shape: fixed(2, 1), build: |i, o| build_table(i, o, |b| vec![!(b[0] ^ b[1])]) },
			Candidate {
				name: "AND_N",
				shape: CandidateShape::Parametric,
				build: |i, o| if o == 1 && i >= 2 { build_table(i, o, |b| vec![b.iter().all(|&x| x)]) } else { None },
			},
			Candidate {
				name: "OR_N",
				shape: CandidateShape::Parametric,
				build: |i, o| if o == 1 && i >= 2 { build_table(i, o, |b| vec![b.iter().any(|&x| x)]) } else { None },
			},
			Candidate {
				name: "XOR_N",
				shape: CandidateShape::Parametric,
				build: |i, o| if o == 1 && i >= 2 { build_table(i, o, |b| vec![b.iter().fold(false, |acc, &x| acc ^ x)]) } else { None },
			},
			Candidate {
				name: "NAND_N",
				shape: CandidateShape::Parametric,
				build: |i, o| if o == 1 && i >= 2 { build_table(i, o, |b| vec![!b.iter().all(|&x| x)]) } else { None },
			},
			Candidate {
				name: "NOR_N",
				shape: CandidateShape::Parametric,
				build: |i, o| if o == 1 && i >= 2 { build_table(i, o, |b| vec![!b.iter().any(|&x| x)]) } else { None },
			},
			Candidate {
				name: "EQUALS_N",
				shape: CandidateShape::Parametric,
				build: |i, o| {
					if o != 1 || i < 2 || i % 2 != 0 {
						return None;
					}
					let n = (i / 2) as usize;
					build_table(i, o, move |b| vec![b[..n] == b[n..]])
				},
			},
			Candidate {
				name: "ADDER_N",
				shape: CandidateShape::Parametric,
				build: |i, o| {
					if i < 2 || !i.is_multiple_of(2) || o != i / 2 + 1 {
						return None;
					}
					let n = (i / 2) as usize;
					build_table(i, o, move |b| {
						let a: u64 = b[..n].iter().enumerate().fold(0, |acc, (idx, &bit)| acc | ((bit as u64) << idx));
						let c: u64 = b[n..].iter().enumerate().fold(0, |acc, (idx, &bit)| acc | ((bit as u64) << idx));
						let sum = a + c;
						(0..=n).map(|idx| (sum >> idx) & 1 == 1).collect()
					})
				},
			},
		]
	})
}

pub fn recognize(in_bits: u32, out_bits: u32, table: &[u32]) -> Option<Box<dyn OptimizedGate>> {
	for candidate in registry() {
		let shape_ok = match candidate.shape {
			CandidateShape::Fixed { in_bits: i, out_bits: o } => i == in_bits && o == out_bits,
			CandidateShape::Parametric => true,
		};
		if !shape_ok {
			continue;
		}
		let Some((reference, gate)) = (candidate.build)(in_bits, out_bits) else { continue };
		if reference.len() == table.len() && reference == table {
			return Some(gate);
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pin_state::LogicState;

	fn eval_bools(gate: &dyn OptimizedGate, in_bits: u32, out_bits: u32, input: &[bool]) -> Vec<bool> {
		let states: Vec<LogicState> = input.iter().map(|&b| LogicState::from_bool(b)).collect();
		assert_eq!(states.len(), in_bits as usize);
		let mut out = vec![LogicState::Low; out_bits as usize];
		gate.eval(&states, &mut out);
		out.iter().map(|s| s.is_high()).collect()
	}

	fn table_for(in_bits: u32, _out_bits: u32, f: impl Fn(&[bool]) -> Vec<bool>) -> Vec<u32> {
		(0..1u64 << in_bits).map(|i| pack_bits(&f(&bits_of(i as u32, in_bits)))).collect()
	}

	#[test]
	fn recognizes_and2() {
		let table = table_for(2, 1, |b| vec![b[0] & b[1]]);
		let gate = recognize(2, 1, &table).expect("AND2 should be recognized");
		assert_eq!(eval_bools(&*gate, 2, 1, &[true, true]), vec![true]);
		assert_eq!(eval_bools(&*gate, 2, 1, &[true, false]), vec![false]);
	}

	#[test]
	fn recognizes_xor3_as_xor_n() {
		let table = table_for(3, 1, |b| vec![b[0] ^ b[1] ^ b[2]]);
		let gate = recognize(3, 1, &table).expect("XOR3 should be recognized");
		assert_eq!(eval_bools(&*gate, 3, 1, &[true, true, true]), vec![true]);
		assert_eq!(eval_bools(&*gate, 3, 1, &[true, true, false]), vec![false]);
	}

	#[test]
	fn recognizes_equals4() {
		let table = table_for(4, 1, |b| vec![b[..2] == b[2..]]);
		let gate = recognize(4, 1, &table).expect("EQUALS4 should be recognized");
		assert_eq!(eval_bools(&*gate, 4, 1, &[true, false, true, false]), vec![true]);
		assert_eq!(eval_bools(&*gate, 4, 1, &[true, false, false, false]), vec![false]);
	}

	#[test]
	fn recognizes_adder2() {
		let table = table_for(4, 3, |b| {
			let a = b[0] as u64 | ((b[1] as u64) << 1);
			let c = b[2] as u64 | ((b[3] as u64) << 1);
			let sum = a + c;
			(0..3).map(|idx| (sum >> idx) & 1 == 1).collect()
		});
		let gate = recognize(4, 3, &table).expect("ADDER2 should be recognized");
		assert_eq!(eval_bools(&*gate, 4, 3, &[true, true, true, false]), vec![false, false, true]);
	}

	#[test]
	fn unrecognized_table_returns_none() {
		let junk = vec![7u32; 4];
		assert!(recognize(2, 1, &junk).is_none());
	}
}
