//! Logging setup: the default filter (our code verbose, third-party crates
//! quiet) and the log file `logging::init` drops next to the save data. The
//! filter is the half that rots silently -- loosen it globally and wgpu's
//! per-frame chatter buries everything the app itself says.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use flexi_logger::{Level, LogSpecification};
use logic_sim::logging::{init, log_dir, DEFAULT_LOG_SPEC};

/// Fresh scratch directory under the OS temp dir, unique per run.
fn scratch(label: &str) -> PathBuf {
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
	std::env::temp_dir().join(format!("dls_rust_test_{label}_{}_{nanos}", std::process::id()))
}

#[test]
fn our_code_logs_at_debug_while_third_parties_stay_at_warn() {
	let spec = LogSpecification::parse(DEFAULT_LOG_SPEC).expect("the default filter parses");

	// Our own crate -- and the `app` binary's own target -- is verbose, but
	// not so verbose that trace-level noise gets through.
	assert!(spec.enabled(Level::Debug, "logic_sim::viewer::app"));
	assert!(spec.enabled(Level::Debug, "logic_sim::gate_op::caching"));
	assert!(spec.enabled(Level::Debug, "app"), "the binary's target counts as ours");
	assert!(!spec.enabled(Level::Trace, "logic_sim::sim"));

	// Third-party crates only get through when they have something to
	// complain about: wgpu & co. log on every frame otherwise.
	assert!(!spec.enabled(Level::Info, "wgpu_core::device"));
	assert!(!spec.enabled(Level::Debug, "wgpu_hal::vulkan"));
	assert!(!spec.enabled(Level::Info, "zbus"));
	assert!(spec.enabled(Level::Warn, "wgpu_core::device"));
	assert!(spec.enabled(Level::Error, "naga::valid"));
}

#[test]
fn init_writes_a_log_file_next_to_the_save_data() {
	let data_dir = scratch("logging");
	let logger = init(&data_dir).expect("the logger installs into a writable directory");
	log::warn!("logging smoke test marker");
	// Flush, then drop: closing the handle is what makes the file readable
	// as a finished log rather than an open one.
	logger.flush();
	drop(logger);

	let logs = log_dir(&data_dir);
	let files: Vec<PathBuf> = std::fs::read_dir(&logs)
		.unwrap_or_else(|e| panic!("log directory {} was created: {e}", logs.display()))
		.filter_map(Result::ok)
		.map(|entry| entry.path())
		.filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("log"))
		.collect();
	assert!(!files.is_empty(), "expected a .log file in {}", logs.display());

	let contents = std::fs::read_to_string(&files[0]).expect("the log file is readable");
	assert!(contents.contains("logging smoke test marker"), "log file did not capture the record: {contents}");
	assert!(contents.contains("WARN"), "log file records the level: {contents}");

	let _ = std::fs::remove_dir_all(&data_dir);
}
