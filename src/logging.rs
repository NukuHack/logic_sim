//! Application-wide logging: installs the `flexi_logger` logger the app runs
//! on, and owns the log filter.
//!
//! Two things are configured here:
//!
//! - **Where** logs go: to the terminal *and* to a rotating log file in
//!   `<save data>/Logs/`, so a crash or a bug report from a machine that was
//!   never launched from a terminal still has a trail.
//! - **What** gets through: our own code at debug, every third-party crate
//!   (wgpu/wgpu_hal/naga, winit, zbus, cpal, ...) held down at warn. Their
//!   info/debug output fires every frame and drowns out everything the app
//!   itself says; [`DEFAULT_LOG_SPEC`] is the single place that decision
//!   lives, and [`LOG_SPEC_ENV`] overrides it wholesale when hunting a
//!   specific subsystem.

use std::io::Write;
use std::path::{Path, PathBuf};

use flexi_logger::{AdaptiveFormat, Age, Cleanup, Criterion, DeferredNow, Duplicate, FileSpec, LogSpecification, Logger, LoggerHandle, Naming};

/// Environment variable that replaces [`DEFAULT_LOG_SPEC`] entirely when set.
pub const LOG_SPEC_ENV: &str = "RUST_LOG";

/// Filter used unless [`LOG_SPEC_ENV`] says otherwise: our own code (the
/// `logic_sim` library and the `app` binary) at debug, *everything else* at
/// warn.
pub const DEFAULT_LOG_SPEC: &str = "warn,logic_sim=debug,app=debug";

/// Sub-directory of the save-data root the log files live in.
const LOG_DIR_NAME: &str = "Logs";
/// Basename of the log files; rotation appends `_rCURRENT` / `_r00000`.
const LOG_FILE_BASENAME: &str = "logic_sim";
/// Rotate once the active file passes this size, or once the local date rolls over.
const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// How many *rotated* files to keep, on top of the one being written to.
const KEPT_LOG_FILES: usize = 4;
/// Records of this level and above are mirrored to the terminal; everything
/// the filter lets through always goes to the file.
const CONSOLE_DUPLICATE: Duplicate = Duplicate::Debug;

/// Directory the rotating log files are written to.
pub fn log_dir(data_dir: &Path) -> PathBuf {
	data_dir.join(LOG_DIR_NAME)
}

/// Installs the global logger: log file plus terminal, falling back to
/// terminal-only if the log file can't be opened (the save directory may be
/// missing or read-only), and to no logging at all if even that fails.
///
/// The returned handle *must be kept alive for the rest of the process* --
/// dropping it flushes and shuts the logger down, which is what makes the
/// last lines of a run show up on disk.
pub fn init(data_dir: &Path) -> Option<LoggerHandle> {
	let (spec, spec_text) = resolve_spec();
	let dir = log_dir(data_dir);

	match start_file_logger(&spec, &dir) {
		Ok(handle) => {
			log::debug!("log filter: {spec_text} (override with {LOG_SPEC_ENV})");
			log::debug!("log files: {}", dir.display());
			Some(handle)
		}
		Err(file_error) => {
			// Nothing is installed yet at this point, so the terminal-only
			// logger below can still claim the global slot.
			eprintln!("log: could not write to {}: {file_error}", dir.display());
			match start_terminal_logger(spec) {
				Ok(handle) => {
					log::warn!("logging to the terminal only -- no log file in {}", dir.display());
					Some(handle)
				}
				Err(e) => {
					eprintln!("log: giving up on logging: {e}");
					None
				}
			}
		}
	}
}

/// Logger with the rotation policy applied: a fixed-name "current" file that
/// is rotated by size or by day, keeping a handful of older runs around.
fn start_file_logger(spec: &LogSpecification, dir: &Path) -> Result<LoggerHandle, flexi_logger::FlexiLoggerError> {
	Logger::with(spec.clone())
		.log_to_file(FileSpec::default().directory(dir).basename(LOG_FILE_BASENAME).suppress_timestamp())
		.rotate(Criterion::AgeOrSize(Age::Day, MAX_LOG_FILE_BYTES), Naming::Numbers, Cleanup::KeepLogFiles(KEPT_LOG_FILES))
		.duplicate_to_stderr(CONSOLE_DUPLICATE)
		.format_for_files(file_format)
		.adaptive_format_for_stderr(AdaptiveFormat::Detailed)
		.start()
}

/// Degraded logger for when the log directory is unusable -- same filter and
/// same (coloured when it's a tty) terminal format, just no file.
fn start_terminal_logger(spec: LogSpecification) -> Result<LoggerHandle, flexi_logger::FlexiLoggerError> {
	Logger::with(spec).log_to_stderr().adaptive_format_for_stderr(AdaptiveFormat::Detailed).start()
}

/// The filter to use: [`LOG_SPEC_ENV`] when it is set and parses, otherwise
/// [`DEFAULT_LOG_SPEC`]. Returns it alongside its normalized text so the
/// first log line can spell out what is actually in effect.
fn resolve_spec() -> (LogSpecification, String) {
	let spec = match LogSpecification::env_or_parse(DEFAULT_LOG_SPEC) {
		Ok(spec) => spec,
		Err(e) => {
			// A typo in RUST_LOG shouldn't cost the user their entire log.
			eprintln!("log: ignoring unusable {LOG_SPEC_ENV}: {e}");
			LogSpecification::parse(DEFAULT_LOG_SPEC).expect("DEFAULT_LOG_SPEC is a valid filter")
		}
	};
	let text = spec.to_string();
	(spec, text)
}

/// Line format for the log file: timestamp, level, thread, target and line,
/// with no ANSI escapes (the terminal copy gets flexi_logger's coloured
/// built-in format instead). The thread is worth naming because the sim,
/// audio and render threads all log into the same file.
fn file_format(write: &mut dyn Write, now: &mut DeferredNow, record: &log::Record<'_>) -> std::io::Result<()> {
	let thread = std::thread::current();
	let thread_name = thread.name().unwrap_or("unnamed");
	write!(
		write,
		"{} {:>5} [{thread_name}] {}:{}: {}",
		now.now().format("%Y-%m-%d %H:%M:%S%.3f"),
		record.level(),
		record.target(),
		record.line().unwrap_or(0),
		record.args()
	)
}
