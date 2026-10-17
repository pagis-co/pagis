//! The Calendar of the Session's Workspace.
use crate::{AppState, auth::Tenant, calendar_mail, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use pagis_core::imip::SchedulingMethod;
use pagis_core::{Author, CalendarEvent, CalendarEventData, CalendarEventId, CalendarOccurrence};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::IntoParams;

#[derive(Deserialize, IntoParams)]
pub struct CalendarRangeQuery {
    /// Inclusive UTC Unix milliseconds. Occurrences that overlap the range are returned.
    pub from: i64,
    /// Exclusive UTC Unix milliseconds. The answer holds at most 1000 occurrences.
    pub to: i64,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct CalendarOccurrenceDto {
    #[serde(flatten)]
    pub occurrence: CalendarOccurrence,
    pub workspace_name: Option<String>,
    pub read_only: bool,
}

#[utoipa::path(get, path = "/api/v1/calendar/events", params(CalendarRangeQuery), responses((status = 200, body = Vec<CalendarOccurrenceDto>)), tag = "calendar")]
pub async fn list_calendar_events(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Query(range): Query<CalendarRangeQuery>,
) -> Result<Json<Vec<CalendarOccurrenceDto>>, ApiError> {
    let range = pagis_core::CalendarRange::new(range.from, range.to)?;
    state
        .mailbox_desk
        .poll_workspace_calendar(&who.workspace_id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?;
    let mut events: Vec<_> = state
        .calendar
        .list(&who.workspace_id, range.from(), range.to())
        .await?
        .into_iter()
        .map(|occurrence| CalendarOccurrenceDto {
            occurrence,
            workspace_name: None,
            read_only: who.workspace_role == Some(pagis_core::WorkspaceRole::Reader),
        })
        .collect();
    if who.is_private() {
        for (owner, rows) in pagis_core::calendar_access::member_events(
            state.calendar.as_ref(),
            state.workspaces.as_ref(),
            state.workspace_members.as_ref(),
            state.users.as_ref(),
            &who.workspace_id,
            range,
        )
        .await?
        {
            events.extend(
                pagis_core::calendar::occurrences(rows, range)?
                    .into_iter()
                    .map(|occurrence| CalendarOccurrenceDto {
                        occurrence,
                        workspace_name: Some(owner.name.clone()),
                        read_only: true,
                    }),
            );
        }
    }
    events.sort_by_cached_key(|value| {
        (
            value.occurrence.start_ms().expect("validated occurrence"),
            value.occurrence.event.id.clone(),
        )
    });
    events.truncate(pagis_core::calendar::MAX_OCCURRENCES);
    Ok(Json(events))
}

#[utoipa::path(post, path = "/api/v1/calendar/events", request_body = CalendarEventData, responses((status = 201, body = CalendarEvent)), tag = "calendar")]
pub async fn create_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Json(data): Json<CalendarEventData>,
) -> Result<(StatusCode, Json<CalendarEvent>), ApiError> {
    data.bounds()?;
    let mut event = CalendarEvent::new(
        who.workspace_id.clone(),
        Author::Person(who.user_id.clone()),
        data,
    );
    let recipients = calendar_mail::prepare(&state, &who, &mut event, None).await?;
    state.calendar.create(&event).await?;
    calendar_mail::send(&state, &who, &event, SchedulingMethod::Request, recipients).await?;
    Ok((StatusCode::CREATED, Json(event)))
}

#[utoipa::path(get, path = "/api/v1/calendar/events/{event_id}", params(("event_id" = String, Path)), responses((status = 200, body = CalendarEvent), (status = 404)), tag = "calendar")]
pub async fn get_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Path(id): Path<String>,
) -> Result<Json<CalendarEvent>, ApiError> {
    Ok(Json(
        state
            .calendar
            .get(&who.workspace_id, &CalendarEventId::from(id))
            .await?
            .ok_or_else(|| ApiError::not_found("Event"))?,
    ))
}

#[utoipa::path(put, path = "/api/v1/calendar/events/{event_id}", params(("event_id" = String, Path)), request_body = CalendarEventData, responses((status = 200, body = CalendarEvent), (status = 404), (status = 409, body = crate::error::ErrorBody)), tag = "calendar")]
pub async fn update_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Path(id): Path<String>,
    Json(data): Json<CalendarEventData>,
) -> Result<Json<CalendarEvent>, ApiError> {
    let id = CalendarEventId::from(id);
    let mut event = state
        .calendar
        .get(&who.workspace_id, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("Event"))?;
    data.bounds()?;
    let previous = event.clone();
    event.data = data;
    let recipients = calendar_mail::prepare(&state, &who, &mut event, Some(&previous)).await?;
    if !state
        .calendar
        .update(&who.workspace_id, &id, &event.data)
        .await?
    {
        return Err(ApiError::conflict("The Event changed. Open it again."));
    }
    calendar_mail::send(&state, &who, &event, SchedulingMethod::Request, recipients).await?;
    let removed: Vec<_> = calendar_mail::outside_attendees(&state, &who, &previous)
        .await?
        .into_iter()
        .filter(|email| {
            !event
                .data
                .attendees
                .iter()
                .any(|a| a.email.eq_ignore_ascii_case(email))
        })
        .collect();
    if !removed.is_empty() {
        let mut cancellation = previous;
        cancellation.data.invitation = event.data.invitation.clone();
        cancellation.data.attendees.retain(|a| {
            removed
                .iter()
                .any(|email| a.email.eq_ignore_ascii_case(email))
        });
        calendar_mail::send(
            &state,
            &who,
            &cancellation,
            SchedulingMethod::Cancel,
            removed,
        )
        .await?;
    }
    Ok(Json(event))
}

#[utoipa::path(delete, path = "/api/v1/calendar/events/{event_id}", params(("event_id" = String, Path)), responses((status = 204), (status = 404), (status = 409, body = crate::error::ErrorBody)), tag = "calendar")]
pub async fn delete_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.workspaces.refuse_delete(&who.workspace_id).await?;
    let id = CalendarEventId::from(id);
    let mut event = state
        .calendar
        .get(&who.workspace_id, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("Event"))?;
    if event.data.invitation.is_some() {
        let previous = event.clone();
        let recipients = calendar_mail::prepare(&state, &who, &mut event, Some(&previous)).await?;
        event.data.invitation.as_mut().expect("invitation").status =
            pagis_core::calendar::EventStatus::Cancelled;
        state
            .calendar
            .update(&who.workspace_id, &id, &event.data)
            .await?;
        calendar_mail::send(&state, &who, &event, SchedulingMethod::Cancel, recipients).await?;
    } else if !state.calendar.delete(&who.workspace_id, &id).await? {
        return Err(ApiError::not_found("Event"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct FreeBusyRequest {
    /// Inclusive RFC 3339 start. The range is at most 62 days.
    pub from: String,
    /// Exclusive RFC 3339 end.
    pub to: String,
    /// At most 50 Person login or Workspace Mailbox addresses.
    pub addresses: Vec<String>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FreeBusyInterval {
    pub start: String,
    pub end: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FreeBusyItem {
    pub address: String,
    pub busy: Vec<FreeBusyInterval>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FreeBusyResponse {
    pub items: Vec<FreeBusyItem>,
}

/// Org members read availability without access to Event details.
#[utoipa::path(post, path = "/api/v1/free-busy", request_body = FreeBusyRequest,
    responses((status = 200, body = FreeBusyResponse), (status = 401), (status = 422)), tag = "calendar")]
pub async fn query_free_busy(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Json(body): Json<FreeBusyRequest>,
) -> Result<Json<FreeBusyResponse>, ApiError> {
    let range = pagis_core::CalendarRange::parse(&body.from, &body.to)?;
    if range.to() - range.from() > 62 * 86_400_000 || body.addresses.len() > 50 {
        return Err(ApiError::validation(
            "Use at most 62 days and 50 addresses.",
        ));
    }
    let person = state
        .users
        .get(&who.user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let people = state.users.list_by_org(&person.org_id).await?;
    let mut addresses = std::collections::HashMap::new();
    for workspace in state.workspaces.list().await? {
        if workspace.kind == pagis_core::WorkspaceKind::Private {
            let Some(person) = people
                .iter()
                .find(|p| Some(&p.id) == workspace.user_id.as_ref() && !p.is_disabled())
            else {
                continue;
            };
            if let Some(email) = &person.email {
                addresses.insert(email.to_lowercase(), workspace.id.clone());
            }
        }
        if let Some(mailbox) = state
            .mailbox_desk
            .workspace_mailbox(&workspace.id)
            .await
            .map_err(crate::mailboxes::mailbox_error)?
        {
            addresses.insert(mailbox.address.to_lowercase(), workspace.id);
        }
    }
    let mut items = Vec::new();
    for address in body.addresses {
        let Some(workspace) = addresses.get(&address.trim().to_lowercase()) else {
            continue;
        };
        let busy = pagis_core::calendar_access::free_busy(
            state.calendar.as_ref(),
            state.workspaces.as_ref(),
            state.workspace_members.as_ref(),
            state.users.as_ref(),
            workspace,
            range,
        )
        .await?
        .into_iter()
        .map(|interval| {
            let (start, end) = interval.rfc3339();
            FreeBusyInterval { start, end }
        })
        .collect();
        items.push(FreeBusyItem { address, busy });
    }
    Ok(Json(FreeBusyResponse { items }))
}
