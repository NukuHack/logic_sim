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
//!
//! Execution model (why this beats stepping tick-by-tick): every lock is
//! acquired once per *pass*, never once per *tick*. A pass grabs the arena
//! and audio state in one scope, runs all currently-due ticks inside it,
//! and gives the locks back -- so a catch-up burst costs two lock cycles
//! total rather than two per tick, and the worker can't end up parked
//! behind the realtime audio callback (which holds the same audio mutex
//! for whole output periods) thousands of times a second. Passes are also
//! time-sliced: if running flat out, the worker hands the arena back
//! after [`PASS_TIME_BUDGET`] instead of monopolising it, keeping render
//! latency bounded while leftover ticks stay owed as debt. Between passes
//! that owe nothing, the worker naps exactly until the next tick falls
//! due (bounded by [`MIN_IDLE_SLEEP`]/[`MAX_IDLE_SLEEP`]) -- the
//! original's `Thread.SpinWait` busy-wait expressed without burning a
//! core or waking on a fixed cadence into contention with the renderer.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::pin_state::PinState;
use crate::sim::Simulator;
use crate::viewer::sim_timing::{accumulate_tick_debt, restore_unfinished_ticks, take_due_ticks, PerfWindow};

/// How long the worker idles between passes while paused -- the
/// `Thread.Sleep(10)` of the original's paused branch.
const PAUSED_SLEEP: Duration = Duration::from_millis(10);

/// Shortest idle nap between passes that owed no ticks. Purely a floor so
/// an almost-due tick can't turn the loop into a spin-wait (the original
/// burned a core for sub-millisecond precision no one can see; the debt
/// accumulator averages over late wakes instead).
const MIN_IDLE_SLEEP: Duration = Duration::from_micros(50);

/// Longest idle nap between passes that owed no ticks: keeps stop/pause/
/// rate-change requests responsive even at tiny target rates (where the
/// next due tick may be many milliseconds away).
const MAX_IDLE_SLEEP: Duration = Duration::from_millis(2);

/// Longest a single pass may hold the arena while stepping when running
/// behind (flat out): past this, remaining due ticks go back to the debt
/// for the next pass so the renderer keeps getting the lock regularly.
const PASS_TIME_BUDGET: Duration = Duration::from_millis(1);

/// How often a pass checks its elapsed time against [`PASS_TIME_BUDGET`]
/// -- checking every step would make the clock read a visible fraction of
/// each step's cost.
const PASS_TIME_CHECK_INTERVAL: u64 = 64;

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

fn lock_sim(sim: &Mutex<Simulator>) -> MutexGuard<'_, Simulator> {
	sim.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Locks the shared buzzer-audio state for the simulation side, recovering
/// from a poisoned lock (an audio panic must not take the editor down).
fn lock_audio(audio: &crate::audio::SharedAudioState) -> std::sync::MutexGuard<'_, crate::audio::AudioState> {
	audio.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
		lock_sim(&self.sim)
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

	/// Snapshots every chip's volatile internal state (RAM/ROM contents,
	/// pulse countdowns, display buffers) so a rebuild can carry it into
	/// the new arena -- see [`Simulator::capture_internal_states`].
	pub(crate) fn capture_internal_states(&self) -> crate::sim::InternalStateMap {
		self.lock().capture_internal_states()
	}

	/// Snapshots every pin's live signal state so a rebuild can carry it
	/// into the new arena -- see [`Simulator::capture_pin_states`].
	pub(crate) fn capture_pin_states(&self) -> crate::sim::PinStateMap {
		self.lock().capture_pin_states()
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

/// Runs up to `max_steps` simulation ticks under a single acquisition of
/// both locks (`SimThread.Run`'s `RunSimulationStep`, with
/// `stepsPerClockTransition` assigned once per batch instead of once per
/// tick). Stops early once the pass has held the arena for
/// [`PASS_TIME_BUDGET`] -- the caller puts un-run ticks back onto the
/// debt -- so running flat out can't monopolise the arena away from the
/// renderer. Returns how many steps actually ran.
fn run_steps_batch(sim: &Mutex<Simulator>, audio: &crate::audio::SharedAudioState, steps_per_clock_transition: u32, max_steps: u64) -> u64 {
	let mut sim = lock_sim(sim);
	let mut audio_guard = lock_audio(audio);
	sim.steps_per_clock_transition = steps_per_clock_transition;
	let started = Instant::now();
	let mut done = 0u64;
	while done < max_steps {
		sim.run_simulation_step(&[], &mut audio_guard.sim_audio);
		done += 1;
		if done.is_multiple_of(PASS_TIME_CHECK_INTERVAL) && started.elapsed() >= PASS_TIME_BUDGET {
			break;
		}
	}
	done
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
			// rather than hanging. Same lock order as every other
			// two-lock scope here (arena before audio).
			{
				let mut sim_guard = lock_sim(&sim);
				let mut audio_guard = lock_audio(&audio);
				sim_guard.update_in_paused_state(&mut audio_guard.sim_audio);
			}
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
			run_steps_batch(&sim, &audio, controls.steps_per_clock_transition.load(Ordering::Relaxed), 1);
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

		let target_ticks_per_second = f64::from(controls.target_ticks_per_second.load(Ordering::Relaxed)).max(1.0);
		let elapsed = now.duration_since(pacing.last_tick.unwrap_or(now));
		pacing.last_tick = Some(now);
		pacing.debt_ticks = accumulate_tick_debt(pacing.debt_ticks, elapsed.as_secs_f64(), target_ticks_per_second);
		let (due, remaining_debt) = take_due_ticks(pacing.debt_ticks);
		pacing.debt_ticks = remaining_debt;

		if due == 0 {
			// Ahead of schedule: nap until the next tick falls due, so a
			// low target rate costs a handful of wakeups per second rather
			// than a wakeup storm hammering the shared locks.
			let until_due = Duration::from_secs_f64(pacing.debt_ticks / target_ticks_per_second);
			std::thread::sleep(until_due.clamp(MIN_IDLE_SLEEP, MAX_IDLE_SLEEP));
			continue;
		}

		let ran = run_steps_batch(&sim, &audio, controls.steps_per_clock_transition.load(Ordering::Relaxed), due);
		if ran < due {
			pacing.debt_ticks = restore_unfinished_ticks(pacing.debt_ticks, due - ran, target_ticks_per_second);
		}
		let post = Instant::now();
		pacing.window.record(post, ran);
		store_avg(&controls, &pacing.window, post);
		// Hand the core back so a renderer waiting on the arena gets it
		// promptly (a batch always runs >= 1 step, so `ran` > 0 here).
		std::thread::yield_now();
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

	/// Regression for the rework: sustained throughput has to land near the
	/// target rate. The old loop woke on a fixed 200µs cadence and paid
	/// two lock acquisitions (arena + realtime audio state) *per tick*,
	/// which capped measured speed well below target even for a blank
	/// chip; batching under one scope per pass must track the target
	/// closely. Generous margins keep it stable on loaded/CI machines --
	/// it only fails on gross pacing regressions.
	#[test]
	fn sustained_throughput_tracks_the_target_rate() {
		const TARGET: u32 = 40_000;
		let h = handle(false, TARGET);
		let reached = wait_until(Duration::from_secs(5), || frame(&h) >= u64::from(TARGET));
		assert!(reached, "only {} frames after the warm-up window", frame(&h));

		let start_frame = frame(&h);
		std::thread::sleep(Duration::from_secs(2));
		let measured = (frame(&h) - start_frame) as f64 / 2.0;
		assert!(measured >= f64::from(TARGET) * 0.7, "sustained {measured} ticks/sec against a target of {TARGET} -- pacing regressed");
		assert!(h.avg_ticks_per_sec() >= f64::from(TARGET) * 0.7, "readout shows {}", h.avg_ticks_per_sec());
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
