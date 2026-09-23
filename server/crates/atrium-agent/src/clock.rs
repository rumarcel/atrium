//! Agent's own clock, formatted as RFC 3339 UTC.
//!
//! Agent's journal timestamps come from Agent, never from Core. This is the
//! whole formatter — civil date from days since the epoch, Howard Hinnant's
//! algorithm — rather than a date-time dependency in the privileged
//! component.

use std::time::{SystemTime, UNIX_EPOCH};

use atrium_protocol::values::Timestamp;

/// A UTC calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    /// Year.
    pub year: i64,
    /// Month, 1 to 12.
    pub month: u32,
    /// Day of the month, 1 to 31.
    pub day: u32,
}

impl Day {
    /// `YYYY-MM-DD`.
    #[must_use]
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn civil_from_days(days: i64) -> Day {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Day {
        year,
        month: u32::try_from(month).unwrap_or(1),
        day: u32::try_from(day).unwrap_or(1),
    }
}

/// Milliseconds since the epoch; `0` for a clock set before 1970.
fn millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The UTC day of `at`.
#[must_use]
pub fn day(at: SystemTime) -> Day {
    civil_from_days(millis(at).div_euclid(86_400_000))
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[must_use]
pub fn format(at: SystemTime) -> String {
    let ms = millis(at);
    let date = civil_from_days(ms.div_euclid(86_400_000));
    let in_day = ms.rem_euclid(86_400_000);
    format!(
        "{}T{:02}:{:02}:{:02}.{:03}Z",
        date.iso(),
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    )
}

/// [`format`], as the protocol's validated type.
#[must_use]
pub fn timestamp(at: SystemTime) -> Timestamp {
    // The formatter's output always has the validated shape; a year past
    // 9999 is the only way out, and that clock is wrong, so it saturates.
    Timestamp::parse(&format(at))
        .unwrap_or_else(|_| Timestamp::parse("9999-12-31T23:59:59.999Z").expect("constant"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64, ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_millis(ms)
    }

    #[test]
    fn known_instants() {
        assert_eq!(format(at(0, 0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(format(at(951_782_400, 5)), "2000-02-29T00:00:00.005Z");
        assert_eq!(format(at(1_790_000_000, 123)), "2026-09-21T14:13:20.123Z");
        assert_eq!(format(at(4_102_444_799, 999)), "2099-12-31T23:59:59.999Z");
    }

    #[test]
    fn every_formatted_value_is_a_valid_timestamp() {
        for seconds in (0..4_000_000_000u64).step_by(7_777_777) {
            let text = format(at(seconds, seconds % 1000));
            assert!(Timestamp::parse(&text).is_ok(), "{text}");
        }
    }

    #[test]
    fn day_matches_the_formatted_date() {
        let instant = at(1_790_000_000, 0);
        assert_eq!(day(instant).iso(), "2026-09-21");
    }
}
