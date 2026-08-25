//! The simulation's own background thread, porting `DLS.Simulation.SimThread`'s
//! role (and `Project.StartSimulation`/`NotifyExit`): the whole `Simulator`
//! lives behind a mutex shared with the main thread, and a dedicated worker
//! steps it against the project's target rate -- using the exact pacing
//! math already ported in [`crate::viewer::sim_timing`] (tick-debt
//! accumulator + rolling throughput window) -- so simulation speed no
//! longer tracks the render framerate.
//!
//! Division of labour mirrors the original's thread/main split:
//! - main thread: builds/replaces simulators (`Project.LoadDevChipOrCreateNewIfDoesntExist`),
//!   toggles player-driven input dev-pins straight into the shared
//!   simulator (`Simulator::driven_inputs`), flips pause/target-rate
//!   prefs, and *reads* pin states freely for rendering (the read half of
//!   `ViewedChip.UpdateStateFromSim`, which ran on the sim thread there --
//!   here rendering simply locks the shared arena);
//! - worker thread: applies those driven inputs every step and runs the
//!   paced step loop, including the paused branch's decay-only
//!   `UpdateInPausedState` beat and the single-step-while-paused counter.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::pin_state::PinState;
use crate::sim::Simulator;
use crate::viewer::sim_timing::{accumulate_tick_debt, take_due_ticks, PerfWindow};

/// How long the worker idles between passes while paused -- the
/// `Thread.Sleep(10)` of the original's paused branch.
const PAUSED_SLEEP: Duration = Duration::from_millis(10);

/// Idle sleep between paced passes that owed no ticks: short enough that
/// the sim never visibly lags its schedule, long enough not to burn a
/// core spinning (the original busy-spin-waits instead; this port prefers
/// leaving the render thread alone).
const IDLE_SLEEP: Duration = Duration::from_micros(200);

/// Control plane shared between the main thread and the worker. Plain
/// atomics with relaxed ordering -- every value stands alone, so the
/// worst a torn ordering can do is apply a change one worker pass late.
struct SimControls {
	stop: AtomicBool,
	paused: AtomicBool,
	/// Set by the host to request exactly one step while paused
	/// (`Project.advanceSingleSimStep`).
	single_step: AtomicBool,
	/// Single steps advanced since the sim paused
	/// (`Project.simPausedSingleStepCounter`).
	step_counter: AtomicU32,
	target_ticks_per_second: AtomicU32,
	steps_per_clock_transition: AtomicU32,
	/// Latest measured average ticks/second, stored as `f64::to_bits`.
	avg_ticks_per_sec_bits: AtomicU64,
}

impl SimControls {
	fn new() -> Self {
		Self {
			stop: AtomicBool::new(false),
			paused: AtomicBool::new(false),
			single_step: AtomicBool::new(false),
			step_counter: AtomicU32::new(0),
			target_ticks_per_second: AtomicU32::new(1),
			steps_per_clock_transition: AtomicU32::new(0),
			avg_ticks_per_sec_bits: AtomicU64::new(0),
		}
	}
}

/// Main-thread handle over the simulated world: owns the shared
/// `Simulator`, the worker thread, and the control plane. Dropping it
/// stops the worker and joins it.
pub(crate) struct SimHandle {
	sim: Arc<Mutex<Simulator>>,
	controls: Arc<SimControls>,
	worker: Option<std::thread::JoinHandle<()>>,
}

impl SimHandle {
	/// Wraps `sim` and starts its background stepping thread.
	pub(crate) fn new(sim: Simulator, audio: crate::audio::SharedAudioState) -> Self {
		let sim = Arc::new(Mutex::new(sim));
		let controls = Arc::new(SimControls::new());
		let worker = spawn_worker(Arc::clone(&sim), Arc::clone(&controls), audio);
		Self { sim, controls, worker: Some(worker) }
	}

	/// Locks the shared simulator for reading/rendering or wholesale
	/// mutation. Recovers from poisoning like every other lock here: an
	/// audio panic must not take the editor down with it.
	pub(crate) fn lock(&self) -> MutexGuard<'_, Simulator> {
		self.sim.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
	}

	/// Swaps in a freshly built simulator (structural edit / chip switch /
	/// project open). Blocks until any in-flight step finishes, then
	/// replaces the arena wholesale -- the worker picks up whatever sits
	/// inside on its next pass, so no restart signalling is needed.
	pub(crate) fn replace(&self, sim: Simulator) {
		*self.lock() = sim;
	}

	// ---- Keyboard state feeding (the old direct field writes) ----

	/// Flips one bit of an input dev-pin's player-driven state -- what
	/// clicking that pin's per-bit grid does (see
	/// `Simulator::toggle_driven_input_bit`).
	pub(crate) fn toggle_driven_input_bit(&self, pin_id: i32, bit_index: u32) {
		self.lock().toggle_driven_input_bit(pin_id, bit_index);
	}

	/// Drops every player-driven input so all root inputs read as LOW --
	/// used when the viewer switches which chip it's simulating.
	pub(crate) fn reset_driven_inputs(&self) {
		self.lock().reset_driven_inputs();
	}

	pub(crate) fn key_modifiers(&self) -> u32 {
		self.lock().key_modifiers
	}

	pub(crate) fn set_key_modifiers(&self, modifiers: u32) {
		self.lock().key_modifiers = modifiers;
	}

	pub(crate) fn held_key_press(&self, key: char) {
		self.lock().held_keys.insert(key);
	}

	pub(crate) fn held_key_release(&self, key: char) {
		self.lock().held_keys.remove(&key);
	}

	pub(crate) fn clear_held_keys(&self) {
		self.lock().held_keys.clear();
	}

	/// Moves the player-driven transient input state (held keys +
	/// modifiers + toggled input dev-pins) out of the outgoing simulator
	/// so [`Self::replace`] can carry it into the rebuilt one -- what
	/// `ViewerState::rebuild_sim` used to do across its plain field swap.
	pub(crate) fn take_transient_input_state(&self) -> (HashSet<char>, u32, HashMap<i32, PinState>) {
		let mut sim = self.lock();
		(std::mem::take(&mut sim.held_keys), sim.key_modifiers, std::mem::take(&mut sim.driven_inputs))
	}

	// ---- Prefs-derived control plumbing ----

	pub(crate) fn set_steps_per_clock_transition(&self, steps: u32) {
		self.controls.steps_per_clock_transition.store(steps, Ordering::Relaxed);
	}

	pub(crate) fn set_paused(&self, paused: bool) {
		self.controls.paused.store(paused, Ordering::Relaxed);
		// No counter reset here: this is called every frame with the current
		// pref value, and the worker itself zeroes the single-step counter
		// on its first non-single-stepping pass -- which is also what makes
		// a re-pause start back at zero.
	}

	pub(crate) fn set_target_ticks_per_second(&self, ticks: u32) {
		self.controls.target_ticks_per_second.store(ticks.max(1), Ordering::Relaxed);
	}

	pub(crate) fn request_single_step(&self) {
		self.controls.single_step.store(true, Ordering::Relaxed);
	}

	/// Latest measured average ticks/second (`0` before anything measured).
	pub(crate) fn avg_ticks_per_sec(&self) -> f64 {
		f64::from_bits(self.controls.avg_ticks_per_sec_bits.load(Ordering::Relaxed))
	}

	pub(crate) fn paused_step_counter(&self) -> u32 {
		self.controls.step_counter.load(Ordering::Relaxed)
	}
}

impl Drop for SimHandle {
	fn drop(&mut self) {
		self.controls.stop.store(true, Ordering::Relaxed);
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

fn spawn_worker(sim: Arc<Mutex<Simulator>>, controls: Arc<SimControls>, audio: crate::audio::SharedAudioState) -> std::thread::JoinHandle<()> {
	std::thread::Builder::new()
		.name("DLS_SimThread".to_string())
		.spawn(move || worker_loop(sim, controls, audio))
		.expect("failed to spawn sim thread")
}

/// One simulation step -- `Simulator.RunSimulationStep(simChip, inputPins,
/// audioState.simAudio)`. The clock-speed pref is pushed in every step,
/// mirroring `SimThread.Run` assigning `Simulator.stepsPerClockTransition`
/// each iteration.
fn step_once(sim: &Mutex<Simulator>, controls: &SimControls, audio: &crate::audio::SharedAudioState) {
	let mut sim = sim.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
	sim.steps_per_clock_transition = controls.steps_per_clock_transition.load(Ordering::Relaxed);
	let mut audio_guard = audio.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
	sim.run_simulation_step(&[], &mut audio_guard.sim_audio);
}

fn worker_loop(sim: Arc<Mutex<Simulator>>, controls: Arc<SimControls>, audio: crate::audio::SharedAudioState) {
	#[derive(Default)]
	struct WorkerPacing {
		last_tick: Option<Instant>,
		debt_ticks: f64,
		window: PerfWindow,
		was_paused: bool,
	}

	let mut pacing = WorkerPacing::default();

	while !controls.stop.load(Ordering::Relaxed) {
		let now = Instant::now();
		let paused = controls.paused.load(Ordering::Relaxed);
		let step_requested = controls.single_step.swap(false, Ordering::Relaxed);

		if paused && !step_requested {
			// Mirrors `SimThread`'s paused branch: sleep-equivalent, no
			// debt accrues. The audio mix still decays
			// (`UpdateInPausedState`) so a sounding buzzer fades away
			// rather than hanging.
			let mut audio_guard = audio.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
			sim.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).update_in_paused_state(&mut audio_guard.sim_audio);
			pacing.last_tick = Some(now);
			pacing.debt_ticks = 0.0;
			pacing.was_paused = true;
			std::thread::sleep(PAUSED_SLEEP);
			continue;
		}

		if step_requested {
			controls.step_counter.fetch_add(1, Ordering::Relaxed);
		} else {
			controls.step_counter.store(0, Ordering::Relaxed);
		}

		if paused {
			// A requested single step runs exactly one tick regardless of
			// pacing (`Project.advanceSingleSimStep`) and mustn't disturb
			// the paused timing hold below.
			step_once(&sim, &controls, &audio);
			pacing.last_tick = Some(now);
			pacing.debt_ticks = 0.0;
			pacing.window.record(now, 1);
			store_avg(&controls, &pacing.window, now);
			std::thread::sleep(PAUSED_SLEEP);
			continue;
		}

		if pacing.was_paused {
			// Just unpaused: start measuring fresh so stale pre-pause
			// samples don't paint a burst onto the readout.
			pacing.window.clear();
			pacing.debt_ticks = 0.0;
			controls.avg_ticks_per_sec_bits.store(0, Ordering::Relaxed);
			pacing.was_paused = false;
			pacing.last_tick = None;
		}

		let elapsed = now.duration_since(pacing.last_tick.unwrap_or(now));
		pacing.last_tick = Some(now);
		let target_ticks_per_second = f64::from(controls.target_ticks_per_second.load(Ordering::Relaxed));
		pacing.debt_ticks = accumulate_tick_debt(pacing.debt_ticks, elapsed.as_secs_f64(), target_ticks_per_second);
		let (steps, remaining_debt) = take_due_ticks(pacing.debt_ticks);
		pacing.debt_ticks = remaining_debt;

		for _ in 0..steps {
			step_once(&sim, &controls, &audio);
		}
		if steps > 0 {
			pacing.window.record(now, steps);
			store_avg(&controls, &pacing.window, now);
			std::thread::yield_now();
		} else {
			std::thread::sleep(IDLE_SLEEP);
		}
	}
}

fn store_avg(controls: &SimControls, window: &PerfWindow, now: Instant) {
	if let Some(avg) = window.avg_per_sec(now) {
		controls.avg_ticks_per_sec_bits.store(avg.to_bits(), Ordering::Relaxed);
	}
}

#[cfg(test)]
mod tests {
	//! White-box: `SimHandle`'s worker only exists to service a live
	//! `ViewerState`, so driving the handle directly (with polling
	//! deadlines instead of sleeps) is the only way to observe the
	//! thread's pacing/single-step behaviour headlessly.

	use super::*;
	use crate::description::{ChipDescription, ChipLibrary, ChipType};

	fn blank_simulator() -> Simulator {
		let mut library = ChipLibrary::new();
		library.add(ChipDescription::new("BLANK", ChipType::Custom));
		Simulator::build(library.get("BLANK"), &library)
	}

	fn handle(paused: bool, ticks_per_second: u32) -> SimHandle {
		let handle = SimHandle::new(blank_simulator(), crate::audio::default_shared_state());
		handle.set_paused(paused);
		handle.set_target_ticks_per_second(ticks_per_second);
		handle.set_steps_per_clock_transition(250);
		handle
	}

	fn wait_until(deadline: Duration, mut predicate: impl FnMut() -> bool) -> bool {
		let start = Instant::now();
		while start.elapsed() < deadline {
			if predicate() {
				return true;
			}
			std::thread::sleep(Duration::from_micros(200));
		}
		predicate()
	}

	#[test]
	fn single_steps_advance_exactly_once_each_while_paused() {
		let h = handle(true, 1);
		assert_eq!(frame(&h), 0);

		for expected in 1..=3 {
			h.request_single_step();
			assert!(wait_until(Duration::from_secs(2), || frame(&h) >= expected), "step {expected} never landed");
		}
		assert_eq!(frame(&h), 3, "each request advances exactly one simulation frame");
		assert_eq!(h.paused_step_counter(), 3);
	}

	fn frame(h: &SimHandle) -> u64 {
		h.lock().simulation_frame
	}

	#[test]
	fn unpaused_sim_runs_on_the_worker_without_main_thread_help() {
		let h = handle(false, 500_000);
		assert!(wait_until(Duration::from_secs(5), || frame(&h) > 0), "worker never stepped");
		assert!(wait_until(Duration::from_secs(5), || h.avg_ticks_per_sec() > 0.0), "throughput window never measured");
	}

	#[test]
	fn replace_swaps_in_the_new_arena_for_subsequent_reads() {
		let h = handle(true, 1);
		h.replace(blank_simulator());
		assert_eq!(frame(&h), 0, "a replaced simulator starts from frame zero");
		// One lock scope: a second `lock()` inside the same statement would
		// deadlock the non-reentrant mutex.
		let no_such_chip = {
			let sim = h.lock();
			let root = sim.root();
			sim.find_sub_chip(root, 9999)
		};
		assert_eq!(no_such_chip, None, "the blank arena has no subchips to find");
	}

	#[test]
	fn transient_input_state_feeds_through_the_handle() {
		let h = handle(true, 1);
		h.set_key_modifiers(7);
		h.held_key_press('A');
		assert_eq!(h.key_modifiers(), 7);
		assert!(h.lock().held_keys.contains(&'A'));
		h.held_key_release('A');
		assert!(h.lock().held_keys.is_empty());
		h.held_key_press('B');
		h.lock().set_driven_input(3, crate::pin_state::PinState::HIGH);
		let (keys, mods, driven) = h.take_transient_input_state();
		assert!(keys.contains(&'B') && mods == 7);
		assert_eq!(driven.get(&3), Some(&crate::pin_state::PinState::HIGH), "toggled inputs travel with the rest");
		assert!(h.lock().held_keys.is_empty(), "take clears the source");
		assert!(h.lock().driven_inputs.is_empty(), "take clears the source");
	}
}
