//! `cargo run --release --example bench [-- <ticks>]`
//! Times build, cache construction and steady-state stepping on synthetic NAND-built circuits.
#![allow(clippy::pedantic, clippy::nursery, clippy::print_stdout)]
mod circuits;

use logic_sim::Simulator;
use logic_sim::audio::SimAudio;
use logic_sim::description::PinAddress;
use std::time::Instant;

fn run(label: &str, root: logic_sim::ChipDescription, lib: &logic_sim::ChipLibrary, ticks: u32, use_caching: bool, outs: &[i32]) -> u64 {
	let t = Instant::now();
	let mut sim = Simulator::build(&root, lib);
	sim.use_caching = use_caching;
	sim.steps_per_clock_transition = 4;
	let build = t.elapsed();
	let mut audio = SimAudio::new();

	let t = Instant::now();
	for _ in 0..20 {
		sim.run_simulation_step(&[], &mut audio); // includes lazy cache construction / order pass
	}
	let warm = t.elapsed();

	let t = Instant::now();
	for _ in 0..ticks {
		sim.run_simulation_step(&[], &mut audio);
	}
	let el = t.elapsed();

	let mut sig = 0u64;
	for &o in outs {
		let p = sim.find_pin(sim.root(), PinAddress::new(o, 0)).unwrap();
		sig = sig * 3 + u64::from(sim.pin(p).state.bit_states());
	}
	if std::env::var_os("BENCH_DETAIL").is_some() && use_caching {
		for n in ["NOT", "AND2", "OR2", "XOR2", "FA", "ADD8"] {
			if let Some(g) = sim.caching.combinational_chip_cache.get(n) {
				println!("    gate {n}: {g}");
			}
		}
		for st in sim.caching.memo.stats() {
			println!("    memo {st:?}");
		}
	}
	println!(
		"{label:<34} build {:>8.2?}  warmup(20 ticks) {:>10.2?}  {:>9.0} ticks/s  ({:.2?}/tick)  sig={sig}",
		build,
		warm,
		f64::from(ticks) / el.as_secs_f64(),
		el / ticks
	);
	sig
}

fn main() {
	let ticks: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
	let lib = circuits::library();
	let seq_outs: Vec<i32> = (0..16).map(|i| 1000 + i).collect();
	let comb_outs: Vec<i32> = (0..9).map(|i| 1000 + i).collect();

	println!("--- sequential: Johnson16 + 32 x REG8 (~300 flip-flops, ~2.7k NANDs) ---");
	run("seq / caching off", circuits::seq_root(32), &lib, ticks, false, &seq_outs);
	run("seq / caching on", circuits::seq_root(32), &lib, ticks, true, &seq_outs);

	println!("--- combinational: 8 chained ADD8 (opted into caching) ---");
	run("comb / caching off", circuits::comb_root(8), &lib, ticks, false, &comb_outs);
	run("comb / caching on", circuits::comb_root(8), &lib, ticks, true, &comb_outs);
}
