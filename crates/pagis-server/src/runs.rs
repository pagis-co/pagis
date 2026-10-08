//! Runs REST: cancel one run, dismiss one from the Needs-You Queue,
//! list runs, read a run's events and its steps.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_agent::CancelOutcome;
use pagis_core::{AgentId, ChannelId, PendingEvidenceId, Run, RunId, RunState, TriggerKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::run_steps::{RunStepsDto, fold_steps};

#[derive(Debug, Default, Deserialize)]
pub struct RunListQuery {
    pub agent_id: Option<String>,
    pub channel_id: Option<String>,
    /// One run state, or several separated by a comma.
    pub state: Option<String>,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStateDto {
    Queued,
    Running,
    Reflecting,
    WaitingForUser,
    WaitingForApproval,
    Completed,
    Failed,
    Canceled,
}

impl From<RunState> for RunStateDto {
    fn from(state: RunState) -> Self {
        match state {
            RunState::Queued => Self::Queued,
            RunState::Running => Self::Running,
            RunState::Reflecting => Self::Reflecting,
            RunState::WaitingForUser => Self::WaitingForUser,
            RunState::WaitingForApproval => Self::WaitingForApproval,
            RunState::Completed => Self::Completed,
            RunState::Failed => Self::Failed,
            RunState::Canceled => Self::Canceled,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunDto {
    pub id: String,
    pub title: String,
    pub agent_id: String,
    pub channel_id: Option<String>,
    pub root_message_id: Option<String>,
    pub trigger_kind: String,
    pub trigger_ref: Option<String>,
    /// The count of agent-to-agent hops behind this run.
    pub hop_count: u32,
    /// The channel the delegation chain owes its answer to.
    /// The Desk Panel reads it to know which Agent works on a
    /// Delegation the Chief of Staff started (ADR-0022).
    pub origin_channel_id: Option<String>,
    pub state: RunStateDto,
    pub failure_kind: Option<pagis_core::FailureKind>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub created_at: i64,
    pub duration_ms: Option<i64>,
    /// When the Person dismissed the Run from the Needs-You Queue.
    pub dismissed_at: Option<i64>,
}

impl From<Run> for RunDto {
    fn from(run: Run) -> Self {
        let duration_ms = run
            .started_at
            .zip(run.ended_at)
            .map(|(started, ended)| ended - started);
        Self {
            id: run.id.to_string(),
            title: run.title,
            agent_id: run.agent_id.to_string(),
            channel_id: run.channel_id.map(|id| id.to_string()),
            root_message_id: run.root_message_id.map(|id| id.to_string()),
            trigger_kind: run.trigger_kind.as_str().to_string(),
            trigger_ref: run.trigger_ref,
            hop_count: run.hop_count,
            origin_channel_id: run
                .origin
                .as_ref()
                .map(|origin| origin.channel_id.to_string()),
            state: run.state.into(),
            failure_kind: run.failure_kind,
            error: run.error,
            started_at: run.started_at,
            ended_at: run.ended_at,
            created_at: run.created_at,
            duration_ms,
            dismissed_at: run.dismissed_at,
        }
    }
}

/// A title derived from a message obeys that message's current source access.
async fn readable_run(app: &AppState, mut run: Run) -> Result<RunDto, ApiError> {
    if run.trigger_kind == TriggerKind::Message
        && let Some(reference) = &run.trigger_ref
    {
        let message = app
            .messages
            .get(
                &run.workspace_id,
                &pagis_core::MessageId::from(reference.clone()),
            )
            .await?;
        let readable = match message {
            Some(message) => crate::channels::message_is_readable(app, &message).await?,
            None => false,
        };
        if !readable {
            run.title = "This message is unavailable".into();
        }
    }
    Ok(run.into())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunPage {
    pub items: Vec<RunDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunEventDto {
    pub id: String,
    pub event_type: String,
    pub agent_id: Option<String>,
    pub channel_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Default, Serialize, ToSchema)]
pub struct RunUsageDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunTranscriptDto {
    pub run: RunDto,
    pub usage: RunUsageDto,
    pub events: Vec<RunEventDto>,
}

#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/events",
    params(("run_id" = String, Path)),
    responses(
        (status = 200, body = RunTranscriptDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn run_events(
    State(app): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<Json<RunTranscriptDto>, ApiError> {
    let run_id = RunId::from(run_id);
    let run = app
        .runs
        .get(&tenant.workspace_id, &run_id)
        .await?
        .ok_or_else(|| ApiError::not_found("run"))?;
    let stored = app
        .events
        .list_for_run(&tenant.workspace_id, &run_id)
        .await?;
    let mut usage = RunUsageDto::default();
    for event in &stored {
        if event.event_type == "turn.completed" {
            usage.input_tokens += event.payload["input_tokens"].as_u64().unwrap_or_default();
            usage.output_tokens += event.payload["output_tokens"].as_u64().unwrap_or_default();
        }
    }
    let events = stored
        .into_iter()
        .map(|event| RunEventDto {
            id: event.id.to_string(),
            event_type: event.event_type,
            agent_id: event.agent_id.map(|id| id.to_string()),
            channel_id: event.channel_id.map(|id| id.to_string()),
            payload: event.payload,
            created_at: event.created_at,
        })
        .collect();
    Ok(Json(RunTranscriptDto {
        run: readable_run(&app, run).await?,
        usage,
        events,
    }))
}

/// One Model Request Capture of a Run (ADR-0031).
#[derive(Debug, Serialize, ToSchema)]
pub struct ModelRequestCaptureDto {
    pub id: String,
    /// `reply`, `compaction` or `reflection`, as on `model.requested`.
    pub phase: String,
    /// The number of the request inside its phase, from 0.
    pub phase_request: i64,
    /// The request as the Agent sent it, after Compaction. Each image is
    /// its media type, its sizes and its SHA-256.
    pub request: serde_json::Value,
    /// The outcome, and the usage or the provider's status and error body.
    pub answer: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelRequestCapturesDto {
    pub items: Vec<ModelRequestCaptureDto>,
}

#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/model-requests",
    params(("run_id" = String, Path)),
    responses(
        (status = 200, body = ModelRequestCapturesDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, description = "No such Run, or the reader is not an Administrator", body = crate::error::ErrorBody),
    )
)]
/// The Model Request Captures of a Run, in the order the Run made the
/// requests. A capture holds Person data, so only an Administrator reads
/// it. A Member gets `404`, as for a Run that does not exist (ADR-0031).
pub async fn model_requests(
    State(app): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<Json<ModelRequestCapturesDto>, ApiError> {
    if tenant.role != pagis_core::UserRole::Administrator {
        return Err(ApiError::not_found("run"));
    }
    let run_id = RunId::from(run_id);
    app.runs
        .get(&tenant.workspace_id, &run_id)
        .await?
        .ok_or_else(|| ApiError::not_found("run"))?;
    let items = app
        .model_request_captures
        .list_for_run(&tenant.workspace_id, &run_id)
        .await?
        .into_iter()
        .map(|capture| ModelRequestCaptureDto {
            id: capture.id.to_string(),
            phase: capture.phase,
            phase_request: capture.phase_request,
            request: capture.request,
            answer: capture.answer,
            created_at: capture.created_at,
        })
        .collect();
    Ok(Json(ModelRequestCapturesDto { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/steps",
    params(("run_id" = String, Path)),
    responses(
        (status = 200, body = RunStepsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn run_steps(
    State(app): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<Json<RunStepsDto>, ApiError> {
    let run_id = RunId::from(run_id);
    let run = app
        .runs
        .get(&tenant.workspace_id, &run_id)
        .await?
        .ok_or_else(|| ApiError::not_found("run"))?;
    let events = app
        .events
        .list_for_run(&tenant.workspace_id, &run_id)
        .await?;
    Ok(Json(fold_steps(&run, &events)))
}

#[utoipa::path(
    get,
    path = "/api/v1/runs",
    params(
        ("agent_id" = Option<String>, Query),
        ("channel_id" = Option<String>, Query),
        ("state" = Option<String>, Query, description = "One run state, or several separated by a comma"),
        ("before" = Option<String>, Query),
        ("limit" = Option<u32>, Query),
    ),
    responses(
        (status = 200, body = RunPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn list_runs(
    State(app): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<RunListQuery>,
) -> Result<Json<RunPage>, ApiError> {
    // `state` carries a comma-separated set; an empty set keeps every
    // state.
    let states = query
        .state
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::parse::<RunState>)
        .collect::<Result<Vec<RunState>, _>>()
        .map_err(ApiError::validation)?;
    let agent_id = query.agent_id.map(AgentId::from);
    let channel_id = query.channel_id.map(ChannelId::from);
    let before = query.before.map(RunId::from);
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let runs = app
        .runs
        .list(
            &tenant.workspace_id,
            agent_id.as_ref(),
            channel_id.as_ref(),
            &states,
            before.as_ref(),
            limit,
        )
        .await?;
    let mut items = Vec::with_capacity(runs.len());
    for run in runs {
        items.push(readable_run(&app, run).await?);
    }
    Ok(Json(RunPage { items }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunCancelResponse {
    pub run_id: String,
    /// `canceling`, or `already_ended` when the run was terminal.
    pub outcome: String,
    /// The terminal state, present with `already_ended`.
    pub state: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/cancel",
    params(("run_id" = String, Path,)),
    responses(
        (status = 202, description = "Cancellation accepted", body = RunCancelResponse),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn cancel_run(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<(StatusCode, Json<RunCancelResponse>), ApiError> {
    let run_id = RunId::from(run_id);
    match state.agents.cancel(&tenant.workspace_id, &run_id).await {
        CancelOutcome::Canceling => Ok((
            StatusCode::ACCEPTED,
            Json(RunCancelResponse {
                run_id: run_id.to_string(),
                outcome: "canceling".to_string(),
                state: None,
            }),
        )),
        CancelOutcome::AlreadyEnded(run_state) => Ok((
            StatusCode::ACCEPTED,
            Json(RunCancelResponse {
                run_id: run_id.to_string(),
                outcome: "already_ended".to_string(),
                state: Some(run_state.as_str().to_string()),
            }),
        )),
        CancelOutcome::NotFound => Err(ApiError::not_found("run")),
    }
}

/// The Person dismisses a Run from the Needs-You Queue. The Run keeps
/// the time, so the queue leaves it out on every client and after a
/// reload. A second dismissal keeps the first time.
#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/dismiss",
    params(("run_id" = String, Path,)),
    responses(
        (status = 204, description = "The Run is out of the Needs-You Queue"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn dismiss_run(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let run_id = RunId::from(run_id);
    if !state
        .runs
        .dismiss(&tenant.workspace_id, &run_id, state.clock.now_ms())
        .await?
    {
        return Err(ApiError::not_found("run"));
    }
    // Every client of the Person drops the queue item.
    let _ = state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "run.dismissed".into(),
            agent_id: None,
            run_id: Some(run_id),
            channel_id: None,
            payload: serde_json::json!({}),
        })
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RunRetryResponse {
    pub run_id: String,
    pub outcome: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/retry",
    params(("run_id" = String, Path,)),
    responses(
        (status = 202, description = "Pending review queued again", body = RunRetryResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn retry_review(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(run_id): Path<String>,
) -> Result<(StatusCode, Json<RunRetryResponse>), ApiError> {
    let run_id = RunId::from(run_id);
    let run = state
        .runs
        .get(&tenant.workspace_id, &run_id)
        .await?
        .ok_or_else(|| ApiError::not_found("run"))?;
    if run.trigger_kind != TriggerKind::Review || run.state != RunState::Failed {
        return Err(ApiError::conflict(
            "only a failed memory review can be retried",
        ));
    }
    let id = run
        .trigger_ref
        .clone()
        .map(PendingEvidenceId::from)
        .ok_or_else(|| ApiError::conflict("the review has no pending evidence"))?;
    if !state
        .pending_evidence
        .retry(&tenant.workspace_id, &id, state.clock.now_ms())
        .await?
    {
        return Err(ApiError::conflict("the pending review is not retriable"));
    }
    let _ = state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "memory.review_retry".into(),
            agent_id: Some(run.agent_id),
            run_id: Some(run_id.clone()),
            channel_id: run.channel_id,
            payload: serde_json::json!({ "pending_id": id.as_str() }),
        })
        .await;
    Ok((
        StatusCode::ACCEPTED,
        Json(RunRetryResponse {
            run_id: run_id.to_string(),
            outcome: "queued".into(),
        }),
    ))
}
