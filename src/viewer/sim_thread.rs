//! The simulation's own background thread, porting `DLS.Simulation.SimThread`'s role (and
//! `Project.StartSimulation`/`NotifyExit`): the whole `Simulator` lives behind a mutex shared
//! with the main thread, and a dedicated worker steps it against the project's target rate,
//! using the pacing math in [`crate::viewer::sim_timing`] (tick-debt accumulator + rolling
//! throughput window), so simulation speed doesn't track the render framerate.
//!
//! A pass runs all currently-due ticks under one arena lock against a worker-owned copy of the
//! buzzer mix, which is exchanged with the shared audio state only at the pass edges -- the
//! realtime audio callback never waits on a whole pass. Passes are time-sliced, and they yield
//! the moment another thread queues for the arena (see [`SimControls::lock_waiters`]): without
//! that, an unfair mutex lets a flat-out worker re-take the lock before the renderer ever wakes,
//! which freezes the UI at very high tick rates.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::audio::SimAudio;
use crate::pin_state::PinState;
use crate::sim::Simulator;
use crate::viewer::sim_timing::{PerfWindow, accumulate_tick_debt, restore_unfinished_ticks, take_due_ticks};

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

/// Target spacing between clock reads while stepping. The read interval adapts to the measured
/// step cost, so a heavy circuit can't overshoot the budget by dozens of slow steps and a trivial
/// one doesn't pay for a clock read per step.
const CLOCK_CHECK_SPACING: Duration = Duration::from_micros(100);

/// Upper bound on steps between clock reads, for circuits whose steps cost almost nothing.
const MAX_STEPS_BETWEEN_CLOCK_CHECKS: u64 = 256;

/// How long the worker waits for a queued lock requester to get in before carrying on anyway --
/// a safety net so a miscounted waiter can never stall the simulation.
const MAX_WAITER_YIELD: Duration = Duration::from_millis(2);

/// Control plane shared between the main thread and the worker. Plain
/// atomics with relaxed ordering -- every value stands alone, so the
/// worst a torn ordering can do is apply a change one worker pass late.
#[derive(Default)]
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
	/// Threads currently blocked in [`SimHandle::lock`]. The worker polls this between steps and
	/// hands the arena over when it's non-zero.
	lock_waiters: AtomicU32,
}

impl SimControls {
	const fn new() -> Self {
		Self {
			stop: AtomicBool::new(false),
			paused: AtomicBool::new(false),
			single_step: AtomicBool::new(false),
			step_counter: AtomicU32::new(0),
			target_ticks_per_second: AtomicU32::new(1),
			steps_per_clock_transition: AtomicU32::new(0),
			avg_ticks_per_sec_bits: AtomicU64::new(0),
			lock_waiters: AtomicU32::new(0),
		}
	}
}

fn lock_sim(sim: &Mutex<Simulator>) -> MutexGuard<'_, Simulator> {
	sim.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Locks the shared buzzer-audio state for the simulation side, recovering
/// from a poisoned lock (an audio panic must not take the editor down).
fn lock_audio(audio: &crate::audio::SharedAudioState) -> MutexGuard<'_, crate::audio::AudioState> {
	audio.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Main-thread handle over the simulated world: owns the shared
/// `Simulator`, the worker thread, and the control plane. Dropping it
/// stops the worker and joins it.
#[derive(Default)]
pub(crate) struct SimHandle {
	sim: Arc<Mutex<Simulator>>,
	controls: Arc<SimControls>,
	worker: Option<std::thread::JoinHandle<()>>,
}

#[allow(unused)]
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
		match self.sim.try_lock() {
			Ok(guard) => return guard,
			Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
			Err(std::sync::TryLockError::WouldBlock) => {}
		}
		// Announce the wait so the worker breaks off its pass; the count only matters while blocked.
		self.controls.lock_waiters.fetch_add(1, Ordering::Relaxed);
		let guard = lock_sim(&self.sim);
		self.controls.lock_waiters.fetch_sub(1, Ordering::Relaxed);
		guard
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

	pub(crate) fn key_modifiers(&self) -> u16 {
		self.lock().key_modifiers()
	}

	/// Back-compat bitmask setter -- see `Simulator::set_modifier_bits`.
	pub(crate) fn set_key_modifiers(&self, modifiers: u16) {
		self.lock().set_modifier_bits(modifiers);
	}

	pub(crate) fn held_key_press(&self, key: char) {
		self.lock().press_key(crate::sim::KeyCode::Char(key));
	}

	pub(crate) fn held_key_release(&self, key: char) {
		self.lock().release_key(crate::sim::KeyCode::Char(key));
	}

	/// Presses or releases any tracked keyboard button -- not just the
	/// alphanumeric ones `held_key_press`/`held_key_release` cover -- in the
	/// simulator's keyboard snapshot.
	pub(crate) fn set_key_pressed(&self, code: crate::sim::KeyCode, pressed: bool) {
		self.lock().set_key_pressed(code, pressed);
	}

	/// Releases every tracked key at once (focus-lost handling -- replaces
	/// the old `clear_held_keys` + `set_key_modifiers(0)` pair now that both
	/// live in the same snapshot).
	pub(crate) fn release_all_keys(&self) {
		self.lock().release_all_keys();
	}

	/// Moves the player-driven transient input state (the whole keyboard
	/// snapshot + toggled input dev-pins) out of the outgoing simulator so
	/// [`Self::replace`] can carry it into the rebuilt one -- what
	/// `ViewerState::rebuild_sim` used to do across its plain field swap.
	pub(crate) fn take_transient_input_state(&self) -> (crate::sim::KeyboardSnapshot, HashMap<i32, PinState>) {
		let mut sim = self.lock();
		(std::mem::take(&mut sim.keyboard), std::mem::take(&mut sim.driven_inputs))
	}

	/// Snapshots every chip's volatile internal state (RAM/ROM contents,
	/// pulse countdowns, display buffers) so a rebuild can carry it into
	/// the new arena -- see [`Simulator::capture_internal_states`].
	pub(crate) fn capture_internal_states(&self) -> crate::sim::InternalStateMap {
		let mut sim = self.lock();
		sim.sync_memo_to_pins();
		sim.capture_internal_states()
	}

	/// Snapshots every pin's live signal state so a rebuild can carry it
	/// into the new arena -- see [`Simulator::capture_pin_states`].
	pub(crate) fn capture_pin_states(&self) -> crate::sim::PinStateMap {
		let mut sim = self.lock();
		sim.sync_memo_to_pins();
		sim.capture_pin_states()
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

	// ---- Combinational-chip caching (see `crate::gate_op::caching`) ----

	/// Flips the shared simulator's "use cached truth tables for
	/// combinational chips" toggle -- the customization checkbox's
	/// effect, applied every frame from `v.prefs.prefs_use_caching`
	pub(crate) fn set_use_caching(&self, enabled: bool) {
		self.lock().use_caching = enabled;
	}

	/// Captures the built-up LUT cache out of the outgoing simulator
	pub(crate) fn capture_caching_state(&self, mut sim: Simulator) {
		let mut guard = self.lock();
		sim.caching = std::mem::take(&mut guard.caching);
		*guard = sim;
	}

	/// Drops one cached LUT
	pub(crate) fn clear_caching(&self, name: &str) {
		let mut sim = self.lock();
		sim.caching.combinational_chip_cache.remove(name);
		sim.caching.not_combinational_chip_cache.remove(name);
		// Learned sequential tables may embed the changed chip anywhere underneath them.
		sim.clear_memo();
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

#[allow(clippy::expect_used)]
fn spawn_worker(sim: Arc<Mutex<Simulator>>, controls: Arc<SimControls>, audio: crate::audio::SharedAudioState) -> std::thread::JoinHandle<()> {
	std::thread::Builder::new()
		.name("DLS_SimThread".to_string())
		.spawn(move || worker_loop(&sim, &controls, &audio))
		.expect("failed to spawn sim thread")
}

/// Lets a queued lock requester take the arena before the worker re-locks it.
fn yield_to_lock_waiters(controls: &SimControls) {
	let started = Instant::now();
	while controls.lock_waiters.load(Ordering::Relaxed) != 0 && started.elapsed() < MAX_WAITER_YIELD {
		std::thread::sleep(MIN_IDLE_SLEEP);
	}
}

/// Runs the audio-touching part of a pass against `local_audio`: the shared mix is copied in
/// before and out after, each under a momentary lock, so the audio callback is never kept
/// waiting for the length of the pass (the copy-in also picks up an external `silence`).
fn with_local_audio<R>(audio: &crate::audio::SharedAudioState, local_audio: &mut SimAudio, pass: impl FnOnce(&mut SimAudio) -> R) -> R {
	local_audio.clone_from(&lock_audio(audio).sim_audio);
	let result = pass(local_audio);
	lock_audio(audio).sim_audio.clone_from(local_audio);
	result
}

/// Runs up to `max_steps` simulation ticks under a single arena lock (`SimThread.Run`'s
/// `RunSimulationStep`, with `stepsPerClockTransition` assigned once per batch instead of once
/// per tick). Stops early when the time budget runs out or another thread is queued for the
/// arena; always runs at least one step.
fn run_steps_batch(
	sim: &Mutex<Simulator>,
	audio: &crate::audio::SharedAudioState,
	local_audio: &mut SimAudio,
	controls: &SimControls,
	max_steps: u64,
) -> u64 {
	let steps_per_clock_transition = controls.steps_per_clock_transition.load(Ordering::Relaxed);
	with_local_audio(audio, local_audio, |local_audio| {
		let mut sim = lock_sim(sim);
		sim.steps_per_clock_transition = steps_per_clock_transition;
		let started = Instant::now();
		let mut done = 0u64;
		let mut next_clock_check = 1u64;
		while done < max_steps {
			sim.run_simulation_step(&[], local_audio);
			done += 1;
			if controls.lock_waiters.load(Ordering::Relaxed) != 0 {
				break;
			}
			if done >= next_clock_check {
				let elapsed = started.elapsed();
				if elapsed >= PASS_TIME_BUDGET {
					break;
				}
				let nanos_per_step = (elapsed.as_nanos() / u128::from(done)).max(1);
				let steps_until_next_check = (CLOCK_CHECK_SPACING.as_nanos() / nanos_per_step) as u64;
				next_clock_check = done + steps_until_next_check.clamp(1, MAX_STEPS_BETWEEN_CLOCK_CHECKS);
			}
		}
		done
	})
}

fn worker_loop(sim: &Arc<Mutex<Simulator>>, controls: &Arc<SimControls>, audio: &crate::audio::SharedAudioState) {
	#[derive(Default)]
	struct WorkerPacing {
		last_tick: Option<Instant>,
		debt_ticks: f64,
		window: PerfWindow,
		was_paused: bool,
	}

	let mut pacing = WorkerPacing::default();
	let mut local_audio = SimAudio::new();

	while !controls.stop.load(Ordering::Relaxed) {
		yield_to_lock_waiters(controls);
		let now = Instant::now();
		let paused = controls.paused.load(Ordering::Relaxed);
		let step_requested = controls.single_step.swap(false, Ordering::Relaxed);

		if paused && !step_requested {
			// Mirrors `SimThread`'s paused branch: sleep-equivalent, no debt accrues. The audio mix
			// still decays (`UpdateInPausedState`) so a sounding buzzer fades away rather than hanging.
			with_local_audio(audio, &mut local_audio, |local_audio| lock_sim(sim).update_in_paused_state(local_audio));
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
			// A requested single step runs exactly one tick regardless of pacing
			// (`Project.advanceSingleSimStep`) and mustn't disturb the paused timing hold below.
			run_steps_batch(sim, audio, &mut local_audio, controls, 1);
			pacing.last_tick = Some(now);
			pacing.debt_ticks = 0.0;
			pacing.window.record(now, 1);
			store_avg(controls, &pacing.window, now);
			std::thread::sleep(PAUSED_SLEEP);
			continue;
		}

		if pacing.was_paused {
			// Just unpaused: start measuring fresh so stale pre-pause samples don't paint a burst onto the readout.
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
			// Ahead of schedule: nap until the next tick falls due, so a low target rate costs a
			// handful of wakeups per second rather than a wakeup storm hammering the shared locks.
			let until_due = Duration::from_secs_f64(pacing.debt_ticks / target_ticks_per_second);
			std::thread::sleep(until_due.clamp(MIN_IDLE_SLEEP, MAX_IDLE_SLEEP));
			continue;
		}

		let ran = run_steps_batch(sim, audio, &mut local_audio, controls, due);
		if ran < due {
			pacing.debt_ticks = restore_unfinished_ticks(pacing.debt_ticks, due - ran, target_ticks_per_second);
		}
		let post = Instant::now();
		pacing.window.record(post, ran);
		store_avg(controls, &pacing.window, post);
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

	/// Regression for the rework: sustained throughput has to land near the target rate. The old
	/// loop woke on a fixed 200µs cadence and paid two lock acquisitions (arena + realtime audio
	/// state) *per tick*, which capped measured speed well below target even for a blank chip;
	/// batching under one scope per pass must track the target closely.
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

	/// Regression: at an unreachable target rate the worker used to re-take the arena before a
	/// waiting renderer woke (std's mutex is unfair), freezing the UI. A lock request must get
	/// through within a few passes however high the target is.
	#[test]
	fn main_thread_lock_stays_prompt_while_the_worker_runs_flat_out() {
		let h = handle(false, u32::MAX);
		assert!(wait_until(Duration::from_secs(5), || frame(&h) > 0), "worker never stepped");

		let mut worst = Duration::ZERO;
		for _ in 0..200 {
			let requested = Instant::now();
			drop(h.lock());
			worst = worst.max(requested.elapsed());
			std::thread::sleep(Duration::from_micros(500));
		}
		assert!(worst < Duration::from_millis(50), "a lock request waited {worst:?} behind the flat-out worker");
	}

	/// Regression: an external `silence` on the shared mix must survive the worker's local copy.
	#[test]
	fn silencing_the_shared_mix_is_not_overwritten_by_the_worker() {
		let audio = crate::audio::default_shared_state();
		audio.lock().expect("audio lock").sim_audio.register_note(10, 15);
		audio.lock().expect("audio lock").sim_audio.silence();

		let mut local = SimAudio::new();
		with_local_audio(&audio, &mut local, |_| {});

		let state = audio.lock().expect("audio lock");
		assert!(state.sim_audio.step_targets().iter().all(|&t| t == 0.0));
		assert!(state.sim_audio.amplitudes().iter().all(|&a| a == 0.0));
	}

	#[test]
	fn replace_swaps_in_the_new_arena_for_subsequent_reads() {
		let h = handle(true, 1);
		*h.lock() = blank_simulator();
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

	/// Regression: a rebuild must never leave the *live* simulator (what
	/// the worker thread is still stepping) with an emptied LUT cache --
	/// not even for the instant it takes to install the replacement. A
	/// version that took the cache out with an earlier, separate lock
	/// (before `Simulator::build`ing the replacement) would leave exactly
	/// that window open.
	#[test]
	fn replacing_moves_the_caching_state_without_ever_leaving_the_live_sim_empty() {
		let h = handle(true, 1);
		h.lock().caching.combinational_chip_cache.insert("ADDER".into(), Box::new(crate::gate_op::Lut::new(vec![vec![0]])));
		h.lock().caching.not_combinational_chip_cache.insert("NOT_COMB".into());

		h.capture_caching_state(blank_simulator());

		// The cache travelled into the newly installed (now live) simulator.
		let live = h.lock();
		assert!(
			live.caching.combinational_chip_cache.contains_key("ADDER"),
			"replace_carrying_caching_state must carry the cache into the new simulator"
		);
		assert!(
			live.caching.not_combinational_chip_cache.contains("NOT_COMB"),
			"replace_carrying_caching_state must carry the cache into the new simulator"
		);
	}

	#[test]
	fn transient_input_state_feeds_through_the_handle() {
		use crate::sim::KeyCode;

		let h = handle(true, 1);
		h.set_key_modifiers(7);
		h.held_key_press('A');
		assert_eq!(h.key_modifiers(), 7);
		assert!(h.lock().keyboard.is_pressed(KeyCode::Char('A')));
		h.held_key_release('A');
		assert!(!h.lock().keyboard.is_pressed(KeyCode::Char('A')));
		h.held_key_press('B');
		h.lock().set_driven_input(3, PinState::HIGH);
		let (keyboard, driven) = h.take_transient_input_state();
		assert!(keyboard.is_pressed(KeyCode::Char('B')) && keyboard.modifiers_bitmask() == 7);
		assert_eq!(driven.get(&3), Some(&PinState::HIGH), "toggled inputs travel with the rest");
		assert!(!h.lock().keyboard.is_pressed(KeyCode::Char('B')), "take clears the source");
		assert!(h.lock().driven_inputs.is_empty(), "take clears the source");
	}
}
