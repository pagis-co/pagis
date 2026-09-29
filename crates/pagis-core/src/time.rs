//! Integer millisecond timestamps, the storage convention for all tables.

pub type UnixMillis = i64;

/// Current wall-clock time as unix milliseconds.
pub fn now_ms() -> UnixMillis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as UnixMillis
}

/// Where a background pass reads its logical now. The daemon
/// runs on [`SystemClock`]; the evaluation driver hands its knowledge
/// worker the fixture clock, so a chronology decides inside its own
/// chronology instead of eight months after it.
///
/// This is logical time only. A budget or a lease deadline still reads
/// the wall clock, because it measures elapsed real time.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> UnixMillis;

    /// Complete when logical time changes. A wall clock never needs
    /// this signal because its wait duration advances with real time.
    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}

/// The machine's wall clock, what the daemon runs on.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> UnixMillis {
        now_ms()
    }
}

/// The calendar day an instant falls on in a named time zone, as prose:
/// "Thursday, September 24, 2026". A time zone name the database does
/// not know reads as UTC, the Workspace default.
pub fn local_date(at: UnixMillis, timezone: &str) -> String {
    use chrono::TimeZone;

    let zone: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let utc = chrono::DateTime::from_timestamp_millis(at).unwrap_or_default();
    zone.from_utc_datetime(&utc.naive_utc())
        .format("%A, %B %-d, %Y")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-25 03:00 UTC: already Friday in UTC, still Thursday
    /// evening in California.
    const FRIDAY_3AM_UTC: UnixMillis = 1_790_305_200_000;

    #[test]
    fn the_local_date_is_the_day_in_the_named_time_zone() {
        assert_eq!(
            local_date(FRIDAY_3AM_UTC, "UTC"),
            "Friday, September 25, 2026"
        );
        assert_eq!(
            local_date(FRIDAY_3AM_UTC, "America/Los_Angeles"),
            "Thursday, September 24, 2026"
        );
    }

    #[test]
    fn an_unknown_time_zone_reads_as_utc() {
        assert_eq!(
            local_date(FRIDAY_3AM_UTC, "Mars/Olympus_Mons"),
            "Friday, September 25, 2026"
        );
    }
}
