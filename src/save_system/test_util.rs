//! Test-only helper for creating unique, collision-free scratch directories
//! under the OS temp dir, without pulling in an extra crate dependency
//! (e.g. `tempfile`). Directories are *not* cleaned up automatically on
//! panic -- individual tests remove their own directory when done.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a fresh, non-existent directory path under the OS temp dir,
/// namespaced by `label` and a process/thread/counter-derived suffix so
/// concurrent test runs never collide. The directory itself is *not*
/// created -- callers create it (or rely on the code under test to create
/// it, e.g. via `SavePaths::ensure_directory_exists`).
pub fn temp_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("dls_rust_test_{label}_{pid}_{nanos}_{n}"))
}
