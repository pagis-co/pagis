//! Listen-Live (ADR-0020): a person in the Thread hears a call
//! while it runs.
//!
//! `WS /api/v1/calls/{call_id}/listen` has first-frame auth, as the
//! dictation socket does. It sends a mono mix of both directions as
//! G.711 frames, one binary message per 20 ms frame, in the codec of
//! the call. The browser decodes them.
//!
//! Nothing is signalled to the Remote Party, and a listener who joins
//! hears the call from that moment: there is no backfill, because the
//! transcript carries the history. The hub subscription is lossy, so a
//! slow or gone listener loses frames and the call goes on. The socket
//! closes with 1008 when the Session of the listener ends, and the call
//! goes on.
//!
//! `POST /api/v1/calls/{call_id}/control` is reserved and answers 501.
//! Media fan-out and control are separate surfaces. Two-way barge-in
//! is not built; it needs no change to Listen-Live.

use std::sync::Arc;

use axum::Json;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pagis_core::{AgentId, Call, CallDirection, CallId, CallState, TrustTier};
use pagis_telephony::hub::MediaHub;
use pagis_telephony::listen::Mixer;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::live_connections::{LiveTenant, close_for_ended_session};

/// Frames the daemon sends beside the audio. `ready` names the codec
/// of the frames that follow; the socket ends after `ended` or
/// `error`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type")]
pub enum ListenServerFrame {
    /// `PCMU` or `PCMA`: the law of every binary frame that follows.
    #[serde(rename = "ready")]
    Ready { codec: String },
    /// The call ended, with the reason it ended for.
    #[serde(rename = "ended")]
    Ended { reason: String },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

impl ListenServerFrame {
    fn error(code: &str, message: impl Into<String>) -> Self {
        ListenServerFrame::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn to_ws(&self) -> WsMessage {
        WsMessage::Text(
            serde_json::to_string(self)
                .expect("listen frame serializes")
                .into(),
        )
    }
}

pub async fn listen(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    Path(call_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let listened = handle_listen(state, live.tenant, live.session_ended, call_id, socket).await;
        if let Err(err) = listened {
            tracing::debug!(error = %err, "listen socket closed");
        }
    })
}

async fn handle_listen(
    state: Arc<AppState>,
    tenant: Tenant,
    session_ended: CancellationToken,
    call_id: String,
    mut socket: WebSocket,
) -> anyhow::Result<()> {
    let call_id = CallId::from(call_id);
    // The live hubs are keyed by the Workspace and the Call, so a Call
    // of another Workspace is a Call that is not live for this listener.
    let Some(hub) = state.live_calls.hub(&tenant.workspace_id, &call_id) else {
        socket
            .send(ListenServerFrame::error("not_found", "no call of that id is live").to_ws())
            .await?;
        return Ok(());
    };
    socket
        .send(
            ListenServerFrame::Ready {
                codec: hub.codec().name().to_string(),
            }
            .to_ws(),
        )
        .await?;
    tokio::select! {
        biased;
        () = session_ended.cancelled() => close_for_ended_session(&mut socket).await,
        carried = carry(&hub, &mut socket) => carried,
    }
}

/// The mix, frame by frame, until the call ends or the listener goes.
async fn carry(hub: &MediaHub, socket: &mut WebSocket) -> anyhow::Result<()> {
    let mut frames = hub.listen();
    let mut ended = hub.watch_ended();
    let mut mixer = Mixer::default();
    loop {
        let recorded = tokio::select! {
            recorded = frames.recv() => recorded,
            _ = ended.changed() => break,
        };
        match recorded {
            Ok(recorded) => {
                if let Some(frame) = mixer.push(recorded) {
                    socket
                        .send(WsMessage::Binary(frame.payload().clone()))
                        .await?;
                }
            }
            // The listener fell behind: it hears the call from now on,
            // because a live call has no backfill.
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    let reason = hub
        .ended()
        .map(|reason| reason.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    socket
        .send(ListenServerFrame::Ended { reason }.to_ws())
        .await?;
    Ok(())
}

/// Control of a live call is reserved (ADR-0020). Two-way barge-in is
/// not built, so it answers 501.
#[utoipa::path(
    post,
    path = "/api/v1/calls/{call_id}/control",
    params(("call_id" = String, Path, description = "The live call")),
    responses((status = 501, description = "Control of a live call is not implemented")),
)]
pub async fn control(Path(_call_id): Path<String>) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "control of a live call is not implemented",
    )
        .into_response()
}

/// The user drops the tier of a live call. `unknown` is the only value:
/// a tier rises by itself and falls by hand (ADR-0021).
#[derive(Debug, Deserialize, ToSchema)]
pub struct DropTierRequest {
    pub tier: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CallTierDto {
    pub call_id: String,
    pub tier: String,
}

/// Drop one live call to Unknown. It asks for a confirmation in the
/// user interface, because it cannot be undone while the call runs.
/// The gate is found under the Person's own Workspace, so a live call of
/// another Workspace answers 404, as a call that is not live.
#[utoipa::path(post, path = "/api/v1/calls/{call_id}/tier",
    request_body = DropTierRequest, responses(
        (status = 200, body = CallTierDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn drop_call_tier(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(call_id): Path<String>,
    Json(request): Json<DropTierRequest>,
) -> Result<Json<CallTierDto>, ApiError> {
    if request.tier != TrustTier::Unknown.as_str() {
        return Err(ApiError::validation(
            "a call tier only moves down, and only to `unknown`",
        ));
    }
    let call_id = CallId::from(call_id);
    state
        .live_tiers
        .drop_to_unknown(&tenant.workspace_id, &call_id)
        .await
        .ok_or_else(|| ApiError::not_found("that live call"))?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(CallTierDto {
        call_id: call_id.to_string(),
        tier: TrustTier::Unknown.as_str().to_string(),
    }))
}

/// One Call in a list, without its transcript. A page of ten-minute
/// calls would carry megabytes of lines that no list shows, so the
/// lines stay in `GET /api/v1/calls/{call_id}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CallSummaryDto {
    pub id: String,
    pub agent_id: String,
    /// The Agent's external display name, and the line it used.
    pub agent_name: String,
    pub own_e164: String,
    /// `outbound` or `inbound`.
    pub direction: String,
    pub remote_e164: String,
    /// What the call is for, from the Call Brief.
    pub purpose: String,
    pub tier: String,
    /// `dialing`, `live` or `ended`.
    pub state: String,
    pub outcome: Option<String>,
    pub ended_reason: Option<String>,
    pub classification: Option<String>,
    pub message_left: bool,
    /// The stereo WAV of the call, once it settles.
    pub recording_artifact_id: Option<String>,
    pub created_at: i64,
    pub answered_at: Option<i64>,
    pub ended_at: Option<i64>,
    /// When the Person dismissed the missed Call from the Needs-You
    /// Queue.
    pub dismissed_at: Option<i64>,
}

impl From<Call> for CallSummaryDto {
    fn from(call: Call) -> Self {
        CallSummaryDto {
            id: call.id.to_string(),
            agent_id: call.agent_id.to_string(),
            agent_name: call.agent_name,
            own_e164: call.own_e164,
            direction: call.direction.as_str().to_string(),
            remote_e164: call.remote_e164,
            purpose: call.purpose,
            tier: call.tier.as_str().to_string(),
            state: call.state.as_str().to_string(),
            outcome: call.outcome.map(|outcome| outcome.as_str().to_string()),
            ended_reason: call.ended_reason,
            classification: call.classification.map(|class| class.as_str().to_string()),
            message_left: call.message_left,
            recording_artifact_id: call
                .recording_artifact_id
                .map(|artifact| artifact.to_string()),
            created_at: call.created_at,
            answered_at: call.answered_at,
            ended_at: call.ended_at,
            dismissed_at: call.dismissed_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CallPage {
    pub items: Vec<CallSummaryDto>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CallListQuery {
    pub agent_id: Option<String>,
    pub direction: Option<String>,
    pub state: Option<String>,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

/// List the Call records of the Workspace, newest first. Home
/// reads it to find the calls that came in today and were missed.
#[utoipa::path(
    get,
    path = "/api/v1/calls",
    params(
        ("agent_id" = Option<String>, Query),
        ("direction" = Option<String>, Query, description = "`inbound` or `outbound`"),
        ("state" = Option<String>, Query, description = "`dialing`, `live` or `ended`"),
        ("before" = Option<String>, Query, description = "The Call the page starts after"),
        ("limit" = Option<u32>, Query),
    ),
    responses(
        (status = 200, body = CallPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn list_calls(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<CallListQuery>,
) -> Result<Json<CallPage>, ApiError> {
    let direction = query
        .direction
        .as_deref()
        .map(str::parse::<CallDirection>)
        .transpose()
        .map_err(ApiError::validation)?;
    let call_state = query
        .state
        .as_deref()
        .map(str::parse::<CallState>)
        .transpose()
        .map_err(ApiError::validation)?;
    let agent_id = query.agent_id.map(AgentId::from);
    let before = query.before.map(CallId::from);
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let items = state
        .calls
        .list(
            &tenant.workspace_id,
            agent_id.as_ref(),
            direction,
            call_state,
            before.as_ref(),
            limit,
        )
        .await?
        .into_iter()
        .map(CallSummaryDto::from)
        .collect();
    Ok(Json(CallPage { items }))
}

/// One line of a Call transcript, as the UI reads it.
#[derive(Debug, Serialize, ToSchema)]
pub struct TranscriptLineDto {
    pub at: i64,
    /// `caller`, `agent` or `daemon`.
    pub speaker: String,
    pub text: String,
}

/// The Call record (ADR-0020). It is the one source of truth the
/// strip, the call inspector and the settled block read. `outcome` and
/// `ended_reason` stay apart, because they answer two questions.
#[derive(Debug, Serialize, ToSchema)]
pub struct CallDto {
    pub id: String,
    pub agent_id: String,
    /// The Agent's external display name, and the line it called from.
    pub agent_name: String,
    pub own_e164: String,
    /// `outbound` or `inbound`.
    pub direction: String,
    pub remote_e164: String,
    /// What the call is for, from the Call Brief.
    pub purpose: String,
    /// The tools the call may use, by name.
    pub tools: Vec<String>,
    pub tier: String,
    /// `dialing`, `live` or `ended`.
    pub state: String,
    pub outcome: Option<String>,
    pub ended_reason: Option<String>,
    pub classification: Option<String>,
    pub message_left: bool,
    pub transcript: Vec<TranscriptLineDto>,
    /// The stereo WAV of the call, once it settles.
    pub recording_artifact_id: Option<String>,
    pub created_at: i64,
    pub answered_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl From<Call> for CallDto {
    fn from(call: Call) -> Self {
        CallDto {
            id: call.id.to_string(),
            agent_id: call.agent_id.to_string(),
            agent_name: call.agent_name,
            own_e164: call.own_e164,
            direction: call.direction.as_str().to_string(),
            remote_e164: call.remote_e164,
            purpose: call.purpose,
            tools: call.tools,
            tier: call.tier.as_str().to_string(),
            state: call.state.as_str().to_string(),
            outcome: call.outcome.map(|outcome| outcome.as_str().to_string()),
            ended_reason: call.ended_reason,
            classification: call.classification.map(|class| class.as_str().to_string()),
            message_left: call.message_left,
            transcript: call
                .transcript
                .into_iter()
                .map(|line| TranscriptLineDto {
                    at: line.at,
                    speaker: line.speaker.as_str().to_string(),
                    text: line.text,
                })
                .collect(),
            recording_artifact_id: call
                .recording_artifact_id
                .map(|artifact| artifact.to_string()),
            created_at: call.created_at,
            answered_at: call.answered_at,
            ended_at: call.ended_at,
        }
    }
}

/// Read one Call record. The strip, the call inspector and the settled
/// block all read this: the record is the one source of truth, and the
/// `call.transcript` events only add the lines that arrive after the
/// read.
#[utoipa::path(get, path = "/api/v1/calls/{call_id}",
    params(("call_id" = String, Path, description = "The Call")),
    responses(
        (status = 200, body = CallDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_call(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(call_id): Path<String>,
) -> Result<Json<CallDto>, ApiError> {
    let call = state
        .calls
        .get(&tenant.workspace_id, &CallId::from(call_id))
        .await
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| ApiError::not_found("that call"))?;
    Ok(Json(CallDto::from(call)))
}

/// The user ends a live call. The daemon hangs up its own leg, and the
/// call settles with a reason like every other call (ADR-0020). The hub
/// is found under the Person's own Workspace, so a live call of another
/// Workspace answers 404, as a call that is not live.
#[utoipa::path(post, path = "/api/v1/calls/{call_id}/hangup",
    params(("call_id" = String, Path, description = "The live Call")),
    responses(
        (status = 204, description = "The call is hung up"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn hang_up(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(call_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let hub = state
        .live_calls
        .hub(&tenant.workspace_id, &CallId::from(call_id))
        .ok_or_else(|| ApiError::not_found("that live call"))?;
    hub.hangup().await;
    Ok(StatusCode::NO_CONTENT)
}

/// The Person dismisses a missed Call from the Needs-You Queue. The
/// record keeps the time, so the queue leaves it out on every client
/// and after a reload. A second dismissal keeps the first time.
#[utoipa::path(post, path = "/api/v1/calls/{call_id}/dismiss",
    params(("call_id" = String, Path, description = "The Call")),
    responses(
        (status = 204, description = "The Call is out of the Needs-You Queue"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn dismiss_call(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(call_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let call_id = CallId::from(call_id);
    if !state
        .calls
        .dismiss(&tenant.workspace_id, &call_id, state.clock.now_ms())
        .await
        .map_err(|_| ApiError::internal())?
    {
        return Err(ApiError::not_found("that call"));
    }
    // Every client of the Person drops the queue item.
    let _ = state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "call.dismissed".into(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({ "call_id": call_id.as_str() }),
        })
        .await;
    Ok(StatusCode::NO_CONTENT)
}
