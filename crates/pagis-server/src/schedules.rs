use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{
    AgentId, ChannelId, CreatorKind, MessageId, ScheduleId, ScheduleOccurrenceId, WakeupId,
};
use pagis_trigger::{NewSchedule, ScheduleEdit, ScheduleTiming};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 100;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateScheduleRequest {
    pub agent_id: String,
    pub name: String,
    pub instruction: String,
    pub channel_id: String,
    pub root_message_id: Option<String>,
    /// `one_shot` (the default), `cron`, or `interval`.
    pub kind: Option<String>,
    /// Wall-clock input in `YYYY-MM-DDTHH:MM:SS` form for a one-shot Schedule.
    pub local_time: Option<String>,
    /// Five-field cron expression.
    pub cron_expression: Option<String>,
    /// Elapsed interval in minutes. The minimum is one minute.
    pub interval_minutes: Option<i64>,
    /// Explicit Unix-millisecond anchor for an interval.
    pub anchor_at: Option<i64>,
    /// IANA timezone, for example `America/Los_Angeles`.
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateScheduleRequest {
    /// `pause`, `resume`, `skip_next`, `edit`, or `archive`.
    pub action: String,
    pub expected_due_at: Option<i64>,
    pub expected_revision: Option<u32>,
    pub agent_id: Option<String>,
    pub name: Option<String>,
    pub instruction: Option<String>,
    pub channel_id: Option<String>,
    /// The Thread root. Absent keeps the Thread while the Channel stays
    /// the same, and drops it when the Channel changes. `null` clears
    /// the Thread.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub root_message_id: Option<Option<String>>,
    pub kind: Option<String>,
    pub local_time: Option<String>,
    pub cron_expression: Option<String>,
    pub interval_minutes: Option<i64>,
    pub anchor_at: Option<i64>,
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleDto {
    pub id: String,
    pub workspace_id: String,
    pub agent_id: String,
    pub name: String,
    pub instruction: String,
    pub subject_page_path: Option<String>,
    pub channel_id: String,
    pub root_message_id: Option<String>,
    pub kind: String,
    pub cron_expression: Option<String>,
    pub interval_ms: Option<i64>,
    pub anchor_at: Option<i64>,
    pub timezone: String,
    pub scheduled_at: i64,
    pub next_due_at: Option<i64>,
    pub last_result: Option<String>,
    pub state: String,
    pub revision: u32,
    pub approved_revision: Option<u32>,
    pub creator: String,
    pub creating_run_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

impl From<pagis_core::Schedule> for ScheduleDto {
    fn from(value: pagis_core::Schedule) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            agent_id: value.agent_id.to_string(),
            name: value.name,
            instruction: value.instruction,
            subject_page_path: value.subject_page_path,
            channel_id: value.channel_id.to_string(),
            root_message_id: value.root_message_id.map(|id| id.to_string()),
            kind: value.kind.as_str().to_string(),
            cron_expression: value.cron_expression,
            interval_ms: value.interval_ms,
            anchor_at: value.anchor_at,
            timezone: value.timezone,
            scheduled_at: value.scheduled_at,
            next_due_at: value.next_due_at,
            last_result: value.last_result,
            state: value.state.as_str().to_string(),
            revision: value.revision,
            approved_revision: value.approved_revision,
            creator: value.creator.as_str().to_string(),
            creating_run_id: value.creating_run_id.map(|id| id.to_string()),
            created_at: value.created_at,
            updated_at: value.updated_at,
            archived_at: value.archived_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SchedulePage {
    pub items: Vec<ScheduleDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleRevisionDto {
    pub schedule_id: String,
    pub revision: u32,
    pub agent_id: String,
    pub name: String,
    pub instruction: String,
    pub subject_page_path: Option<String>,
    pub channel_id: String,
    pub root_message_id: Option<String>,
    pub kind: String,
    pub cron_expression: Option<String>,
    pub interval_ms: Option<i64>,
    pub anchor_at: Option<i64>,
    pub timezone: String,
    pub scheduled_at: i64,
    pub created_at: i64,
    pub creating_run_id: Option<String>,
}

impl From<pagis_core::ScheduleRevision> for ScheduleRevisionDto {
    fn from(value: pagis_core::ScheduleRevision) -> Self {
        Self {
            schedule_id: value.schedule_id.to_string(),
            revision: value.revision,
            agent_id: value.agent_id.to_string(),
            name: value.name,
            instruction: value.instruction,
            subject_page_path: value.subject_page_path,
            channel_id: value.channel_id.to_string(),
            root_message_id: value.root_message_id.map(|id| id.to_string()),
            kind: value.kind.as_str().to_string(),
            cron_expression: value.cron_expression,
            interval_ms: value.interval_ms,
            anchor_at: value.anchor_at,
            timezone: value.timezone,
            scheduled_at: value.scheduled_at,
            created_at: value.created_at,
            creating_run_id: value.creating_run_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleRevisionPage {
    pub items: Vec<ScheduleRevisionDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleOccurrenceDto {
    pub id: String,
    pub schedule_id: String,
    pub schedule_revision: u32,
    pub scheduled_at: i64,
    pub processed_at: i64,
    pub outcome: String,
    pub wakeup_id: Option<String>,
}

impl From<pagis_core::ScheduleOccurrence> for ScheduleOccurrenceDto {
    fn from(value: pagis_core::ScheduleOccurrence) -> Self {
        Self {
            id: value.id.to_string(),
            schedule_id: value.schedule_id.to_string(),
            schedule_revision: value.schedule_revision,
            scheduled_at: value.scheduled_at,
            processed_at: value.processed_at,
            outcome: value.outcome,
            wakeup_id: value.wakeup_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleOccurrencePage {
    pub items: Vec<ScheduleOccurrenceDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WakeupDto {
    pub id: String,
    /// `schedule`, `event_subscription`, or `arrival`.
    pub source_kind: String,
    /// The rule this Wake-up delivers for.
    pub rule_id: String,
    pub rule_revision: u32,
    pub rule_name: String,
    pub agent_id: String,
    /// The destination for a conversation Run. An arrival has no Channel.
    pub channel_id: Option<String>,
    pub root_message_id: Option<String>,
    pub instruction: String,
    pub scheduled_at: i64,
    pub state: String,
    pub run_id: Option<String>,
    /// How many source occurrences this Wake-up combines.
    pub source_count: u32,
    pub created_at: i64,
    pub started_at: Option<i64>,
}

impl From<pagis_core::Wakeup> for WakeupDto {
    fn from(value: pagis_core::Wakeup) -> Self {
        Self {
            id: value.id.to_string(),
            source_kind: value.rule.kind_str().to_string(),
            rule_id: value.rule.id_str().to_string(),
            rule_revision: value.rule_revision,
            rule_name: value.rule_name,
            agent_id: value.agent_id.to_string(),
            channel_id: value.channel_id.map(|id| id.to_string()),
            root_message_id: value.root_message_id.map(|id| id.to_string()),
            instruction: value.instruction,
            scheduled_at: value.scheduled_at,
            state: value.state.as_str().to_string(),
            run_id: value.run_id.map(|id| id.to_string()),
            source_count: value.source_count,
            created_at: value.created_at,
            started_at: value.started_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WakeupPage {
    pub items: Vec<WakeupDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[utoipa::path(post, path = "/api/v1/schedules", request_body = CreateScheduleRequest, responses(
    (status = 201, body = ScheduleDto),
    (status = 401, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
    (status = 422, body = crate::error::ErrorBody),
))]
pub async fn create_schedule(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduleDto>), ApiError> {
    let timezone = match request.timezone {
        Some(timezone) => timezone,
        None => {
            state
                .workspaces
                .get(&tenant.workspace_id)
                .await?
                .ok_or_else(|| ApiError::not_found("workspace"))?
                .timezone
        }
    };
    let timing = schedule_timing(
        request.kind.as_deref(),
        request.local_time,
        request.cron_expression,
        request.interval_minutes,
        request.anchor_at,
        timezone,
    )?;
    let schedule = state
        .trigger
        .create_schedule(NewSchedule {
            workspace_id: tenant.workspace_id.clone(),
            agent_id: AgentId::from(request.agent_id),
            name: request.name,
            instruction: request.instruction,
            subject_page_path: None,
            channel_id: ChannelId::from(request.channel_id),
            root_message_id: request.root_message_id.map(MessageId::from),
            timing,
            creator: CreatorKind::User,
            creating_run_id: None,
            now: state.clock.now_ms(),
        })
        .await
        .map_err(trigger_error)?;
    Ok((StatusCode::CREATED, Json(schedule.into())))
}

#[utoipa::path(post, path = "/api/v1/schedules/{schedule_id}", request_body = UpdateScheduleRequest, params(("schedule_id" = String, Path)), responses(
    (status = 200, body = ScheduleDto),
    (status = 404, body = crate::error::ErrorBody),
    (status = 409, body = crate::error::ErrorBody),
    (status = 422, body = crate::error::ErrorBody),
))]
pub async fn update_schedule(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleDto>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    let current = owned_schedule(&state, &tenant, id.clone()).await?;
    let at = state.clock.now_ms();
    let schedule = match request.action.as_str() {
        "pause" => {
            state
                .trigger
                .pause_schedule(&tenant.workspace_id, &id, at)
                .await
        }
        "resume" => {
            state
                .trigger
                .resume_schedule(&tenant.workspace_id, &id, at)
                .await
        }
        "archive" => {
            state
                .trigger
                .archive_schedule(&tenant.workspace_id, &id, at)
                .await
        }
        "skip_next" => {
            let expected = request
                .expected_due_at
                .or(current.next_due_at)
                .ok_or_else(|| ApiError::validation("the Schedule has no next occurrence"))?;
            state
                .trigger
                .skip_next(&tenant.workspace_id, &id, expected, at)
                .await
                .map_err(trigger_error)?;
            return Ok(Json(owned_schedule(&state, &tenant, id).await?.into()));
        }
        "edit" => {
            let timing = schedule_timing(
                request.kind.as_deref().or(Some(current.kind.as_str())),
                request.local_time,
                request.cron_expression.or(current.cron_expression.clone()),
                request
                    .interval_minutes
                    .or(current.interval_ms.map(|value| value / 60_000)),
                request.anchor_at.or(current.anchor_at),
                request.timezone.unwrap_or_else(|| current.timezone.clone()),
            )?;
            state
                .trigger
                .edit_schedule(
                    &tenant.workspace_id,
                    &id,
                    ScheduleEdit {
                        expected_revision: request.expected_revision.unwrap_or(current.revision),
                        agent_id: request
                            .agent_id
                            .map(AgentId::from)
                            .unwrap_or(current.agent_id),
                        name: request.name.unwrap_or(current.name),
                        instruction: request.instruction.unwrap_or(current.instruction),
                        channel_id: request
                            .channel_id
                            .map(ChannelId::from)
                            .unwrap_or(current.channel_id),
                        root_message_id: request
                            .root_message_id
                            .map(|root| root.map(MessageId::from)),
                        timing,
                    },
                    at,
                )
                .await
        }
        _ => return Err(ApiError::validation("unknown Schedule action")),
    }
    .map_err(trigger_error)?
    .ok_or_else(|| ApiError::not_found("schedule"))?;
    Ok(Json(schedule.into()))
}

#[utoipa::path(get, path = "/api/v1/schedules", params(PageQuery), responses(
    (status = 200, body = SchedulePage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_schedules(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<PageQuery>,
) -> Result<Json<SchedulePage>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let schedules = state
        .trigger
        .list_schedules(
            &tenant.workspace_id,
            query.before.map(ScheduleId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(schedules, limit, |item| item.id.to_string());
    Ok(Json(SchedulePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/schedules/{schedule_id}", params(("schedule_id" = String, Path)), responses(
    (status = 200, body = ScheduleDto),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn get_schedule(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
) -> Result<Json<ScheduleDto>, ApiError> {
    let schedule = owned_schedule(&state, &tenant, ScheduleId::from(schedule_id)).await?;
    Ok(Json(schedule.into()))
}

#[utoipa::path(get, path = "/api/v1/schedules/{schedule_id}/revisions", params(("schedule_id" = String, Path)), responses(
    (status = 200, body = ScheduleRevisionPage),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn list_revisions(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
) -> Result<Json<ScheduleRevisionPage>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    owned_schedule(&state, &tenant, id.clone()).await?;
    let items = state
        .trigger
        .list_revisions(&tenant.workspace_id, &id)
        .await
        .map_err(trigger_error)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(ScheduleRevisionPage { items }))
}

#[utoipa::path(post, path = "/api/v1/schedules/{schedule_id}/archive", params(("schedule_id" = String, Path)), responses(
    (status = 200, body = ScheduleDto),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn archive_schedule(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
) -> Result<Json<ScheduleDto>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    owned_schedule(&state, &tenant, id.clone()).await?;
    let schedule = state
        .trigger
        .archive_schedule(&tenant.workspace_id, &id, state.clock.now_ms())
        .await
        .map_err(trigger_error)?
        .ok_or_else(|| ApiError::not_found("schedule"))?;
    Ok(Json(schedule.into()))
}

/// Wake a Schedule now, ahead of its cadence (ADR-0022). Home
/// asks for the Report this way. The call is idempotent while a
/// Wake-up waits: it returns the one that already waits.
#[utoipa::path(post, path = "/api/v1/schedules/{schedule_id}/run", params(("schedule_id" = String, Path)), responses(
    (status = 200, body = WakeupDto),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn run_schedule_now(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
) -> Result<Json<WakeupDto>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    owned_schedule(&state, &tenant, id.clone()).await?;
    let wakeup = state
        .trigger
        .run_now(&tenant.workspace_id, &id, state.clock.now_ms())
        .await
        .map_err(trigger_error)?
        .ok_or_else(|| ApiError::not_found("schedule"))?;
    Ok(Json(wakeup.into()))
}

#[utoipa::path(get, path = "/api/v1/schedules/{schedule_id}/occurrences", params(("schedule_id" = String, Path), PageQuery), responses(
    (status = 200, body = ScheduleOccurrencePage),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn list_occurrences(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<ScheduleOccurrencePage>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    owned_schedule(&state, &tenant, id.clone()).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let values = state
        .trigger
        .list_occurrences(
            &tenant.workspace_id,
            &id,
            query.before.map(ScheduleOccurrenceId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(values, limit, |item| item.id.to_string());
    Ok(Json(ScheduleOccurrencePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/schedules/{schedule_id}/wakeups", params(("schedule_id" = String, Path), PageQuery), responses(
    (status = 200, body = WakeupPage),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn list_wakeups(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(schedule_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<WakeupPage>, ApiError> {
    let id = ScheduleId::from(schedule_id);
    owned_schedule(&state, &tenant, id.clone()).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let values = state
        .trigger
        .list_wakeups(
            &tenant.workspace_id,
            &id,
            query.before.map(WakeupId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(values, limit, |item| item.id.to_string());
    Ok(Json(WakeupPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// Move one active cron Schedule to another timezone, and keep its
/// cadence. The next occurrence follows the new timezone. A Schedule
/// whose Agent is archived does not move: it does not wake again, and
/// due processing blocks it.
pub(crate) async fn move_to_timezone(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    id: &ScheduleId,
    timezone: &str,
) -> Result<(), ApiError> {
    let Some(current) = state
        .trigger
        .get_schedule(workspace_id, id)
        .await
        .map_err(trigger_error)?
    else {
        return Ok(());
    };
    if current.timezone == timezone || current.state != pagis_core::ScheduleState::Active {
        return Ok(());
    }
    let Some(expression) = current.cron_expression.clone() else {
        return Ok(());
    };
    let moved = state
        .trigger
        .edit_schedule(
            workspace_id,
            id,
            ScheduleEdit {
                expected_revision: current.revision,
                agent_id: current.agent_id,
                name: current.name,
                instruction: current.instruction,
                channel_id: current.channel_id,
                root_message_id: None,
                timing: ScheduleTiming::Cron {
                    expression,
                    timezone: timezone.to_string(),
                },
            },
            state.clock.now_ms(),
        )
        .await;
    match moved {
        Ok(_) | Err(pagis_trigger::TriggerError::AgentNotFound) => Ok(()),
        Err(error) => Err(trigger_error(error)),
    }
}

async fn owned_schedule(
    state: &AppState,
    tenant: &Tenant,
    id: ScheduleId,
) -> Result<pagis_core::Schedule, ApiError> {
    state
        .trigger
        .get_schedule(&tenant.workspace_id, &id)
        .await
        .map_err(trigger_error)?
        .ok_or_else(|| ApiError::not_found("schedule"))
}

pub(crate) fn page<T>(
    mut items: Vec<T>,
    limit: u32,
    id: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    let has_more = items.len() > limit as usize;
    if has_more {
        items.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| id(items.last().expect("non-empty page")));
    (items, next_cursor)
}

/// A request field that tells an absent value from `null`: absent is
/// `None`, `null` is `Some(None)`, and a value is `Some(Some(value))`.
/// JSON Merge Patch (RFC 7396) reads a field the same way.
pub(crate) fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(crate) fn trigger_error(error: pagis_trigger::TriggerError) -> ApiError {
    match error {
        // A target of another Workspace reads the same as a missing
        // one, so the answer names what is missing and no more.
        pagis_trigger::TriggerError::AgentNotFound => ApiError::not_found("agent"),
        pagis_trigger::TriggerError::ChannelNotFound => ApiError::not_found("channel"),
        pagis_trigger::TriggerError::ThreadRootNotFound => ApiError::not_found("thread root"),
        // A lost race is the caller's news, not an internal failure:
        // skip-next answers 409 when due processing won.
        pagis_trigger::TriggerError::Store(pagis_core::StoreError::Conflict(message)) => {
            ApiError::conflict(message)
        }
        pagis_trigger::TriggerError::Store(error) => error.into(),
        other => ApiError::validation(other.to_string()),
    }
}

fn schedule_timing(
    kind: Option<&str>,
    local_time: Option<String>,
    cron_expression: Option<String>,
    interval_minutes: Option<i64>,
    anchor_at: Option<i64>,
    timezone: String,
) -> Result<ScheduleTiming, ApiError> {
    match kind.unwrap_or("one_shot") {
        "one_shot" => Ok(ScheduleTiming::OneShot {
            local_time: local_time.ok_or_else(|| ApiError::validation("local_time is required"))?,
            timezone,
        }),
        "cron" => Ok(ScheduleTiming::Cron {
            expression: cron_expression
                .ok_or_else(|| ApiError::validation("cron_expression is required"))?,
            timezone,
        }),
        "interval" => Ok(ScheduleTiming::Interval {
            every_ms: interval_minutes
                .ok_or_else(|| ApiError::validation("interval_minutes is required"))?
                .saturating_mul(60_000),
            anchor: anchor_at.ok_or_else(|| ApiError::validation("anchor_at is required"))?,
        }),
        _ => Err(ApiError::validation(
            "kind must be one_shot, cron, or interval",
        )),
    }
}
