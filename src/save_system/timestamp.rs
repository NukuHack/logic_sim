//! Minimal ISO-8601 timestamp formatting, used to stamp `CreationTime` /
//! `LastSaveTime` on `ProjectDescription`. Both fields are plain strings as
//! far as this port is concerned (nothing here parses them back into a
//! structured date), so a small hand-rolled UTC formatter is enough and
//! avoids pulling in `chrono` or `time` as a dependency.
//!
//! Note on compatibility: the original C# game writes local time with a
//! UTC offset (e.g. `2025-05-10T19:41:18.761+02:00`). This port writes UTC
//! instead (`...Z`) since Rust's standard library has no portable, safe way
//! to read the local UTC offset. Existing offset-suffixed timestamps from
//! C#-saved files are read back just fine (they're opaque strings here) --
//! only newly-written timestamps differ in which offset they use.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current UTC time formatted as `yyyy-MM-ddTHH:mm:ss.fffZ`.
pub fn now_iso8601() -> String {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
	format_iso8601(now.as_secs() as i64, now.subsec_millis())
}

fn format_iso8601(unix_secs: i64, millis: u32) -> String {
	let (y, mo, d) = civil_from_days(unix_secs.div_euclid(86_400));
	let secs_of_day = unix_secs.rem_euclid(86_400);
	let h = secs_of_day / 3600;
	let mi = (secs_of_day % 3600) / 60;
	let s = secs_of_day % 60;
	format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days` algorithm: converts a day count
/// (days since 1970-01-01) into a proleptic-Gregorian (year, month, day).
/// Public-domain algorithm, see http://howardhinnant.github.io/date_algorithms.html
fn civil_from_days(z: i64) -> (i64, u32, u32) {
	let z = z + 719_468;
	let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
	let doe = (z - era * 146_097) as u64; // [0, 146096]
	let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
	let y = yoe as i64 + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
	let mp = (5 * doy + 2) / 153; // [0, 11]
	let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
	let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
	let y = if m <= 2 { y + 1 } else { y };
	(y, m, d)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn formats_known_unix_timestamp() {
		// 2024-01-01T00:00:00.000Z
		assert_eq!(format_iso8601(1_704_067_200, 0), "2024-01-01T00:00:00.000Z");
	}

	#[test]
	fn formats_with_milliseconds_and_time_of_day() {
		// 2025-05-10T19:41:18.761Z
		assert_eq!(format_iso8601(1_746_906_078, 761), "2025-05-10T19:41:18.761Z");
	}

	#[test]
	fn unix_epoch_formats_correctly() {
		assert_eq!(format_iso8601(0, 0), "1970-01-01T00:00:00.000Z");
	}

	#[test]
	fn now_iso8601_produces_well_formed_string() {
		let s = now_iso8601();
		// yyyy-MM-ddTHH:mm:ss.fffZ -- exactly 24 characters.
		assert_eq!(s.len(), 24);
		assert_eq!(s.as_bytes()[4], b'-');
		assert_eq!(s.as_bytes()[7], b'-');
		assert_eq!(s.as_bytes()[10], b'T');
		assert_eq!(s.as_bytes()[13], b':');
		assert_eq!(s.as_bytes()[16], b':');
		assert_eq!(s.as_bytes()[19], b'.');
		assert!(s.ends_with('Z'));
	}
}
