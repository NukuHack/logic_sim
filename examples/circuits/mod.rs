//! Synthetic NAND-built circuits shared by `examples/bench.rs` and the differential tests in
//! `tests/sim_memo_test.rs`: full adders, an 8-bit ripple adder (combinational), and D latches /
//! master-slave flip-flops / registers / shift registers (sequential), all built from nothing but
//! the NAND builtin, the way a player would. Pin conventions (same as the real fixtures): a
//! chip's own dev-pin is addressed as `(owner = its pin id, pin = 0)`; a subchip's pin as
//! `(owner = subchip id, pin = that pin's id)`; NAND is `in 0, in 1 -> out 2`.
#![allow(dead_code, clippy::pedantic, clippy::nursery)]

use glam::Vec2;
use logic_sim::description::{CacheKind, ChipDescription, ChipType, PinAddress, PinBitCount, PinDescription, SubChipDescription, WireDescription};
use logic_sim::{ChipLibrary, register_all_builtins};

pub struct B {
	pub desc: ChipDescription,
	next_sub: i32,
}

impl B {
	pub fn new(name: &str) -> Self {
		let mut desc = ChipDescription::new(name, ChipType::Custom);
		desc.cache_kind = CacheKind::None; // what Save's auto-cache rule gives a small combinational chip
		Self { desc, next_sub: 100 }
	}
	pub fn input(mut self, name: &str, id: i32) -> Self {
		self.desc.input_pins.push(PinDescription::new(name, id, PinBitCount::Bit1));
		self
	}
	pub fn output(mut self, name: &str, id: i32) -> Self {
		self.desc.output_pins.push(PinDescription::new(name, id, PinBitCount::Bit1));
		self
	}
	pub fn sub(&mut self, name: &str) -> i32 {
		let id = self.next_sub;
		self.next_sub += 1;
		self.desc.sub_chips.push(SubChipDescription {
			name: name.into(),
			id,
			internal_data: None,
			position: Vec2::ZERO,
			label: None,
			pin_colour_info: vec![],
		});
		id
	}
	/// Wire `(owner, pin)` -> `(owner, pin)`. Dev-pins pass `pin = 0`.
	pub fn wire(&mut self, from: (i32, i32), to: (i32, i32)) {
		self.desc.wires.push(WireDescription::new(PinAddress::new(from.0, from.1), PinAddress::new(to.0, to.1)));
	}
	pub fn done(self) -> ChipDescription {
		self.desc
	}
}

fn nand(b: &mut B) -> i32 {
	b.sub("NAND")
}

pub fn not_chip() -> ChipDescription {
	let mut b = B::new("NOT").input("IN", 0).output("OUT", 1);
	let n = nand(&mut b);
	b.wire((0, 0), (n, 0));
	b.wire((0, 0), (n, 1));
	b.wire((n, 2), (1, 0));
	b.done()
}

pub fn and2() -> ChipDescription {
	let mut b = B::new("AND2").input("A", 0).input("B", 1).output("OUT", 2);
	let n1 = nand(&mut b);
	let n2 = nand(&mut b);
	b.wire((0, 0), (n1, 0));
	b.wire((1, 0), (n1, 1));
	b.wire((n1, 2), (n2, 0));
	b.wire((n1, 2), (n2, 1));
	b.wire((n2, 2), (2, 0));
	b.done()
}

pub fn or2() -> ChipDescription {
	let mut b = B::new("OR2").input("A", 0).input("B", 1).output("OUT", 2);
	let (n1, n2, n3) = (nand(&mut b), nand(&mut b), nand(&mut b));
	b.wire((0, 0), (n1, 0));
	b.wire((0, 0), (n1, 1));
	b.wire((1, 0), (n2, 0));
	b.wire((1, 0), (n2, 1));
	b.wire((n1, 2), (n3, 0));
	b.wire((n2, 2), (n3, 1));
	b.wire((n3, 2), (2, 0));
	b.done()
}

pub fn xor2() -> ChipDescription {
	let mut b = B::new("XOR2").input("A", 0).input("B", 1).output("OUT", 2);
	let (n1, n2, n3, n4) = (nand(&mut b), nand(&mut b), nand(&mut b), nand(&mut b));
	b.wire((0, 0), (n1, 0));
	b.wire((1, 0), (n1, 1));
	b.wire((0, 0), (n2, 0));
	b.wire((n1, 2), (n2, 1));
	b.wire((1, 0), (n3, 0));
	b.wire((n1, 2), (n3, 1));
	b.wire((n2, 2), (n4, 0));
	b.wire((n3, 2), (n4, 1));
	b.wire((n4, 2), (2, 0));
	b.done()
}

/// `A(0) B(1) CIN(2) -> S(3) COUT(4)`
pub fn full_adder() -> ChipDescription {
	let mut b = B::new("FA").input("A", 0).input("B", 1).input("CIN", 2).output("S", 3).output("COUT", 4);
	let (x1, x2, a1, a2, o) = (b.sub("XOR2"), b.sub("XOR2"), b.sub("AND2"), b.sub("AND2"), b.sub("OR2"));
	b.wire((0, 0), (x1, 0));
	b.wire((1, 0), (x1, 1));
	b.wire((x1, 2), (x2, 0));
	b.wire((2, 0), (x2, 1));
	b.wire((x2, 2), (3, 0));
	b.wire((0, 0), (a1, 0));
	b.wire((1, 0), (a1, 1));
	b.wire((x1, 2), (a2, 0));
	b.wire((2, 0), (a2, 1));
	b.wire((a1, 2), (o, 0));
	b.wire((a2, 2), (o, 1));
	b.wire((o, 2), (4, 0));
	b.done()
}

/// `A0..A7 = 0..7, B0..B7 = 8..15, CIN = 16 -> S0..S7 = 17..24, COUT = 25` (17 input bits).
pub fn adder8() -> ChipDescription {
	let mut b = B::new("ADD8").input("CIN", 16);
	b.desc.input_pins.clear();
	for i in 0..8 {
		b.desc.input_pins.push(PinDescription::new(format!("A{i}"), i, PinBitCount::Bit1));
	}
	for i in 0..8 {
		b.desc.input_pins.push(PinDescription::new(format!("B{i}"), 8 + i, PinBitCount::Bit1));
	}
	b.desc.input_pins.push(PinDescription::new("CIN", 16, PinBitCount::Bit1));
	for i in 0..8 {
		b.desc.output_pins.push(PinDescription::new(format!("S{i}"), 17 + i, PinBitCount::Bit1));
	}
	b.desc.output_pins.push(PinDescription::new("COUT", 25, PinBitCount::Bit1));
	let mut carry = (16, 0);
	for i in 0..8 {
		let fa = b.sub("FA");
		b.wire((i, 0), (fa, 0));
		b.wire((8 + i, 0), (fa, 1));
		b.wire(carry, (fa, 2));
		b.wire((fa, 3), (17 + i, 0));
		carry = (fa, 4);
	}
	b.wire(carry, (25, 0));
	b.done()
}

/// `D(0) E(1) -> Q(2)`: gated NAND D latch.
pub fn dlatch() -> ChipDescription {
	let mut b = B::new("DLATCH").input("D", 0).input("E", 1).output("Q", 2);
	b.desc.cache_kind = CacheKind::Off; // sequential
	let (n1, n2, q, qb) = (nand(&mut b), nand(&mut b), nand(&mut b), nand(&mut b));
	b.wire((0, 0), (n1, 0));
	b.wire((1, 0), (n1, 1));
	b.wire((n1, 2), (n2, 0));
	b.wire((1, 0), (n2, 1));
	b.wire((n1, 2), (q, 0));
	b.wire((qb, 2), (q, 1));
	b.wire((n2, 2), (qb, 0));
	b.wire((q, 2), (qb, 1));
	b.wire((q, 2), (2, 0));
	b.done()
}

/// `D(0) CLK(1) -> Q(2)`: master-slave, rising-edge D flip-flop.
pub fn dff() -> ChipDescription {
	let mut b = B::new("DFF").input("D", 0).input("CLK", 1).output("Q", 2);
	b.desc.cache_kind = CacheKind::Off;
	let (inv, m, s) = (b.sub("NOT"), b.sub("DLATCH"), b.sub("DLATCH"));
	b.wire((1, 0), (inv, 0));
	b.wire((0, 0), (m, 0));
	b.wire((inv, 1), (m, 1));
	b.wire((m, 2), (s, 0));
	b.wire((1, 0), (s, 1));
	b.wire((s, 2), (2, 0));
	b.done()
}

/// `D0..D7 = 0..7, CLK = 8 -> Q0..Q7 = 9..16`
pub fn reg8() -> ChipDescription {
	let mut b = B::new("REG8");
	b.desc.cache_kind = CacheKind::Off;
	for i in 0..8 {
		b.desc.input_pins.push(PinDescription::new(format!("D{i}"), i, PinBitCount::Bit1));
	}
	b.desc.input_pins.push(PinDescription::new("CLK", 8, PinBitCount::Bit1));
	for i in 0..8 {
		b.desc.output_pins.push(PinDescription::new(format!("Q{i}"), 9 + i, PinBitCount::Bit1));
	}
	for i in 0..8 {
		let f = b.sub("DFF");
		b.wire((i, 0), (f, 0));
		b.wire((8, 0), (f, 1));
		b.wire((f, 2), (9 + i, 0));
	}
	b.done()
}

/// Johnson (twisted-ring) counter: `CLK(0) -> Q0..Q15 = 1..16`. Runs forever from any power-up state.
pub fn johnson16() -> ChipDescription {
	let mut b = B::new("JOHNSON16").input("CLK", 0);
	b.desc.cache_kind = CacheKind::Off;
	for i in 0..16 {
		b.desc.output_pins.push(PinDescription::new(format!("Q{i}"), 1 + i, PinBitCount::Bit1));
	}
	let inv = b.sub("NOT");
	let ffs: Vec<i32> = (0..16).map(|_| b.sub("DFF")).collect();
	for (i, &f) in ffs.iter().enumerate() {
		b.wire((0, 0), (f, 1));
		if i == 0 {
			b.wire((inv, 1), (f, 0));
		} else {
			b.wire((ffs[i - 1], 2), (f, 0));
		}
		b.wire((f, 2), (1 + i as i32, 0));
	}
	b.wire((ffs[15], 2), (inv, 0));
	b.done()
}

/// `D(0) CLK(1) -> Q0..Q(n-1) = 2..` : `n` flip-flops in a chain (shift register).
pub fn shift_chip(n: i32) -> ChipDescription {
	let mut b = B::new(&format!("SHIFT{n}")).input("D", 0).input("CLK", 1);
	b.desc.cache_kind = CacheKind::Off;
	for i in 0..n {
		b.desc.output_pins.push(PinDescription::new(format!("Q{i}"), 2 + i, PinBitCount::Bit1));
	}
	let mut prev = (0, 0);
	for i in 0..n {
		let f = b.sub("DFF");
		b.wire(prev, (f, 0));
		b.wire((1, 0), (f, 1));
		b.wire((f, 2), (2 + i, 0));
		prev = (f, 2);
	}
	b.done()
}

/// Root with one subchip `sub`, whose input pins are fed by root input dev-pins `0..n_in`
/// (pin ids == the subchip's own input pin ids) and whose outputs go to root outputs `1000..`.
pub fn testbench(sub: &str, in_ids: &[i32], out_ids: &[i32]) -> ChipDescription {
	let mut b = B::new("TB");
	b.desc.cache_kind = CacheKind::Off;
	for &i in in_ids {
		b.desc.input_pins.push(PinDescription::new(format!("I{i}"), i, PinBitCount::Bit1));
	}
	for (k, _) in out_ids.iter().enumerate() {
		b.desc.output_pins.push(PinDescription::new(format!("O{k}"), 1000 + k as i32, PinBitCount::Bit1));
	}
	let s = b.sub(sub);
	for &i in in_ids {
		b.wire((i, 0), (s, i));
	}
	for (k, &o) in out_ids.iter().enumerate() {
		b.wire((s, o), (1000 + k as i32, 0));
	}
	b.done()
}

pub fn library() -> ChipLibrary {
	let mut lib = ChipLibrary::new();
	register_all_builtins(&mut lib);
	for d in [not_chip(), and2(), or2(), xor2(), full_adder(), adder8(), dlatch(), dff(), reg8(), johnson16(), shift_chip(4), shift_chip(24)] {
		lib.add(d);
	}
	lib
}

/// Root: CLOCK -> JOHNSON16 -> `n_regs` x REG8 (each loads Q0..Q7 on the shared clock).
/// Outputs `OUT0..OUT7` (ids 1000..) = REG8[0].Q, `OUT8..OUT15` (1008..) = Johnson Q8..Q15.
pub fn seq_root(n_regs: usize) -> ChipDescription {
	let mut b = B::new("BENCH_SEQ");
	b.desc.cache_kind = CacheKind::Off;
	for i in 0..16 {
		b.desc.output_pins.push(PinDescription::new(format!("OUT{i}"), 1000 + i, PinBitCount::Bit1));
	}
	let clk = b.sub("CLOCK");
	let j = b.sub("JOHNSON16");
	b.wire((clk, 0), (j, 0));
	for r in 0..n_regs {
		let reg = b.sub("REG8");
		for i in 0..8 {
			b.wire((j, 1 + i), (reg, i));
		}
		b.wire((clk, 0), (reg, 8));
		if r == 0 {
			for i in 0..8 {
				b.wire((reg, 9 + i), (1000 + i, 0));
			}
		}
	}
	for i in 0..8 {
		b.wire((j, 9 + i), (1008 + i, 0));
	}
	b.done()
}

/// Root: CLOCK -> JOHNSON16 -> chain of `n` ADD8 (A = previous sum / Johnson bits, B = Johnson taps).
/// Outputs `OUT0..OUT7` (ids 1000..) = last sum, `OUT8` (1008) = last carry.
pub fn comb_root(n: usize) -> ChipDescription {
	let mut b = B::new("BENCH_COMB");
	b.desc.cache_kind = CacheKind::Off;
	for i in 0..9 {
		b.desc.output_pins.push(PinDescription::new(format!("OUT{i}"), 1000 + i, PinBitCount::Bit1));
	}
	let clk = b.sub("CLOCK");
	let j = b.sub("JOHNSON16");
	b.wire((clk, 0), (j, 0));
	let mut prev: Option<i32> = None;
	for _ in 0..n {
		let a = b.sub("ADD8");
		for i in 0..8 {
			match prev {
				None => b.wire((j, 1 + i), (a, i)),
				Some(p) => b.wire((p, 17 + i), (a, i)),
			}
			b.wire((j, 9 + i), (a, 8 + i));
		}
		b.wire((j, 1), (a, 16));
		prev = Some(a);
	}
	let last = prev.unwrap();
	for i in 0..8 {
		b.wire((last, 17 + i), (1000 + i, 0));
	}
	b.wire((last, 25), (1008, 0));
	b.done()
}
