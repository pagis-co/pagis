//! A fake Coding Harness for tests: an ACP agent that a [`Script`]
//! drives.
//!
//! [`pair`] joins a [`FakeHarness`] to an [`AcpSession`] over two
//! `tokio::io::duplex` pipes, and [`serve_client_app`] runs one on each
//! stream of a session socket. The fake records each request and
//! notification that it gets, and each answer to its own requests, so a
//! test can read what the client sent. The script speaks in ACP types,
//! which this module re-exports as [`acp`].

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
pub use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, Error, JsonRpcMessage, SentRequest,
};
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use pagis_broker::OpenAnswer;
use pagis_broker::host_sessions::{REQUEST_LIMIT, read_line, yamux_config};
use serde::Serialize;
use tokio::sync::{Notify, mpsc};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::{AcpSession, AskHandler, CodingError, Opening, SessionEvent};

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
    ask: Option<Ask>,
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
            ask: None,
        }
    }

    /// Sends `updates`, then waits for `session/cancel` and ends with
    /// `cancelled`.
    #[must_use]
    pub fn until_cancel(updates: Vec<acp::SessionUpdate>) -> Self {
        Self {
            updates,
            end: TurnEnd::UntilCancel,
            ask: None,
        }
    }

    /// Sends `ask` first, then the updates. The fake then waits for the
    /// answer, records it, and ends the turn.
    #[must_use]
    pub fn asks(mut self, ask: Ask) -> Self {
        self.ask = Some(ask);
        self
    }
}

/// A request that the fake sends to the client in a turn.
#[derive(Debug, Clone)]
pub struct Ask {
    request: AskRequest,
    withdrawn: bool,
}

#[derive(Debug, Clone)]
enum AskRequest {
    Permission {
        tool_call: acp::ToolCallUpdate,
        options: Vec<acp::PermissionOption>,
    },
    Form {
        message: String,
        schema: acp::ElicitationSchema,
    },
    Url {
        message: String,
        url: String,
    },
}

impl Ask {
    /// A `session/request_permission` for `tool_call` with `options`.
    #[must_use]
    pub fn permission(tool_call: acp::ToolCallUpdate, options: Vec<acp::PermissionOption>) -> Self {
        Self::new(AskRequest::Permission { tool_call, options })
    }

    /// An `elicitation/create` in form mode.
    #[must_use]
    pub fn form(message: &str, schema: acp::ElicitationSchema) -> Self {
        Self::new(AskRequest::Form {
            message: message.to_owned(),
            schema,
        })
    }

    /// An `elicitation/create` in URL mode.
    #[must_use]
    pub fn url(message: &str, url: &str) -> Self {
        Self::new(AskRequest::Url {
            message: message.to_owned(),
            url: url.to_owned(),
        })
    }

    /// After the updates, the fake waits for [`FakeHarness::withdraw_ask`]
    /// and then sends `$/cancel_request` for the request.
    #[must_use]
    pub fn withdrawn(mut self) -> Self {
        self.withdrawn = true;
        self
    }

    fn new(request: AskRequest) -> Self {
        Self {
            request,
            withdrawn: false,
        }
    }

    /// Sends the request. The answer comes as JSON.
    fn send(
        &self,
        cx: &ConnectionTo<Client>,
        session_id: &acp::SessionId,
    ) -> SentRequest<serde_json::Value> {
        let scope = || acp::ElicitationSessionScope::new(session_id.clone());
        match &self.request {
            AskRequest::Permission { tool_call, options } => cx
                .send_request(acp::RequestPermissionRequest::new(
                    session_id.clone(),
                    tool_call.clone(),
                    options.clone(),
                ))
                .map(to_json),
            AskRequest::Form { message, schema } => cx
                .send_request(acp::CreateElicitationRequest::new(
                    acp::ElicitationFormMode::new(scope(), schema.clone()),
                    message.clone(),
                ))
                .map(to_json),
            AskRequest::Url { message, url } => cx
                .send_request(acp::CreateElicitationRequest::new(
                    acp::ElicitationUrlMode::new(scope(), "fake-elicitation", url.clone()),
                    message.clone(),
                ))
                .map(to_json),
        }
    }
}

fn to_json(response: impl Serialize) -> Result<serde_json::Value, Error> {
    serde_json::to_value(response).map_err(Error::into_internal_error)
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
    answers: Mutex<Vec<Result<serde_json::Value, Error>>>,
    cancel: Notify,
    withdraw: Notify,
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

    fn record_answer(&self, answer: Result<serde_json::Value, Error>) {
        self.answers
            .lock()
            .expect("the fake's answer lock is not poisoned")
            .push(answer);
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
            answers: Mutex::new(Vec::new()),
            cancel: Notify::new(),
            withdraw: Notify::new(),
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

    /// The answer to each request of the fake, in order: the result as
    /// JSON, or the error.
    #[must_use]
    pub fn answers(&self) -> Vec<Result<serde_json::Value, Error>> {
        self.state
            .answers
            .lock()
            .expect("the fake's answer lock is not poisoned")
            .clone()
    }

    /// Sends `$/cancel_request` for the open request of an [`Ask`] that is
    /// [`withdrawn`](Ask::withdrawn).
    pub fn withdraw_ask(&self) {
        self.state.withdraw.notify_one();
    }

    /// Ends the fake's connection, so the client's side of the stream
    /// closes.
    pub fn close_stream(&self) {
        self.state.close.notify_one();
    }
}

/// Joins a new fake harness to a new [`AcpSession`] over two
/// `tokio::io::duplex` pipes, and opens the session with `asks`.
pub async fn pair(
    script: Script,
    opening: Opening,
    asks: Arc<dyn AskHandler>,
) -> (
    FakeHarness,
    Result<(AcpSession, mpsc::UnboundedReceiver<SessionEvent>), CodingError>,
) {
    let (client_out, harness_in) = tokio::io::duplex(64 * 1024);
    let (harness_out, client_in) = tokio::io::duplex(64 * 1024);
    let harness = FakeHarness::serve(script, harness_out.compat_write(), harness_in.compat());
    let opened =
        AcpSession::open(client_out.compat_write(), client_in.compat(), opening, asks).await;
    (harness, opened)
}

/// Serves the Client App's end of a session socket: it answers each open
/// request with `ok` in `cwd` and runs a fake harness of `script` on the
/// stream. It returns when the socket ends.
pub async fn serve_client_app<T>(socket: T, script: Script, cwd: String)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut connection = yamux::Connection::new(socket, yamux_config(), yamux::Mode::Server);
    while let Some(Ok(mut stream)) =
        futures::future::poll_fn(|cx| connection.poll_next_inbound(cx)).await
    {
        let script = script.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            if read_line(&mut stream, REQUEST_LIMIT).await.is_err() {
                return;
            }
            let answer = OpenAnswer::Opened { cwd }.line();
            if stream.write_all(answer.as_bytes()).await.is_err() || stream.flush().await.is_err() {
                return;
            }
            let (read, write) = stream.split();
            FakeHarness::serve(script, write, read);
        });
    }
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
                        let asked = turn
                            .ask
                            .as_ref()
                            .map(|ask| (ask.withdrawn, ask.send(&cx, &session_id)));
                        for update in turn.updates {
                            cx.send_notification(acp::SessionNotification::new(
                                session_id.clone(),
                                update,
                            ))?;
                        }
                        if let Some((withdrawn, sent)) = asked {
                            if withdrawn {
                                state.withdraw.notified().await;
                                sent.cancel()?;
                            }
                            // The answer does not change the turn.
                            state.record_answer(sent.block_task().await);
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
