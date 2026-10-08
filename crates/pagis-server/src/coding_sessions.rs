//! The Coding Sessions of the Workspace and their transcripts, as the
//! clients read them (ADR-0033), and the Person's Stop.
//!
//! The record and the transcript are the one source of truth. The
//! `coding_session.changed` and `coding_session.transcript` events only
//! tell a client to read again. `harness_name`, `machine_name`,
//! `last_activity` and `pending` are read on each request, so they are
//! never stale. The transcript is foreign text: the routes answer it as
//! data.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_coding::{CloseReason, SessionError, WaitsFor};
use pagis_core::{
    AgentId, CodingSession, CodingSessionEvent, CodingSessionEventKind as Kind, CodingSessionId,
    CodingSessionPlace, CodingSessionState, CodingSessionUsage, HostId, SessionApprovalMode,
    WorkspaceId, harness,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// The longest `last_activity` from the text of a message, in
/// characters.
const ACTIVITY_CHARS: usize = 120;

/// The rows that can give the last activity: every kind but `usage`.
const ACTIVITY_KINDS: [Kind; 11] = [
    Kind::Prompt,
    Kind::AgentMessage,
    Kind::Thought,
    Kind::ToolCall,
    Kind::ToolCallUpdate,
    Kind::Plan,
    Kind::Permission,
    Kind::Decision,
    Kind::Question,
    Kind::Answer,
    Kind::TurnEnd,
];

/// The rows of an ask and of its answer.
const DECISION_KINDS: [Kind; 4] = [
    Kind::Permission,
    Kind::Question,
    Kind::Decision,
    Kind::Answer,
];

/// A Coding Session as a client reads it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CodingSessionDto {
    pub id: String,
    /// The Agent that owns and supervises the session.
    pub agent_id: String,
    /// The id of the Coding Harness in the Harness Catalog.
    pub harness_id: String,
    /// The display name of the Coding Harness.
    pub harness_name: String,
    pub harness_version: String,
    pub place: CodingSessionPlace,
    /// The Host of a `host` session.
    pub host_id: Option<String>,
    /// The name of the Host. A `computer` session has none.
    pub machine_name: Option<String>,
    /// The directory that the Agent named.
    pub directory: String,
    /// The directory that the process runs in: the worktree path when
    /// the session has a worktree.
    pub working_directory: Option<String>,
    pub worktree_branch: Option<String>,
    pub approval_mode: SessionApprovalMode,
    pub title: String,
    pub state: CodingSessionState,
    /// Why the session ended, for example `closed`, `stopped`,
    /// `harness_error` or `harness_exited`.
    pub end_reason: Option<String>,
    /// The exit code and the stderr tail of a failed harness, or the
    /// message of the request that it failed. It is harness text.
    pub end_detail: Option<String>,
    /// The harness's own ACP session id.
    pub acp_session_id: Option<String>,
    /// The Channel of the Thread that shows the session.
    pub channel_id: String,
    /// The root message of that Thread.
    pub root_message_id: String,
    /// The message that holds the block of the session.
    pub message_id: String,
    /// The Run that started the session.
    pub run_id: String,
    pub usage: CodingSessionUsage,
    pub created_at: i64,
    pub updated_at: i64,
    pub ended_at: Option<i64>,
    /// One line about the last row of the transcript.
    pub last_activity: Option<String>,
    /// The ask that waits for an answer while the session is
    /// `needs_decision`.
    pub pending: Option<PendingDecisionDto>,
}

/// An ask of the harness that waits for its answer.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct PendingDecisionDto {
    pub kind: PendingDecisionKind,
    pub waits_for: WaitsFor,
    /// The transcript row of the ask.
    pub seq: i64,
}

/// What an ask asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PendingDecisionKind {
    /// A Harness Permission.
    Permission,
    /// A question of the harness.
    Question,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CodingSessionPage {
    pub items: Vec<CodingSessionDto>,
}

/// One row of a transcript. The payload is foreign text.
#[derive(Debug, Serialize, ToSchema)]
pub struct CodingSessionEventDto {
    /// The place of the row in its session, from 1.
    pub seq: i64,
    /// The time of the first chunk of the row.
    pub at: i64,
    pub kind: Kind,
    /// `{"text", "message_id"}` for `prompt`, `agent_message` and
    /// `thought`, and the ACP object for each other kind.
    #[schema(value_type = HashMap<String, serde_json::Value>)]
    pub payload: serde_json::Value,
}

impl From<CodingSessionEvent> for CodingSessionEventDto {
    fn from(row: CodingSessionEvent) -> Self {
        CodingSessionEventDto {
            seq: row.seq,
            at: row.at,
            kind: row.kind,
            payload: row.payload,
        }
    }
}

/// One page of a transcript, oldest row first.
#[derive(Debug, Serialize, ToSchema)]
pub struct CodingSessionTranscript {
    pub items: Vec<CodingSessionEventDto>,
    /// The `after` of the next page, when more rows follow.
    pub next_after: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CodingSessionListQuery {
    pub agent_id: Option<String>,
    pub state: Option<String>,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct TranscriptQuery {
    pub after: Option<i64>,
    pub limit: Option<u32>,
}

/// List the Coding Sessions of the Workspace, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/coding-sessions",
    params(
        ("agent_id" = Option<String>, Query),
        ("state" = Option<String>, Query, description = "A state of a Coding Session, for example `working`"),
        ("before" = Option<String>, Query, description = "The Coding Session the page starts after"),
        ("limit" = Option<u32>, Query, description = "1 to 100, 50 by default"),
    ),
    responses(
        (status = 200, body = CodingSessionPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn list_coding_sessions(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<CodingSessionListQuery>,
) -> Result<Json<CodingSessionPage>, ApiError> {
    let session_state = query
        .state
        .as_deref()
        .map(str::parse::<CodingSessionState>)
        .transpose()
        .map_err(ApiError::validation)?;
    let agent_id = query.agent_id.map(AgentId::from);
    let before = query.before.map(CodingSessionId::from);
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let sessions = state
        .coding_session_store
        .list(
            &tenant.workspace_id,
            agent_id.as_ref(),
            session_state,
            before.as_ref(),
            limit,
        )
        .await?;
    let machines = machine_names(&state, &tenant.workspace_id).await?;
    let mut items = Vec::with_capacity(sessions.len());
    for session in sessions {
        items.push(session_dto(&state, session, &machines).await?);
    }
    Ok(Json(CodingSessionPage { items }))
}

/// Read one Coding Session.
#[utoipa::path(
    get,
    path = "/api/v1/coding-sessions/{coding_session_id}",
    params(("coding_session_id" = String, Path, description = "The Coding Session")),
    responses(
        (status = 200, body = CodingSessionDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_coding_session(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(coding_session_id): Path<String>,
) -> Result<Json<CodingSessionDto>, ApiError> {
    let session = read_session(&state, &tenant, coding_session_id).await?;
    let machines = machine_names(&state, &tenant.workspace_id).await?;
    Ok(Json(session_dto(&state, session, &machines).await?))
}

/// Read one page of the transcript of a Coding Session, oldest row
/// first. A merge makes a row longer in place, so a client that holds
/// row N reads from `after = N - 1` to get its last words.
#[utoipa::path(
    get,
    path = "/api/v1/coding-sessions/{coding_session_id}/transcript",
    params(
        ("coding_session_id" = String, Path, description = "The Coding Session"),
        ("after" = Option<i64>, Query, description = "The `seq` the page starts after"),
        ("limit" = Option<u32>, Query, description = "1 to 500, 200 by default"),
    ),
    responses(
        (status = 200, body = CodingSessionTranscript),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn coding_session_transcript(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(coding_session_id): Path<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Json<CodingSessionTranscript>, ApiError> {
    let session = read_session(&state, &tenant, coding_session_id).await?;
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    // One row past the page says whether another page follows.
    let mut rows = state
        .coding_session_store
        .list_events(&tenant.workspace_id, &session.id, query.after, limit + 1)
        .await?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_after = more.then(|| rows.last().map(|row| row.seq)).flatten();
    Ok(Json(CodingSessionTranscript {
        items: rows.into_iter().map(CodingSessionEventDto::from).collect(),
        next_after,
    }))
}

/// The Person stops a Coding Session: the daemon cancels the turn that
/// runs and closes the session with the end reason `stopped`.
#[utoipa::path(
    post,
    path = "/api/v1/coding-sessions/{coding_session_id}/stop",
    params(("coding_session_id" = String, Path, description = "The Coding Session")),
    responses(
        (status = 204, description = "The session is closed"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The session is not open"),
    )
)]
pub async fn stop_coding_session(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(coding_session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let stopped = state
        .coding_sessions
        .close(
            &tenant.workspace_id,
            &CodingSessionId::from(coding_session_id),
            CloseReason::Stopped,
        )
        .await;
    match stopped {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(SessionError::NotFound) => Err(ApiError::not_found("that coding session")),
        Err(SessionError::NotOpen(session_state)) => Err(ApiError::conflict(format!(
            "the coding session is {}",
            session_state.as_str()
        ))),
        Err(SessionError::Store(error)) => Err(error.into()),
        Err(SessionError::Harness(message)) => {
            tracing::warn!(message, "a Coding Session did not stop");
            Err(ApiError::internal())
        }
    }
}

async fn read_session(
    state: &AppState,
    tenant: &Tenant,
    coding_session_id: String,
) -> Result<CodingSession, ApiError> {
    state
        .coding_session_store
        .get(
            &tenant.workspace_id,
            &CodingSessionId::from(coding_session_id),
        )
        .await?
        .ok_or_else(|| ApiError::not_found("that coding session"))
}

/// The name of each Host of the Workspace.
async fn machine_names(
    state: &AppState,
    workspace_id: &WorkspaceId,
) -> Result<HashMap<HostId, String>, ApiError> {
    Ok(state
        .hosts
        .list(workspace_id)
        .await?
        .into_iter()
        .map(|host| (host.id, host.name))
        .collect())
}

async fn session_dto(
    state: &AppState,
    session: CodingSession,
    machines: &HashMap<HostId, String>,
) -> Result<CodingSessionDto, ApiError> {
    let store = &state.coding_session_store;
    let activity = store
        .latest_event(&session.workspace_id, &session.id, &ACTIVITY_KINDS)
        .await?;
    let decision = if session.state == CodingSessionState::NeedsDecision {
        store
            .latest_event(&session.workspace_id, &session.id, &DECISION_KINDS)
            .await?
    } else {
        None
    };
    // A harness that a later release removed from the catalog shows its
    // id.
    let harness_name = harness::entry(&session.harness_id).map_or_else(
        || session.harness_id.clone(),
        |entry| entry.label.to_string(),
    );
    let machine_name = session
        .host_id
        .as_ref()
        .and_then(|host_id| machines.get(host_id))
        .cloned();
    Ok(CodingSessionDto {
        id: session.id.to_string(),
        agent_id: session.agent_id.to_string(),
        harness_id: session.harness_id,
        harness_name,
        harness_version: session.harness_version,
        place: session.place,
        host_id: session.host_id.map(|host_id| host_id.to_string()),
        machine_name,
        directory: session.directory,
        working_directory: session.working_directory,
        worktree_branch: session.worktree_branch,
        approval_mode: session.approval_mode,
        title: session.title,
        state: session.state,
        end_reason: session.end_reason,
        end_detail: session.end_detail,
        acp_session_id: session.acp_session_id,
        channel_id: session.channel_id.to_string(),
        root_message_id: session.root_message_id.to_string(),
        message_id: session.message_id.to_string(),
        run_id: session.run_id.to_string(),
        usage: session.usage,
        created_at: session.created_at,
        updated_at: session.updated_at,
        ended_at: session.ended_at,
        last_activity: activity.as_ref().and_then(last_activity),
        pending: decision.as_ref().and_then(pending),
    })
}

/// One line about the last row of a transcript that is not `usage`.
fn last_activity(row: &CodingSessionEvent) -> Option<String> {
    match row.kind {
        Kind::ToolCall => row.payload.get("title")?.as_str().map(str::to_string),
        Kind::AgentMessage | Kind::Thought => {
            let text = row.payload.get("text")?.as_str()?;
            let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
            Some(line.chars().take(ACTIVITY_CHARS).collect())
        }
        Kind::Plan => Some("Updated the plan".to_string()),
        Kind::TurnEnd => Some("Finished the turn".to_string()),
        Kind::Prompt => Some("Received a prompt".to_string()),
        Kind::ToolCallUpdate
        | Kind::Usage
        | Kind::Permission
        | Kind::Decision
        | Kind::Question
        | Kind::Answer => None,
    }
}

/// The ask that waits, from the last row of an ask or of its answer. An
/// answer row means that no ask waits.
fn pending(row: &CodingSessionEvent) -> Option<PendingDecisionDto> {
    let kind = match row.kind {
        Kind::Permission => PendingDecisionKind::Permission,
        Kind::Question => PendingDecisionKind::Question,
        _ => return None,
    };
    let waits_for = serde_json::from_value(row.payload.get("waits_for")?.clone()).ok()?;
    Some(PendingDecisionDto {
        kind,
        waits_for,
        seq: row.seq,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn row(seq: i64, kind: Kind, payload: Value) -> CodingSessionEvent {
        CodingSessionEvent {
            workspace_id: WorkspaceId::generate(),
            coding_session_id: CodingSessionId::generate(),
            seq,
            at: 0,
            kind,
            payload,
        }
    }

    fn activity(kind: Kind, payload: Value) -> Option<String> {
        last_activity(&row(1, kind, payload))
    }

    #[test]
    fn a_tool_call_gives_its_title() {
        assert_eq!(
            activity(
                Kind::ToolCall,
                json!({"toolCallId": "call-1", "title": "Run the tests", "kind": "execute"})
            )
            .as_deref(),
            Some("Run the tests")
        );
    }

    #[test]
    fn a_message_or_a_thought_gives_the_first_line_of_its_text() {
        let text = json!({"text": "I ran the tests.\nThey pass.", "message_id": "m1"});
        assert_eq!(
            activity(Kind::AgentMessage, text.clone()).as_deref(),
            Some("I ran the tests.")
        );
        assert_eq!(
            activity(Kind::Thought, text).as_deref(),
            Some("I ran the tests.")
        );
        assert_eq!(
            activity(
                Kind::AgentMessage,
                json!({"text": "\n  Done.\n", "message_id": null})
            )
            .as_deref(),
            Some("Done."),
            "the first line is the first line that holds text"
        );
    }

    #[test]
    fn a_first_line_of_300_characters_is_cut_to_120() {
        let line = "é".repeat(300);

        let cut = activity(
            Kind::AgentMessage,
            json!({"text": line, "message_id": "m1"}),
        )
        .expect("a line");

        assert_eq!(cut.chars().count(), 120);
        assert_eq!(cut, "é".repeat(120));
    }

    #[test]
    fn a_plan_a_turn_end_and_a_prompt_give_a_fixed_line() {
        assert_eq!(
            activity(Kind::Plan, json!({"entries": []})).as_deref(),
            Some("Updated the plan")
        );
        assert_eq!(
            activity(Kind::TurnEnd, json!({"stop_reason": "end_turn"})).as_deref(),
            Some("Finished the turn")
        );
        assert_eq!(
            activity(
                Kind::Prompt,
                json!({"text": "Fix the login bug.", "message_id": null})
            )
            .as_deref(),
            Some("Received a prompt"),
            "a prompt does not show its text"
        );
    }

    #[test]
    fn each_other_kind_gives_no_line() {
        for kind in [
            Kind::ToolCallUpdate,
            Kind::Usage,
            Kind::Permission,
            Kind::Decision,
            Kind::Question,
            Kind::Answer,
        ] {
            assert_eq!(activity(kind, json!({"title": "x", "text": "x"})), None);
        }
    }

    #[test]
    fn a_permission_row_is_pending_and_names_who_it_waits_for() {
        let asked = row(
            4,
            Kind::Permission,
            json!({"ask_id": "a1", "waits_for": "person"}),
        );

        assert_eq!(
            pending(&asked),
            Some(PendingDecisionDto {
                kind: PendingDecisionKind::Permission,
                waits_for: WaitsFor::Person,
                seq: 4,
            })
        );
        let question = row(
            5,
            Kind::Question,
            json!({"ask_id": "a2", "waits_for": "agent"}),
        );
        assert_eq!(
            pending(&question).map(|pending| (pending.kind, pending.waits_for)),
            Some((PendingDecisionKind::Question, WaitsFor::Agent))
        );
    }

    #[test]
    fn nothing_is_pending_after_the_decision_row() {
        let decided = row(
            5,
            Kind::Decision,
            json!({"ask_id": "a1", "decision": "reject_once"}),
        );

        assert_eq!(pending(&decided), None);
        let answered = row(6, Kind::Answer, json!({"ask_id": "a2", "answer": "cancel"}));
        assert_eq!(pending(&answered), None);
    }
}
