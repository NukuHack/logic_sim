//! Buzzer audio synthesis, ported from DLS's `DLS.Simulation.SimAudio`,
//! `AudioState` and the sample-processing half of Unity's `AudioUnity`.
//! Split along the same seam as the original: [`SimAudio`] is the
//! simulation-side note registry (advanced once per simulation step --
//! see [`crate::sim::Simulator::run_simulation_step`]), [`AudioState`]
//! turns the current amplitude mix into a waveform sample, and
//! [`spawn_player`] runs the real output device, applying the gain +
//! soft-clip stage `AudioUnity.OnAudioFilterRead` performs.

use std::sync::{Arc, Mutex};

/// One entry per semitone-ish frequency slot; a buzzer drives one of these
/// by its 8-bit pitch pin. Mirrors `SimAudio.freqCount`.
pub const FREQ_COUNT: usize = 256;

/// Master volume multiplier applied by the output callback
/// (`AudioUnity.gain`). Kept deliberately small: many simultaneous
/// harmonics sum well past unity, and the clipper below shapes the rest.
pub const GAIN: f32 = 0.05;

/// Raw samples beyond this magnitude are flattened to it
/// (`AudioUnity.clipThreshold`) -- a hard ceiling that keeps stacked
/// notes from jumping to full scale.
pub const CLIP_THRESHOLD: f32 = 0.1;

/// Harmonic count of the band-limited square/saw waves
/// (`AudioState.waveIterations`).
const WAVE_ITERATIONS: u32 = 20;

/// Output-stream period in frames (cpal's ALSA backend doubles this for the
/// ring buffer). 2048 frames @ 48 kHz = ~42.7 ms per wake, ~85 ms of buffer.
const AUDIO_PERIOD_FRAMES: u32 = 2048;

/// A0 in Hz -- the base of the frequency table
/// (`SimAudio.CalculateFrequency`).
const A0_FREQUENCY_HZ: f64 = 27.5;

/// Semitone ratio (2^(1/12)) used to climb the frequency table.
const SEMITONE_RATIO: f64 = 1.059_463_094_359;

/// Simulation-side registry of which frequency slots are sounding and how
/// loud, smoothed toward their targets over time. Ported from
/// `DLS.Simulation.SimAudio`; lives outside the [`crate::sim::Simulator`]
/// (like the original, which keeps it on `Project.audioState`) so
/// rebuilding the simulated graph on every edit doesn't audibly reset it.
pub struct SimAudio {
	freqs_all: [f32; FREQ_COUNT],
	target_amplitudes_per_freq_temp: [f64; FREQ_COUNT],
	target_amplitudes_per_freq: [f64; FREQ_COUNT],
	perceptual_gain_correction: [f32; FREQ_COUNT],

	/// Whether any note was registered since the last `init_frame`.
	has_input_since_last_init: bool,
	/// Whether any amplitude is still fading toward its target.
	is_smoothing: bool,
}

impl SimAudio {
	pub fn new() -> Self {
		let mut freqs_all = [0.0; FREQ_COUNT];
		let mut perceptual_gain_correction = [0.0; FREQ_COUNT];

		for (i, freq) in freqs_all.iter_mut().enumerate() {
			*freq = calculate_frequency(i as f64 / 3.0);
			// Very crude correction factors to make different frequencies
			// sound more equal in volume (boosts amplitude of low frequencies).
			let freq_t = i as f32 / 255.0;
			perceptual_gain_correction[i] = lerp(2.0, 0.35, ease_quad_in_out(freq_t));
		}

		Self {
			freqs_all,
			target_amplitudes_per_freq_temp: [0.0; FREQ_COUNT],
			target_amplitudes_per_freq: [0.0; FREQ_COUNT],
			perceptual_gain_correction,
			has_input_since_last_init: false,
			is_smoothing: false,
		}
	}

	pub fn freqs_all(&self) -> &[f32; FREQ_COUNT] {
		&self.freqs_all
	}

	/// Current smoothed amplitude per frequency slot (what [`AudioState::sample`]
	/// mixes); exposed for tests/visualization.
	pub fn amplitudes(&self) -> &[f64; FREQ_COUNT] {
		&self.target_amplitudes_per_freq
	}

	/// This step's unsmoothed note targets -- what [`Self::register_note`]
	/// accumulates into and [`Self::init_frame`] clears.
	pub fn step_targets(&self) -> &[f64; FREQ_COUNT] {
		&self.target_amplitudes_per_freq_temp
	}

	/// Clears the per-step note targets -- called at the top of every
	/// simulation step (`RunSimulationStep`'s leading `InitFrame`, and its
	/// paused-state equivalent). Deliberately does nothing when no notes
	/// were registered since the last clear, so an idle sim doesn't churn.
	pub fn init_frame(&mut self) {
		if !self.has_input_since_last_init {
			return;
		}
		self.has_input_since_last_init = false;
		self.target_amplitudes_per_freq_temp.fill(0.0);
	}

	/// Registers one buzzer output this step: `index` selects the frequency
	/// slot, `volume` is the raw 4-bit volume-pin reading. Louder volumes
	/// scale toward full amplitude (clamped at 15+); stacking several
	/// buzzers on one slot adds up.
	pub fn register_note(&mut self, index: i32, volume: u32) {
		if volume == 0 || !(0..FREQ_COUNT as i32).contains(&index) {
			return;
		}
		let index = index as usize;

		self.has_input_since_last_init = true;
		let amplitude_t = (volume as f32 / 15.0).min(1.0);
		self.target_amplitudes_per_freq_temp[index] += (amplitude_t * self.perceptual_gain_correction[index]) as f64;
	}

	/// Advances every amplitude toward its target for this step, using the
	/// real-time delta since the previous step so the fade speed stays
	/// constant regardless of tick rate (`NotifyAllNotesRegistered`).
	pub fn notify_all_notes_registered(&mut self, delta_time: f64) {
		if !self.has_input_since_last_init && !self.is_smoothing {
			return;
		}

		const SMOOTH_SPEED: f64 = 30.0;
		let step = (delta_time * SMOOTH_SPEED).min(1.0);
		self.is_smoothing = false;

		for i in 0..FREQ_COUNT {
			// Crude smoothing to avoid jarring frequency jumps
			let curr = self.target_amplitudes_per_freq[i];
			let target = self.target_amplitudes_per_freq_temp[i];
			let val_new = curr + (target - curr) * step;
			self.target_amplitudes_per_freq[i] = if (val_new - target).abs() <= 0.0001 { target } else { val_new };

			self.is_smoothing |= val_new > 0.0;
		}
	}

	/// Immediately silences everything -- both the per-step targets and the
	/// smoothed mix. Used when the editor closes outright: nothing will
	/// advance the mix there anymore, so a sounding buzzer must not drone on.
	pub fn silence(&mut self) {
		self.target_amplitudes_per_freq_temp.fill(0.0);
		self.target_amplitudes_per_freq.fill(0.0);
		self.has_input_since_last_init = false;
		self.is_smoothing = false;
	}
}

impl Default for SimAudio {
	fn default() -> Self {
		Self::new()
	}
}

/// Waveform sampler over the current amplitude mix. Ported from
/// `AudioState` (square wave, 20 harmonic iterations).
#[derive(Default)]
pub struct AudioState {
	pub sim_audio: SimAudio,
}

impl AudioState {
	/// The mixed waveform value at absolute `time` (seconds since the
	/// player started). Summing only audible slots keeps idle playback cheap.
	pub fn sample(&self, time: f64) -> f32 {
		mix_sample(&self.sim_audio.freqs_all, &self.sim_audio.target_amplitudes_per_freq, time)
	}
}

/// One mixed waveform sample over an explicit `(frequencies, amplitudes)`
/// pair -- the computation behind [`AudioState::sample`], split out so the
/// output callback can snapshot those two arrays under its shortest
/// possible lock and sum harmonics outside it (see [`spawn_player`]).
fn mix_sample(freqs_all: &[f32; FREQ_COUNT], amplitudes: &[f64; FREQ_COUNT], time: f64) -> f32 {
	let mut sum = 0.0f32;
	for i in 0..FREQ_COUNT {
		let amplitude = amplitudes[i] as f32;
		if amplitude < 0.001 {
			continue;
		}
		let phase = time * 2.0 * std::f64::consts::PI * freqs_all[i] as f64;
		sum += square_wave(phase) * amplitude;
	}
	sum
}

/// Output-device handle playing whatever [`AudioState`] the app shares in.
/// Dropping this stops playback (the underlying stream is closed).
pub struct AudioPlayer {
	_stream: cpal::Stream,
}

/// The app-wide audio state shared between the simulation (writer) and the
/// output callback (reader). See [`spawn_player`].
pub type SharedAudioState = Arc<Mutex<AudioState>>;

/// A fresh, silent shared state -- what the app creates at startup.
pub fn default_shared_state() -> SharedAudioState {
	Arc::new(Mutex::new(AudioState::default()))
}

/// Starts the real output stream driving `shared`. Fails gracefully
/// (with a reason) where no audio device/config is available -- the app
/// runs fine without sound rather than refusing to start.
pub fn spawn_player(shared: SharedAudioState) -> Result<AudioPlayer, String> {
	use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

	let host = cpal::default_host();
	let Some(device) = host.default_output_device() else { return Err("no default audio output device".to_string()) };
	let config = device.default_output_config().map_err(|e| format!("no default audio config: {e}"))?;

	let sample_rate = config.sample_rate() as f64;
	let channels = config.channels() as usize;
	let mut config: cpal::StreamConfig = config.into();
	// Pin a generous period instead of accepting the device default: cpal's
	// ALSA backend pairs the default negotiation with only a two-period ring
	// (here: 2048-frame periods => ~85 ms total buffer), which rides out
	// stalls that would underrun a ~10 ms period -- routine under load even
	// when outputting silence (the errors also fire while no buzzer sounds,
	// since the stream runs from app start either way). Buzzer audio has no
	// interactive-latency requirement (the mix follows the simulation), so
	// trading ~43 ms of latency for ~85 ms of buffer is free.
	config.buffer_size = cpal::BufferSize::Fixed(AUDIO_PERIOD_FRAMES);
	// Sample count since start -- dividing by the rate reconstructs the
	// callback's exact time without float drift accumulating per sample.
	let samples_elapsed = Arc::new(std::sync::atomic::AtomicU64::new(0));
	let samples_elapsed_callback = Arc::clone(&samples_elapsed);

	let stream = device
		.build_output_stream(
			config,
			move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
				let mut time = samples_elapsed_callback.load(std::sync::atomic::Ordering::Relaxed) as f64 / sample_rate;
				// Poison recovery, not just a failed lock: an audio panic on
				// either side must degrade to silence, never to repeating
				// whatever stale bytes were last left in the buffer.
				//
				// The lock is held only for a ~3 KB array copy: the mix
				// itself (up to 20 harmonics per sounding slot, per frame)
				// runs on the snapshot outside it. Synthesizing under the
				// lock made this RT thread block the simulation worker for
				// milliseconds per period -- and queue behind its batch
				// holds -- which is exactly how a well-buffered stream
				// still underruns.
				let (freqs, amplitudes) = {
					let state = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
					(*state.sim_audio.freqs_all(), *state.sim_audio.amplitudes())
				};
				for frame in data.chunks_mut(channels) {
					let sample = process_output_sample(GAIN * mix_sample(&freqs, &amplitudes, time));
					frame.fill(sample);
					time += 1.0 / sample_rate;
				}
				let frames_played = (data.len() / channels) as u64;
				samples_elapsed_callback.fetch_add(frames_played, std::sync::atomic::Ordering::Relaxed);
			},
			|err| eprintln!("audio stream error: {err}"),
			None,
		)
		.map_err(|e| format!("failed to build audio stream: {e}"))?;

	stream.play().map_err(|e| format!("failed to start audio stream: {e}"))?;
	promote_worker_to_realtime();
	Ok(AudioPlayer { _stream: stream })
}

/// Asks rtkit to promote cpal's ALSA worker thread to SCHED_FIFO.
///
/// The worker is plain SCHED_OTHER otherwise, and long system/process stalls
/// (render-path hitches, CPU saturation) then leave it unable to feed the
/// device in time -- reported as `BufferUnderrun` errors even in total
/// silence. Realtime scheduling lets it run the moment a stall lifts; the
/// enlarged period set by [`AUDIO_PERIOD_FRAMES`] covers the rest. No-op off
/// Linux (the thread name and /proc scan are ALSA/Linux specifics).
///
/// Fire-and-forget on a helper thread (dbus setup must not delay startup),
/// degrading silently wherever rtkit isn't running or refuses the request:
/// audio then just behaves as before. Pure-zbus, so no system dbus headers
/// are needed to build.
#[cfg(target_os = "linux")]
fn promote_worker_to_realtime() {
	std::thread::spawn(|| {
		const THREAD_NAME: &str = "cpal_alsa_out";
		// The worker was spawned during build_output_stream/play(), but give
		// the scheduler a moment rather than failing on a naming race.
		let tid = (0..10).find_map(|_| {
			std::thread::sleep(std::time::Duration::from_millis(20));
			read_thread_ids().into_iter().find(|(_, comm)| comm == THREAD_NAME).map(|(tid, _)| tid)
		});
		let Some(tid) = tid else { return };

		let Ok(connection) = pollster::block_on(zbus::connection::Connection::system()) else { return };
		let service = "org.freedesktop.RealtimeKit1";
		let path = "/org/freedesktop/RealtimeKit1";
		let interface = "org.freedesktop.RealtimeKit1";
		let pid = std::process::id() as u64;
		// rtkit's allowed ceiling varies by distro config; walk down until one sticks.
		for priority in [20u32, 15, 10, 5] {
			let call: Result<(), _> =
				pollster::block_on(connection.call_method(Some(service), path, Some(interface), "MakeThreadRealtimeWithPID", &(pid, tid, priority)))
					.map(|_: zbus::Message| ());
			if call.is_ok() {
				eprintln!("audio: worker thread promoted to SCHED_FIFO {priority} via rtkit");
				return;
			}
		}
	});
}

/// Lists `(tid, comm)` for every thread of this process (`/proc/self/task`).
#[cfg(target_os = "linux")]
fn read_thread_ids() -> Vec<(u64, String)> {
	let Ok(entries) = std::fs::read_dir("/proc/self/task") else { return Vec::new() };
	entries
		.filter_map(|entry| {
			let entry = entry.ok()?;
			let tid: u64 = entry.file_name().to_str()?.parse().ok()?;
			let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
			Some((tid, comm.trim().to_string()))
		})
		.collect()
}

/// Non-Linux platforms have nothing to promote (see [`promote_worker_to_realtime`]).
#[cfg(not(target_os = "linux"))]
fn promote_worker_to_realtime() {}

/// The gain + clip stage of `AudioUnity.OnAudioFilterRead`: anything past
/// [`CLIP_THRESHOLD`] is flattened to exactly that magnitude, preserving sign.
pub fn process_output_sample(raw: f32) -> f32 {
	if raw.abs() > CLIP_THRESHOLD {
		CLIP_THRESHOLD * raw.signum()
	} else {
		raw
	}
}

/// Band-limited square wave (`AudioState.SquareWave`): odd harmonics only.
fn square_wave(t: f64) -> f32 {
	let mut sum = 0.0f64;
	for i in 1..=WAVE_ITERATIONS {
		let harmonic = 2 * i - 1;
		sum += (harmonic as f64 * t).sin() / harmonic as f64;
	}
	(sum * 4.0 / std::f64::consts::PI) as f32
}

/// Frequency `num_above_a0` semitones above A0 (`CalculateFrequency`;
/// buzzer slots pass `index / 3`, i.e. three slots per semitone).
pub fn calculate_frequency(num_above_a0: f64) -> f32 {
	(A0_FREQUENCY_HZ * SEMITONE_RATIO.powf(num_above_a0)) as f32
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a * (1.0 - t) + b * t
}

/// `Maths.EaseQuadInOut`: 3t^2 - 2t^3 clamped to 0..=1 (smoothstep).
fn ease_quad_in_out(t: f32) -> f32 {
	let t = t.clamp(0.0, 1.0);
	3.0 * t * t - 2.0 * t * t * t
}
