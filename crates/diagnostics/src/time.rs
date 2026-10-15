//! Minimal UTC time formatting (avoids pulling a date/time dependency).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Converts days since 1970-01-01 to (year, month, day).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Formats a Unix timestamp in milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn iso8601(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let ms = unix_ms % 1000;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn now_iso8601() -> String {
    iso8601(unix_ms())
}

/// `HH:MM:SS` (UTC) for compact log display.
pub fn clock(unix_ms: u64) -> String {
    let rem = ((unix_ms / 1000) % 86_400) as i64;
    format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-14T12:34:56.789Z
        assert_eq!(iso8601(1_791_981_296_789), "2026-10-14T12:34:56.789Z");
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // Leap day.
        assert_eq!(iso8601(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
