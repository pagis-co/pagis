use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, Error, JsonRpcResponse, RequestCancellation,
    Responder,
};
use futures::io::{AsyncRead, AsyncWrite};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::event::raw_json;
use crate::{
    AskHandler, CodingError, PermissionAnswer, PermissionAsk, QuestionAnswer, QuestionAsk,
    SessionEvent, StopReason, ToolKind,
};

/// How `AcpSession::open` starts the ACP session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// A new session in `cwd`.
    New { cwd: PathBuf },
    /// The session that the harness knows as `acp_session_id`, with
    /// `session/resume` or `session/load`.
    Restore {
        acp_session_id: String,
        cwd: PathBuf,
    },
}

/// What the harness declared in `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// The name in `agentInfo`, or `None` when the harness sends none.
    pub name: Option<String>,
    /// The version in `agentInfo`, or `None` when the harness sends none.
    pub version: Option<String>,
    /// The harness declares `sessionCapabilities.resume`.
    pub can_resume: bool,
    /// The harness declares `loadSession`.
    pub can_load: bool,
    /// The terminal sign-in methods of `authMethods`. A method of the
    /// `agent` type is not kept: Pagis never sends `authenticate`.
    pub sign_in_methods: Vec<SignInMethod>,
}

/// One terminal sign-in method of a harness: the harness's own program
/// with `args` and `env`, in an interactive terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInMethod {
    pub id: String,
    pub name: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// One ACP session of one Coding Harness, over one byte stream.
///
/// Dropping the session ends the connection, as `close` does.
pub struct AcpSession {
    connection: ConnectionTo<Agent>,
    harness: HarnessInfo,
    session_id: acp::SessionId,
    flags: Arc<Flags>,
    events: mpsc::UnboundedSender<SessionEvent>,
    /// Counts the cancels, so that each ask that waits at a cancel ends.
    cancels: watch::Sender<u64>,
    close: oneshot::Sender<()>,
}

/// The state that the dispatch loop and the caller share.
#[derive(Default)]
struct Flags {
    /// A prompt waits for its answer.
    turn_runs: AtomicBool,
    /// `session/load` replays the history, which the transcript holds
    /// already.
    replaying: AtomicBool,
    /// The connection ended.
    closed: AtomicBool,
}

impl AcpSession {
    /// Connects over `outgoing` and `incoming`, the two halves of one byte
    /// stream, and opens the session.
    ///
    /// The connection runs on a new tokio task. The receiver gets the
    /// events of the session in the order that the harness sent them, and
    /// `SessionEvent::Closed` last. `asks` answers each permission request
    /// and each question of the harness.
    pub async fn open<W, R>(
        outgoing: W,
        incoming: R,
        opening: Opening,
        asks: Arc<dyn AskHandler>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionEvent>), CodingError>
    where
        W: AsyncWrite + Send + 'static,
        R: AsyncRead + Send + 'static,
    {
        let flags = Arc::new(Flags::default());
        let (events, receiver) = mpsc::unbounded_channel();
        let (cancels, cancels_rx) = watch::channel(0);
        let (connection_tx, connection_rx) = oneshot::channel();
        let (close, close_rx) = oneshot::channel::<()>();
        tokio::spawn(run_connection(
            ByteStreams::new(outgoing, incoming),
            flags.clone(),
            Asks {
                handler: asks,
                cancels: cancels_rx,
                events: events.clone(),
            },
            connection_tx,
            close_rx,
        ));
        let connection = connection_rx.await.map_err(|_| CodingError::Closed)?;

        let initialized = connection
            .send_request(initialize_request())
            .block_task()
            .await
            .map_err(CodingError::from_acp)?;
        if initialized.protocol_version != ProtocolVersion::V1 {
            return Err(CodingError::Protocol(format!(
                "the harness answers protocol version {}, and Pagis speaks version 1",
                initialized.protocol_version
            )));
        }
        let harness = HarnessInfo::from_initialize(initialized);

        let session_id = match opening {
            Opening::New { cwd } => {
                connection
                    .send_request(acp::NewSessionRequest::new(cwd))
                    .block_task()
                    .await
                    .map_err(CodingError::from_acp)?
                    .session_id
            }
            Opening::Restore {
                acp_session_id,
                cwd,
            } => {
                let session_id = acp::SessionId::new(acp_session_id);
                if harness.can_resume {
                    connection
                        .send_request(acp::ResumeSessionRequest::new(session_id.clone(), cwd))
                        .block_task()
                        .await
                        .map_err(CodingError::from_acp)?;
                } else if harness.can_load {
                    load(
                        &connection,
                        &flags,
                        acp::LoadSessionRequest::new(session_id.clone(), cwd),
                    )
                    .await?;
                } else {
                    return Err(CodingError::CannotRestore);
                }
                session_id
            }
        };

        let session = Self {
            connection,
            harness,
            session_id,
            flags,
            events,
            cancels,
            close,
        };
        Ok((session, receiver))
    }

    /// What the harness declared in `initialize`.
    #[must_use]
    pub fn harness(&self) -> &HarnessInfo {
        &self.harness
    }

    /// The harness's own session id, which the record keeps for a resume.
    #[must_use]
    pub fn acp_session_id(&self) -> &str {
        &self.session_id.0
    }

    /// Sends one `session/prompt` with one text block, and returns at once.
    ///
    /// The turn's updates come as events, then `TurnEnded` or `TurnFailed`.
    /// A prompt while a turn runs is `CodingError::Busy`, because ACP v1
    /// has no steering.
    pub fn prompt(&self, text: impl Into<String>) -> Result<(), CodingError> {
        if self.is_closed() {
            return Err(CodingError::Closed);
        }
        if self
            .flags
            .turn_runs
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CodingError::Busy);
        }
        let request = acp::PromptRequest::new(
            self.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
        );
        let flags = self.flags.clone();
        let events = self.events.clone();
        // The callback runs in dispatch order, after each update that the
        // harness sent before its answer.
        let sent = self
            .connection
            .prepare_request(request)
            .on_receiving_result(move |result| async move {
                let event = match result {
                    Ok(response) => match StopReason::from_acp(response.stop_reason) {
                        Some(stop_reason) => SessionEvent::TurnEnded { stop_reason },
                        None => SessionEvent::TurnFailed {
                            message: format!("unknown stop reason {:?}", response.stop_reason),
                        },
                    },
                    Err(error) => SessionEvent::TurnFailed {
                        message: error.message,
                    },
                };
                flags.turn_runs.store(false, Ordering::SeqCst);
                // The caller can drop the receiver; the turn ends all the same.
                let _ = events.send(event);
                Ok(())
            });
        sent.map_err(|error| {
            self.flags.turn_runs.store(false, Ordering::SeqCst);
            CodingError::from_acp(error)
        })
    }

    /// Sends `session/cancel`. The turn still ends with its own
    /// `TurnEnded { stop_reason: cancelled }`.
    ///
    /// Each permission request that waits gets the answer `cancelled`, as
    /// ACP requires, and each question that waits gets `cancel`. Each of
    /// them gives `SessionEvent::AskWithdrawn`.
    pub fn cancel(&self) -> Result<(), CodingError> {
        if self.is_closed() {
            return Err(CodingError::Closed);
        }
        self.connection
            .send_notification(acp::CancelNotification::new(self.session_id.clone()))
            .map_err(CodingError::from_acp)?;
        self.cancels.send_modify(|cancels| *cancels += 1);
        Ok(())
    }

    /// Ends the connection. The harness process ends when its byte stream
    /// closes.
    pub fn close(self) {
        // The connection task can be gone already; it then ended by itself.
        let _ = self.close.send(());
    }

    fn is_closed(&self) -> bool {
        self.flags.closed.load(Ordering::SeqCst) || self.connection.is_incoming_closed()
    }
}

impl HarnessInfo {
    fn from_initialize(response: acp::InitializeResponse) -> Self {
        let capabilities = response.agent_capabilities;
        let sign_in_methods = response
            .auth_methods
            .into_iter()
            .filter_map(|method| match method {
                acp::AuthMethod::Terminal(terminal) => Some(SignInMethod {
                    id: terminal.id.0.to_string(),
                    name: terminal.name,
                    args: terminal.args,
                    env: terminal.env.into_iter().collect(),
                }),
                _ => None,
            })
            .collect();
        Self {
            name: response.agent_info.as_ref().map(|info| info.name.clone()),
            version: response.agent_info.map(|info| info.version),
            can_resume: capabilities.session_capabilities.resume.is_some(),
            can_load: capabilities.load_session,
            sign_in_methods,
        }
    }
}

/// `initialize` with protocol 1. The harness uses its own file system and
/// terminal where it runs, and it offers its own sign-in as terminal
/// methods. It asks its questions in form mode only, because Pagis opens
/// no URL for a harness.
fn initialize_request() -> acp::InitializeRequest {
    acp::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .fs(acp::FileSystemCapabilities::new()
                    .read_text_file(false)
                    .write_text_file(false))
                .terminal(false)
                .auth(acp::AuthCapabilities::new().terminal(true))
                .elicitation(
                    acp::ElicitationCapabilities::new()
                        .form(acp::ElicitationFormCapabilities::new()),
                ),
        )
        .client_info(acp::Implementation::new("pagis", env!("CARGO_PKG_VERSION")))
}

/// Sends `session/load` and drops the history that it replays. The
/// callback ends the replay in dispatch order, so the first update after
/// the answer is an event again.
async fn load(
    connection: &ConnectionTo<Agent>,
    flags: &Arc<Flags>,
    request: acp::LoadSessionRequest,
) -> Result<(), CodingError> {
    let (answer_tx, answer_rx) = oneshot::channel();
    flags.replaying.store(true, Ordering::SeqCst);
    let replay_flags = flags.clone();
    connection
        .prepare_request(request)
        .on_receiving_result(move |result| async move {
            replay_flags.replaying.store(false, Ordering::SeqCst);
            // `open` can be gone already; then nobody waits for the answer.
            let _ = answer_tx.send(result);
            Ok(())
        })
        .map_err(CodingError::from_acp)?;
    answer_rx
        .await
        .map_err(|_| CodingError::Closed)?
        .map_err(CodingError::from_acp)?;
    Ok(())
}

/// Runs the ACP client until the incoming side closes or `close` is
/// called, then sends `SessionEvent::Closed`.
async fn run_connection<W, R>(
    transport: ByteStreams<W, R>,
    flags: Arc<Flags>,
    asks: Asks,
    connection_tx: oneshot::Sender<ConnectionTo<Agent>>,
    close: oneshot::Receiver<()>,
) where
    W: AsyncWrite + Send + 'static,
    R: AsyncRead + Send + 'static,
{
    let events = asks.events.clone();
    let result = Client
        .builder()
        .name("pagis")
        .on_receive_notification(
            {
                let flags = flags.clone();
                let events = events.clone();
                async move |notification: acp::SessionNotification, _cx| {
                    if flags.replaying.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    if let Some(event) = SessionEvent::from_update(notification.update) {
                        // The caller can drop the receiver and keep the session.
                        let _ = events.send(event);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        // A handler runs on the dispatch loop, which handles no other
        // message while it runs. So each handler moves the wait for the
        // answer to a task of the connection.
        .on_receive_request(
            {
                let asks = asks.clone();
                async move |request: acp::RequestPermissionRequest,
                            responder: Responder<acp::RequestPermissionResponse>,
                            cx: ConnectionTo<Agent>| {
                    let ask = PermissionAsk::from_acp(responder.id(), &request.tool_call);
                    let handler = asks.handler.clone();
                    let waited = asks.wait(
                        ask.ask_id.clone(),
                        async move { handler.permission(ask).await },
                        responder.cancellation(),
                    );
                    let options = request.options;
                    cx.spawn(async move {
                        let answer = match waited.await {
                            Waited::Answered(answer) => Ok(permission_response(answer, &options)),
                            Waited::Cancelled => Ok(acp::RequestPermissionResponse::new(
                                acp::RequestPermissionOutcome::Cancelled,
                            )),
                            Waited::Withdrawn => Err(Error::request_cancelled()),
                        };
                        respond(responder, answer);
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let asks = asks.clone();
                async move |request: acp::CreateElicitationRequest,
                            responder: Responder<acp::CreateElicitationResponse>,
                            cx: ConnectionTo<Agent>| {
                    let acp::ElicitationMode::Form(form) = request.mode else {
                        // Pagis declares form mode only.
                        return responder.respond(acp::CreateElicitationResponse::new(
                            acp::ElicitationAction::Decline,
                        ));
                    };
                    let ask = QuestionAsk {
                        ask_id: responder.id().to_string(),
                        message: request.message,
                        schema: raw_json(&form.requested_schema),
                    };
                    let handler = asks.handler.clone();
                    let waited = asks.wait(
                        ask.ask_id.clone(),
                        async move { handler.question(ask).await },
                        responder.cancellation(),
                    );
                    cx.spawn(async move {
                        let answer = match waited.await {
                            Waited::Answered(answer) => Ok(elicitation_response(answer)),
                            Waited::Cancelled => Ok(acp::CreateElicitationResponse::new(
                                acp::ElicitationAction::Cancel,
                            )),
                            Waited::Withdrawn => Err(Error::request_cancelled()),
                        };
                        respond(responder, answer);
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
            if connection_tx.send(connection.clone()).is_err() {
                // `open` is gone, so nobody uses the connection.
                return Ok(());
            }
            let incoming_closed = pin!(connection.incoming_closed());
            futures::future::select(incoming_closed, close).await;
            Ok(())
        })
        .await;
    if let Err(error) = result {
        tracing::warn!(error = %error.message, "the ACP connection of a Coding Session failed");
    }
    flags.closed.store(true, Ordering::SeqCst);
    let _ = events.send(SessionEvent::Closed);
}

/// What the handlers of the permission requests and the questions share.
#[derive(Clone)]
struct Asks {
    handler: Arc<dyn AskHandler>,
    cancels: watch::Receiver<u64>,
    events: mpsc::UnboundedSender<SessionEvent>,
}

/// How the wait for an answer ended.
enum Waited<T> {
    Answered(T),
    /// `AcpSession::cancel` was called.
    Cancelled,
    /// The harness sent `$/cancel_request` for the request.
    Withdrawn,
}

impl Asks {
    /// Waits for the first of `answer`, a cancel of the session, and the
    /// withdrawal of the request by the harness. In the last two cases it
    /// drops `answer` and sends `SessionEvent::AskWithdrawn`.
    ///
    /// It reads the cancel count when it is called, on the dispatch loop,
    /// so each cancel after the request ends the wait.
    fn wait<T>(
        &self,
        ask_id: String,
        answer: impl Future<Output = T> + Send + 'static,
        withdrawal: RequestCancellation,
    ) -> impl Future<Output = Waited<T>> + Send + 'static
    where
        T: Send,
    {
        let mut cancels = self.cancels.clone();
        let asked_at = *cancels.borrow();
        let events = self.events.clone();
        async move {
            let session_cancelled = async move {
                let cancelled = cancels
                    .wait_for(|cancels| *cancels != asked_at)
                    .await
                    .is_ok();
                if !cancelled {
                    // The session was dropped, so the connection ends and
                    // this task with it.
                    std::future::pending::<()>().await;
                }
            };
            let waited = tokio::select! {
                answer = answer => return Waited::Answered(answer),
                () = session_cancelled => Waited::Cancelled,
                () = withdrawal.cancelled() => Waited::Withdrawn,
            };
            // The caller can drop the receiver; the wait ends all the same.
            let _ = events.send(SessionEvent::AskWithdrawn { ask_id });
            waited
        }
    }
}

impl PermissionAsk {
    fn from_acp(id: &acp::RequestId, tool_call: &acp::ToolCallUpdate) -> Self {
        let fields = &tool_call.fields;
        Self {
            ask_id: id.to_string(),
            tool_call_id: tool_call.tool_call_id.0.to_string(),
            title: fields.title.clone(),
            kind: fields.kind.map_or(ToolKind::Other, ToolKind::from_acp),
            locations: fields
                .locations
                .iter()
                .flatten()
                .map(|location| location.path.clone())
                .collect(),
            raw_input: fields.raw_input.clone(),
        }
    }
}

/// Selects the offered option of the answer's kind. When the harness
/// offers no option of that kind, the answer is `cancelled`.
fn permission_response(
    answer: PermissionAnswer,
    options: &[acp::PermissionOption],
) -> acp::RequestPermissionResponse {
    let kind = match answer {
        PermissionAnswer::AllowOnce => acp::PermissionOptionKind::AllowOnce,
        PermissionAnswer::RejectOnce => acp::PermissionOptionKind::RejectOnce,
    };
    let outcome = options.iter().find(|option| option.kind == kind).map_or(
        acp::RequestPermissionOutcome::Cancelled,
        |option| {
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        },
    );
    acp::RequestPermissionResponse::new(outcome)
}

/// The ACP answer to a question. A value that ACP cannot carry makes the
/// answer `cancel`, so the harness never gets a part of a form.
fn elicitation_response(answer: QuestionAnswer) -> acp::CreateElicitationResponse {
    let action = match answer {
        QuestionAnswer::Accept(values) => {
            let content = values
                .into_iter()
                .map(|(name, value)| serde_json::from_value(value).map(|value| (name, value)))
                .collect::<Result<BTreeMap<String, acp::ElicitationContentValue>, _>>();
            match content {
                Ok(content) => acp::ElicitationAction::Accept(
                    acp::ElicitationAcceptAction::new().content(content),
                ),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "an answer to a harness question has a value that ACP cannot carry"
                    );
                    acp::ElicitationAction::Cancel
                }
            }
        }
        QuestionAnswer::Decline => acp::ElicitationAction::Decline,
        QuestionAnswer::Cancel => acp::ElicitationAction::Cancel,
    };
    acp::CreateElicitationResponse::new(action)
}

/// Sends the answer to a request of the harness. A send fails only when
/// the connection closed, and `SessionEvent::Closed` tells the caller.
fn respond<T: JsonRpcResponse>(responder: Responder<T>, answer: Result<T, Error>) {
    if let Err(error) = responder.respond_with_result(answer) {
        tracing::warn!(
            error = %error.message,
            "the answer to a request of a Coding Harness was not sent"
        );
    }
}
