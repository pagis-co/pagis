//! The multiplexed WebSocket: first-frame auth, `{seq,
//! type, payload}` envelope, domain-event firehose of `events` rows,
//! channel subscribe/unsubscribe gating the ephemeral `message.delta`
//! and `progress.state` frames with mid-stream catch-up, `last_seq`
//! resume over the ring buffer, and the `resync` fallback.
//!
//! Every frame the socket sends belongs to the Workspace of the Session
//! that opened it. The firehose subscribes in the tenant's scope,
//! live and on replay, and a `subscribe` frame names a Channel the
//! signed-in person may read or it is refused.
//!
//! The socket also carries work the other way. A client that can
//! act on its own machine registers as a Host, and is present for as long
//! as this socket lives. The daemon then dispatches a host command to it
//! and reads the result off the same socket, correlated by the id it
//! minted. Only the Host connection that received a command answers it.
//! Nothing about this is a second connection: the transport was
//! already bidirectional, and the client already proved who it is.
//!
//! The socket lives no longer than the Session that opened it. When the
//! Session ends, the Host the socket registered becomes absent and the
//! socket closes with 1008 (see [`crate::live_connections`]).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures::StreamExt;
use pagis_agent::{DeltaFrame, ProgressFrame};
use pagis_broker::{HostCommand, HostConnection, HostOutcome};
use pagis_core::{ChannelId, Event, EventScope, Host};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::live_connections::{LiveTenant, close_for_ended_session};

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Frames the client sends. The first frame must be `auth`. It carries
/// no credential: the session cookie authenticated the upgrade.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Auth {
        /// The event id (`seq`) of the last frame the client saw; the
        /// daemon replays the gap when the ring buffer still covers it.
        last_seq: Option<String>,
    },
    Subscribe {
        channel_id: String,
    },
    Unsubscribe {
        channel_id: String,
    },
    /// Register this client as a Host of the signed-in person. The
    /// machine is present while this socket lives, and absent the moment
    /// it closes. The client is the authority for all three fields: what
    /// the machine is called, what it runs on, and what it can do.
    RegisterHost {
        name: String,
        platform: String,
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// What one dispatched command did. `id` is the id the
    /// `dispatch` frame carried.
    Result {
        id: String,
        exit_code: Option<i64>,
        #[serde(default)]
        stdout: String,
        #[serde(default)]
        stderr: String,
    },
    Ping,
}

/// The server envelope. Domain events carry `seq` (the event id) and an
/// `EventRow` payload; control frames (`ready`, `pong`, `resync`,
/// `error`) carry no `seq` and are never replayed.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServerFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<String>,
    /// True for a domain event replayed after reconnect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<bool>,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// An `events` row as sent on the firehose, verbatim.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventRow {
    pub id: String,
    pub event_type: String,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub channel_id: Option<String>,
    pub payload: serde_json::Value,
}

impl From<&Event> for EventRow {
    fn from(event: &Event) -> Self {
        EventRow {
            id: event.id.to_string(),
            event_type: event.event_type.clone(),
            agent_id: event.agent_id.as_ref().map(|a| a.to_string()),
            run_id: event.run_id.as_ref().map(|r| r.to_string()),
            channel_id: event.channel_id.as_ref().map(|c| c.to_string()),
            payload: event.payload.clone(),
        }
    }
}

impl ServerFrame {
    fn control(r#type: &str) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: r#type.to_string(),
            payload: None,
        }
    }

    fn error(code: &str, message: &str) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: "error".to_string(),
            payload: Some(serde_json::json!({ "code": code, "message": message })),
        }
    }

    fn event(event: &Event, replay: bool) -> Self {
        ServerFrame {
            seq: Some(event.id.to_string()),
            replay: Some(replay),
            r#type: event.event_type.clone(),
            payload: Some(
                serde_json::to_value(EventRow::from(event)).expect("event row serializes"),
            ),
        }
    }

    /// An ephemeral streaming delta: no `seq`, never replayed.
    fn delta(frame: &DeltaFrame) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: "message.delta".to_string(),
            payload: Some(serde_json::to_value(frame).expect("delta frame serializes")),
        }
    }

    /// The Host the client registered, so the client knows the id
    /// its machine is known by and the person's surfaces name one record.
    fn host_registered(host: &Host) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: "host.registered".to_string(),
            payload: Some(serde_json::json!({
                "host_id": host.id.as_str(),
                "name": host.name,
                "platform": host.platform,
                "capabilities": host.capabilities,
            })),
        }
    }

    /// One command for the Host this socket registered: no `seq`,
    /// never replayed. A command is work of this connection, and a client
    /// that reconnects must not receive the commands of the socket it
    /// lost.
    fn dispatch(command: &HostCommand) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: "dispatch".to_string(),
            payload: Some(serde_json::to_value(command).expect("host command serializes")),
        }
    }

    /// An ephemeral progress line: no `seq`, never replayed.
    fn progress(frame: &ProgressFrame) -> Self {
        ServerFrame {
            seq: None,
            replay: None,
            r#type: "progress.state".to_string(),
            payload: Some(serde_json::to_value(frame).expect("progress frame serializes")),
        }
    }

    fn to_ws(&self) -> WsMessage {
        WsMessage::Text(
            serde_json::to_string(self)
                .expect("server frame serializes")
                .into(),
        )
    }
}

pub async fn upgrade(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = handle(state, live.tenant, live.session_ended, socket).await {
            tracing::debug!(error = %err, "websocket closed");
        }
    })
}

async fn handle(
    state: Arc<AppState>,
    tenant: Tenant,
    session_ended: CancellationToken,
    mut socket: WebSocket,
) -> anyhow::Result<()> {
    // First frame: auth, within the timeout.
    let last_seq = match authenticate(&mut socket).await? {
        Some(last_seq) => last_seq,
        None => return Ok(()), // rejected; error frame already sent
    };

    // Resolve the resume point before going live.
    let mut resync = false;
    let after_seq = match &last_seq {
        None => None,
        Some(event_id) => match state.ring.position(event_id) {
            Some(seq) => Some(seq),
            None => {
                resync = true;
                None
            }
        },
    };
    let replay_through = state.events.latest_seq().await?;
    let events = state
        .bus
        .subscribe(EventScope::workspace(&tenant.workspace_id), after_seq)
        .await;
    let deltas = state.hub.subscribe();
    let progress = state.progress.subscribe();

    socket.send(ServerFrame::control("ready").to_ws()).await?;
    if resync {
        socket.send(ServerFrame::control("resync").to_ws()).await?;
    }

    let mut connection = Connection {
        state,
        tenant,
        socket,
        subscriptions: HashSet::new(),
        host: None,
        events,
        deltas,
        progress,
        replay_through,
    };
    // The socket's own loop is one future, so whatever ends it — a closed
    // socket, a send that failed, an error, the end of the Session — the
    // Host it registered is deregistered and its last-seen time written
    // once, below. The end of the Session is polled first, so no frame
    // goes out after it.
    let served = tokio::select! {
        biased;
        () = session_ended.cancelled() => None,
        outcome = connection.serve() => Some(outcome),
    };
    // The machine is absent from here on, before the socket closes, so a
    // host action after the close finds it absent. The record says when
    // the machine was last here, so a list can show "last seen" for a
    // machine that is not connected now.
    if let Some(RegisteredHost {
        host,
        connection: presence,
    }) = connection.host.take()
    {
        drop(presence);
        let now = connection.state.clock.now_ms();
        let _ = connection.state.hosts.touch(&host.id, now).await;
    }
    match served {
        Some(outcome) => outcome,
        None => close_for_ended_session(&mut connection.socket).await,
    }
}

/// Everything one authenticated socket holds for as long as it is open.
///
/// The frame loop reads and writes all of it, so it is one value rather
/// than nine arguments: the streams it selects over, the socket it
/// answers on, the Channels this client asked for, and the Host it
/// registered.
struct Connection {
    state: Arc<AppState>,
    /// The Workspace of the Session that opened this socket. Every
    /// frame out belongs to it, and every frame in is read against it.
    tenant: Tenant,
    socket: WebSocket,
    /// The Channels this client asked for the ephemeral frames of.
    subscriptions: HashSet<String>,
    /// The Host this client registered, if it registered one. It lives as
    /// long as this socket: dropping it makes the machine absent.
    host: Option<RegisteredHost>,
    /// The domain-event firehose of the tenant's scope.
    events: pagis_core::EventStream,
    deltas: broadcast::Receiver<DeltaFrame>,
    progress: broadcast::Receiver<ProgressFrame>,
    /// The last `seq` that was already in the table when the socket
    /// opened: an event at or below it is marked as replay.
    replay_through: i64,
}

/// One Host registration of one socket: the record, and the presence
/// connection that holds the machine present until it is dropped.
struct RegisteredHost {
    host: Host,
    connection: HostConnection,
}

/// The next command for the registered Host, or a future that never
/// finishes while this client registered none.
async fn next_command(host: &mut Option<RegisteredHost>) -> Option<HostCommand> {
    match host {
        Some(registered) => registered.connection.next().await,
        None => std::future::pending().await,
    }
}

impl Connection {
    /// The frame loop of one authenticated socket: the firehose out, the
    /// ephemeral streams out, a dispatched host command out, and the
    /// client's own frames in.
    async fn serve(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                event = self.events.next() => {
                    match event {
                        Some(event) => {
                            let replay = event.seq <= self.replay_through;
                            self.socket.send(ServerFrame::event(&event, replay).to_ws()).await?;
                        }
                        None => return Ok(()),
                    }
                }
                delta = self.deltas.recv() => {
                    match delta {
                        Ok(frame) if self.subscriptions.contains(&frame.channel_id) => {
                            self.socket.send(ServerFrame::delta(&frame).to_ws()).await?;
                        }
                        Ok(_) => {}
                        // Lagged: the client dedups on `seq`, so resend
                        // the accumulated state for every subscription.
                        Err(broadcast::error::RecvError::Lagged(_)) => self.catch_up_all().await?,
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }
                frame = self.progress.recv() => {
                    match frame {
                        Ok(frame) if self.subscriptions.contains(&frame.channel_id) => {
                            self.socket.send(ServerFrame::progress(&frame).to_ws()).await?;
                        }
                        Ok(_) => {}
                        // Lagged: a progress frame carries the whole line,
                        // so the catch-up state replaces what was missed.
                        Err(broadcast::error::RecvError::Lagged(_)) => self.catch_up_all().await?,
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }
                command = next_command(&mut self.host) => {
                    match command {
                        Some(command) => {
                            self.socket.send(ServerFrame::dispatch(&command).to_ws()).await?;
                        }
                        // The registry dropped this connection, which means
                        // another socket of the same machine replaced it.
                        None => self.host = None,
                    }
                }
                incoming = self.socket.recv() => {
                    let frame = match incoming {
                        None => return Ok(()),
                        Some(frame) => frame?,
                    };
                    if let WsMessage::Text(text) = frame {
                        let action = handle_client_frame(&text, &mut self.subscriptions);
                        if self.act(action).await? {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// Carry out what the client's frame asked for. `true` ends the loop;
    /// no action ends it.
    async fn act(&mut self, action: FrameAction) -> anyhow::Result<bool> {
        match action {
            FrameAction::None => {}
            FrameAction::Reply(reply) => self.socket.send(reply.to_ws()).await?,
            FrameAction::Subscribe(channel_id) => {
                // The membership check runs before the insert, so an
                // unreadable channel never reaches the delta or progress
                // filter and no in-flight text is sent.
                if readable(&self.state, &self.tenant, &channel_id).await? {
                    self.subscriptions.insert(channel_id.clone());
                    send_catch_up(&self.state, &mut self.socket, &channel_id).await?;
                } else {
                    self.socket
                        .send(ServerFrame::error("not_found", "no such channel").to_ws())
                        .await?;
                }
            }
            FrameAction::RegisterHost {
                name,
                platform,
                capabilities,
            } => {
                match register_host(&self.state, &self.tenant, &name, &platform, &capabilities)
                    .await
                {
                    Ok(registered) => {
                        self.socket
                            .send(ServerFrame::host_registered(&registered.host).to_ws())
                            .await?;
                        // The new registration replaces any older one of
                        // this socket, and dropping the old connection is
                        // what takes it out of the registry.
                        self.host = Some(registered);
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "host registration failed");
                        self.socket
                            .send(
                                ServerFrame::error(
                                    "temporarily_unavailable",
                                    "the host could not be registered",
                                )
                                .to_ws(),
                            )
                            .await?;
                    }
                }
            }
            FrameAction::HostResult { id, outcome } => {
                // A result completes a host action only when it comes
                // from the connection that received the command: the
                // presence compares the Host id and the registration
                // epoch of this socket's Host connection with those of
                // the waiting call. A socket that registered no Host
                // received no command, so its result is dropped. A
                // result that does not match is dropped, and the host
                // action keeps waiting for its own connection.
                if let Some(registered) = &self.host {
                    self.state
                        .host_presence
                        .complete(&registered.connection, &id, outcome);
                }
            }
        }
        Ok(false)
    }

    /// Resend the accumulated state of every subscription, after a
    /// broadcast stream lagged.
    async fn catch_up_all(&mut self) -> anyhow::Result<()> {
        // The set is read while the socket is written, so the ids are
        // taken first.
        let channels: Vec<String> = self.subscriptions.iter().cloned().collect();
        for channel_id in channels {
            send_catch_up(&self.state, &mut self.socket, &channel_id).await?;
        }
        Ok(())
    }
}

/// Register the client on this socket as a Host of the signed-in person,
/// and make it present. The Workspace comes from the Session, so a
/// client registers a machine of its own person and of nobody else.
async fn register_host(
    state: &Arc<AppState>,
    tenant: &Tenant,
    name: &str,
    platform: &str,
    capabilities: &[String],
) -> anyhow::Result<RegisteredHost> {
    let host = state
        .hosts
        .register(
            &tenant.workspace_id,
            name,
            platform,
            capabilities,
            state.clock.now_ms(),
        )
        .await?;
    let connection = state.host_presence.connect(&host.id);
    Ok(RegisteredHost { host, connection })
}

/// Whether the signed-in person may read the Channel: it is a Channel
/// of their own Workspace. A Channel of another tenant reads as absent,
/// which is what the REST routes answer too.
async fn readable(state: &AppState, tenant: &Tenant, channel_id: &str) -> anyhow::Result<bool> {
    Ok(state
        .channels
        .get(
            &tenant.workspace_id,
            &ChannelId::from(channel_id.to_string()),
        )
        .await?
        .is_some())
}

/// The mid-stream catch-up: every live stream in the
/// channel as one accumulated `message.delta` frame, and every live
/// run in it as one `progress.state` frame.
async fn send_catch_up(
    state: &AppState,
    socket: &mut WebSocket,
    channel_id: &str,
) -> anyhow::Result<()> {
    for frame in state.hub.snapshot(channel_id) {
        socket.send(ServerFrame::delta(&frame).to_ws()).await?;
    }
    for frame in state.progress.snapshot(channel_id) {
        socket.send(ServerFrame::progress(&frame).to_ws()).await?;
    }
    Ok(())
}

/// Wait for the auth frame. Returns `Some(last_seq)` when accepted,
/// `None` when the first frame is not one (an error frame is sent and
/// the socket closed). The cookie already proved who the client is.
async fn authenticate(socket: &mut WebSocket) -> anyhow::Result<Option<Option<String>>> {
    let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
    let frame = match first {
        Err(_) | Ok(None) => return Ok(None),
        Ok(Some(frame)) => frame?,
    };
    let WsMessage::Text(text) = frame else {
        return reject(socket).await;
    };
    match serde_json::from_str::<ClientFrame>(&text) {
        Ok(ClientFrame::Auth { last_seq }) => Ok(Some(last_seq)),
        _ => reject(socket).await,
    }
}

async fn reject(socket: &mut WebSocket) -> anyhow::Result<Option<Option<String>>> {
    socket
        .send(
            ServerFrame::error("unauthorized", "the first frame must be a valid auth frame")
                .to_ws(),
        )
        .await?;
    Ok(None)
}

enum FrameAction {
    None,
    Reply(ServerFrame),
    /// A subscribe frame. The caller checks the Channel is the
    /// tenant's, records the subscription and sends the catch-up
    /// snapshot; the parse decides none of that.
    Subscribe(String),
    /// A register-host frame. The caller writes the record in the
    /// tenant's Workspace and makes the machine present.
    RegisterHost {
        name: String,
        platform: String,
        capabilities: Vec<String>,
    },
    /// What one dispatched command did. The caller hands it on only
    /// from the Host connection this socket registered.
    HostResult {
        id: String,
        outcome: HostOutcome,
    },
}

fn handle_client_frame(text: &str, subscriptions: &mut HashSet<String>) -> FrameAction {
    match serde_json::from_str::<ClientFrame>(text) {
        Ok(ClientFrame::Ping) => FrameAction::Reply(ServerFrame::control("pong")),
        Ok(ClientFrame::Subscribe { channel_id }) => FrameAction::Subscribe(channel_id),
        Ok(ClientFrame::Unsubscribe { channel_id }) => {
            subscriptions.remove(&channel_id);
            FrameAction::None
        }
        Ok(ClientFrame::RegisterHost {
            name,
            platform,
            capabilities,
        }) => {
            let name = name.trim().to_string();
            let platform = platform.trim().to_string();
            if name.is_empty() || platform.is_empty() {
                return FrameAction::Reply(ServerFrame::error(
                    "validation",
                    "a host registers with a name and a platform",
                ));
            }
            FrameAction::RegisterHost {
                name,
                platform,
                capabilities,
            }
        }
        Ok(ClientFrame::Result {
            id,
            exit_code,
            stdout,
            stderr,
        }) => FrameAction::HostResult {
            id,
            outcome: HostOutcome {
                exit_code,
                stdout,
                stderr,
            },
        },
        Ok(ClientFrame::Auth { .. }) => {
            FrameAction::Reply(ServerFrame::error("validation", "already authenticated"))
        }
        Err(_) => FrameAction::Reply(ServerFrame::error("validation", "unknown frame")),
    }
}
