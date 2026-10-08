//! A fake Coding Harness for tests: an ACP agent that a [`Script`]
//! drives.
//!
//! [`pair`] joins a [`FakeHarness`] to an [`AcpSession`] over two
//! `tokio::io::duplex` pipes. The fake records each request and
//! notification that it gets, so a test can read what the client sent.
//! The script speaks in ACP types, which this module re-exports as
//! [`acp`].

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
pub use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Error, JsonRpcMessage};
use futures::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Notify, mpsc};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::{AcpSession, CodingError, Opening, SessionEvent};

/// The session id that the fake gives to a new session.
pub const NEW_SESSION_ID: &str = "fake-session";

/// What the fake declares and how it answers.
#[derive(Debug, Clone)]
pub struct Script {
    name: String,
    version: String,
    protocol_version: u16,
    resume: bool,
    load: bool,
    auth_methods: Vec<acp::AuthMethod>,
    new_session_auth_required: bool,
    turns: Vec<Turn>,
    history: Vec<acp::SessionUpdate>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            name: "fake-harness".to_owned(),
            version: "1.0.0".to_owned(),
            protocol_version: 1,
            resume: false,
            load: false,
            auth_methods: Vec::new(),
            new_session_auth_required: false,
            turns: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl Script {
    /// The `agentInfo` that `initialize` gives.
    #[must_use]
    pub fn agent(mut self, name: &str, version: &str) -> Self {
        self.name = name.to_owned();
        self.version = version.to_owned();
        self
    }

    /// The protocol version that `initialize` answers. It is 1 by default.
    #[must_use]
    pub fn protocol_version(mut self, version: u16) -> Self {
        self.protocol_version = version;
        self
    }

    /// Declares `sessionCapabilities.resume`.
    #[must_use]
    pub fn resume(mut self) -> Self {
        self.resume = true;
        self
    }

    /// Declares `loadSession`.
    #[must_use]
    pub fn load(mut self) -> Self {
        self.load = true;
        self
    }

    /// Adds one of the `authMethods` that `initialize` gives.
    #[must_use]
    pub fn auth_method(mut self, method: acp::AuthMethod) -> Self {
        self.auth_methods.push(method);
        self
    }

    /// Makes `session/new` fail with `AuthRequired`.
    #[must_use]
    pub fn new_session_auth_required(mut self) -> Self {
        self.new_session_auth_required = true;
        self
    }

    /// Adds the turn that answers the next prompt. A prompt after the last
    /// scripted turn ends at once with `end_turn`.
    #[must_use]
    pub fn turn(mut self, turn: Turn) -> Self {
        self.turns.push(turn);
        self
    }

    /// The updates that `session/load` replays before it answers.
    #[must_use]
    pub fn history(mut self, updates: Vec<acp::SessionUpdate>) -> Self {
        self.history = updates;
        self
    }
}

/// How the fake answers one prompt.
#[derive(Debug, Clone)]
pub struct Turn {
    updates: Vec<acp::SessionUpdate>,
    end: TurnEnd,
    asks_permission: bool,
}

#[derive(Debug, Clone)]
enum TurnEnd {
    Stop(acp::StopReason),
    UntilCancel,
}

impl Turn {
    /// Sends `updates`, then ends with `stop_reason`.
    #[must_use]
    pub fn new(updates: Vec<acp::SessionUpdate>, stop_reason: acp::StopReason) -> Self {
        Self {
            updates,
            end: TurnEnd::Stop(stop_reason),
            asks_permission: false,
        }
    }

    /// Sends `updates`, then waits for `session/cancel` and ends with
    /// `cancelled`.
    #[must_use]
    pub fn until_cancel(updates: Vec<acp::SessionUpdate>) -> Self {
        Self {
            updates,
            end: TurnEnd::UntilCancel,
            asks_permission: false,
        }
    }

    /// Sends one `session/request_permission` first, and waits for its
    /// answer.
    #[must_use]
    pub fn asks_permission(mut self) -> Self {
        self.asks_permission = true;
        self
    }
}

/// One request or notification that the fake got.
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub method: String,
    pub params: serde_json::Value,
}

/// A running fake harness.
#[derive(Clone)]
pub struct FakeHarness {
    state: Arc<State>,
}

struct State {
    script: Script,
    turns: Mutex<VecDeque<Turn>>,
    received: Mutex<Vec<Received>>,
    cancel: Notify,
    close: Notify,
}

impl State {
    fn record(&self, message: &impl JsonRpcMessage) {
        let params = message
            .to_untyped_message()
            .map(|untyped| untyped.params().clone())
            .unwrap_or_else(|error| serde_json::json!({ "error": error.message }));
        self.received
            .lock()
            .expect("the fake's record lock is not poisoned")
            .push(Received {
                method: message.method().to_owned(),
                params,
            });
    }

    fn next_turn(&self) -> Option<Turn> {
        self.turns
            .lock()
            .expect("the fake's turn lock is not poisoned")
            .pop_front()
    }
}

impl FakeHarness {
    /// Serves `script` over one byte stream, on a new tokio task.
    pub fn serve<W, R>(script: Script, outgoing: W, incoming: R) -> Self
    where
        W: AsyncWrite + Send + 'static,
        R: AsyncRead + Send + 'static,
    {
        let state = Arc::new(State {
            turns: Mutex::new(script.turns.iter().cloned().collect()),
            script,
            received: Mutex::new(Vec::new()),
            cancel: Notify::new(),
            close: Notify::new(),
        });
        let connection = connect(state.clone(), ByteStreams::new(outgoing, incoming));
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::warn!(error = %error.message, "the fake harness failed");
            }
        });
        Self { state }
    }

    /// Each request and notification that the fake got, in order.
    #[must_use]
    pub fn received(&self) -> Vec<Received> {
        self.state
            .received
            .lock()
            .expect("the fake's record lock is not poisoned")
            .clone()
    }

    /// The params of each message of `method` that the fake got.
    #[must_use]
    pub fn params(&self, method: &str) -> Vec<serde_json::Value> {
        self.received()
            .into_iter()
            .filter(|received| received.method == method)
            .map(|received| received.params)
            .collect()
    }

    /// Ends the fake's connection, so the client's side of the stream
    /// closes.
    pub fn close_stream(&self) {
        self.state.close.notify_one();
    }
}

/// Joins a new fake harness to a new [`AcpSession`] over two
/// `tokio::io::duplex` pipes, and opens the session.
pub async fn pair(
    script: Script,
    opening: Opening,
) -> (
    FakeHarness,
    Result<(AcpSession, mpsc::UnboundedReceiver<SessionEvent>), CodingError>,
) {
    let (client_out, harness_in) = tokio::io::duplex(64 * 1024);
    let (harness_out, client_in) = tokio::io::duplex(64 * 1024);
    let harness = FakeHarness::serve(script, harness_out.compat_write(), harness_in.compat());
    let opened = AcpSession::open(client_out.compat_write(), client_in.compat(), opening).await;
    (harness, opened)
}

/// A new session at `cwd`.
#[must_use]
pub fn new_session(cwd: &str) -> Opening {
    Opening::New {
        cwd: PathBuf::from(cwd),
    }
}

async fn connect<W, R>(state: Arc<State>, transport: ByteStreams<W, R>) -> Result<(), Error>
where
    W: AsyncWrite + Send + 'static,
    R: AsyncRead + Send + 'static,
{
    let script = state.script.clone();
    Agent
        .builder()
        .name("fake-harness")
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: acp::InitializeRequest, responder, _cx| {
                    state.record(&request);
                    let mut sessions = acp::SessionCapabilities::new();
                    if script.resume {
                        sessions = sessions.resume(acp::SessionResumeCapabilities::new());
                    }
                    responder.respond(
                        acp::InitializeResponse::new(ProtocolVersion::from(
                            script.protocol_version,
                        ))
                        .agent_capabilities(
                            acp::AgentCapabilities::new()
                                .load_session(script.load)
                                .session_capabilities(sessions),
                        )
                        .auth_methods(script.auth_methods.clone())
                        .agent_info(acp::Implementation::new(
                            script.name.clone(),
                            script.version.clone(),
                        )),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: acp::NewSessionRequest, responder, _cx| {
                    state.record(&request);
                    if state.script.new_session_auth_required {
                        return responder.respond_with_error(Error::auth_required());
                    }
                    responder.respond(acp::NewSessionResponse::new(NEW_SESSION_ID))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: acp::ResumeSessionRequest, responder, _cx| {
                    state.record(&request);
                    responder.respond(acp::ResumeSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: acp::LoadSessionRequest,
                            responder,
                            cx: ConnectionTo<Client>| {
                    state.record(&request);
                    for update in &state.script.history {
                        cx.send_notification(acp::SessionNotification::new(
                            request.session_id.clone(),
                            update.clone(),
                        ))?;
                    }
                    responder.respond(acp::LoadSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: acp::PromptRequest, responder, cx: ConnectionTo<Client>| {
                    state.record(&request);
                    let turn = state.next_turn();
                    let state = state.clone();
                    let session_id = request.session_id;
                    // A handler that awaits blocks the dispatch loop, so the
                    // turn runs on a task of its own.
                    cx.clone().spawn(async move {
                        let Some(turn) = turn else {
                            return responder
                                .respond(acp::PromptResponse::new(acp::StopReason::EndTurn));
                        };
                        if turn.asks_permission {
                            // The answer does not change the turn.
                            let _answer = cx
                                .send_request(permission_request(session_id.clone()))
                                .block_task()
                                .await;
                        }
                        for update in turn.updates {
                            cx.send_notification(acp::SessionNotification::new(
                                session_id.clone(),
                                update,
                            ))?;
                        }
                        let stop_reason = match turn.end {
                            TurnEnd::Stop(stop_reason) => stop_reason,
                            TurnEnd::UntilCancel => {
                                state.cancel.notified().await;
                                acp::StopReason::Cancelled
                            }
                        };
                        responder.respond(acp::PromptResponse::new(stop_reason))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |notification: acp::CancelNotification, _cx| {
                    state.record(&notification);
                    state.cancel.notify_one();
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Client>| {
            let incoming_closed = pin!(cx.incoming_closed());
            let close = pin!(state.close.notified());
            futures::future::select(incoming_closed, close).await;
            Ok(())
        })
        .await
}

fn permission_request(session_id: acp::SessionId) -> acp::RequestPermissionRequest {
    acp::RequestPermissionRequest::new(
        session_id,
        acp::ToolCallUpdate::new("permission-call", acp::ToolCallUpdateFields::new()),
        vec![acp::PermissionOption::new(
            "allow",
            "Allow",
            acp::PermissionOptionKind::AllowOnce,
        )],
    )
}
