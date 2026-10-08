use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Error};
use futures::io::{AsyncRead, AsyncWrite};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::{CodingError, SessionEvent, StopReason};

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
    /// `SessionEvent::Closed` last.
    pub async fn open<W, R>(
        outgoing: W,
        incoming: R,
        opening: Opening,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SessionEvent>), CodingError>
    where
        W: AsyncWrite + Send + 'static,
        R: AsyncRead + Send + 'static,
    {
        let flags = Arc::new(Flags::default());
        let (events, receiver) = mpsc::unbounded_channel();
        let (connection_tx, connection_rx) = oneshot::channel();
        let (close, close_rx) = oneshot::channel::<()>();
        tokio::spawn(run_connection(
            ByteStreams::new(outgoing, incoming),
            flags.clone(),
            events.clone(),
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
    pub fn cancel(&self) -> Result<(), CodingError> {
        if self.is_closed() {
            return Err(CodingError::Closed);
        }
        self.connection
            .send_notification(acp::CancelNotification::new(self.session_id.clone()))
            .map_err(CodingError::from_acp)
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
/// methods.
fn initialize_request() -> acp::InitializeRequest {
    acp::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .fs(acp::FileSystemCapabilities::new()
                    .read_text_file(false)
                    .write_text_file(false))
                .terminal(false)
                .auth(acp::AuthCapabilities::new().terminal(true)),
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
    events: mpsc::UnboundedSender<SessionEvent>,
    connection_tx: oneshot::Sender<ConnectionTo<Agent>>,
    close: oneshot::Receiver<()>,
) where
    W: AsyncWrite + Send + 'static,
    R: AsyncRead + Send + 'static,
{
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
        // The SDK keeps an unhandled request that names a session for a
        // later handler, so the harness would wait for ever. Pagis answers
        // no permission request, so it answers each with an error.
        .on_receive_request(
            async move |request: acp::RequestPermissionRequest, responder, _cx| {
                tracing::debug!(
                    tool_call = %request.tool_call.tool_call_id.0,
                    "a Coding Harness asks permission, and the ACP client refuses"
                );
                responder.respond_with_error(Error::method_not_found())
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
