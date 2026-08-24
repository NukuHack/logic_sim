//! Simulation pacing + throughput measurement for the viewer's main-thread
//! stepping, adapting `DLS.Simulation.SimThread`. The original runs a
//! dedicated thread that spins until each tick's target duration elapses and
//! keeps a rolling window of per-tick timestamps to measure the achieved
//! ticks/second; here the simulation steps inside the render loop instead, so
//! the same two jobs are expressed as pure bookkeeping the frame loop feeds:
//! a tick-debt accumulator ("how many ticks should have run by now") and a
//! windowed counter pair (mirroring `SimThread.SimulationPerformanceTimeWindowSec`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many seconds of recent history the average-ticks-per-second
/// measurement covers. Mirrors `SimThread.SimulationPerformanceTimeWindowSec`.
pub const PERF_WINDOW_SECS: f64 = 1.5;

/// Upper bound on catch-up work in a single frame: without it, returning
/// from a suspend/unfocused gap (or raising the target rate sharply) would
/// try to replay every tick "owed" since the last frame at once.
pub const MAX_CATCHUP_SECS: f64 = 0.25;

/// Hard cap on steps executed per frame, so an absurdly high target rate
/// degrades to "runs flat out, measured speed reports the truth" (what the
/// original's thread does) rather than freezing the render loop.
pub const MAX_STEPS_PER_FRAME: u64 = 100_000;

/// Folds `elapsed` seconds at `ticks_per_second` into the outstanding tick
/// debt, clamped so at most [`MAX_CATCHUP_SECS`] worth of ticks can ever be
/// owed. Returns the new debt (fractional -- whole ticks are stepped and
/// subtracted by the caller).
pub fn accumulate_tick_debt(debt_ticks: f64, elapsed: f64, ticks_per_second: f64) -> f64 {
	let max_debt = MAX_CATCHUP_SECS * ticks_per_second.max(1.0);
	(debt_ticks + elapsed * ticks_per_second).clamp(0.0, max_debt)
}

/// How many whole ticks are due, and what remains of the debt after taking
/// them -- capped at [`MAX_STEPS_PER_FRAME`] so excess debt is *dropped*
/// (matching the original falling behind its target rather than bursting).
pub fn take_due_ticks(debt_ticks: f64) -> (u64, f64) {
	let due = debt_ticks.floor();
	if due < 1.0 {
		return (0, debt_ticks.max(0.0));
	}
	let steps = (due as u64).min(MAX_STEPS_PER_FRAME);
	(steps, debt_ticks - steps as f64)
}

/// Rolling "average ticks per second over the last
/// [`PERF_WINDOW_SECS`]" measurement. The original enqueues one timestamp
/// per tick; this records one `(time, count)` entry per frame batch, which
/// yields exactly the same ratio while keeping the queue tiny even at very
/// high tick rates.
#[derive(Default)]
pub struct PerfWindow {
	entries: VecDeque<(Instant, u64)>,
	total: u64,
}

impl PerfWindow {
	/// Records `count` ticks completed at time `now`, dropping entries that
	/// have slid out of the measurement window.
	pub fn record(&mut self, now: Instant, count: u64) {
		self.entries.push_back((now, count));
		self.total += count;
		self.prune(now);
	}

	fn prune(&mut self, now: Instant) {
		while let Some(&(oldest, count)) = self.entries.front() {
			if now.duration_since(oldest) <= Duration::from_secs_f64(PERF_WINDOW_SECS) {
				break;
			}
			self.entries.pop_front();
			self.total -= count;
		}
	}

	/// Average ticks/second across the window ending at `now`, or `None`
	/// when nothing has been recorded recently enough to measure (the
	/// caller decides what to display then -- the original just leaves its
	/// last value alone).
	pub fn avg_per_sec(&self, now: Instant) -> Option<f64> {
		let &(oldest, _) = self.entries.front()?;
		let active = now.duration_since(oldest).as_secs_f64();
		if active <= 0.0 {
			return None;
		}
		Some(self.total as f64 / active)
	}

	/// Drops all history -- used when leaving the paused state, so stale
	/// pre-pause samples can't paint a burst onto the fresh measurement.
	pub fn clear(&mut self) {
		self.entries.clear();
		self.total = 0;
	}
}
