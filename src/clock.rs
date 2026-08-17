//! Wall-clock time, in the two shapes Vairë needs.
//!
//! Deliberately not a date library: the corpus wants `YYYY-MM-DD` on a release record, and
//! machine state wants an epoch integer it can compare without parsing. Everything else —
//! zones, formats, durations — is somebody else's problem until something actually needs it.

/// Seconds since the Unix epoch, or `0` if the clock is before it (which no real machine
/// reports, and which no caller here would do anything useful about).
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// An epoch timestamp as a UTC `YYYY-MM-DD` date.
pub fn date_utc(epoch_secs: i64) -> String {
    let (y, m, d) = civil_from_days(epoch_secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today in UTC as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    date_utc(now())
}

/// An epoch timestamp as an RFC 3339 UTC instant (`2026-08-01T09:30:00Z`) — the form the
/// registry wire contract uses for `published_at` (registry.md §9.1).
///
/// Seconds precision, `Z` only. A registry timestamp is provenance a human reads, never
/// something ordering depends on (versions order themselves), so sub-second resolution and
/// offset zones would be detail nobody consumes.
pub fn timestamp_utc(epoch_secs: i64) -> String {
    let (y, m, d) = civil_from_days(epoch_secs.div_euclid(86_400));
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (h, min, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

/// Days since the Unix epoch → civil `(year, month, day)`. Howard Hinnant's `chrono`
/// algorithm, inlined rather than taking a date dependency for one format call.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = era * 400 + yoe + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(date_utc(0), "1970-01-01");
        assert_eq!(date_utc(19_723 * 86_400), "2024-01-01"); // a leap year's start
        assert_eq!(date_utc(19_783 * 86_400), "2024-03-01"); // just past leap day
        assert_eq!(date_utc(-1), "1969-12-31");
    }

    #[test]
    fn rfc3339_carries_the_time_of_day() {
        assert_eq!(timestamp_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp_utc(86_399), "1970-01-01T23:59:59Z");
        // The spec's own example instant (registry.md §9.1).
        assert_eq!(timestamp_utc(1_785_576_600), "2026-08-01T09:30:00Z");
    }
}
