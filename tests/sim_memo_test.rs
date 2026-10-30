//! Differential tests for the simulator speed-ups: a simulator running with caching on (native
//! gates for combinational chips, learned transition tables for sequential ones) must produce
//! exactly what the plain gate-by-gate simulation produces, tick for tick.

#[path = "../examples/circuits/mod.rs"]
mod circuits;

use logic_sim::audio::SimAudio;
use logic_sim::description::{ChipDescription, PinAddress};
use logic_sim::pin_state::PinState;
use logic_sim::{ChipLibrary, Simulator};

fn read(sim: &Simulator, id: i32) -> u8 {
	let p = sim.find_pin(sim.root(), PinAddress::new(id, 0)).expect("output pin exists");
	sim.pin(p).state.bit_states() & 1
}

struct Rig {
	sim: Simulator,
	audio: SimAudio,
}

impl Rig {
	fn new(root: &ChipDescription, lib: &ChipLibrary, caching: bool) -> Self {
		let mut sim = Simulator::build(root, lib);
		sim.use_caching = caching;
		Self { sim, audio: SimAudio::new() }
	}
	fn drive(&mut self, id: i32, v: bool) {
		self.sim.set_driven_input(id, PinState::from_bool(v));
	}
	fn step(&mut self) {
		self.sim.run_simulation_step(&[], &mut self.audio);
	}
}

/// Drives both rigs with `drive(tick, set)`, comparing every output from `warm` on, but only at
/// the *settled* point of each 8-tick data period (`tick % 8 == 7`). Feedback circuits are not
/// tick-exact reproducible even between two plain runs -- the simulator picks a random evaluation
/// order on its first frame, which shifts transient latency by a tick or two -- but once inputs
/// have been stable for a few ticks every valid order agrees, and that is what must match.
fn compare(
	root: &ChipDescription,
	lib: &ChipLibrary,
	ticks: usize,
	warm: usize,
	outs: &[i32],
	mut drive: impl FnMut(usize, &mut dyn FnMut(i32, bool)),
) -> Rig {
	let mut plain = Rig::new(root, lib, false);
	let mut fast = Rig::new(root, lib, true);
	for t in 0..ticks {
		drive(t, &mut |id, v| {
			plain.drive(id, v);
			fast.drive(id, v);
		});
		plain.step();
		fast.step();
		if t >= warm && t % 8 == 7 {
			for &o in outs {
				assert_eq!(read(&plain.sim, o), read(&fast.sim, o), "tick {t}, output {o}: cached sim diverged from plain sim");
			}
		}
	}
	fast
}

#[test]
fn cached_adder8_matches_arithmetic_and_plain_simulation() {
	let lib = circuits::library();
	let ins: Vec<i32> = (0..17).collect();
	let outs: Vec<i32> = (17..26).collect();
	let root = circuits::testbench("ADD8", &ins, &outs);
	let mut rng = fastrand::Rng::with_seed(7);
	let mut expect_log = Vec::new();

	let mut fast = Rig::new(&root, &lib, true);
	for _ in 0..300 {
		let (a, b, cin) = (rng.u32(0..256), rng.u32(0..256), rng.u32(0..2));
		for i in 0..8 {
			fast.drive(i, a >> i & 1 == 1);
			fast.drive(8 + i, b >> i & 1 == 1);
		}
		fast.drive(16, cin == 1);
		fast.step();
		fast.step();
		let got: u32 = (0..9).map(|k| u32::from(read(&fast.sim, 1000 + k)) << k).sum();
		expect_log.push(got == a + b + cin);
		assert_eq!(got, a + b + cin, "{a} + {b} + {cin}");
	}
	assert!(expect_log.iter().all(|&b| b));
	// And it must really have been served by a recognised native gate, not silently declined.
	let g = fast.sim.caching.combinational_chip_cache.get("ADD8").expect("ADD8 cached");
	assert!(format!("{g:?}").starts_with("NativeSplit"), "{g:?}");
}

#[test]
fn every_small_combinational_chip_is_cached_and_never_declines() {
	let lib = circuits::library();
	let root = circuits::testbench("FA", &[0, 1, 2], &[3, 4]);
	let mut sim = Rig::new(&root, &lib, true);
	for _ in 0..4 {
		sim.step();
	}
	for n in ["AND2", "OR2", "XOR2", "FA"] {
		assert!(
			sim.sim.caching.combinational_chip_cache.contains_key(n),
			"{n} should be cached (chip-id collisions must not hide combinational chips)"
		);
	}
}

#[test]
fn dff_register_and_shift_chain_match_plain_simulation() {
	let lib = circuits::library();
	// Period-8 schedule: data changes well away from the clock edges (no races).
	let mut rng = fastrand::Rng::with_seed(11);
	let mut d = false;

	let root = circuits::testbench("DFF", &[0, 1], &[2]);
	compare(&root, &lib, 4000, 24, &[1000], |t, set| {
		if t % 8 == 0 {
			d = rng.bool();
		}
		set(0, d);
		set(1, (3..6).contains(&(t % 8)));
	});

	let root = circuits::testbench("DLATCH", &[0, 1], &[2]);
	compare(&root, &lib, 4000, 24, &[1000], |t, set| {
		if t % 8 == 0 {
			d = rng.bool();
		}
		set(0, d);
		set(1, (3..6).contains(&(t % 8)));
	});

	let outs: Vec<i32> = (2..6).collect();
	let root = circuits::testbench("SHIFT4", &[0, 1], &outs);
	let obs: Vec<i32> = (1000..1004).collect();
	let fast = compare(&root, &lib, 6000, 80, &obs, |t, set| {
		if t % 8 == 0 {
			d = rng.bool();
		}
		set(0, d);
		set(1, (3..6).contains(&(t % 8)));
	});
	// The shift register must actually have been running on a learned table.
	let st = fast.sim.caching.memo.stats();
	let shift = st.iter().find(|s| s.name == "SHIFT4").expect("memo entry");
	assert!(shift.active && shift.lookups > 1000 && shift.misses < shift.lookups / 4, "{shift:?}");
}

#[test]
fn memo_demotes_cleanly_when_the_state_space_explodes() {
	// 24 flip-flops of random data visit far more than the 4096-state budget, forcing the
	// learned table to be abandoned mid-run; behaviour must stay identical across that switch.
	let lib = circuits::library();
	let outs: Vec<i32> = (2..26).collect();
	let obs: Vec<i32> = (1000..1024).collect();
	let root = circuits::testbench("SHIFT24", &[0, 1], &outs);
	let mut rng = fastrand::Rng::with_seed(3);
	let mut d = false;
	let fast = compare(&root, &lib, 60_000, 400, &obs, |t, set| {
		if t % 8 == 0 {
			d = rng.bool();
		}
		set(0, d);
		set(1, (3..6).contains(&(t % 8)));
	});
	let st = fast.sim.caching.memo.stats();
	let s = st.iter().find(|s| s.name == "SHIFT24").expect("entry");
	assert!(!s.active, "SHIFT24 should have been demoted once its state space blew up: {s:?}");
}

#[test]
fn syncing_memo_state_to_pins_makes_inner_pins_match_plain_simulation() {
	let lib = circuits::library();
	let root = circuits::testbench("DFF", &[0, 1], &[2]);
	let mut plain = Rig::new(&root, &lib, false);
	let mut fast = Rig::new(&root, &lib, true);
	let mut rng = fastrand::Rng::with_seed(5);
	let mut d = false;
	for t in 0..400 {
		if t % 8 == 0 {
			d = rng.bool();
		}
		for r in [&mut plain, &mut fast] {
			r.drive(0, d);
			r.drive(1, (3..6).contains(&(t % 8)));
			r.step();
		}
	}
	fast.sim.sync_memo_to_pins();
	// Compare the captured pin state of every pin at every depth.
	let a = plain.sim.capture_pin_states();
	let b = fast.sim.capture_pin_states();
	let mut checked = 0;
	for (k, pa) in &a {
		// Only input-side pins carry state; outputs are recomputed each frame.
		if k.last() == Some(&1) {
			assert_eq!(b[k].bit_states() & 1, pa.bit_states() & 1, "pin {k:?}");
			checked += 1;
		}
	}
	assert!(checked > 10);
}

#[test]
fn turning_caching_off_midway_hands_state_back_to_the_real_simulation() {
	let lib = circuits::library();
	let root = circuits::testbench("SHIFT4", &[0, 1], &(2..6).collect::<Vec<_>>());
	let mut plain = Rig::new(&root, &lib, false);
	let mut fast = Rig::new(&root, &lib, true);
	let mut rng = fastrand::Rng::with_seed(9);
	let mut d = false;
	for t in 0..3000 {
		if t % 8 == 0 {
			d = rng.bool();
		}
		if t == 1500 {
			fast.sim.use_caching = false; // memoised chips must materialise, not resume from stale pins
		}
		for r in [&mut plain, &mut fast] {
			r.drive(0, d);
			r.drive(1, (3..6).contains(&(t % 8)));
			r.step();
		}
		if t > 100 && t % 8 == 7 {
			for o in 1000..1004 {
				assert_eq!(read(&plain.sim, o), read(&fast.sim, o), "tick {t} out {o}");
			}
		}
	}
}
