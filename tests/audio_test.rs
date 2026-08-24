//! Buzzer-audio integration tests: the `SimAudio` note registry's
//! frequency table, volume handling and smoothing, plus `AudioState`'s
//! waveform mixing and the output clip stage -- all pure logic, no device.

use logic_sim::audio::{process_output_sample, AudioState, SimAudio, FREQ_COUNT};

#[test]
fn frequency_table_climbs_semitones_from_a0() {
	let sim = SimAudio::new();
	let freqs = sim.freqs_all();

	assert_eq!(freqs[0], 27.5, "slot 0 is A0 itself");
	// Slots advance three per semitone: slot 3 is one semitone above A0.
	let expected_semitone_up = 27.5 * 1.059_463_094_359_f64;
	assert!((freqs[3] as f64 - expected_semitone_up).abs() < 1e-3);
	assert_eq!(freqs.len(), FREQ_COUNT);
}

#[test]
fn perceptual_gain_correction_boosts_low_frequencies_only() {
	// The gain curve lerps 2 -> 0.35 across the table, so the same
	// full-scale volume registers much stronger on a low slot than a high one.
	let mut low = SimAudio::new();
	let mut high = SimAudio::new();
	low.register_note(0, 15);
	high.register_note(255, 15);

	let low_amp = low.step_targets()[0];
	let high_amp = high.step_targets()[255];
	assert!(low_amp > high_amp * 2.0, "low {low_amp} should dominate high {high_amp}");
}

#[test]
fn register_note_scales_with_volume_and_fades_after_the_step_ends() {
	let mut sim = SimAudio::new();
	sim.register_note(100, 15);
	sim.register_note(100, 15);
	sim.register_note(100, 15); // three buzzers on one slot add up
	let stacked = sim.step_targets()[100];

	sim.register_note(100, 0); // silent: a no-op
	assert!((sim.step_targets()[100] - stacked).abs() < 1e-12);

	// Next step: init_frame clears the targets, so the mix fades to silence.
	sim.init_frame();
	sim.notify_all_notes_registered(10.0);
	assert!(sim.amplitudes()[100] < stacked * 1e-6, "cleared targets fade away");
}

#[test]
fn volume_clamps_at_full_scale_but_still_stacks() {
	let mut quiet = SimAudio::new();
	let mut once = SimAudio::new();
	let mut twice = SimAudio::new();
	quiet.register_note(50, 7);
	once.register_note(50, 15);
	twice.register_note(50, 15);
	twice.register_note(50, 200); // beyond full scale: same reading as 15

	assert!(quiet.step_targets()[50] < once.step_targets()[50]);
	assert!((twice.step_targets()[50] - 2.0 * once.step_targets()[50]).abs() < 1e-9);
}

#[test]
fn smoothing_moves_toward_targets_proportional_to_delta_time() {
	// One simulation step's audio beat: init -> register -> notify.
	let mut sim = SimAudio::new();
	sim.init_frame();
	sim.register_note(10, 15);
	sim.notify_all_notes_registered(10.0); // huge delta saturates the step at 1: instant reach
	let target = sim.amplitudes()[10];
	assert!(target > 0.0);

	// From silence, the same note under a tiny delta only advances by the
	// clamped fraction (dt * smooth-speed) of the distance to its target.
	let mut fresh = SimAudio::new();
	fresh.init_frame();
	fresh.register_note(10, 15);
	fresh.notify_all_notes_registered(1.0 / 3000.0); // step = 30/3000 = 1%
	let moved = fresh.amplitudes()[10];
	assert!(moved > 0.0 && moved < target * 0.02, "small delta takes a small step: {moved} vs {target}");
}

#[test]
fn idle_state_never_registers_anything() {
	let mut sim = SimAudio::new();
	sim.notify_all_notes_registered(10.0); // nothing ever registered
	assert!(sim.amplitudes().iter().all(|&a| a == 0.0));
}

#[test]
fn sample_mixes_square_waves_of_registered_slots() {
	let mut audio = AudioState::default();
	assert_eq!(audio.sample(0.123), 0.0, "silent mix samples to silence");

	audio.sim_audio.register_note(45, 15);
	audio.sim_audio.init_frame();
	audio.sim_audio.notify_all_notes_registered(10.0);

	// Recompute the expected value straight from the documented formula:
	// 20 odd harmonics of the square wave scaled by the smoothed amplitude.
	let amp = audio.sim_audio.amplitudes()[45] as f32;
	let freq = audio.sim_audio.freqs_all()[45] as f64;
	let t = 0.25_f64;
	let mut expected = 0.0_f32;
	for i in 1..=20u32 {
		let harmonic = 2 * i - 1;
		expected += (((harmonic as f64) * 2.0 * std::f64::consts::PI * freq * t).sin() / harmonic as f64) as f32;
	}
	expected *= 4.0 / std::f32::consts::PI;
	expected *= amp;

	let got = audio.sample(t);
	assert!((got - expected).abs() < 1e-5, "got {got}, want {expected}");
}

#[test]
fn output_stage_flattens_peaks_but_passes_quiet_samples_through() {
	assert_eq!(process_output_sample(0.05), 0.05);
	assert_eq!(process_output_sample(-0.07), -0.07);
	assert_eq!(process_output_sample(0.5), 0.1);
	assert_eq!(process_output_sample(-0.5), -0.1);
}
