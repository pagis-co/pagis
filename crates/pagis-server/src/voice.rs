//! Voice in a Thread (ADR-0020): the Dictation socket, the
//! spoken block, and the voice catalogue.
//!
//! Dictation has its own socket, `WS /api/v1/channels/{id}/dictate`,
//! as Listen-Live does in ADR-0020: PCM16 frames up, transcript frames
//! down, first-frame auth. It is not the multiplexed `/api/v1/ws`,
//! whose ring buffer replays frames, because audio in a replayable
//! stream is waste. Where the provider has a realtime socket the text
//! arrives as the user speaks; where it has none the held clip is
//! transcribed on release. Either way the transcript is a draft: this
//! module writes no message. The socket closes with 1008 when the
//! Session of the speaker ends, and nothing is transcribed.
//!
//! Speaking synthesizes one `markdown` block per request and returns
//! the bytes with `Cache-Control: no-store`. Nothing spoken is stored,
//! so a replay synthesizes again.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use pagis_core::{AgentId, AuthorKind, Block, ChannelId, KnownBlock, MessageId};
use pagis_voice::{Clip, DictationSession, SAMPLE_RATE, Transcript, VoiceError};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::live_connections::{LiveTenant, close_for_ended_session};

/// The longest hold. Push-to-talk is one utterance, not a recording.
const MAX_HOLD: Duration = Duration::from_secs(300);

/// The most PCM16 the buffered path keeps: `MAX_HOLD` of audio.
const MAX_CLIP_BYTES: usize = SAMPLE_RATE as usize * 2 * 300;

/// Frames the browser sends on the dictation socket. The session cookie
/// authenticated the upgrade, so the socket carries no
/// credential: PCM16 audio goes as binary frames, and `commit` is the
/// release.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DictateClientFrame {
    Commit,
}

/// Frames the daemon sends on the dictation socket. `ready` says whether
/// text arrives live; the socket ends after `transcript.final` or
/// `error`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type")]
pub enum DictateServerFrame {
    #[serde(rename = "ready")]
    Ready { live: bool },
    #[serde(rename = "transcript.delta")]
    TranscriptDelta { text: String },
    #[serde(rename = "transcript.final")]
    TranscriptFinal { text: String },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

impl DictateServerFrame {
    fn error(code: &str, message: impl Into<String>) -> Self {
        DictateServerFrame::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn to_ws(&self) -> WsMessage {
        WsMessage::Text(
            serde_json::to_string(self)
                .expect("dictation frame serializes")
                .into(),
        )
    }
}

pub async fn dictate(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    Path(channel_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let dictated =
            handle_dictation(state, live.tenant, live.session_ended, channel_id, socket).await;
        if let Err(err) = dictated {
            tracing::debug!(error = %err, "dictation socket closed");
        }
    })
}

async fn handle_dictation(
    state: Arc<AppState>,
    tenant: Tenant,
    session_ended: CancellationToken,
    channel_id: String,
    mut socket: WebSocket,
) -> anyhow::Result<()> {
    if state
        .channels
        .get(&tenant.workspace_id, &ChannelId::from(channel_id))
        .await?
        .is_none()
    {
        socket
            .send(DictateServerFrame::error("not_found", "channel not found").to_ws())
            .await?;
        return Ok(());
    }
    let held = tokio::select! {
        biased;
        () = session_ended.cancelled() => return close_for_ended_session(&mut socket).await,
        held = tokio::time::timeout(MAX_HOLD, dictate_once(&state, &tenant, &mut socket)) => held,
    };
    match held {
        Ok(result) => result,
        Err(_) => {
            socket
                .send(
                    DictateServerFrame::error(
                        "hold_too_long",
                        "release the button within five minutes",
                    )
                    .to_ws(),
                )
                .await?;
            Ok(())
        }
    }
}

/// One utterance: live when the provider has a socket, buffered when it
/// has none (ADR-0005: absent, not emulated).
async fn dictate_once(
    state: &AppState,
    tenant: &Tenant,
    socket: &mut WebSocket,
) -> anyhow::Result<()> {
    match state.voice.dictate(&tenant.workspace_id).await {
        Ok(Some(session)) => {
            socket
                .send(DictateServerFrame::Ready { live: true }.to_ws())
                .await?;
            dictate_live(socket, session).await
        }
        Ok(None) => {
            socket
                .send(DictateServerFrame::Ready { live: false }.to_ws())
                .await?;
            dictate_buffered(state, tenant, socket).await
        }
        Err(error) => {
            socket
                .send(DictateServerFrame::error("voice_unavailable", error.to_string()).to_ws())
                .await?;
            Ok(())
        }
    }
}

/// What one browser frame asks for.
enum Held {
    Audio(Vec<u8>),
    Commit,
    Gone,
}

async fn next_held(socket: &mut WebSocket) -> anyhow::Result<Held> {
    loop {
        let Some(frame) = socket.recv().await else {
            return Ok(Held::Gone);
        };
        match frame? {
            WsMessage::Binary(bytes) => return Ok(Held::Audio(bytes.to_vec())),
            WsMessage::Text(text) => match serde_json::from_str::<DictateClientFrame>(&text) {
                Ok(DictateClientFrame::Commit) => return Ok(Held::Commit),
                Err(_) => {
                    socket
                        .send(DictateServerFrame::error("validation", "unknown frame").to_ws())
                        .await?;
                }
            },
            WsMessage::Close(_) => return Ok(Held::Gone),
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
        }
    }
}

async fn dictate_buffered(
    state: &AppState,
    tenant: &Tenant,
    socket: &mut WebSocket,
) -> anyhow::Result<()> {
    let mut clip: Vec<u8> = Vec::new();
    loop {
        match next_held(socket).await? {
            Held::Audio(bytes) => {
                if clip.len() + bytes.len() > MAX_CLIP_BYTES {
                    socket
                        .send(
                            DictateServerFrame::error(
                                "clip_too_long",
                                "release the button within five minutes",
                            )
                            .to_ws(),
                        )
                        .await?;
                    return Ok(());
                }
                clip.extend_from_slice(&bytes);
            }
            Held::Commit => break,
            Held::Gone => return Ok(()),
        }
    }
    // The clip is dropped with this frame: nothing spoken is stored.
    let frame = if clip.is_empty() {
        DictateServerFrame::TranscriptFinal {
            text: String::new(),
        }
    } else {
        match state
            .voice
            .transcribe(&tenant.workspace_id, Clip { pcm16: clip.into() })
            .await
        {
            Ok(text) => DictateServerFrame::TranscriptFinal { text },
            Err(error) => transcription_error(error),
        }
    };
    socket.send(frame.to_ws()).await?;
    Ok(())
}

async fn dictate_live(socket: &mut WebSocket, session: DictationSession) -> anyhow::Result<()> {
    let DictationSession {
        mut input,
        mut transcripts,
    } = session;
    let mut appended = 0usize;
    let mut committed = false;
    loop {
        tokio::select! {
            held = next_held(socket), if !committed => {
                match held? {
                    Held::Audio(bytes) => {
                        appended += bytes.len();
                        if let Err(error) = input.append(&bytes).await {
                            socket.send(transcription_error(error).to_ws()).await?;
                            return Ok(());
                        }
                    }
                    Held::Commit if appended == 0 => {
                        // Nothing was said: the provider would refuse an
                        // empty commit, and an empty draft is the answer.
                        socket
                            .send(DictateServerFrame::TranscriptFinal { text: String::new() }.to_ws())
                            .await?;
                        return Ok(());
                    }
                    Held::Commit => {
                        committed = true;
                        if let Err(error) = input.commit().await {
                            socket.send(transcription_error(error).to_ws()).await?;
                            return Ok(());
                        }
                    }
                    Held::Gone => return Ok(()),
                }
            }
            piece = transcripts.next() => {
                match piece {
                    Some(Ok(Transcript::Delta(text))) => {
                        socket.send(DictateServerFrame::TranscriptDelta { text }.to_ws()).await?;
                    }
                    Some(Ok(Transcript::Final(text))) => {
                        socket.send(DictateServerFrame::TranscriptFinal { text }.to_ws()).await?;
                        return Ok(());
                    }
                    Some(Err(error)) => {
                        socket.send(transcription_error(error).to_ws()).await?;
                        return Ok(());
                    }
                    None => {
                        socket
                            .send(
                                DictateServerFrame::error(
                                    "transcription_failed",
                                    "the provider closed the session before the transcript",
                                )
                                .to_ws(),
                            )
                            .await?;
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn transcription_error(error: VoiceError) -> DictateServerFrame {
    tracing::warn!(error = %error, "dictation failed");
    DictateServerFrame::error("transcription_failed", error.to_string())
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SpeechQuery {
    /// The index of the block to speak, in the message's block array.
    pub block: usize,
}

/// The header that names the voice that spoke, so a client can say
/// which voice was used when the Agent declares none (ADR-0020).
pub const VOICE_HEADER: &str = "x-pagis-voice";

#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/messages/{message_id}/speech",
    params(("channel_id" = String, Path,), ("message_id" = String, Path,), SpeechQuery),
    responses(
        (status = 200, description = "The spoken block as audio bytes", content_type = "audio/mpeg"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    )
)]
pub async fn speak_block(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((channel_id, message_id)): Path<(String, String)>,
    Query(query): Query<SpeechQuery>,
) -> Result<Response, ApiError> {
    let channel_id = ChannelId::from(channel_id);
    let message = state
        .messages
        .get(&tenant.workspace_id, &MessageId::from(message_id))
        .await?
        .filter(|message| message.channel_id == channel_id)
        .ok_or_else(|| ApiError::not_found("message"))?;
    if message.author_kind != AuthorKind::Agent {
        return Err(ApiError::validation("only an Agent's message is spoken"));
    }
    if !crate::channels::message_is_readable(&state, &message).await? {
        return Err(ApiError::not_found("message"));
    }
    let block = message
        .blocks
        .get(query.block)
        .ok_or_else(|| ApiError::validation("the message has no block at that index"))?;
    let text = match block {
        Block::Known(KnownBlock::Markdown { text }) => text.clone(),
        other => {
            return Err(ApiError::validation(format!(
                "only a markdown block is spoken; a `{}` block is silent",
                block_type(other)
            )));
        }
    };
    let wanted = match &message.author_agent_id {
        Some(agent_id) => state
            .agent_store
            .get(&tenant.workspace_id, &AgentId::from(agent_id.to_string()))
            .await?
            .and_then(|agent| agent.voice),
        None => None,
    };
    // The voice of the model that speaks: the Agent's when the model
    // has it, else the model's first voice (ADR-0020).
    let list = crate::voice_list::speaking_voices(&state, &tenant.workspace_id)
        .await?
        .ok_or_else(|| {
            voice_unavailable(format!(
                "no key serves spoken replies; add a key for {}",
                crate::voice_list::speaking_providers()
            ))
        })?;
    let voice = list.voice_for(wanted.as_deref()).ok_or_else(|| {
        voice_unavailable(format!(
            "{}/{} names no voice",
            list.provider.id(),
            list.model
        ))
    })?;
    let speech = state
        .voice
        .speak(&tenant.workspace_id, &text, voice)
        .await
        .map_err(voice_error)?;
    if !crate::channels::message_is_readable(&state, &message).await? {
        return Err(ApiError::not_found("message"));
    }
    Ok((
        [
            (header::CONTENT_TYPE, speech.media_type),
            (header::HeaderName::from_static(VOICE_HEADER), speech.voice),
        ],
        speech.audio,
    )
        .into_response())
}

fn block_type(block: &Block) -> String {
    serde_json::to_value(block)
        .ok()
        .and_then(|value| value["type"].as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn voice_error(error: VoiceError) -> ApiError {
    tracing::warn!(error = %error, "speech failed");
    voice_unavailable(error.to_string())
}

fn voice_unavailable(message: String) -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "voice_unavailable",
        message,
    }
}

/// The Provider Voice List (ADR-0020): the names an Agent Voice can take,
/// which are the voices of the model that speaks for the Workspace.
#[derive(Debug, Serialize, ToSchema)]
pub struct VoicePage {
    /// The provider of the model that speaks, or `null` when no key
    /// serves spoken replies.
    pub provider: Option<String>,
    /// The model that speaks, as its provider names it.
    pub model: Option<String>,
    /// Its voices; the first is the default.
    pub items: Vec<VoiceDto>,
}

/// One voice of the model that speaks.
#[derive(Debug, Serialize, ToSchema)]
pub struct VoiceDto {
    /// What an Agent Voice holds.
    pub id: String,
    /// The name a person reads, or `null` when the id is the name.
    pub name: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/voices",
    responses(
        (status = 200, body = VoicePage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_voices(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<VoicePage>, ApiError> {
    let list = crate::voice_list::speaking_voices(&state, &tenant.workspace_id).await?;
    Ok(Json(match list {
        Some(list) => VoicePage {
            provider: Some(list.provider.id().to_string()),
            model: Some(list.model),
            items: list
                .voices
                .into_iter()
                .map(|voice| VoiceDto {
                    id: voice.id,
                    name: voice.name,
                })
                .collect(),
        },
        None => VoicePage {
            provider: None,
            model: None,
            items: Vec::new(),
        },
    }))
}
