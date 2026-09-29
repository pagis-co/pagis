//! The Workspace and its Chief of Staff (ADR-0022).
//!
//! One active Agent is the Chief of Staff. The shell reads it on load
//! and on `workspace.updated`, and the user moves the designation from
//! the Roster. The setting grants nothing: the Roster stays flat
//! and the Chief of Staff delegates through the same channels as any
//! Agent.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use pagis_core::{Agent, AgentId, AgentStatus, NewEvent, ScheduleId, Workspace};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// The Workspace the shell reads once at start.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceDto {
    pub id: String,
    pub name: String,
    /// The IANA timezone new wall-clock schedules copy.
    pub timezone: String,
    /// The Chief of Staff; `null` when no Agent is active.
    pub chief_of_staff_agent_id: Option<String>,
    /// The Schedule that writes the Report for Home (ADR-0022).
    pub report_schedule_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetChiefOfStaffRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetTimezoneRequest {
    /// An IANA timezone, for example `America/Los_Angeles`.
    pub timezone: String,
}

/// An IANA timezone as the daemon keeps it, or a refusal that names it.
pub(crate) fn checked_timezone(raw: &str) -> Result<String, ApiError> {
    let timezone = raw.trim();
    if timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(ApiError::validation(format!(
            "{timezone} is not an IANA timezone"
        )));
    }
    Ok(timezone.to_string())
}

/// Make `timezone` the Person's own: the Workspace timezone, which new
/// wall-clock Schedules copy, and the timezone of the Report Schedule,
/// whose "morning" is the Person's morning. The Report moves only while
/// it is active: a paused Report keeps what the Person left.
pub(crate) async fn set_timezone(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    timezone: &str,
) -> Result<(), ApiError> {
    let workspace = state
        .workspaces
        .get(workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    state
        .workspaces
        .set_timezone(workspace_id, timezone)
        .await?;
    let Some(report_id) = workspace.report_schedule_id else {
        return Ok(());
    };
    crate::schedules::move_to_timezone(state, workspace_id, &report_id, timezone).await
}

/// The timezone a browser or a Client App reports at a Person's first
/// sign-in, which becomes theirs. A value the daemon cannot read is
/// left out: the sign-in goes on, and the Person sets it in Settings.
pub(crate) async fn take_first_timezone(
    state: &AppState,
    user_id: &pagis_core::UserId,
    reported: Option<&str>,
) -> Result<(), ApiError> {
    let Some(timezone) = reported.and_then(|raw| checked_timezone(raw).ok()) else {
        return Ok(());
    };
    let Some(workspace) = state.workspaces.for_user(user_id).await? else {
        return Ok(());
    };
    set_timezone(state, &workspace.id, &timezone).await
}

fn workspace_dto(workspace: Workspace) -> WorkspaceDto {
    WorkspaceDto {
        id: workspace.id.as_str().to_string(),
        name: workspace.name,
        timezone: workspace.timezone,
        chief_of_staff_agent_id: workspace
            .chief_of_staff_agent_id
            .map(|id| id.as_str().to_string()),
        report_schedule_id: workspace
            .report_schedule_id
            .map(|id| id.as_str().to_string()),
    }
}

async fn load(state: &AppState, tenant: &Tenant) -> Result<Workspace, ApiError> {
    state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))
}

/// The active Agents of the Workspace, oldest first. ADR-0022 names the
/// oldest active Agent when the designation must move by itself.
pub async fn active_agents_oldest_first(
    state: &AppState,
    tenant: &Tenant,
) -> Result<Vec<Agent>, ApiError> {
    let mut agents = state
        .agent_store
        .list_by_workspace(&tenant.workspace_id)
        .await?;
    agents.retain(|agent| agent.status == AgentStatus::Active);
    agents.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    Ok(agents)
}

/// Write the designation and tell the shell (ADR-0022).
pub async fn designate(
    state: &AppState,
    tenant: &Tenant,
    agent_id: Option<&AgentId>,
) -> Result<(), ApiError> {
    state
        .workspaces
        .set_chief_of_staff(&tenant.workspace_id, agent_id)
        .await?;
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "workspace.updated".to_string(),
            agent_id: agent_id.cloned(),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "chief_of_staff_agent_id": agent_id.map(AgentId::as_str),
            }),
        })
        .await?;
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace",
    responses(
        (status = 200, body = WorkspaceDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<WorkspaceDto>, ApiError> {
    Ok(Json(workspace_dto(load(&state, &tenant).await?)))
}

#[utoipa::path(
    put,
    path = "/api/v1/workspace/chief-of-staff",
    request_body = SetChiefOfStaffRequest,
    responses(
        (status = 200, body = WorkspaceDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_chief_of_staff(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SetChiefOfStaffRequest>,
) -> Result<Json<WorkspaceDto>, ApiError> {
    let agent_id = AgentId::from(request.agent_id);
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    if agent.status != AgentStatus::Active {
        return Err(ApiError::validation(
            "an archived agent cannot be the chief of staff",
        ));
    }
    designate(&state, &tenant, Some(&agent.id)).await?;
    Ok(Json(workspace_dto(load(&state, &tenant).await?)))
}

#[utoipa::path(
    put,
    path = "/api/v1/workspace/timezone",
    request_body = SetTimezoneRequest,
    responses(
        (status = 200, body = WorkspaceDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The Person's own timezone, from their Settings.
pub async fn put_timezone(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SetTimezoneRequest>,
) -> Result<Json<WorkspaceDto>, ApiError> {
    let timezone = checked_timezone(&request.timezone)?;
    set_timezone(&state, &tenant.workspace_id, &timezone).await?;
    Ok(Json(workspace_dto(load(&state, &tenant).await?)))
}

/// The Report that Home reads (ADR-0022): the newest message the Report
/// Schedule wrote, and the state of the Report that runs now.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReportDto {
    /// The Schedule behind the Report; `null` when the Workspace has none.
    pub schedule_id: Option<String>,
    /// When the next Report is due.
    pub next_due_at: Option<i64>,
    /// The newest written Report; `null` before the first one.
    pub message: Option<crate::channels::MessageDto>,
    /// The Run writing a Report now; `null` when none writes.
    pub writing_run_id: Option<String>,
}

/// Walk the Wake-ups of the Report Schedule, newest first, and read the
/// Report out of each one's Run. A Schedule Run speaks at the top level
/// of its channel, so the Report is the last complete Agent message
/// that Run wrote.
async fn latest_report(
    state: &AppState,
    tenant: &Tenant,
    schedule_id: &ScheduleId,
) -> Result<(Option<crate::channels::MessageDto>, Option<String>), ApiError> {
    let wakeups = state
        .trigger
        .list_wakeups(&tenant.workspace_id, schedule_id, None, REPORT_LOOKBACK)
        .await
        .map_err(crate::schedules::trigger_error)?;
    let mut writing = None;
    for wakeup in wakeups {
        let Some(run_id) = wakeup.run_id else {
            continue;
        };
        let Some(run) = state.runs.get(&tenant.workspace_id, &run_id).await? else {
            continue;
        };
        if !run.state.is_terminal() && writing.is_none() {
            writing = Some(run.id.as_str().to_string());
        }
        if let Some(report) = state
            .messages
            .latest_agent_message_of_run(&tenant.workspace_id, &run.id)
            .await?
        {
            return Ok((Some(report.into()), writing));
        }
    }
    Ok((None, writing))
}

/// How many Wake-ups back Home looks for a written Report. A Report is
/// daily, so this covers more than a month of them.
const REPORT_LOOKBACK: u32 = 40;

#[utoipa::path(
    get,
    path = "/api/v1/workspace/report",
    responses(
        (status = 200, body = ReportDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_report(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ReportDto>, ApiError> {
    let workspace = load(&state, &tenant).await?;
    let Some(schedule_id) = workspace.report_schedule_id else {
        return Ok(Json(ReportDto {
            schedule_id: None,
            next_due_at: None,
            message: None,
            writing_run_id: None,
        }));
    };
    let next_due_at = state
        .trigger
        .get_schedule(&tenant.workspace_id, &schedule_id)
        .await
        .map_err(crate::schedules::trigger_error)?
        .and_then(|schedule| schedule.next_due_at);
    let (message, writing_run_id) = latest_report(&state, &tenant, &schedule_id).await?;
    Ok(Json(ReportDto {
        schedule_id: Some(schedule_id.as_str().to_string()),
        next_due_at,
        message,
        writing_run_id,
    }))
}
