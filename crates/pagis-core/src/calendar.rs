//! Events in one Workspace Calendar.
use crate::{Author, CalendarEventId, StoreError, WorkspaceId};
use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::{Frequency, RRule, RRuleSet, Tz, Unvalidated};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The maximum number of occurrences in one range answer.
pub const MAX_OCCURRENCES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ParticipationStatus {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalendarAttendee {
    pub email: String,
    pub name: String,
    pub status: ParticipationStatus,
}

/// Scheduling state received or sent through the Workspace Mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CalendarInvitation {
    pub organizer: String,
    pub sequence: u32,
    pub status: EventStatus,
    pub occurrence: Option<InvitationOccurrence>,
}

/// A separate Event for one occurrence of an invitation series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InvitationOccurrence {
    pub series_uid: String,
    /// The original start as a UTC RFC 3339 time or an all-day date.
    pub recurrence_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

/// Local times use YYYY-MM-DDTHH:MM:SS. All-day dates use YYYY-MM-DD.
/// The end is exclusive. EXDATE values have the same format as the start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalendarEventData {
    pub title: String,
    pub description: String,
    pub location: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub timezone: String,
    pub rrule: Option<String>,
    pub exdates: Vec<String>,
    pub attendees: Vec<CalendarAttendee>,
    /// Minutes before the start. This is stored data; no delivery runs.
    pub reminders: Vec<u32>,
    pub meeting_url: Option<String>,
    #[serde(default)]
    pub invitation: Option<CalendarInvitation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CalendarEvent {
    #[schema(value_type = String)]
    pub id: CalendarEventId,
    pub uid: String,
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    pub created_by: Author,
    pub data: CalendarEventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CalendarOccurrence {
    pub event: CalendarEvent,
    /// RFC 3339 with the occurrence offset, or an all-day date.
    pub start: String,
    pub end: String,
}

impl CalendarEvent {
    pub fn new(workspace_id: WorkspaceId, created_by: Author, data: CalendarEventData) -> Self {
        let id = CalendarEventId::generate();
        Self {
            uid: id.to_string(),
            id,
            workspace_id,
            created_by,
            data,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("{0}")]
    Invalid(String),
    #[error("The recurrence exceeds the expansion limit.")]
    ExpansionLimit,
}

impl From<CalendarError> for StoreError {
    fn from(error: CalendarError) -> Self {
        Self::Corrupt(error.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CalendarRange {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

impl CalendarRange {
    pub fn parse(from: &str, to: &str) -> Result<Self, CalendarError> {
        let from =
            DateTime::parse_from_rfc3339(from).map_err(|_| invalid("Invalid range start."))?;
        let to = DateTime::parse_from_rfc3339(to).map_err(|_| invalid("Invalid range end."))?;
        Self::new(from.timestamp_millis(), to.timestamp_millis())
    }

    pub fn new(from: i64, to: i64) -> Result<Self, CalendarError> {
        let from =
            DateTime::from_timestamp_millis(from).ok_or_else(|| invalid("Invalid range start."))?;
        let to =
            DateTime::from_timestamp_millis(to).ok_or_else(|| invalid("Invalid range end."))?;
        if from >= to {
            return Err(invalid("The range end must follow its start."));
        }
        Ok(Self { from, to })
    }
    pub fn from(&self) -> i64 {
        self.from.timestamp_millis()
    }
    pub fn to(&self) -> i64 {
        self.to.timestamp_millis()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarBounds {
    pub starts_at: i64,
    pub ends_by: Option<i64>,
}

fn invalid(message: impl Into<String>) -> CalendarError {
    CalendarError::Invalid(message.into())
}

impl CalendarEventData {
    /// A stored invitation keeps its organizer and does not move to an older sequence.
    pub fn can_replace(&self, previous: &Self) -> bool {
        match (&previous.invitation, &self.invitation) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(old), Some(new)) => {
                new.organizer.eq_ignore_ascii_case(&old.organizer)
                    && new.occurrence == old.occurrence
                    && new.sequence >= old.sequence
                    && !(old.status == EventStatus::Cancelled
                        && new.sequence == old.sequence
                        && new.status != EventStatus::Cancelled)
            }
        }
    }

    fn zone(&self) -> Result<Tz, CalendarError> {
        self.timezone
            .parse::<chrono_tz::Tz>()
            .map(Tz::Tz)
            .map_err(|_| invalid("Use an IANA time zone."))
    }
    fn local(&self, value: &str) -> Result<NaiveDateTime, CalendarError> {
        if self.all_day {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        } else {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").ok()
        }
        .filter(|d| (1..=9999).contains(&chrono::Datelike::year(d)))
        .ok_or_else(|| invalid("Use a valid local date or time."))
    }
    fn at(&self, value: &str) -> Result<DateTime<Tz>, CalendarError> {
        self.zone()?
            .from_local_datetime(&self.local(value)?)
            .single()
            .ok_or_else(|| invalid("The local time is absent or ambiguous in this time zone."))
    }
    fn rule_set(&self) -> Result<RRuleSet, CalendarError> {
        let start = self.at(&self.start)?;
        let mut set = RRuleSet::new(start);
        if let Some(text) = &self.rrule {
            if text.len() > 2048 || text.contains(['\r', '\n']) {
                return Err(invalid("Use one RRULE."));
            }
            let rule = text
                .parse::<RRule<Unvalidated>>()
                .map_err(|_| invalid("Invalid RRULE."))?;
            if self.all_day
                && (matches!(
                    rule.get_freq(),
                    Frequency::Secondly | Frequency::Minutely | Frequency::Hourly
                ) || !rule.get_by_hour().is_empty()
                    || !rule.get_by_minute().is_empty()
                    || !rule.get_by_second().is_empty())
            {
                return Err(invalid(
                    "All-day recurrence uses dates, not hours, minutes or seconds.",
                ));
            }
            let rule = rule
                .validate(start)
                .map_err(|_| invalid("Invalid RRULE."))?;
            set = set.rrule(rule);
        } else {
            set = set.rdate(start);
        }
        for value in &self.exdates {
            set = set.exdate(self.at(value)?);
        }
        Ok(set)
    }
    fn finish(&self, at: DateTime<Tz>) -> Result<DateTime<Tz>, CalendarError> {
        if self.all_day {
            let days = self
                .local(&self.end)?
                .signed_duration_since(self.local(&self.start)?)
                .num_days();
            let local = at
                .naive_local()
                .checked_add_signed(Duration::days(days))
                .ok_or_else(|| invalid("Invalid event end."))?;
            self.zone()?
                .from_local_datetime(&local)
                .earliest()
                .ok_or_else(|| invalid("Invalid event end."))
        } else {
            let duration = self
                .at(&self.end)?
                .signed_duration_since(self.at(&self.start)?);
            at.checked_add_signed(duration)
                .ok_or_else(|| invalid("Invalid event end."))
        }
    }

    pub(crate) fn single_occurrence(&self, start: &str) -> Result<Self, CalendarError> {
        let mut data = self.clone();
        let end = self.finish(self.at(start)?)?;
        data.start = start.into();
        data.end = end
            .format(if self.all_day {
                "%Y-%m-%d"
            } else {
                "%Y-%m-%dT%H:%M:%S"
            })
            .to_string();
        data.rrule = None;
        data.exdates.clear();
        Ok(data)
    }

    /// The envelope of the occurrences, after EXDATE. An empty Event has
    /// an empty envelope at its original start. NULL denotes an endless series.
    pub fn bounds(&self) -> Result<CalendarBounds, CalendarError> {
        self.validate()?;
        let set = self.rule_set()?;
        let endless = set
            .get_rrule()
            .iter()
            .any(|rule| rule.get_count().is_none() && rule.get_until().is_none());
        let result = set.all(if endless { 1 } else { u16::MAX });
        if !endless && result.limited {
            return Err(CalendarError::ExpansionLimit);
        }
        let starts_at = result
            .dates
            .first()
            .copied()
            .unwrap_or(self.at(&self.start)?)
            .timestamp_millis();
        let ends_by = if endless {
            None
        } else {
            Some(match result.dates.last() {
                Some(at) => self.finish(*at)?.timestamp_millis(),
                None => starts_at,
            })
        };
        Ok(CalendarBounds { starts_at, ends_by })
    }

    pub fn validate(&self) -> Result<(), CalendarError> {
        if self.title.trim().is_empty()
            || self.title.len() > 1024
            || self.description.len() > 65536
            || self.location.len() > 4096
        {
            return Err(invalid(
                "Supply a title and keep event text within its limits.",
            ));
        }
        if self.at(&self.end)? <= self.at(&self.start)? {
            return Err(invalid("The end must follow the start."));
        }
        if self.exdates.len() > 1000 || self.attendees.len() > 1000 || self.reminders.len() > 20 {
            return Err(invalid("Too many event entries."));
        }
        for attendee in &self.attendees {
            if !attendee.email.contains('@')
                || attendee.email.contains(char::is_whitespace)
                || attendee.email.len() > 320
                || attendee.name.len() > 1024
            {
                return Err(invalid("Supply a valid attendee email."));
            }
        }
        if let Some(value) = &self.meeting_url {
            let url = url::Url::parse(value).map_err(|_| invalid("Use an HTTPS meeting link."))?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(invalid("Use an HTTPS meeting link without credentials."));
            }
        }
        self.rule_set()?;
        Ok(())
    }
}

/// Read occurrences that overlap [from, to), in UTC milliseconds.
/// Expansion uses the library's iteration and validation limits.
pub fn occurrences(
    events: Vec<CalendarEvent>,
    range: CalendarRange,
) -> Result<Vec<CalendarOccurrence>, CalendarError> {
    expand(events, range, false)
}

fn expand(
    events: Vec<CalendarEvent>,
    range: CalendarRange,
    busy_only: bool,
) -> Result<Vec<CalendarOccurrence>, CalendarError> {
    let CalendarRange { from, to } = range;
    let mut found = Vec::new();
    for event in events {
        let data = &event.data;
        if busy_only
            && (data.all_day
                || data
                    .invitation
                    .as_ref()
                    .is_some_and(|i| i.status == EventStatus::Cancelled))
        {
            continue;
        }
        let start = data.at(&data.start)?;
        let end = data.at(&data.end)?;
        let duration = end.signed_duration_since(start);
        // All-day duration follows local dates across offset changes.
        let lookback = if data.all_day {
            duration + Duration::days(2)
        } else {
            duration
        };
        let earliest = from
            .checked_sub_signed(lookback)
            .ok_or_else(|| invalid("Invalid range."))?;
        let set = data
            .rule_set()?
            .after(earliest.with_timezone(&data.zone()?))
            .before(to.with_timezone(&data.zone()?));
        let expanded = set.all((MAX_OCCURRENCES + 4) as u16);
        if busy_only && expanded.limited {
            return Err(invalid(
                "Too many occurrences. Use a smaller Free/Busy range.",
            ));
        }
        for at in expanded.dates {
            let finish = data.finish(at)?;
            if at < to && finish > from {
                found.push((
                    at.timestamp_millis(),
                    CalendarOccurrence {
                        event: event.clone(),
                        start: if data.all_day {
                            at.format("%Y-%m-%d").to_string()
                        } else {
                            at.to_rfc3339()
                        },
                        end: if data.all_day {
                            finish.format("%Y-%m-%d").to_string()
                        } else {
                            finish.to_rfc3339()
                        },
                    },
                ));
            }
        }
        found.sort_by(|a, b| (a.0, &a.1.event.id).cmp(&(b.0, &b.1.event.id)));
        if !busy_only {
            found.truncate(MAX_OCCURRENCES);
        }
    }
    Ok(found.into_iter().map(|(_, value)| value).collect())
}

/// A busy interval contains no Event details. Its end is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusyInterval {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl BusyInterval {
    /// RFC 3339 UTC start and exclusive end.
    pub fn rfc3339(&self) -> (String, String) {
        let format = |ms| {
            DateTime::from_timestamp_millis(ms)
                .expect("validated Calendar range")
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        };
        (format(self.start_ms), format(self.end_ms))
    }
}

/// Merge timed occurrences within the half-open UTC range. All-day Events are free.
pub fn free_busy(
    events: Vec<CalendarEvent>,
    range: CalendarRange,
) -> Result<Vec<BusyInterval>, CalendarError> {
    let mut busy: Vec<BusyInterval> = Vec::new();
    for occurrence in expand(events, range, true)? {
        let start_ms = DateTime::parse_from_rfc3339(&occurrence.start)
            .map_err(|_| invalid("Invalid occurrence start."))?
            .timestamp_millis()
            .max(range.from());
        let end_ms = DateTime::parse_from_rfc3339(&occurrence.end)
            .map_err(|_| invalid("Invalid occurrence end."))?
            .timestamp_millis()
            .min(range.to());
        if let Some(last) = busy.last_mut()
            && start_ms <= last.end_ms
        {
            last.end_ms = last.end_ms.max(end_ms);
        } else {
            busy.push(BusyInterval { start_ms, end_ms });
        }
    }
    Ok(busy)
}

#[async_trait]
pub trait CalendarStore: Send + Sync {
    async fn linked_occurrences(
        &self,
        workspace_id: &WorkspaceId,
        series_uid: &str,
    ) -> Result<Vec<CalendarEvent>, StoreError>;

    async fn get_by_uid(
        &self,
        workspace_id: &WorkspaceId,
        uid: &str,
    ) -> Result<Option<CalendarEvent>, StoreError>;

    /// Search stored title, description and location with a case-insensitive substring.
    /// Return at most `limit` master Events, newest first by first occurrence.
    async fn search(
        &self,
        workspace_id: &WorkspaceId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<CalendarEvent>, StoreError>;
    /// Read only merged busy intervals of this Workspace in [from, to).
    async fn free_busy(
        &self,
        workspace_id: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<BusyInterval>, StoreError>;
    async fn create(&self, event: &CalendarEvent) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<Option<CalendarEvent>, StoreError>;
    /// Change Event data. Identity, owner and author stay with the record.
    /// Return false if the Event is absent or its scheduling state is newer.
    async fn update(
        &self,
        workspace_id: &WorkspaceId,
        id: &CalendarEventId,
        data: &CalendarEventData,
    ) -> Result<bool, StoreError>;
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<bool, StoreError>;
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<CalendarOccurrence>, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data() -> CalendarEventData {
        CalendarEventData {
            title: "Review".into(),
            description: String::new(),
            location: String::new(),
            start: "2026-03-07T09:00:00".into(),
            end: "2026-03-07T10:00:00".into(),
            timezone: "America/New_York".into(),
            all_day: false,
            rrule: None,
            exdates: vec![],
            attendees: vec![],
            reminders: vec![15],
            meeting_url: None,
            invitation: None,
        }
    }
    fn ms(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn calendar_bounds_follow_count_until_exdate_and_dst() {
        let mut value = data();
        let single = value.bounds().unwrap();
        assert_eq!(single.starts_at, ms("2026-03-07T14:00:00Z"));
        assert_eq!(single.ends_by, Some(ms("2026-03-07T15:00:00Z")));
        value.rrule = Some("FREQ=DAILY;COUNT=4".into());
        assert_eq!(
            value.bounds().unwrap().ends_by,
            Some(ms("2026-03-10T14:00:00Z"))
        );
        value.exdates = vec!["2026-03-07T09:00:00".into(), "2026-03-10T09:00:00".into()];
        let bounds = value.bounds().unwrap();
        assert_eq!(bounds.starts_at, ms("2026-03-08T13:00:00Z"));
        assert_eq!(bounds.ends_by, Some(ms("2026-03-09T14:00:00Z")));
        value.exdates.clear();
        value.rrule = Some("FREQ=DAILY;UNTIL=20260309T130000Z".into());
        assert_eq!(
            value.bounds().unwrap().ends_by,
            Some(ms("2026-03-09T14:00:00Z"))
        );
        value.rrule = Some("FREQ=DAILY".into());
        assert_eq!(value.bounds().unwrap().ends_by, None);
    }

    #[test]
    fn calendar_bounds_use_local_dates_for_all_day_end() {
        let mut value = data();
        value.all_day = true;
        value.start = "2026-03-07".into();
        value.end = "2026-03-09".into();
        value.rrule = Some("FREQ=DAILY;COUNT=3".into());
        assert_eq!(
            value.bounds().unwrap().ends_by,
            Some(ms("2026-03-11T04:00:00Z"))
        );
        value.exdates = vec!["2026-03-09".into()];
        assert_eq!(
            value.bounds().unwrap().ends_by,
            Some(ms("2026-03-10T04:00:00Z"))
        );
        value.rrule = None;
        value.exdates = vec!["2026-03-07".into()];
        let bounds = value.bounds().unwrap();
        assert_eq!(bounds.ends_by, Some(bounds.starts_at));
    }

    #[test]
    fn calendar_range_rejects_invalid_input_with_a_calendar_error() {
        let error: CalendarError = CalendarRange::new(10, 10).unwrap_err();
        assert!(error.to_string().contains("range"));
        assert!(CalendarRange::new(20, 10).is_err());
        assert!(CalendarRange::new(i64::MIN, i64::MAX).is_err());
    }

    #[test]
    fn calendar_rejects_invalid_event_data() {
        let valid = data();
        assert!(valid.validate().is_ok());
        let mut bad = valid.clone();
        bad.meeting_url = Some("http://meet.example.com".into());
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.timezone = "Unknown/Zone".into();
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.end = bad.start.clone();
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.start = "2026-03-08T02:30:00".into();
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.rrule = Some("FREQ=NOPE".into());
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.rrule = Some("FREQ=DAILY\nRRULE:FREQ=SECONDLY".into());
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.title.clear();
        assert!(bad.validate().is_err());
        bad = valid;
        bad.exdates.push("invalid".into());
        assert!(bad.validate().is_err());
    }
    #[test]
    fn calendar_cap_applies_after_overlap_filter() {
        let mut data = data();
        data.rrule = Some("FREQ=SECONDLY;COUNT=10000".into());
        data.end = "2026-03-07T09:00:01".into();
        let row = CalendarEvent::new(WorkspaceId::generate(), Author::System, data);
        let from = 1_772_892_100_000;
        assert_eq!(
            occurrences(
                vec![row],
                CalendarRange::new(from, from + 2_000_000).unwrap()
            )
            .unwrap()
            .len(),
            MAX_OCCURRENCES
        );
    }
    #[test]
    fn calendar_range_is_half_open_and_includes_overlaps() {
        let row = CalendarEvent::new(WorkspaceId::generate(), Author::System, data());
        let start = 1_772_892_000_000;
        let end = start + 3_600_000;
        assert_eq!(
            occurrences(vec![row.clone()], CalendarRange::new(start, end).unwrap())
                .unwrap()
                .len(),
            1
        );
        assert!(
            occurrences(vec![row.clone()], CalendarRange::new(end, end + 1).unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(
            occurrences(vec![row], CalendarRange::new(start - 1, start).unwrap())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn free_busy_does_not_use_the_event_list_cap() {
        let row = CalendarEvent::new(WorkspaceId::generate(), Author::System, data());
        let mut events = vec![row.clone(); MAX_OCCURRENCES];
        let mut later = row;
        later.data.start = "2026-03-08T09:00:00".into();
        later.data.end = "2026-03-08T10:00:00".into();
        events.push(later);
        let range = CalendarRange::parse("2026-03-07T00:00:00Z", "2026-03-09T00:00:00Z").unwrap();
        assert_eq!(free_busy(events, range).unwrap().len(), 2);
    }

    #[test]
    fn free_busy_refuses_incomplete_expansion() {
        let mut row = CalendarEvent::new(WorkspaceId::generate(), Author::System, data());
        row.data.rrule = Some("FREQ=MINUTELY".into());
        let range = CalendarRange::parse("2026-03-07T00:00:00Z", "2026-03-09T00:00:00Z").unwrap();
        assert!(free_busy(vec![row], range).is_err());
    }
}
