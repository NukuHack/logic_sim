//! Simulation-pacing tests: the tick-debt accumulator and rolling
//! throughput window that pace the viewer's main-thread stepping (the
//! `SimThread` adaptation in `viewer::sim_timing`), exercised through
//! their public API.

use logic_sim::viewer::sim_timing::{accumulate_tick_debt, take_due_ticks, PerfWindow, MAX_CATCHUP_SECS, MAX_STEPS_PER_FRAME, PERF_WINDOW_SECS};
use std::time::{Duration, Instant};

#[test]
fn debt_accumulates_and_never_exceeds_the_catchup_cap() {
	assert_eq!(accumulate_tick_debt(0.0, 0.016, 1000.0), 16.0, "16ms at 1000tps owes 16 ticks");
	assert_eq!(accumulate_tick_debt(3.5, 0.002, 1000.0), 5.5, "fractional debt carries over");

	let capped = accumulate_tick_debt(0.0, 10.0, 1000.0);
	assert_eq!(capped, MAX_CATCHUP_SECS * 1000.0, "a long stall only ever owes the cap");
}

#[test]
fn take_due_ticks_hands_out_whole_ticks_and_drops_excess_past_the_frame_cap() {
	let (steps, rest) = take_due_ticks(16.75);
	assert_eq!((steps, rest), (16, 0.75));

	assert_eq!(take_due_ticks(0.75), (0, 0.75), "nothing whole yet");

	let owed = MAX_STEPS_PER_FRAME as f64 + 500.0;
	let (steps, rest) = take_due_ticks(owed);
	assert_eq!(steps, MAX_STEPS_PER_FRAME);
	assert_eq!(rest, 500.0, "debt beyond the cap is discarded, not banked");
}

#[test]
fn perf_window_averages_over_recorded_history_only() {
	let t0 = Instant::now();
	let mut w = PerfWindow::default();

	assert_eq!(w.avg_per_sec(t0), None);

	w.record(t0, 100);
	let avg = w.avg_per_sec(t0 + Duration::from_millis(50)).expect("measures right after recording");
	assert!((avg - 100.0 / 0.05).abs() < 1e-9);

	// Entries older than the window stop counting...
	let much_later = t0 + Duration::from_secs_f64(PERF_WINDOW_SECS + 1.0);
	w.record(much_later, 10);
	assert!(w.avg_per_sec(much_later).is_none(), "the lone fresh sample has zero elapsed span");

	// ...and clear() forgets everything (the un-pause path).
	w.clear();
	assert_eq!(w.avg_per_sec(much_later), None);
}
