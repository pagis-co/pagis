//! The Calendar of the Session's Workspace.
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
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

#[utoipa::path(get, path = "/api/v1/calendar/events", params(CalendarRangeQuery), responses((status = 200, body = Vec<CalendarOccurrence>)), tag = "calendar")]
pub async fn list_calendar_events(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Query(range): Query<CalendarRangeQuery>,
) -> Result<Json<Vec<CalendarOccurrence>>, ApiError> {
    let range = pagis_core::CalendarRange::new(range.from, range.to)?;
    Ok(Json(
        state
            .calendar
            .list(&who.workspace_id, range.from(), range.to())
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/calendar/events", request_body = CalendarEventData, responses((status = 201, body = CalendarEvent)), tag = "calendar")]
pub async fn create_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Json(data): Json<CalendarEventData>,
) -> Result<(StatusCode, Json<CalendarEvent>), ApiError> {
    data.bounds()?;
    let event = CalendarEvent::new(who.workspace_id, Author::Person(who.user_id), data);
    state.calendar.create(&event).await?;
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

#[utoipa::path(put, path = "/api/v1/calendar/events/{event_id}", params(("event_id" = String, Path)), request_body = CalendarEventData, responses((status = 200, body = CalendarEvent), (status = 404)), tag = "calendar")]
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
    if !state.calendar.update(&who.workspace_id, &id, &data).await? {
        return Err(ApiError::not_found("Event"));
    }
    event.data = data;
    Ok(Json(event))
}

#[utoipa::path(delete, path = "/api/v1/calendar/events/{event_id}", params(("event_id" = String, Path)), responses((status = 204), (status = 404)), tag = "calendar")]
pub async fn delete_calendar_event(
    State(state): State<Arc<AppState>>,
    who: Tenant,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !state
        .calendar
        .delete(&who.workspace_id, &CalendarEventId::from(id))
        .await?
    {
        return Err(ApiError::not_found("Event"));
    }
    Ok(StatusCode::NO_CONTENT)
}
