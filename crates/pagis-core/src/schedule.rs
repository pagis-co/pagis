//! Pure due-time rules for a Schedule. All times are Unix milliseconds.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use std::str::FromStr;

use crate::{Schedule, ScheduleKind, StoreError, UnixMillis};

#[derive(Debug, thiserror::Error)]
pub enum ScheduleTimeError {
    #[error("cron expression: {0}")]
    InvalidCron(String),
    #[error("cron timezone: {0}")]
    InvalidTimezone(String),
    #[error("Schedule time is out of range")]
    OutOfRange,
    #[error("interval cadence must be positive")]
    InvalidInterval,
    #[error("{0} is missing")]
    MissingField(&'static str),
}

impl From<ScheduleTimeError> for StoreError {
    fn from(error: ScheduleTimeError) -> Self {
        Self::Corrupt(error.to_string())
    }
}

/// The latest due time at or before `now`, and the next time after `now`.
/// Before the anchor, no time is due and the next time is the anchor.
pub fn interval_window(
    anchor: UnixMillis,
    interval_ms: UnixMillis,
    now: UnixMillis,
) -> Result<(Option<UnixMillis>, UnixMillis), ScheduleTimeError> {
    if interval_ms <= 0 {
        return Err(ScheduleTimeError::InvalidInterval);
    }
    if now < anchor {
        return Ok((None, anchor));
    }
    let interval = i128::from(interval_ms);
    let due = i128::from(anchor) + ((i128::from(now) - i128::from(anchor)) / interval) * interval;
    let next = i64::try_from(due + interval).map_err(|_| ScheduleTimeError::OutOfRange)?;
    Ok((Some(due as i64), next))
}

/// The latest cron due time at or before `now`, and the next time after it.
/// The five-field expression uses the named IANA timezone.
pub fn cron_window(
    expression: &str,
    timezone: &str,
    now: UnixMillis,
) -> Result<(UnixMillis, UnixMillis), ScheduleTimeError> {
    let (cron, zone) = parse_cron(expression, timezone)?;
    let now = cron_time(now)?.with_timezone(&zone);
    let latest = cron
        .find_previous_occurrence(&now, true)
        .map_err(|error| ScheduleTimeError::InvalidCron(error.to_string()))?;
    let next = cron
        .find_next_occurrence(&latest, false)
        .map_err(|error| ScheduleTimeError::InvalidCron(error.to_string()))?;
    Ok((latest.timestamp_millis(), next.timestamp_millis()))
}

/// The first cron due time strictly after `after`, in the named timezone.
pub fn next_cron_occurrence(
    expression: &str,
    timezone: &str,
    after: UnixMillis,
) -> Result<UnixMillis, ScheduleTimeError> {
    let (cron, zone) = parse_cron(expression, timezone)?;
    let after = cron_time(after)?.with_timezone(&zone);
    Ok(cron
        .find_next_occurrence(&after, false)
        .map_err(|error| ScheduleTimeError::InvalidCron(error.to_string()))?
        .timestamp_millis())
}

/// The latest due time at or before `now`, and the next time after `now`.
/// A one-shot has no next time after it is due. An interval has no latest
/// time before its anchor. The stored `next_due_at` does not set the cadence.
pub fn schedule_window(
    schedule: &Schedule,
    now: UnixMillis,
) -> Result<(Option<UnixMillis>, Option<UnixMillis>), ScheduleTimeError> {
    match schedule.kind {
        ScheduleKind::OneShot => Ok((
            (schedule.scheduled_at <= now).then_some(schedule.scheduled_at),
            (schedule.scheduled_at > now).then_some(schedule.scheduled_at),
        )),
        ScheduleKind::Interval => {
            let (anchor, interval) = interval_fields(schedule)?;
            let (latest, next) = interval_window(anchor, interval, now)?;
            Ok((latest, Some(next)))
        }
        ScheduleKind::Cron => {
            let (latest, next) = cron_window(cron_expression(schedule)?, &schedule.timezone, now)?;
            Ok((Some(latest), Some(next)))
        }
    }
}

/// The first due time strictly after `after`. A completed one-shot has none.
pub fn next_after(
    schedule: &Schedule,
    after: UnixMillis,
) -> Result<Option<UnixMillis>, ScheduleTimeError> {
    match schedule.kind {
        ScheduleKind::Cron => {
            next_cron_occurrence(cron_expression(schedule)?, &schedule.timezone, after).map(Some)
        }
        _ => schedule_window(schedule, after).map(|(_, next)| next),
    }
}

/// The missed count between two due times, capped at `u32::MAX`.
/// An interval counts the earlier time and excludes the latest time.
/// Cron excludes both endpoints. Equal or reversed times and one-shots
/// have a count of zero.
pub fn missed_count(
    schedule: &Schedule,
    previous_due_at: UnixMillis,
    latest_due_at: UnixMillis,
) -> Result<u32, ScheduleTimeError> {
    if latest_due_at <= previous_due_at {
        return Ok(0);
    }
    match schedule.kind {
        ScheduleKind::Interval => {
            let (_, interval) = interval_fields(schedule)?;
            let count =
                (i128::from(latest_due_at) - i128::from(previous_due_at)) / i128::from(interval);
            Ok(u32::try_from(count).unwrap_or(u32::MAX))
        }
        ScheduleKind::Cron => {
            let (cron, zone) = parse_cron(cron_expression(schedule)?, &schedule.timezone)?;
            let mut occurrence = cron_time(previous_due_at)?.with_timezone(&zone);
            let mut count = 0_u32;
            loop {
                let next = cron
                    .find_next_occurrence(&occurrence, false)
                    .map_err(|error| ScheduleTimeError::InvalidCron(error.to_string()))?;
                if next.timestamp_millis() >= latest_due_at {
                    return Ok(count);
                }
                count = count.saturating_add(1);
                occurrence = next;
            }
        }
        ScheduleKind::OneShot => Ok(0),
    }
}

fn interval_fields(schedule: &Schedule) -> Result<(UnixMillis, UnixMillis), ScheduleTimeError> {
    let anchor = schedule
        .anchor_at
        .ok_or(ScheduleTimeError::MissingField("interval anchor"))?;
    let interval = schedule
        .interval_ms
        .ok_or(ScheduleTimeError::MissingField("interval cadence"))?;
    if interval <= 0 {
        return Err(ScheduleTimeError::InvalidInterval);
    }
    Ok((anchor, interval))
}

fn cron_expression(schedule: &Schedule) -> Result<&str, ScheduleTimeError> {
    schedule
        .cron_expression
        .as_deref()
        .ok_or(ScheduleTimeError::MissingField("cron expression"))
}

fn parse_cron(expression: &str, timezone: &str) -> Result<(Cron, Tz), ScheduleTimeError> {
    if expression.split_whitespace().count() != 5 {
        return Err(ScheduleTimeError::InvalidCron(
            "expected five fields".into(),
        ));
    }
    let zone = timezone
        .parse::<Tz>()
        .map_err(|_| ScheduleTimeError::InvalidTimezone(timezone.to_string()))?;
    let cron = Cron::from_str(expression)
        .map_err(|error| ScheduleTimeError::InvalidCron(error.to_string()))?;
    Ok((cron, zone))
}

fn cron_time(at: UnixMillis) -> Result<DateTime<Utc>, ScheduleTimeError> {
    DateTime::from_timestamp_millis(at).ok_or(ScheduleTimeError::OutOfRange)
}
