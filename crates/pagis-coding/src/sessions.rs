//! The Coding Sessions of the daemon (ADR-0033): the start on a place,
//! the transcript of each update, the prompts, the cancel, the close and
//! the exit of the harness process.
//!
//! [`CodingSessions`] runs one tokio task for each live session. The task
//! holds the [`AcpSession`] and its events, and it is the one writer of
//! the record and the transcript while the session lives. The calls of
//! the API and the asks of the harness reach it as commands on a channel.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::io::AsyncReadExt;
use pagis_core::{
    AgentId, Clock, CodingSession, CodingSessionEventKind as Kind, CodingSessionId,
    CodingSessionPlace, CodingSessionState as State, CodingSessionStore, CodingSessionUsage,
    EventBus, HostId, MessageId, NewCodingSessionEvent, NewEvent, RunId, RunStore,
    SessionApprovalMode, StoreError, WorkspaceId, harness,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    AcpSession, AskHandler, CodingError, OpenFailure, OpenRequest, Opening, PermissionAnswer,
    PermissionAsk, QuestionAnswer, QuestionAsk, SessionDecisions, SessionEvent, SessionExit,
    SessionPlace, WorktreeRequest,
};

/// The event that each write to a transcript publishes.
pub const UPDATED_EVENT: &str = "coding_session.updated";

/// How long a session waits for the exit report of its process after
/// the stream of the process closed.
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// What [`CodingSessions`] is built from.
pub struct CodingSessionsDeps {
    pub sessions: Arc<dyn CodingSessionStore>,
    /// The Run that starts a session names the Thread that shows it.
    pub runs: Arc<dyn RunStore>,
    pub place: Arc<dyn SessionPlace>,
    pub decisions: Arc<dyn SessionDecisions>,
    pub bus: Arc<dyn EventBus>,
    pub clock: Arc<dyn Clock>,
    /// Ends the task of each live session when the daemon stops.
    pub cancel: CancellationToken,
}

/// A Coding Session that an Agent starts on a Host.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCodingSession {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    /// The Run that starts the session. Its Channel and its Thread show
    /// the session.
    pub run_id: RunId,
    pub host_id: HostId,
    /// The id of the harness in the Harness Catalog.
    pub harness_id: String,
    /// The directory that the Agent named.
    pub directory: String,
    pub worktree: Option<WorktreeRequest>,
    pub approval_mode: SessionApprovalMode,
    pub title: String,
    /// The first prompt.
    pub prompt: String,
}

/// Why a session did not start. A failure after the record is written
/// names the session, which is then `failed`.
#[derive(Debug, thiserror::Error)]
pub enum StartFailure {
    #[error("the Coding Harness {0:?} is not in the Harness Catalog")]
    UnknownHarness(String),
    #[error("the Workspace has no Run {0}")]
    RunNotFound(RunId),
    /// The Run has no Channel, so no Thread can show the session.
    #[error("the Run has no conversation that can show the session")]
    NoConversation,
    #[error("{failure}")]
    Open {
        session_id: CodingSessionId,
        failure: OpenFailure,
    },
    /// The harness failed before the first prompt. The message is
    /// harness text.
    #[error("the Coding Harness failed: {message}")]
    Harness {
        session_id: CodingSessionId,
        message: String,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What happened to a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    /// The prompt started a turn.
    Sent,
    /// A turn runs, so the prompt waits for its end.
    Queued,
}

/// Why a call on a session did nothing.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The Workspace holds no such session.
    #[error("the Workspace has no such Coding Session")]
    NotFound,
    /// The session does not take the call in its state.
    #[error("the Coding Session is {}", .0.as_str())]
    NotOpen(State),
    /// The harness refused the call. The message is harness text.
    #[error("the Coding Harness failed: {0}")]
    Harness(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The Coding Sessions of the daemon.
pub struct CodingSessions {
    records: Records,
    runs: Arc<dyn RunStore>,
    place: Arc<dyn SessionPlace>,
    decisions: Arc<dyn SessionDecisions>,
    cancel: CancellationToken,
    live: Arc<Mutex<HashMap<CodingSessionId, Live>>>,
}

/// The task of one live session.
struct Live {
    workspace_id: WorkspaceId,
    commands: mpsc::UnboundedSender<Command>,
}

impl CodingSessions {
    pub fn new(deps: CodingSessionsDeps) -> Self {
        Self {
            records: Records {
                store: deps.sessions,
                bus: deps.bus,
                clock: deps.clock,
            },
            runs: deps.runs,
            place: deps.place,
            decisions: deps.decisions,
            cancel: deps.cancel,
            live: Arc::default(),
        }
    }

    /// Starts a Coding Session on a Host and sends its first prompt.
    ///
    /// It returns when the prompt is sent, and the session is then
    /// `working`. A turn can run for an hour; its updates go on in the
    /// task of the session.
    pub async fn start(&self, new: NewCodingSession) -> Result<CodingSession, StartFailure> {
        let entry = harness::entry(&new.harness_id)
            .ok_or_else(|| StartFailure::UnknownHarness(new.harness_id.clone()))?;
        let run = self
            .runs
            .get(&new.workspace_id, &new.run_id)
            .await?
            .ok_or_else(|| StartFailure::RunNotFound(new.run_id.clone()))?;
        let channel_id = run.channel_id.ok_or(StartFailure::NoConversation)?;
        let now = self.records.clock.now_ms();
        let message_id = MessageId::generate();
        let mut record = CodingSession {
            id: CodingSessionId::generate(),
            workspace_id: new.workspace_id.clone(),
            agent_id: new.agent_id,
            harness_id: entry.id.to_string(),
            harness_version: entry.version.to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(new.host_id.clone()),
            directory: new.directory.clone(),
            working_directory: None,
            worktree_branch: new
                .worktree
                .as_ref()
                .map(|worktree| worktree.branch.clone()),
            approval_mode: new.approval_mode,
            title: new.title,
            state: State::Starting,
            end_reason: None,
            end_detail: None,
            acp_session_id: None,
            channel_id,
            // A Run outside a Thread posts the block at the top level,
            // and the block becomes the root of the session's Thread.
            root_message_id: run.root_message_id.unwrap_or_else(|| message_id.clone()),
            message_id,
            run_id: new.run_id,
            usage: CodingSessionUsage::default(),
            created_at: now,
            updated_at: now,
            ended_at: None,
        };
        self.records.store.insert(&record).await?;

        let (command, args) = harness::launch_command(entry);
        let request = OpenRequest {
            session_id: record.id.clone(),
            command: command.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            cwd: new.directory,
            env: BTreeMap::new(),
            worktree: new.worktree,
        };
        let opened = match self
            .place
            .open(&new.workspace_id, &new.host_id, request)
            .await
        {
            Ok(opened) => opened,
            Err(failure) => {
                // The caller hears the message. The end detail holds
                // harness text only.
                let end = End::new(failure.code.as_str(), None);
                self.records.end_start(&mut record, end).await;
                return Err(StartFailure::Open {
                    session_id: record.id,
                    failure,
                });
            }
        };
        record.working_directory = Some(opened.cwd.clone());
        self.records.write(&mut record).await?;

        let (commands, commands_rx) = mpsc::unbounded_channel();
        let (snapshot, snapshot_rx) = watch::channel(record.clone());
        let asks = Arc::new(SessionAsks {
            decisions: Arc::clone(&self.decisions),
            commands: commands.clone(),
            session: snapshot_rx,
        });
        let (incoming, outgoing) = opened.stream.split();
        let opening = Opening::New {
            cwd: PathBuf::from(&opened.cwd),
        };
        // A failed `open` drops its connection, which closes the stream.
        let (acp, events) = match AcpSession::open(outgoing, incoming, opening, asks).await {
            Ok(opened) => opened,
            Err(error) => return Err(self.harness_failed(&mut record, error).await),
        };
        record.acp_session_id = Some(acp.acp_session_id().to_string());
        self.records.write(&mut record).await?;

        if let Err(error) = acp.prompt(new.prompt.clone()) {
            acp.close();
            return Err(self.harness_failed(&mut record, error).await);
        }
        self.records
            .append(&record, Kind::Prompt, text_payload(&new.prompt, None))
            .await?;
        self.records
            .transition(&mut record, State::Working, None)
            .await?;
        snapshot.send_replace(record.clone());

        self.live.lock().expect("the live sessions").insert(
            record.id.clone(),
            Live {
                workspace_id: record.workspace_id.clone(),
                commands,
            },
        );
        let task = Task {
            records: self.records.clone(),
            record: record.clone(),
            snapshot,
            acp: Some(acp),
            queue: Vec::new(),
            asks: HashMap::new(),
        };
        tokio::spawn(task.run(
            events,
            commands_rx,
            opened.exit,
            self.cancel.clone(),
            Arc::clone(&self.live),
        ));
        Ok(record)
    }

    /// Sends a prompt to an `idle` session, or queues it while a turn
    /// runs. ACP v1 has no steering inside a turn, so the queued texts go
    /// as one prompt when the turn ends.
    pub async fn prompt(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
        text: String,
    ) -> Result<PromptOutcome, SessionError> {
        self.command(workspace_id, session_id, |reply| Command::Prompt {
            text,
            reply,
        })
        .await
    }

    /// Cancels the turn that runs and empties the queue. The turn ends
    /// with the stop reason `cancelled`.
    pub async fn cancel(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
    ) -> Result<(), SessionError> {
        self.command(workspace_id, session_id, |reply| Command::Cancel { reply })
            .await
    }

    /// Cancels the turn that runs, closes the stream and moves the
    /// session to `closed`. The Client App stops the process when its
    /// stream closes.
    pub async fn close(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
    ) -> Result<(), SessionError> {
        self.command(workspace_id, session_id, |reply| Command::Close { reply })
            .await
    }

    /// The session of one Workspace. A session of another Workspace reads
    /// as absent.
    pub async fn get(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
    ) -> Result<Option<CodingSession>, StoreError> {
        self.records.store.get(workspace_id, session_id).await
    }

    /// Hands a command to the task of a live session. A session with no
    /// task answers from its record.
    async fn command<T>(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
        command: impl FnOnce(oneshot::Sender<Result<T, SessionError>>) -> Command,
    ) -> Result<T, SessionError> {
        let commands = self
            .live
            .lock()
            .expect("the live sessions")
            .get(session_id)
            .filter(|live| live.workspace_id == *workspace_id)
            .map(|live| live.commands.clone());
        if let Some(commands) = commands {
            let (reply, replied) = oneshot::channel();
            // The task can end between the read of the map and the send.
            if commands.send(command(reply)).is_ok()
                && let Ok(answer) = replied.await
            {
                return answer;
            }
        }
        match self.records.store.get(workspace_id, session_id).await? {
            Some(record) => Err(SessionError::NotOpen(record.state)),
            None => Err(SessionError::NotFound),
        }
    }

    /// Ends a start that the harness failed, and gives its failure.
    async fn harness_failed(&self, record: &mut CodingSession, error: CodingError) -> StartFailure {
        let message = match error {
            CodingError::Protocol(message) => message,
            other => other.to_string(),
        };
        let end = End::new(HARNESS_ERROR, Some(message.clone()));
        self.records.end_start(record, end).await;
        StartFailure::Harness {
            session_id: record.id.clone(),
            message,
        }
    }
}

/// The end reason of a harness that failed a request.
const HARNESS_ERROR: &str = "harness_error";

/// The end reason of a harness process that exited by itself.
const HARNESS_EXITED: &str = "harness_exited";

/// Why a session ended.
struct End {
    reason: String,
    /// The exit code and the stderr tail of a failed harness, or the
    /// message of a failure. It is harness text.
    detail: Option<String>,
}

impl End {
    fn new(reason: &str, detail: Option<String>) -> Self {
        Self {
            reason: reason.to_string(),
            detail,
        }
    }
}

/// The writes of a session: its record, its transcript and the event of
/// each transcript write.
#[derive(Clone)]
struct Records {
    store: Arc<dyn CodingSessionStore>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
}

impl Records {
    /// Moves a session to `state` and writes the record. A terminal state
    /// takes its end, and writes its end time. A terminal session does
    /// not move.
    ///
    /// It publishes no event: the store reports each write of the record.
    pub(crate) async fn transition(
        &self,
        record: &mut CodingSession,
        state: State,
        end: Option<End>,
    ) -> Result<(), StoreError> {
        debug_assert_eq!(
            state.is_terminal(),
            end.is_some(),
            "only an end has a reason"
        );
        if record.state.is_terminal() {
            return Ok(());
        }
        record.state = state;
        if let Some(end) = end {
            record.end_reason = Some(end.reason);
            record.end_detail = end.detail;
            record.ended_at = Some(self.clock.now_ms());
        }
        self.write(record).await
    }

    /// Writes the whole record again.
    async fn write(&self, record: &mut CodingSession) -> Result<(), StoreError> {
        record.updated_at = self.clock.now_ms();
        if !self.store.update(record).await? {
            tracing::warn!(session = %record.id, "a Coding Session record is gone");
        }
        Ok(())
    }

    /// Adds one update to the transcript and publishes the new `seq`.
    async fn append(
        &self,
        record: &CodingSession,
        kind: Kind,
        payload: Value,
    ) -> Result<(), StoreError> {
        let event = NewCodingSessionEvent {
            at: self.clock.now_ms(),
            kind,
            payload,
        };
        let rows = self
            .store
            .append_event(&record.workspace_id, &record.id, event)
            .await?;
        let Some(seq) = rows.iter().map(|row| row.seq).max() else {
            return Ok(());
        };
        self.bus
            .publish(NewEvent {
                workspace_id: record.workspace_id.clone(),
                event_type: UPDATED_EVENT.to_string(),
                agent_id: Some(record.agent_id.clone()),
                run_id: None,
                channel_id: Some(record.channel_id.clone()),
                payload: json!({"coding_session_id": record.id, "seq": seq}),
            })
            .await?;
        Ok(())
    }

    /// Fails a session that did not start. The failure of the start is
    /// what the caller hears, so a failed write is only logged.
    async fn end_start(&self, record: &mut CodingSession, end: End) {
        if let Err(error) = self.transition(record, State::Failed, Some(end)).await {
            tracing::warn!(session = %record.id, %error, "a failed Coding Session was not written");
        }
    }
}

/// A command to the task of a live session.
enum Command {
    Prompt {
        text: String,
        reply: oneshot::Sender<Result<PromptOutcome, SessionError>>,
    },
    Cancel {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Close {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// An ask of the harness waits for its answer.
    Asked {
        ask_id: String,
        ask: AskKind,
        payload: Value,
    },
    /// An ask of the harness got its answer.
    Answered { ask_id: String, payload: Value },
}

/// The kind of an ask of the harness.
#[derive(Debug, Clone, Copy)]
enum AskKind {
    Permission,
    Question,
}

impl AskKind {
    fn asked(self) -> Kind {
        match self {
            AskKind::Permission => Kind::Permission,
            AskKind::Question => Kind::Question,
        }
    }

    fn answered(self) -> Kind {
        match self {
            AskKind::Permission => Kind::Decision,
            AskKind::Question => Kind::Answer,
        }
    }

    /// The answer row of an ask that the harness withdrew or a cancel
    /// ended.
    fn withdrawn(self, ask_id: &str) -> Value {
        match self {
            AskKind::Permission => json!({"ask_id": ask_id, "decision": "withdrawn"}),
            AskKind::Question => json!({"ask_id": ask_id, "answer": "withdrawn"}),
        }
    }
}

/// The `AskHandler` of one session: it gets each answer from
/// [`SessionDecisions`], and the task of the session records the ask and
/// its answer.
struct SessionAsks {
    decisions: Arc<dyn SessionDecisions>,
    commands: mpsc::UnboundedSender<Command>,
    session: watch::Receiver<CodingSession>,
}

impl SessionAsks {
    fn send(&self, command: Command) {
        // The task ends with the session; the harness then gets its
        // answer all the same.
        let _ = self.commands.send(command);
    }

    fn asked(&self, ask_id: &str, ask: AskKind, mut payload: Value, waits_for: Value) {
        payload["waits_for"] = waits_for;
        self.send(Command::Asked {
            ask_id: ask_id.to_string(),
            ask,
            payload,
        });
    }
}

#[async_trait]
impl AskHandler for SessionAsks {
    async fn permission(&self, ask: PermissionAsk) -> PermissionAnswer {
        let session = self.session.borrow().clone();
        let ask_id = ask.ask_id.clone();
        let payload = json!(ask);
        let pending = self.decisions.permission(&session, ask).await;
        self.asked(
            &ask_id,
            AskKind::Permission,
            payload,
            json!(pending.waits_for),
        );
        let answer = pending.answer.await;
        self.send(Command::Answered {
            payload: json!({"ask_id": ask_id, "decision": answer}),
            ask_id,
        });
        answer
    }

    async fn question(&self, ask: QuestionAsk) -> QuestionAnswer {
        let session = self.session.borrow().clone();
        let ask_id = ask.ask_id.clone();
        let payload = json!(ask);
        let pending = self.decisions.question(&session, ask).await;
        self.asked(
            &ask_id,
            AskKind::Question,
            payload,
            json!(pending.waits_for),
        );
        let answer = pending.answer.await;
        self.send(Command::Answered {
            payload: json!({"ask_id": ask_id, "answer": answer}),
            ask_id,
        });
        answer
    }
}

/// Whether the task of a session goes on.
enum Flow {
    Go,
    End,
}

/// The task of one live session, and the one writer of its record.
struct Task {
    records: Records,
    record: CodingSession,
    /// The record as the task last wrote it, for the asks.
    snapshot: watch::Sender<CodingSession>,
    acp: Option<AcpSession>,
    /// The prompts that wait for the end of the turn.
    queue: Vec<String>,
    /// The asks that wait for their answer, by `ask_id`.
    asks: HashMap<String, AskKind>,
}

impl Task {
    async fn run(
        mut self,
        mut events: mpsc::UnboundedReceiver<SessionEvent>,
        mut commands: mpsc::UnboundedReceiver<Command>,
        exit: oneshot::Receiver<SessionExit>,
        cancel: CancellationToken,
        live: Arc<Mutex<HashMap<CodingSessionId, Live>>>,
    ) {
        let mut exit = Some(exit);
        let mut stream_closed = false;
        let mut grace: Option<Pin<Box<tokio::time::Sleep>>> = None;
        loop {
            // The commands go first: an ask is always recorded before the
            // event that withdraws it.
            let flow = tokio::select! {
                biased;
                () = cancel.cancelled() => Flow::End,
                Some(command) = commands.recv() => self.command(command).await,
                event = events.recv(), if !stream_closed => match event {
                    Some(SessionEvent::Closed) | None => {
                        stream_closed = true;
                        grace = Some(Box::pin(tokio::time::sleep(EXIT_GRACE)));
                        Flow::Go
                    }
                    Some(event) => self.event(event).await,
                },
                exited = async { exit.as_mut().expect("an exit to wait for").await }, if exit.is_some() => {
                    exit = None;
                    match exited {
                        Ok(exited) => self.exited(Some(exited)).await,
                        Err(_) => {
                            // The session socket ended: the place is lost.
                            tracing::info!(session = %self.record.id, "the place of a Coding Session is lost");
                            Flow::End
                        }
                    }
                }
                () = async { grace.as_mut().expect("a grace to wait for").await }, if grace.is_some() => {
                    self.exited(None).await
                }
            };
            if let Flow::End = flow {
                break;
            }
        }
        live.lock()
            .expect("the live sessions")
            .remove(&self.record.id);
        if let Some(acp) = self.acp.take() {
            acp.close();
        }
    }

    async fn command(&mut self, command: Command) -> Flow {
        match command {
            Command::Prompt { text, reply } => {
                let outcome = match self.record.state {
                    State::Idle => self.send_prompt(text).await.map(|()| PromptOutcome::Sent),
                    State::Working | State::NeedsDecision => {
                        self.queue.push(text);
                        Ok(PromptOutcome::Queued)
                    }
                    state => Err(SessionError::NotOpen(state)),
                };
                let _ = reply.send(outcome);
                Flow::Go
            }
            Command::Cancel { reply } => {
                self.queue.clear();
                let cancelled = match (&self.acp, self.turn_runs()) {
                    (Some(acp), true) => acp
                        .cancel()
                        .map_err(|error| SessionError::Harness(error.to_string())),
                    _ => Ok(()),
                };
                let _ = reply.send(cancelled);
                Flow::Go
            }
            Command::Close { reply } => {
                self.queue.clear();
                if let Some(acp) = self.acp.take() {
                    if self.turn_runs() {
                        // The stream closes next, so a failed cancel
                        // changes nothing.
                        let _ = acp.cancel();
                    }
                    acp.close();
                }
                let closed = self
                    .records
                    .transition(
                        &mut self.record,
                        State::Closed,
                        Some(End::new("closed", None)),
                    )
                    .await;
                self.snapshot.send_replace(self.record.clone());
                let _ = reply.send(closed.map_err(SessionError::from));
                Flow::End
            }
            Command::Asked {
                ask_id,
                ask,
                payload,
            } => {
                self.asks.insert(ask_id, ask);
                self.append(ask.asked(), payload).await;
                if self.record.state == State::Working {
                    self.transition(State::NeedsDecision, None).await;
                }
                Flow::Go
            }
            Command::Answered { ask_id, payload } => {
                if let Some(ask) = self.asks.remove(&ask_id) {
                    self.append(ask.answered(), payload).await;
                    self.settle().await;
                }
                Flow::Go
            }
        }
    }

    async fn event(&mut self, event: SessionEvent) -> Flow {
        match event {
            SessionEvent::AgentMessage { message_id, text } => {
                self.append(Kind::AgentMessage, text_payload(&text, message_id))
                    .await;
            }
            SessionEvent::Thought { text } => {
                self.append(Kind::Thought, text_payload(&text, None)).await;
            }
            SessionEvent::ToolCall { raw, .. } => self.append(Kind::ToolCall, raw).await,
            SessionEvent::ToolCallUpdate { raw, .. } => {
                self.append(Kind::ToolCallUpdate, raw).await;
            }
            SessionEvent::Plan { entries } => {
                self.append(Kind::Plan, json!({ "entries": entries })).await;
            }
            SessionEvent::Usage { used, size, cost } => {
                let usage = &mut self.record.usage;
                usage.context_used = Some(used);
                usage.context_size = Some(size);
                if let Some(cost) = &cost {
                    usage.cost_amount = Some(cost.amount);
                    usage.cost_currency = Some(cost.currency.clone());
                }
                if let Err(error) = self.records.write(&mut self.record).await {
                    tracing::warn!(session = %self.record.id, %error, "the usage of a Coding Session was not written");
                }
                self.snapshot.send_replace(self.record.clone());
                self.append(
                    Kind::Usage,
                    json!({"used": used, "size": size, "cost": cost}),
                )
                .await;
            }
            SessionEvent::TurnEnded { stop_reason } => {
                self.append(Kind::TurnEnd, json!({ "stop_reason": stop_reason }))
                    .await;
                self.transition(State::Idle, None).await;
                if !self.queue.is_empty() {
                    let text = std::mem::take(&mut self.queue).join("\n\n");
                    if let Err(error) = self.send_prompt(text).await {
                        tracing::warn!(session = %self.record.id, %error, "the queued prompts of a Coding Session were not sent");
                    }
                }
            }
            SessionEvent::TurnFailed { message } => {
                self.transition(State::Failed, Some(End::new(HARNESS_ERROR, Some(message))))
                    .await;
                return Flow::End;
            }
            SessionEvent::AskWithdrawn { ask_id } => {
                // An ask that the handler had not recorded yet has no row
                // to answer.
                if let Some(ask) = self.asks.remove(&ask_id) {
                    self.append(ask.answered(), ask.withdrawn(&ask_id)).await;
                    self.settle().await;
                }
            }
            // `run` reads the end of the stream.
            SessionEvent::Closed => {}
        }
        Flow::Go
    }

    /// The process exited by itself, or its stream closed and no exit
    /// came in time.
    async fn exited(&mut self, exit: Option<SessionExit>) -> Flow {
        let detail = exit.map(|exit| {
            let code = exit.exit_code.map_or_else(
                || "no exit code".to_string(),
                |code| format!("exit code {code}"),
            );
            if exit.stderr_tail.is_empty() {
                code
            } else {
                format!("{code}\n{}", exit.stderr_tail)
            }
        });
        self.transition(State::Failed, Some(End::new(HARNESS_EXITED, detail)))
            .await;
        Flow::End
    }

    /// Sends one prompt, writes its row and moves the session to
    /// `working`.
    async fn send_prompt(&mut self, text: String) -> Result<(), SessionError> {
        let acp = self
            .acp
            .as_ref()
            .ok_or(SessionError::NotOpen(self.record.state))?;
        acp.prompt(text.clone())
            .map_err(|error| SessionError::Harness(error.to_string()))?;
        self.append(Kind::Prompt, text_payload(&text, None)).await;
        self.transition(State::Working, None).await;
        Ok(())
    }

    fn turn_runs(&self) -> bool {
        matches!(self.record.state, State::Working | State::NeedsDecision)
    }

    /// Moves a session back to `working` when its last ask is answered.
    async fn settle(&mut self) {
        if self.asks.is_empty() && self.record.state == State::NeedsDecision {
            self.transition(State::Working, None).await;
        }
    }

    async fn transition(&mut self, state: State, end: Option<End>) {
        if let Err(error) = self.records.transition(&mut self.record, state, end).await {
            tracing::warn!(session = %self.record.id, %error, "a Coding Session record was not written");
        }
        self.snapshot.send_replace(self.record.clone());
    }

    async fn append(&self, kind: Kind, payload: Value) {
        if let Err(error) = self.records.append(&self.record, kind, payload).await {
            tracing::warn!(session = %self.record.id, %error, "a transcript row of a Coding Session was not written");
        }
    }
}

/// The payload of a text row.
fn text_payload(text: &str, message_id: Option<String>) -> Value {
    json!({"text": text, "message_id": message_id})
}
