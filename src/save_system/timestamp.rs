//! Minimal ISO-8601 timestamp formatting, used to stamp `CreationTime` / `LastSaveTime` on
//! `ProjectDescription`. Both fields are plain strings as far as this port is concerned, so a small
//! hand-rolled UTC formatter is enough and avoids pulling in `chrono` or `time` as a dependency.
//! Note on compatibility: the original writes local time with a UTC offset; this port writes UTC
//! instead, since Rust's standard library has no portable, safe way to read the local offset.

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

/// Formats an ISO-8601-ish timestamp into a human-readable relative string
/// like "5 minutes ago", "3 hours ago", "2 days ago".
/// Returns the raw string if parsing fails.
pub fn to_relative_time(timestamp: &str) -> String {
	let saved_secs = parse_to_unix_seconds(timestamp);
	if saved_secs == i64::MIN {
		return timestamp.to_string();
	}
	let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
	let now_secs = now.as_secs() as i64;
	let diff = now_secs - saved_secs;
	if diff < 0 {
		return "just now".to_string();
	}
	let mins = diff / 60;
	let hours = diff / 3600;
	let days = diff / 86_400;
	let months = days / 30;
	let years = days / 365;
	if years > 0 {
		format!("{} year{}", years, if years == 1 { "" } else { "s" })
	} else if months > 0 {
		format!("{} month{}", months, if months == 1 { "" } else { "s" })
	} else if days > 0 {
		format!("{} day{}", days, if days == 1 { "" } else { "s" })
	} else if hours > 0 {
		format!("{} hour{}", hours, if hours == 1 { "" } else { "s" })
	} else if mins > 0 {
		format!("{} minute{}", mins, if mins == 1 { "" } else { "s" })
	} else {
		"just now".to_string()
	}
}

/// Parses an ISO-8601-ish timestamp (`yyyy-MM-ddTHH:mm:ss.fff` with an
/// optional `Z`, `+HH:mm`/`-HH:mm`/`±HHMM` offset suffix) into comparable
/// unix seconds. Files written by the Unity build carry *local* times with
/// offsets while this port writes UTC, so raw string comparison mis-orders
/// a mixed directory; the project list sorts by this key instead.
/// Unparseable text sorts as `i64::MIN` (oldest).
pub fn parse_to_unix_seconds(text: &str) -> i64 {
	let bytes = text.as_bytes();
	if bytes.len() < 19 {
		return i64::MIN;
	}
	let num = |range: std::ops::Range<usize>| text.get(range).and_then(|s| s.parse::<i64>().ok());
	let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (num(0..4), num(5..7), num(8..10), num(11..13), num(14..16), num(17..19)) else {
		return i64::MIN;
	};
	if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
		return i64::MIN;
	}

	// Days since epoch via the same civil algorithm run in reverse.
	let y_adj = if mo <= 2 { y - 1 } else { y };
	let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
	let yoe = y_adj - era * 400;
	let mp = (mo + 9) % 12;
	let doy = (153 * mp + 2) / 5 + d - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	let days = era * 146_097 + doe - 719_468;

	let secs = days * 86_400 + h * 3600 + mi * 60 + s;

	// Apply any explicit offset so all stamps compare on the same clock.
	// No/!-quite-parseable offset is treated as UTC -- only garbage stamps
	// (rejected above) sort as oldest.
	let mut offset_secs = 0i64;
	let tail = &text[19.min(text.len())..];
	if let Some(sign_at) = tail.find(['Z', '+', '-']) {
		let (sign, digits): (i64, String) = {
			let rest = &tail[sign_at..];
			match rest.chars().next() {
				Some('Z') => (1, String::new()),
				Some('+') => (1, rest.chars().skip(1).filter(|c| c.is_ascii_digit()).collect()),
				Some('-') => (-1, rest.chars().skip(1).filter(|c| c.is_ascii_digit()).collect()),
				_ => (1, String::new()),
			}
		};
		if !digits.is_empty() && digits.len() >= 4 {
			let oh: i64 = digits[..2].parse().unwrap_or(0);
			let om: i64 = digits[2..4].parse().unwrap_or(0);
			offset_secs = sign * (oh * 3600 + om * 60);
		}
	}
	secs - offset_secs
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

	#[test]
	fn parse_respects_explicit_offsets_so_mixed_stamps_order_correctly() {
		// Same instant written as UTC and as +02:00 local must compare equal.
		assert_eq!(parse_to_unix_seconds("2024-01-01T12:00:00.000Z"), parse_to_unix_seconds("2024-01-01T14:00:00.000+02:00"));
		assert_eq!(parse_to_unix_seconds("2024-01-01T12:00:00.000Z"), parse_to_unix_seconds("2024-01-01T09:00:00.000-0300"));
		// Plain (offset-less) stamps are treated as UTC.
		assert_eq!(parse_to_unix_seconds("2024-01-01T12:00:00.000"), 1_704_110_400);
	}

	#[test]
	fn parse_rejects_garbage_as_oldest() {
		assert_eq!(parse_to_unix_seconds("nonsense"), i64::MIN);
		assert_eq!(parse_to_unix_seconds(""), i64::MIN);
		assert_eq!(parse_to_unix_seconds("9999-99-99T99:99:99.999Z"), i64::MIN, "out-of-range fields are rejected");
	}
}
