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
    AgentId, AuthorKind, Block, Clock, CodingSession, CodingSessionEventKind as Kind,
    CodingSessionId, CodingSessionPlace, CodingSessionState as State, CodingSessionStore,
    CodingSessionUsage, EventBus, HostId, HostStore, Message, MessageId, MessageStatus,
    MessageStore, NewCodingSessionEvent, NewEvent, RunId, RunStore, SessionApprovalMode,
    StoreError, WorkspaceId, blocks_text, harness,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::events::{DecisionKind, SessionNews, is_end, session_batch};
use crate::{
    AcpSession, AskHandler, CodingError, OpenFailure, OpenRequest, Opening, Pending,
    PermissionAnswer, PermissionAsk, QuestionAnswer, QuestionAsk, SessionDecisions, SessionEvent,
    SessionEvents, SessionExit, SessionPlace, SessionRules, WorktreeRequest,
};

/// How long a session waits for the exit report of its process after
/// the stream of the process closed.
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// What [`CodingSessions`] is built from.
pub struct CodingSessionsDeps {
    /// The store reports each write to the clients.
    pub sessions: Arc<dyn CodingSessionStore>,
    /// The Run that starts a session names the Thread that shows it.
    pub runs: Arc<dyn RunStore>,
    /// The Host of a session gives the machine name of its block.
    pub hosts: Arc<dyn HostStore>,
    /// The Thread holds the block of each session.
    pub messages: Arc<dyn MessageStore>,
    /// Reports the message of each block to the clients.
    pub bus: Arc<dyn EventBus>,
    pub place: Arc<dyn SessionPlace>,
    pub decisions: Arc<dyn SessionDecisions>,
    /// The Session Rule of each session, which wakes the owning Agent.
    pub rules: Arc<dyn SessionRules>,
    /// Where the news of each session goes to its Session Rule.
    pub events: Arc<dyn SessionEvents>,
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
    #[error("the Workspace has no Host {0}")]
    HostNotFound(HostId),
    /// The Run has no Channel, so no Thread can show the session.
    #[error("the Run has no conversation that can show the session")]
    NoConversation,
    /// The Session Rule was not made, so the Agent would not hear of
    /// the session.
    #[error("the session cannot wake its Agent: {message}")]
    Rules {
        session_id: CodingSessionId,
        message: String,
    },
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

/// Who closed a session. It gives the end reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The supervising Agent closed the session: `closed`.
    Closed,
    /// The Person stopped the session: `stopped`.
    Stopped,
}

impl CloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CloseReason::Closed => "closed",
            CloseReason::Stopped => "stopped",
        }
    }
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
    hosts: Arc<dyn HostStore>,
    messages: Arc<dyn MessageStore>,
    bus: Arc<dyn EventBus>,
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
                rules: deps.rules,
                events: deps.events,
                clock: deps.clock,
            },
            runs: deps.runs,
            hosts: deps.hosts,
            messages: deps.messages,
            bus: deps.bus,
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
        let host = self
            .hosts
            .get(&new.workspace_id, &new.host_id)
            .await?
            .ok_or_else(|| StartFailure::HostNotFound(new.host_id.clone()))?;
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
        // The Person sees the session from `starting` on.
        self.post_session_block(&record, entry.label, &host.name)
            .await?;
        // The Trigger module checks that the root of the rule's Thread is
        // a message of the Channel, so the rule comes after the block. It
        // comes before the stream opens, so no news of the session is
        // lost.
        if let Err(error) = self.records.rules.create(&record).await {
            let end = End::new(RULES_UNAVAILABLE, None);
            self.records.end_start(&mut record, end).await;
            return Err(StartFailure::Rules {
                session_id: record.id,
                message: error.to_string(),
            });
        }

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
            machine: host.name,
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
    /// session to `closed` with the end reason of `reason`. The Client
    /// App stops the process when its stream closes.
    pub async fn close(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
        reason: CloseReason,
    ) -> Result<(), SessionError> {
        self.command(workspace_id, session_id, |reply| Command::Close {
            reason,
            reply,
        })
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

    /// Posts the daemon-made block of a session in its Thread (ADR-0033),
    /// under the ids of the record. The record is written first, so the
    /// block never names a row that does not exist.
    ///
    /// The block is the root of the session's Thread when the record's
    /// root is its own message, and a reply in the Run's Thread when
    /// not.
    async fn post_session_block(
        &self,
        record: &CodingSession,
        harness: &str,
        machine: &str,
    ) -> Result<(), StoreError> {
        let blocks = vec![Block::coding_session(
            record.id.as_str(),
            harness,
            machine,
            &record.directory,
            &record.title,
        )];
        let parent_message_id =
            (record.root_message_id != record.message_id).then(|| record.root_message_id.clone());
        let message = Message {
            id: record.message_id.clone(),
            workspace_id: record.workspace_id.clone(),
            channel_id: record.channel_id.clone(),
            parent_message_id,
            author_kind: AuthorKind::System,
            author_agent_id: None,
            run_id: Some(record.run_id.clone()),
            status: MessageStatus::Complete,
            text_content: blocks_text(&blocks),
            blocks,
            pending_id: None,
            created_at: record.created_at,
            completed_at: Some(record.created_at),
        };
        // The block holds the daemon's own session fields and no
        // conversation text, so it has no exposure.
        self.messages.insert_stamped(&message, &[]).await?;
        let published = self
            .bus
            .publish(NewEvent {
                workspace_id: message.workspace_id.clone(),
                event_type: "message.completed".to_string(),
                agent_id: Some(record.agent_id.clone()),
                run_id: Some(record.run_id.clone()),
                channel_id: Some(message.channel_id.clone()),
                payload: json!({
                    "message_id": message.id.as_str(),
                    "parent_message_id": message.parent_message_id.as_ref().map(MessageId::as_str),
                    "author_kind": AuthorKind::System,
                }),
            })
            .await;
        // The message is stored, so a client that reads the Thread
        // again finds the block.
        if let Err(error) = published {
            tracing::warn!(session = %record.id, %error, "the block of a Coding Session did not reach the clients");
        }
        Ok(())
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

/// The end reason of a start whose Session Rule was not made.
const RULES_UNAVAILABLE: &str = "temporarily_unavailable";

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

/// The writes of a session: its record and its transcript. The store
/// reports each write to the clients.
///
/// The Session Rule hears the news of the session from here.
#[derive(Clone)]
struct Records {
    store: Arc<dyn CodingSessionStore>,
    rules: Arc<dyn SessionRules>,
    events: Arc<dyn SessionEvents>,
    clock: Arc<dyn Clock>,
}

impl Records {
    /// Moves a session to `state` and writes the record. A terminal state
    /// takes its end, and writes its end time. A terminal session does
    /// not move.
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

    /// Gives one piece of news of a session to its Session Rule. The
    /// session goes on when the news does not arrive, as a Call does.
    async fn raise(&self, record: &CodingSession, machine: &str, news: SessionNews) {
        let batch = session_batch(record, machine, news, self.clock.now_ms());
        if let Err(error) = self.events.ingest(batch).await {
            tracing::warn!(session = %record.id, %error, kind = news.event_kind(), "the news of a Coding Session did not reach its Session Rule");
        }
    }

    /// Ends the Session Rule of a session that closed or failed. The
    /// Wake-up of the last event stays.
    async fn end_rules(&self, record: &CodingSession) {
        if let Err(error) = self.rules.end(record).await {
            tracing::warn!(session = %record.id, %error, "the Session Rule of a Coding Session did not end");
        }
    }

    /// Writes the whole record again.
    async fn write(&self, record: &mut CodingSession) -> Result<(), StoreError> {
        record.updated_at = self.clock.now_ms();
        if !self.store.update(record).await? {
            tracing::warn!(session = %record.id, "a Coding Session record is gone");
        }
        Ok(())
    }

    /// Adds one update to the transcript, and answers the `seq` of the
    /// row that holds it.
    async fn append(
        &self,
        record: &CodingSession,
        kind: Kind,
        payload: Value,
    ) -> Result<Option<i64>, StoreError> {
        let event = NewCodingSessionEvent {
            at: self.clock.now_ms(),
            kind,
            payload,
        };
        let rows = self
            .store
            .append_event(&record.workspace_id, &record.id, event)
            .await?;
        Ok(rows.last().map(|row| row.seq))
    }

    /// Fails a session that did not start, and ends its Session Rule.
    /// The failure of the start is what the caller hears, so a failed
    /// write is only logged, and the rule raises no news: the starting
    /// Run reads the failure in its tool result.
    async fn end_start(&self, record: &mut CodingSession, end: End) {
        if let Err(error) = self.transition(record, State::Failed, Some(end)).await {
            tracing::warn!(session = %record.id, %error, "a failed Coding Session was not written");
        }
        self.end_rules(record).await;
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
        reason: CloseReason,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    /// An ask of the harness came. When it `waits`, the session is
    /// `needs_decision` until its answer comes.
    Asked {
        ask_id: String,
        ask: AskKind,
        payload: Value,
        waits: bool,
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
    fn decision_kind(self) -> DecisionKind {
        match self {
            AskKind::Permission => DecisionKind::Permission,
            AskKind::Question => DecisionKind::Question,
        }
    }

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

    /// Records the ask and, when it comes, its answer. The row of the
    /// ask names who the session waits for, or none for an answer that
    /// the daemon gives at once. The row of a decision names its decider.
    async fn answer<T: serde::Serialize>(
        &self,
        ask_id: String,
        ask: AskKind,
        mut payload: Value,
        pending: Pending<T>,
    ) -> T {
        let (waits_for, decider) = match &pending {
            Pending::Decided { decider, .. } => (None, *decider),
            Pending::Waits { waits_for, .. } => (Some(*waits_for), Some(waits_for.decider())),
        };
        payload["waits_for"] = json!(waits_for);
        self.send(Command::Asked {
            ask_id: ask_id.clone(),
            ask,
            payload,
            waits: waits_for.is_some(),
        });
        let answer = match pending {
            Pending::Decided { answer, .. } => answer,
            Pending::Waits { answer, .. } => answer.await,
        };
        let payload = match ask {
            AskKind::Permission => {
                json!({"ask_id": ask_id, "decision": answer, "decider": decider})
            }
            AskKind::Question => json!({"ask_id": ask_id, "answer": answer}),
        };
        self.send(Command::Answered { payload, ask_id });
        answer
    }
}

#[async_trait]
impl AskHandler for SessionAsks {
    async fn permission(&self, ask: PermissionAsk) -> PermissionAnswer {
        let session = self.session.borrow().clone();
        let ask_id = ask.ask_id.clone();
        let payload = json!(ask);
        let pending = self.decisions.permission(&session, ask).await;
        self.answer(ask_id, AskKind::Permission, payload, pending)
            .await
    }

    async fn question(&self, ask: QuestionAsk) -> QuestionAnswer {
        let session = self.session.borrow().clone();
        let ask_id = ask.ask_id.clone();
        let payload = json!(ask);
        let pending = self.decisions.question(&session, ask).await;
        self.answer(ask_id, AskKind::Question, payload, pending)
            .await
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
    /// The name of the Host, which the news of the session names.
    machine: String,
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
            Command::Close { reason, reply } => {
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
                    .move_to(State::Closed, Some(End::new(reason.as_str(), None)))
                    .await;
                let _ = reply.send(closed.map_err(SessionError::from));
                Flow::End
            }
            Command::Asked {
                ask_id,
                ask,
                payload,
                waits,
            } => {
                self.asks.insert(ask_id, ask);
                let seq = self.append(ask.asked(), payload).await;
                if waits && self.record.state == State::Working {
                    self.transition(State::NeedsDecision, None).await;
                    if let Some(seq) = seq {
                        let news = SessionNews::NeedsDecision {
                            decision_kind: ask.decision_kind(),
                            seq,
                        };
                        self.raise(news).await;
                    }
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
            SessionEvent::ToolCall { raw, .. } => {
                self.append(Kind::ToolCall, raw).await;
            }
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
                let seq = self
                    .append(Kind::TurnEnd, json!({ "stop_reason": stop_reason }))
                    .await;
                self.transition(State::Idle, None).await;
                if let Some(seq) = seq {
                    self.raise(SessionNews::TurnEnded { stop_reason, seq })
                        .await;
                }
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

    /// Moves the session, and logs a record that was not written.
    async fn transition(&mut self, state: State, end: Option<End>) {
        if let Err(error) = self.move_to(state, end).await {
            tracing::warn!(session = %self.record.id, %error, "a Coding Session record was not written");
        }
    }

    /// Moves the session. An end goes to the Session Rule, and a terminal
    /// state then ends the rule, also when the record was not written:
    /// the session ends all the same.
    async fn move_to(&mut self, state: State, end: Option<End>) -> Result<(), StoreError> {
        let ended_before = self.record.state.is_terminal();
        let written = self.records.transition(&mut self.record, state, end).await;
        self.snapshot.send_replace(self.record.clone());
        if !ended_before && is_end(state) {
            self.raise(SessionNews::Ended).await;
            if state.is_terminal() {
                self.records.end_rules(&self.record).await;
            }
        }
        written
    }

    async fn raise(&self, news: SessionNews) {
        self.records.raise(&self.record, &self.machine, news).await;
    }

    /// Adds one row to the transcript, and answers its `seq`.
    async fn append(&self, kind: Kind, payload: Value) -> Option<i64> {
        match self.records.append(&self.record, kind, payload).await {
            Ok(seq) => seq,
            Err(error) => {
                tracing::warn!(session = %self.record.id, %error, "a transcript row of a Coding Session was not written");
                None
            }
        }
    }
}

/// The payload of a text row.
fn text_payload(text: &str, message_id: Option<String>) -> Value {
    json!({"text": text, "message_id": message_id})
}
