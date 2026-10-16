//! iTIP scheduling messages carried by the Workspace Mailbox.
use crate::calendar::{CalendarInvitation, EventStatus, InvitationOccurrence};
use crate::{
    Author, CalendarAttendee, CalendarError, CalendarEvent, CalendarEventData, CalendarStore,
    ParticipationStatus, StoreError, WorkspaceId,
};
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use icalendar::parser::{Component as ParsedComponent, Property as ParsedProperty};
use icalendar::{Component, EventLike, Property};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingMethod {
    Request,
    Reply,
    Cancel,
}
impl SchedulingMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "REQUEST",
            Self::Reply => "REPLY",
            Self::Cancel => "CANCEL",
        }
    }
}

pub struct SchedulingMessage {
    pub method: SchedulingMethod,
    pub uid: String,
    pub organizer: String,
    pub sequence: u32,
    pub recurrence_id: Option<String>,
    pub attendees: Vec<CalendarAttendee>,
    pub data: Option<CalendarEventData>,
}
fn invalid() -> CalendarError {
    CalendarError::Invalid("The calendar message is not supported or is not valid.".into())
}
fn value(component: &ParsedComponent<'_>, key: &str) -> Option<String> {
    component.find_prop(key).map(|p| p.val.to_string())
}
fn parameter<'a>(property: &'a ParsedProperty<'_>, key: &str) -> Option<&'a str> {
    property
        .params
        .iter()
        .find(|p| p.key == key)
        .and_then(|p| p.val.as_ref())
        .map(|p| p.as_str())
}
fn address(value: &str) -> Result<String, CalendarError> {
    let address = value
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("mailto:"))
        .and_then(|_| value.get(7..))
        .ok_or_else(invalid)?;
    if !address.contains('@')
        || address.chars().any(|c| c.is_whitespace() || c.is_control())
        || address.len() > 320
    {
        return Err(invalid());
    }
    Ok(address.to_string())
}
fn time(property: &ParsedProperty<'_>) -> Result<(String, String, bool), CalendarError> {
    let raw = property.val.as_str();
    if parameter(property, "VALUE") == Some("DATE") {
        let date = NaiveDate::parse_from_str(raw, "%Y%m%d").map_err(|_| invalid())?;
        return Ok((date.format("%Y-%m-%d").to_string(), "UTC".into(), true));
    }
    let utc = raw.ends_with('Z');
    let date = NaiveDateTime::parse_from_str(raw.trim_end_matches('Z'), "%Y%m%dT%H%M%S")
        .map_err(|_| invalid())?;
    let zone = if utc {
        "UTC"
    } else {
        parameter(property, "TZID").ok_or_else(invalid)?
    };
    Ok((
        date.format("%Y-%m-%dT%H:%M:%S").to_string(),
        resolve_zone(zone)?.name().into(),
        false,
    ))
}
fn resolve_zone(name: &str) -> Result<chrono_tz::Tz, CalendarError> {
    name.parse::<chrono_tz::Tz>()
        .or_else(|_| {
            name.parse::<windows_timezones::WindowsTimezone>()
                .map(Into::into)
        })
        .map_err(|_| invalid())
}

fn duration_end(
    start: &str,
    zone: &str,
    all_day: bool,
    raw: Option<&str>,
) -> Result<String, CalendarError> {
    let raw = raw
        .or(if all_day { Some("P1D") } else { None })
        .ok_or_else(invalid)?;
    // RFC 5545 permits weeks, days and whole time units, with no months or years.
    static SYNTAX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\+?P([0-9]+W|([0-9]+D)?(T([0-9]+H)?([0-9]+M)?([0-9]+S)?)?)$").unwrap()
    });
    if !SYNTAX.is_match(raw) || raw.ends_with('T') {
        return Err(invalid());
    }
    let (days, seconds) = match raw
        .trim_start_matches('+')
        .parse::<iso8601::Duration>()
        .map_err(|_| invalid())?
    {
        iso8601::Duration::Weeks(weeks) => (u64::from(weeks) * 7, 0),
        iso8601::Duration::YMDHMS {
            year: 0,
            month: 0,
            day,
            hour,
            minute,
            second,
            millisecond: 0,
        } => (
            u64::from(day),
            i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second),
        ),
        _ => return Err(invalid()),
    };
    if days == 0 && seconds == 0 || all_day && raw.contains('T') {
        return Err(invalid());
    }
    if all_day {
        return NaiveDate::parse_from_str(start, "%Y-%m-%d")
            .map_err(|_| invalid())?
            .checked_add_days(chrono::Days::new(days))
            .map(|d| d.to_string())
            .ok_or_else(invalid);
    }
    let local = NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| invalid())?
        .checked_add_days(chrono::Days::new(days))
        .ok_or_else(invalid)?;
    let end = resolve_zone(zone)?
        .from_local_datetime(&local)
        .single()
        .ok_or_else(invalid)?
        .checked_add_signed(chrono::Duration::seconds(seconds))
        .ok_or_else(invalid)?;
    Ok(end.format("%Y-%m-%dT%H:%M:%S").to_string())
}

fn status(value: &str) -> Result<ParticipationStatus, CalendarError> {
    Ok(match value {
        "NEEDS-ACTION" => ParticipationStatus::NeedsAction,
        "ACCEPTED" => ParticipationStatus::Accepted,
        "DECLINED" => ParticipationStatus::Declined,
        "TENTATIVE" => ParticipationStatus::Tentative,
        _ => return Err(invalid()),
    })
}
pub fn participation_value(status: &ParticipationStatus) -> &'static str {
    match status {
        ParticipationStatus::NeedsAction => "NEEDS-ACTION",
        ParticipationStatus::Accepted => "ACCEPTED",
        ParticipationStatus::Declined => "DECLINED",
        ParticipationStatus::Tentative => "TENTATIVE",
    }
}
impl SchedulingMessage {
    pub fn parse(input: &str) -> Result<Self, CalendarError> {
        if input.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let unfolded = icalendar::parser::unfold(input);
        let calendar = icalendar::parser::read_calendar(&unfolded).map_err(|_| invalid())?;
        let method = calendar
            .properties
            .iter()
            .find(|p| p.name == "METHOD")
            .ok_or_else(invalid)?;
        let method = match method.val.as_str() {
            "REQUEST" => SchedulingMethod::Request,
            "REPLY" => SchedulingMethod::Reply,
            "CANCEL" => SchedulingMethod::Cancel,
            _ => return Err(invalid()),
        };
        let events: Vec<_> = calendar
            .components
            .iter()
            .filter(|c| c.name == "VEVENT")
            .collect();
        if events.len() != 1 {
            return Err(invalid());
        }
        let event = events[0];
        let recurrence_id = event
            .find_prop("RECURRENCE-ID")
            .map(|p| {
                if parameter(p, "RANGE").is_some() {
                    return Err(invalid());
                }
                let (start, zone, all_day) = time(p)?;
                if all_day {
                    return Ok(start);
                }
                let local = NaiveDateTime::parse_from_str(&start, "%Y-%m-%dT%H:%M:%S")
                    .map_err(|_| invalid())?;
                Ok(resolve_zone(&zone)?
                    .from_local_datetime(&local)
                    .single()
                    .ok_or_else(invalid)?
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339())
            })
            .transpose()?;
        let uid = value(event, "UID")
            .filter(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
            .ok_or_else(invalid)?;
        let organizer = address(&value(event, "ORGANIZER").ok_or_else(invalid)?)?;
        let sequence = value(event, "SEQUENCE")
            .unwrap_or_else(|| "0".into())
            .parse()
            .map_err(|_| invalid())?;
        let attendees = event
            .properties
            .iter()
            .filter(|p| p.name == "ATTENDEE")
            .map(|p| {
                Ok(CalendarAttendee {
                    email: address(p.val.as_str())?,
                    name: parameter(p, "CN").unwrap_or("").into(),
                    status: status(parameter(p, "PARTSTAT").unwrap_or("NEEDS-ACTION"))?,
                })
            })
            .collect::<Result<Vec<_>, CalendarError>>()?;
        let data = if method == SchedulingMethod::Request {
            let (start, timezone, all_day) = time(event.find_prop("DTSTART").ok_or_else(invalid)?)?;
            let end = if let Some(property) = event.find_prop("DTEND") {
                if event.find_prop("DURATION").is_some() {
                    return Err(invalid());
                }
                let (end, end_zone, end_day) = time(property)?;
                if timezone != end_zone || all_day != end_day {
                    return Err(invalid());
                }
                end
            } else {
                duration_end(
                    &start,
                    &timezone,
                    all_day,
                    value(event, "DURATION").as_deref(),
                )?
            };
            let mut exdates = Vec::new();
            for property in event.properties.iter().filter(|p| p.name == "EXDATE") {
                for raw in property.val.as_str().split(',') {
                    let mut one = property.clone();
                    one.val = raw.to_string().into();
                    let (date, zone, day) = time(&one)?;
                    if zone != timezone || day != all_day {
                        return Err(invalid());
                    }
                    exdates.push(date);
                }
            }
            let data = CalendarEventData {
                title: value(event, "SUMMARY").ok_or_else(invalid)?,
                description: value(event, "DESCRIPTION").unwrap_or_default(),
                location: value(event, "LOCATION").unwrap_or_default(),
                start,
                end,
                timezone,
                all_day,
                rrule: value(event, "RRULE"),
                exdates,
                attendees: attendees.clone(),
                reminders: vec![],
                meeting_url: value(event, "URL"),
                invitation: Some(CalendarInvitation {
                    organizer: organizer.clone(),
                    sequence,
                    status: EventStatus::Tentative,
                    occurrence: None,
                }),
            };
            data.bounds()?;
            Some(data)
        } else {
            None
        };
        Ok(Self {
            method,
            uid,
            organizer,
            sequence,
            recurrence_id,
            attendees,
            data,
        })
    }

    pub fn build(event: &CalendarEvent, method: SchedulingMethod) -> Result<String, CalendarError> {
        let data = &event.data;
        let invitation = data.invitation.as_ref().ok_or_else(invalid)?;
        data.validate()?;
        let mut component = icalendar::Event::new();
        component
            .uid(
                invitation
                    .occurrence
                    .as_ref()
                    .map_or(event.uid.as_str(), |o| o.series_uid.as_str()),
            )
            .summary(&data.title)
            .description(&data.description)
            .location(&data.location)
            .add_property("ORGANIZER", format!("mailto:{}", invitation.organizer))
            .add_property("SEQUENCE", invitation.sequence.to_string());
        if let Some(occurrence) = &invitation.occurrence {
            let mut property = if data.all_day {
                let date = NaiveDate::parse_from_str(&occurrence.recurrence_id, "%Y-%m-%d")
                    .map_err(|_| invalid())?;
                let mut property =
                    Property::new("RECURRENCE-ID", date.format("%Y%m%d").to_string());
                property.add_parameter("VALUE", "DATE");
                property
            } else {
                let date = chrono::DateTime::parse_from_rfc3339(&occurrence.recurrence_id)
                    .map_err(|_| invalid())?;
                Property::new(
                    "RECURRENCE-ID",
                    date.with_timezone(&chrono::Utc)
                        .format("%Y%m%dT%H%M%SZ")
                        .to_string(),
                )
            };
            component.append_property(property.done());
        }
        if method == SchedulingMethod::Cancel {
            component.add_property("STATUS", "CANCELLED");
        } else {
            component.add_property(
                "STATUS",
                if invitation.status == EventStatus::Tentative {
                    "TENTATIVE"
                } else {
                    "CONFIRMED"
                },
            );
        }
        for (key, value) in [("DTSTART", &data.start), ("DTEND", &data.end)] {
            component.append_property(time_property(key, value, data)?);
        }
        for value in &data.exdates {
            component.append_multi_property(time_property("EXDATE", value, data)?);
        }
        if let Some(rule) = &data.rrule {
            component.add_property("RRULE", rule);
        }
        if let Some(url) = &data.meeting_url {
            component.add_property("URL", url);
        }
        for attendee in &data.attendees {
            let mut property = Property::new("ATTENDEE", format!("mailto:{}", attendee.email));
            property
                .add_parameter("CN", &attendee.name)
                .add_parameter("PARTSTAT", participation_value(&attendee.status));
            if method == SchedulingMethod::Request {
                property.add_parameter("RSVP", "TRUE");
            }
            component.append_multi_property(property);
        }
        let mut calendar = icalendar::Calendar::new();
        calendar
            .append_property(Property::new("METHOD", method.as_str()))
            .push(component.done());
        Ok(calendar.to_string())
    }

    /// A sender can change only its own participation or its own invitation.
    pub async fn apply(
        self,
        store: &dyn CalendarStore,
        workspace: &WorkspaceId,
        own: &str,
        sender: &str,
    ) -> Result<Option<CalendarEvent>, StoreError> {
        let previous = store.get_by_uid(workspace, &self.uid).await?;
        if self.recurrence_id.is_some() {
            return self
                .apply_occurrence(store, workspace, own, sender, previous)
                .await;
        }
        let Some(mut event) = previous else {
            if self.method != SchedulingMethod::Request
                || !self.organizer.eq_ignore_ascii_case(sender)
                || !self
                    .attendees
                    .iter()
                    .any(|a| a.email.eq_ignore_ascii_case(own))
            {
                return Ok(None);
            }
            let mut data = self.data.ok_or_else(invalid)?;
            for attendee in &mut data.attendees {
                if attendee.email.eq_ignore_ascii_case(own) {
                    attendee.status = ParticipationStatus::NeedsAction;
                }
            }
            let mut event = CalendarEvent::new(workspace.clone(), Author::System, data);
            event.uid = self.uid;
            store.create(&event).await?;
            return Ok(Some(event));
        };
        let Some(invitation) = event.data.invitation.as_ref() else {
            return Ok(None);
        };
        if !invitation.organizer.eq_ignore_ascii_case(&self.organizer)
            || self.sequence < invitation.sequence
        {
            return Ok(Some(event));
        }
        let linked = store.linked_occurrences(workspace, &self.uid).await?;
        if self.method != SchedulingMethod::Reply
            && linked.iter().any(|child| {
                child
                    .data
                    .invitation
                    .as_ref()
                    .is_some_and(|i| i.sequence > self.sequence)
            })
        {
            return Ok(Some(event));
        }
        match self.method {
            SchedulingMethod::Request => {
                if !self.organizer.eq_ignore_ascii_case(sender)
                    || self.sequence <= invitation.sequence
                {
                    return Ok(Some(event));
                }
                let mut data = self.data.ok_or_else(invalid)?;
                if !data
                    .attendees
                    .iter()
                    .any(|a| a.email.eq_ignore_ascii_case(own))
                {
                    return Ok(Some(event));
                }
                for attendee in &mut data.attendees {
                    if attendee.email.eq_ignore_ascii_case(own) {
                        attendee.status = ParticipationStatus::NeedsAction;
                    }
                }
                for child in &linked {
                    let occurrence = child
                        .data
                        .invitation
                        .as_ref()
                        .and_then(|i| i.occurrence.as_ref())
                        .ok_or_else(invalid)?;
                    let date = occurrence_local(&occurrence.recurrence_id, &data)?;
                    if !data.exdates.contains(&date) {
                        data.exdates.push(date);
                    }
                }
                event.data = data;
            }
            SchedulingMethod::Reply => {
                if !invitation.organizer.eq_ignore_ascii_case(own)
                    || self.sequence != invitation.sequence
                    || self.attendees.len() != 1
                {
                    return Ok(Some(event));
                }
                let reply = &self.attendees[0];
                if !reply.email.eq_ignore_ascii_case(sender) {
                    return Ok(Some(event));
                }
                if let Some(attendee) = event
                    .data
                    .attendees
                    .iter_mut()
                    .find(|a| a.email.eq_ignore_ascii_case(sender))
                {
                    attendee.status = reply.status.clone();
                }
            }
            SchedulingMethod::Cancel => {
                if !self.organizer.eq_ignore_ascii_case(sender) {
                    return Ok(Some(event));
                }
                let invitation = event.data.invitation.as_mut().ok_or_else(invalid)?;
                invitation.sequence = self.sequence;
                invitation.status = EventStatus::Cancelled;
            }
        }
        if store.update(workspace, &event.id, &event.data).await? {
            if self.method == SchedulingMethod::Cancel {
                for mut child in linked {
                    let invitation = child.data.invitation.as_mut().ok_or_else(invalid)?;
                    invitation.sequence = self.sequence;
                    invitation.status = EventStatus::Cancelled;
                    store.update(workspace, &child.id, &child.data).await?;
                }
            }
            Ok(Some(event))
        } else {
            store.get(workspace, &event.id).await
        }
    }

    async fn apply_occurrence(
        self,
        store: &dyn CalendarStore,
        workspace: &WorkspaceId,
        own: &str,
        sender: &str,
        previous: Option<CalendarEvent>,
    ) -> Result<Option<CalendarEvent>, StoreError> {
        let Some(mut series) = previous else {
            return Ok(None);
        };
        if self.method == SchedulingMethod::Reply {
            return Ok(Some(series));
        }
        let Some(invitation) = series.data.invitation.as_ref() else {
            return Ok(Some(series));
        };
        if !self.organizer.eq_ignore_ascii_case(sender)
            || !invitation.organizer.eq_ignore_ascii_case(sender)
            || self.sequence < invitation.sequence
            || invitation.status == EventStatus::Cancelled
            || series.data.rrule.is_none()
            || !series
                .data
                .attendees
                .iter()
                .any(|a| a.email.eq_ignore_ascii_case(own))
        {
            return Ok(Some(series));
        }
        let recurrence_id = self.recurrence_id.as_ref().ok_or_else(invalid)?;
        let date = occurrence_local(recurrence_id, &series.data)?;
        let previous = store
            .linked_occurrences(workspace, &self.uid)
            .await?
            .into_iter()
            .find(|e| {
                e.data
                    .invitation
                    .as_ref()
                    .and_then(|i| i.occurrence.as_ref())
                    .is_some_and(|o| &o.recurrence_id == recurrence_id)
            });
        if let Some(child) = &previous {
            let old = child.data.invitation.as_ref().ok_or_else(invalid)?;
            if self.sequence < old.sequence
                || self.method == SchedulingMethod::Request && self.sequence == old.sequence
            {
                return Ok(Some(child.clone()));
            }
        }
        let mut data = if self.method == SchedulingMethod::Request {
            let data = self.data.ok_or_else(invalid)?;
            if !data
                .attendees
                .iter()
                .any(|a| a.email.eq_ignore_ascii_case(own))
            {
                return Ok(Some(series));
            }
            data
        } else if let Some(child) = &previous {
            child.data.clone()
        } else {
            series.data.single_occurrence(&date)?
        };
        data.rrule = None;
        data.exdates.clear();
        for attendee in &mut data.attendees {
            if attendee.email.eq_ignore_ascii_case(own) && self.method == SchedulingMethod::Request
            {
                attendee.status = ParticipationStatus::NeedsAction;
            }
        }
        data.invitation = Some(CalendarInvitation {
            organizer: self.organizer,
            sequence: self.sequence,
            status: if self.method == SchedulingMethod::Cancel {
                EventStatus::Cancelled
            } else {
                EventStatus::Tentative
            },
            occurrence: Some(InvitationOccurrence {
                series_uid: self.uid,
                recurrence_id: recurrence_id.clone(),
            }),
        });
        if !series.data.exdates.contains(&date) {
            series.data.exdates.push(date);
        }
        if !store.update(workspace, &series.id, &series.data).await? {
            return store.get(workspace, &series.id).await;
        }
        let child = if let Some(mut child) = previous {
            if store.update(workspace, &child.id, &data).await? {
                child.data = data;
            }
            child
        } else {
            let child = CalendarEvent::new(workspace.clone(), Author::System, data);
            store.create(&child).await?;
            child
        };
        Ok(Some(child))
    }
}
fn occurrence_local(id: &str, data: &CalendarEventData) -> Result<String, CalendarError> {
    if data.all_day {
        return NaiveDate::parse_from_str(id, "%Y-%m-%d")
            .map(|d| d.to_string())
            .map_err(|_| invalid());
    }
    Ok(chrono::DateTime::parse_from_rfc3339(id)
        .map_err(|_| invalid())?
        .with_timezone(&resolve_zone(&data.timezone)?)
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string())
}

fn time_property(
    key: &str,
    value: &str,
    data: &CalendarEventData,
) -> Result<Property, CalendarError> {
    let mut property = if data.all_day {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid())?;
        let mut property = Property::new(key, date.format("%Y%m%d").to_string());
        property.add_parameter("VALUE", "DATE");
        property
    } else {
        let date =
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").map_err(|_| invalid())?;
        Property::new(key, date.format("%Y%m%dT%H%M%S").to_string())
    };
    if !data.all_day {
        property.add_parameter("TZID", &data.timezone);
    }
    Ok(property)
}
