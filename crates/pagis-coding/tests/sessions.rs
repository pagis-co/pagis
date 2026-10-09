//! The Coding Sessions of the daemon on a Host, against the SQLite store
//! and the fake harness. A `SessionPlace` over `tokio::io::duplex` stands
//! in for the session socket, and the last tests run over the session
//! socket registry itself.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncWrite};
use pagis_audit::AuditEventBus;
use pagis_broker::{
    AuthorizedCall, CoreTool, Decider, HostPresence, HostSessions, ToolExecutor, ToolResult,
    ToolRoute, not_connected_message,
};
use pagis_coding::fake::{Ask, FakeHarness, NEW_SESSION_ID, Script, Turn, acp, serve_client_app};
use pagis_coding::{
    AgentAsks, CloseReason, CodingSessionStarts, CodingSessions, CodingSessionsDeps,
    CodingToolRuntime, InterruptReason, NewCodingSession, OpenFailure, OpenFailureCode,
    OpenRequest, OpenedStream, PERMISSION_DECIDED_EVENT, Pending, PermissionAnswer, PermissionAsk,
    PolicyDecisions, PolicyDecisionsDeps, PromptOutcome, QuestionAnswer, QuestionAsk,
    RefuseDecisions, ResumeFailure, SIGN_IN_CHANGED_EVENT, SessionDecisions, SessionError,
    SessionEvents, SessionExit, SessionPlace, SessionRuleError, SessionRules, SignInReports,
    StartFailure, Waited, WaitsFor, WorktreeRequest,
};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, AuthorKind, Block, Channel, ChannelId, ChannelKind,
    ChannelStore, CodingSession, CodingSessionEvent, CodingSessionEventKind as Kind,
    CodingSessionId, CodingSessionPlace, CodingSessionState as State, CodingSessionStore, Event,
    EventBus, EventId, EventLog, EventScope, EventStream, Grant, GrantId, GrantStore, HostId,
    HostStore, IngestBatch, Message, MessageId, MessageStatus, MessageStore, NewEvent, Request,
    RequestState, RequestStore, Run, RunId, RunState, RunStore, SessionApprovalMode, StoreError,
    SystemClock, TriggerKind, Workspace, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteChannelStore, SqliteCodingSessionStore, SqliteEventLog,
    SqliteGrantStore, SqliteHostStore, SqliteMessageStore, SqliteRequestStore, SqliteRunStore,
    SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::{Notify, oneshot};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(10);
const DIRECTORY: &str = "/Users/bo/code/app";
const WORKTREE_DIRECTORY: &str = "/Users/bo/.pagis-worktrees/app/pagis/fix-login";

/// The rows that a Coding Session points at, and the stores that the
/// tests read.
struct World {
    pool: SqlitePool,
    sessions: Arc<SqliteCodingSessionStore>,
    /// The event log that Pagis policy writes its audit facts to.
    events: Arc<SqliteEventLog>,
    /// The bus of Pagis policy, on that log.
    bus: Arc<AuditEventBus>,
    /// The Harness Permissions that wait for the Agent.
    agent_asks: Arc<AgentAsks>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    channel_id: ChannelId,
    host_id: HostId,
    /// A Run in the Channel, outside any Thread.
    run_id: RunId,
    /// The Trigger module, as the sessions see it.
    rules: Arc<RecordedRules>,
    /// The machines that are connected, whose departures the sessions
    /// hear.
    presence: Arc<HostPresence>,
    /// The harnesses that need a Harness Sign-In, on the bus of the tests.
    sign_in_reports: Arc<SignInReports>,
}

async fn world(pool: SqlitePool) -> World {
    let person = pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", now_ms())
        .await
        .unwrap();
    let workspace_id = workspace(&pool, person.id).await;
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: "Robin".to_string(),
        job: "engineer".to_string(),
        description: String::new(),
        personality: "calm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteAgentStore::new(pool.clone())
        .create(&agent)
        .await
        .unwrap();
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: workspace_id.clone(),
        kind: ChannelKind::Dm,
        title: Some("general".to_string()),
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteChannelStore::new(pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let host = SqliteHostStore::new(pool.clone())
        .register(&workspace_id, "Air", "macos", &[], now_ms())
        .await
        .unwrap();
    let events = Arc::new(SqliteEventLog::new(pool.clone()));
    let bus = Arc::new(AuditEventBus::new(events.clone()));
    let mut world = World {
        sessions: Arc::new(SqliteCodingSessionStore::new(pool.clone())),
        sign_in_reports: Arc::new(SignInReports::new(bus.clone(), Arc::new(SystemClock))),
        bus,
        events,
        agent_asks: Arc::default(),
        pool,
        workspace_id,
        agent_id: agent.id,
        channel_id: channel.id,
        host_id: host.id,
        run_id: RunId::generate(),
        rules: Arc::default(),
        presence: Arc::new(HostPresence::new()),
    };
    world.run_id = world.run(Some(world.channel_id.clone()), None).await;
    world
}

async fn workspace(pool: &SqlitePool, user_id: pagis_core::UserId) -> WorkspaceId {
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id,
        name: "Workspace".to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    workspace.id
}

impl World {
    async fn run(&self, channel_id: Option<ChannelId>, root: Option<MessageId>) -> RunId {
        let run = Run {
            id: RunId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            channel_id,
            root_message_id: root,
            trigger_kind: TriggerKind::Message,
            trigger_ref: None,
            hop_count: 0,
            origin: None,
            state: RunState::Running,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at: Some(now_ms()),
            ended_at: None,
            created_at: now_ms(),
        };
        SqliteRunStore::new(self.pool.clone())
            .create(&run)
            .await
            .unwrap();
        run.id
    }

    /// A Run in the Thread of a new message of the Person.
    async fn run_in_thread(&self) -> (RunId, MessageId) {
        let message = Message {
            id: MessageId::generate(),
            workspace_id: self.workspace_id.clone(),
            channel_id: self.channel_id.clone(),
            parent_message_id: None,
            author_kind: AuthorKind::User,
            author_agent_id: None,
            run_id: None,
            status: MessageStatus::Complete,
            blocks: vec![Block::markdown("Fix the login.")],
            text_content: "Fix the login.".to_string(),
            pending_id: Some(format!("pending-{}", MessageId::generate())),
            created_at: now_ms(),
            completed_at: Some(now_ms()),
        };
        SqliteMessageStore::new(self.pool.clone())
            .insert(&message)
            .await
            .unwrap();
        let run_id = self
            .run(Some(self.channel_id.clone()), Some(message.id.clone()))
            .await;
        (run_id, message.id)
    }

    fn coding_sessions(
        &self,
        place: Arc<dyn SessionPlace>,
        decisions: Arc<dyn SessionDecisions>,
    ) -> CodingSessions {
        CodingSessions::new(CodingSessionsDeps {
            sessions: self.sessions.clone(),
            runs: Arc::new(SqliteRunStore::new(self.pool.clone())),
            hosts: Arc::new(SqliteHostStore::new(self.pool.clone())),
            messages: Arc::new(SqliteMessageStore::new(self.pool.clone())),
            bus: Arc::new(SilentBus),
            place,
            decisions,
            rules: self.rules.clone(),
            events: self.rules.clone(),
            sign_in_reports: self.sign_in_reports.clone(),
            clock: Arc::new(SystemClock),
            departures: self.presence.departures(),
            cancel: CancellationToken::new(),
        })
    }

    fn new_session(&self, run_id: &RunId) -> NewCodingSession {
        NewCodingSession {
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            run_id: run_id.clone(),
            host_id: self.host_id.clone(),
            harness_id: "claude".to_string(),
            directory: DIRECTORY.to_string(),
            worktree: Some(WorktreeRequest {
                repo: DIRECTORY.to_string(),
                branch: "pagis/fix-login".to_string(),
                base: "main".to_string(),
            }),
            approval_mode: SessionApprovalMode::Person,
            title: "Fix the login".to_string(),
            prompt: "Fix the login bug.".to_string(),
        }
    }

    /// Pagis policy on the SQLite Grant store and the bus of the tests.
    fn policy(&self) -> Arc<PolicyDecisions> {
        Arc::new(PolicyDecisions::new(PolicyDecisionsDeps {
            grants: Arc::new(SqliteGrantStore::new(self.pool.clone())),
            requests: Arc::new(SqliteRequestStore::new(self.pool.clone())),
            messages: Arc::new(SqliteMessageStore::new(self.pool.clone())),
            hosts: Arc::new(SqliteHostStore::new(self.pool.clone())),
            runs: Arc::new(SqliteRunStore::new(self.pool.clone())),
            agent: self.agent_asks.clone(),
            bus: self.bus.clone(),
        }))
    }

    /// A live host Grant of the Agent on the machine, with a widest
    /// Session Approval Mode and Allow Rules.
    async fn host_grant(&self, mode: SessionApprovalMode, allow: &[&str]) -> Grant {
        let grant = Grant {
            id: GrantId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            resource_kind: Grant::HOST_KIND.to_string(),
            resource_id: Some(self.host_id.to_string()),
            scope: json!({"allow": allow, "session_approval_mode": mode.as_str()}),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        };
        SqliteGrantStore::new(self.pool.clone())
            .create(&grant)
            .await
            .unwrap();
        grant
    }

    /// Waits until the bus holds `count` audit facts of Harness
    /// Permissions, and answers them, oldest first.
    async fn wait_for_facts(&self, count: usize) -> Vec<Event> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let mut facts = self
                .events
                .list_by_types(&self.workspace_id, &[PERMISSION_DECIDED_EVENT], None, 100)
                .await
                .unwrap();
            if facts.len() >= count {
                facts.sort_by_key(|fact| fact.seq);
                return facts;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} audit facts, not {count}",
                facts.len()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Whether the report says that Claude Code needs a sign-in on the
    /// machine.
    async fn needs_sign_in(&self) -> bool {
        self.sign_in_reports
            .needs_sign_in_since(&self.host_id, "claude")
            .await
            .is_some()
    }

    /// The payloads of the `harness.sign_in_changed` events, oldest
    /// first.
    async fn sign_in_changes(&self) -> Vec<Value> {
        let mut events = self
            .events
            .list_by_types(&self.workspace_id, &[SIGN_IN_CHANGED_EVENT], None, 100)
            .await
            .unwrap();
        events.sort_by_key(|event| event.seq);
        events.into_iter().map(|event| event.payload).collect()
    }

    /// One `harness.sign_in_changed` payload of Claude Code on the
    /// machine.
    fn sign_in_change(&self, needs_sign_in: bool) -> Value {
        json!({
            "host_id": self.host_id.as_str(),
            "harness": "claude",
            "needs_sign_in": needs_sign_in,
        })
    }

    async fn record(&self, id: &CodingSessionId) -> CodingSession {
        self.sessions
            .get(&self.workspace_id, id)
            .await
            .unwrap()
            .expect("the session record")
    }

    async fn rows(&self, id: &CodingSessionId) -> Vec<CodingSessionEvent> {
        self.sessions
            .list_events(&self.workspace_id, id, None, 1_000)
            .await
            .unwrap()
    }

    /// Waits until the record is in `state`.
    async fn wait_for_state(&self, id: &CodingSessionId, state: State) -> CodingSession {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let record = self.record(id).await;
            if record.state == state {
                return record;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the session is {:?}, not {state:?}",
                record.state
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Waits until the transcript holds a row of `kind`, and answers the
    /// transcript.
    async fn wait_for_row(&self, id: &CodingSessionId, kind: Kind) -> Vec<CodingSessionEvent> {
        self.wait_for_rows(id, kind, 1).await
    }

    /// Waits until the transcript holds `count` rows of `kind`.
    async fn wait_for_rows(
        &self,
        id: &CodingSessionId,
        kind: Kind,
        count: usize,
    ) -> Vec<CodingSessionEvent> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let rows = self.rows(id).await;
            if rows.iter().filter(|row| row.kind == kind).count() >= count {
                return rows;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "fewer than {count} {kind:?} rows in {:?}",
                kinds(&rows)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// One call of a session to the Trigger module.
#[derive(Debug, Clone)]
enum RuleCall {
    Create(CodingSessionId),
    Ingest(IngestBatch),
    End(CodingSessionId),
}

/// The Session Rules and the `ingest` of the Trigger module. It records
/// each call in order, and refuses to make a rule when told to.
#[derive(Default)]
struct RecordedRules {
    calls: Mutex<Vec<RuleCall>>,
    refuse_create: AtomicBool,
}

impl RecordedRules {
    fn calls(&self) -> Vec<RuleCall> {
        self.calls.lock().unwrap().clone()
    }

    /// The batches of `kind`, oldest first.
    fn batches(&self, kind: &str) -> Vec<IngestBatch> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                RuleCall::Ingest(batch) if batch.event_kind == kind => Some(batch),
                _ => None,
            })
            .collect()
    }

    /// Waits until the rules got `count` batches of `kind`.
    async fn wait_for_batches(&self, kind: &str, count: usize) -> Vec<IngestBatch> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let batches = self.batches(kind);
            if batches.len() >= count {
                return batches;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "fewer than {count} {kind} batches in {:?}",
                self.calls()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl SessionRules for RecordedRules {
    async fn create(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        if self.refuse_create.load(Ordering::SeqCst) {
            return Err(SessionRuleError(
                "the trigger module is not ready".to_string(),
            ));
        }
        self.calls
            .lock()
            .unwrap()
            .push(RuleCall::Create(session.id.clone()));
        Ok(())
    }

    async fn end(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        self.calls
            .lock()
            .unwrap()
            .push(RuleCall::End(session.id.clone()));
        Ok(())
    }
}

#[async_trait]
impl SessionEvents for RecordedRules {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), SessionRuleError> {
        self.calls.lock().unwrap().push(RuleCall::Ingest(batch));
        Ok(())
    }
}

fn kinds(rows: &[CodingSessionEvent]) -> Vec<Kind> {
    rows.iter().map(|row| row.kind).collect()
}

fn rows_of(rows: &[CodingSessionEvent], kind: Kind) -> Vec<&CodingSessionEvent> {
    rows.iter().filter(|row| row.kind == kind).collect()
}

/// The `prompt` texts that the fake got.
fn prompts(harness: &FakeHarness) -> Vec<String> {
    harness
        .params("session/prompt")
        .into_iter()
        .map(|params| params["prompt"][0]["text"].as_str().unwrap().to_string())
        .collect()
}

/// A bus that drops each event. The full-daemon tests read the events.
struct SilentBus;

#[async_trait]
impl EventBus for SilentBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        Ok(Event {
            id: EventId::generate(),
            seq: 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _: EventScope, _: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// A place that runs the fake harness at the other end of a
/// `tokio::io::duplex` pipe, or refuses as the Client App does.
struct DuplexPlace {
    script: Script,
    refusal: Option<OpenFailure>,
    requests: Mutex<Vec<(WorkspaceId, HostId, OpenRequest)>>,
    harness: Mutex<Option<FakeHarness>>,
    exit: Mutex<Option<oneshot::Sender<SessionExit>>>,
    /// Set when the daemon drops its end of the stream.
    dropped: Arc<AtomicBool>,
}

impl DuplexPlace {
    fn new(script: Script) -> Arc<Self> {
        Self::answering(script, None)
    }

    fn refusing(refusal: OpenFailure) -> Arc<Self> {
        Self::answering(Script::default(), Some(refusal))
    }

    fn answering(script: Script, refusal: Option<OpenFailure>) -> Arc<Self> {
        Arc::new(Self {
            script,
            refusal,
            requests: Mutex::default(),
            harness: Mutex::default(),
            exit: Mutex::default(),
            dropped: Arc::default(),
        })
    }

    fn harness(&self) -> FakeHarness {
        self.harness
            .lock()
            .unwrap()
            .clone()
            .expect("the harness runs")
    }

    /// Sends the exit of the process, as a `session_exit` frame does.
    fn exit(&self, exit: SessionExit) -> bool {
        let sender = self.exit.lock().unwrap().take().expect("one exit");
        sender.send(exit).is_ok()
    }
}

#[async_trait]
impl SessionPlace for DuplexPlace {
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        request: OpenRequest,
    ) -> Result<OpenedStream, OpenFailure> {
        self.requests
            .lock()
            .unwrap()
            .push((workspace_id.clone(), host_id.clone(), request));
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let (daemon_end, harness_end) = tokio::io::duplex(64 * 1024);
        let (harness_read, harness_write) = tokio::io::split(harness_end);
        let harness = FakeHarness::serve(
            self.script.clone(),
            harness_write.compat_write(),
            harness_read.compat(),
        );
        *self.harness.lock().unwrap() = Some(harness);
        let (exit, exited) = oneshot::channel();
        *self.exit.lock().unwrap() = Some(exit);
        Ok(OpenedStream {
            stream: Box::new(Tracked {
                inner: daemon_end.compat(),
                dropped: self.dropped.clone(),
            }),
            cwd: WORKTREE_DIRECTORY.to_string(),
            exit: exited,
        })
    }
}

/// A stream that says when it is dropped.
struct Tracked {
    inner: Compat<tokio::io::DuplexStream>,
    dropped: Arc<AtomicBool>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl AsyncRead for Tracked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tracked {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

/// `RefuseDecisions`, with each answer held until the test opens the
/// gate.
struct GatedDecisions {
    gate: Arc<Notify>,
}

impl GatedDecisions {
    fn new() -> (Arc<Self>, Arc<Notify>) {
        let gate = Arc::new(Notify::new());
        (Arc::new(Self { gate: gate.clone() }), gate)
    }
}

#[async_trait]
impl SessionDecisions for GatedDecisions {
    async fn permission(
        &self,
        _session: &CodingSession,
        _ask: PermissionAsk,
    ) -> Pending<PermissionAnswer> {
        let gate = self.gate.clone();
        Pending::Waits {
            waits_for: WaitsFor::Person,
            answer: Box::pin(async move {
                gate.notified().await;
                Waited::answered(PermissionAnswer::RejectOnce, Some(Decider::Person))
            }),
        }
    }

    async fn question(
        &self,
        _session: &CodingSession,
        _ask: QuestionAsk,
    ) -> Pending<QuestionAnswer> {
        let gate = self.gate.clone();
        Pending::Waits {
            waits_for: WaitsFor::Person,
            answer: Box::pin(async move {
                gate.notified().await;
                Waited::answered(QuestionAnswer::Cancel, None)
            }),
        }
    }
}

fn message(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(
        acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text)))
            .message_id(acp::MessageId::new("m1")),
    )
}

fn run_tests() -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new(
        "call-1",
        acp::ToolCallUpdateFields::new()
            .title("Run cargo test")
            .kind(acp::ToolKind::Execute),
    )
}

fn options() -> Vec<acp::PermissionOption> {
    vec![
        acp::PermissionOption::new(
            "allow-once",
            "Allow once",
            acp::PermissionOptionKind::AllowOnce,
        ),
        acp::PermissionOption::new(
            "reject-once",
            "Reject",
            acp::PermissionOptionKind::RejectOnce,
        ),
    ]
}

/// A turn that asks one permission, then ends.
fn asks_permission() -> Turn {
    Turn::new(vec![], acp::StopReason::EndTurn).asks(Ask::permission(run_tests(), options()))
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn start_writes_the_record_and_a_prompt_row_and_leaves_the_session_working(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));

    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .expect("the session starts");

    let record = world.record(&session.id).await;
    assert_eq!(record, session, "start answers the record that it wrote");
    assert_eq!(record.state, State::Working);
    assert_eq!(record.workspace_id, world.workspace_id);
    assert_eq!(record.agent_id, world.agent_id);
    assert_eq!(record.run_id, world.run_id);
    assert_eq!(record.harness_id, "claude");
    assert_eq!(
        record.harness_version,
        pagis_core::harness::entry("claude").unwrap().version
    );
    assert_eq!(record.place, CodingSessionPlace::Host);
    assert_eq!(record.host_id.as_ref(), Some(&world.host_id));
    assert_eq!(record.directory, DIRECTORY);
    assert_eq!(
        record.working_directory.as_deref(),
        Some(WORKTREE_DIRECTORY)
    );
    assert_eq!(record.worktree_branch.as_deref(), Some("pagis/fix-login"));
    assert_eq!(record.approval_mode, SessionApprovalMode::Person);
    assert_eq!(record.title, "Fix the login");
    assert_eq!(record.acp_session_id.as_deref(), Some(NEW_SESSION_ID));
    assert_eq!(record.channel_id, world.channel_id);
    assert_eq!(
        record.root_message_id, record.message_id,
        "a Run with no Thread makes the block the root of a new Thread"
    );
    assert_eq!(record.end_reason, None);

    let rows = world.rows(&session.id).await;
    assert_eq!(kinds(&rows), [Kind::Prompt]);
    assert_eq!(rows[0].payload["text"], "Fix the login bug.");

    let requests = place.requests.lock().unwrap().clone();
    let (workspace_id, host_id, request) = &requests[0];
    assert_eq!(
        (workspace_id, host_id),
        (&world.workspace_id, &world.host_id)
    );
    let (command, args) =
        pagis_core::harness::launch_command(pagis_core::harness::entry("claude").unwrap());
    assert_eq!(request.session_id, session.id);
    assert_eq!(request.command, command);
    assert_eq!(request.args, args);
    assert_eq!(request.cwd, DIRECTORY);
    assert_eq!(
        request
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.as_str()),
        Some("pagis/fix-login")
    );

    let harness = place.harness();
    assert_eq!(
        harness.params("session/new")[0]["cwd"],
        WORKTREE_DIRECTORY,
        "the harness runs in the directory of the open answer"
    );
    assert_eq!(prompts(&harness), ["Fix the login bug."]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_run_in_a_thread_gives_the_root_of_its_thread(pool: SqlitePool) {
    let world = world(pool).await;
    let (run_id, root) = world.run_in_thread().await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));

    let session = sessions.start(world.new_session(&run_id)).await.unwrap();

    assert_eq!(session.root_message_id, root);
    assert_ne!(session.message_id, root);
    assert_eq!(session.channel_id, world.channel_id);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_run_with_no_channel_starts_no_session(pool: SqlitePool) {
    let world = world(pool).await;
    let run_id = world.run(None, None).await;
    let place = DuplexPlace::new(Script::default());
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));

    let refused = sessions.start(world.new_session(&run_id)).await;

    assert!(
        matches!(refused, Err(StartFailure::NoConversation)),
        "{refused:?}"
    );
    assert!(place.requests.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_of_another_workspace_starts_no_session(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default());
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let stranger = HostId::generate();

    let refused = sessions
        .start(NewCodingSession {
            host_id: stranger.clone(),
            ..world.new_session(&world.run_id)
        })
        .await;

    assert!(
        matches!(&refused, Err(StartFailure::HostNotFound(host_id)) if *host_id == stranger),
        "{refused:?}"
    );
    assert!(place.requests.lock().unwrap().is_empty());
    let written = world
        .sessions
        .list(&world.workspace_id, None, None, None, 10)
        .await
        .unwrap();
    assert!(written.is_empty(), "no record: {written:?}");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_updates_of_a_turn_make_their_rows_and_its_end_makes_the_session_idle(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let updates = vec![
        message("I ran "),
        message("the tests, "),
        message("and they pass."),
        acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("call-1", "Run cargo test").kind(acp::ToolKind::Execute),
        ),
        acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            "call-1",
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        )),
        acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
            "Run the tests",
            acp::PlanEntryPriority::High,
            acp::PlanEntryStatus::Completed,
        )])),
        acp::SessionUpdate::UsageUpdate(
            acp::UsageUpdate::new(1200, 200_000).cost(acp::Cost::new(0.25, "USD")),
        ),
    ];
    let place =
        DuplexPlace::new(Script::default().turn(Turn::new(updates, acp::StopReason::MaxTokens)));
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));

    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let record = world.wait_for_state(&session.id, State::Idle).await;

    let rows = world.rows(&session.id).await;
    assert_eq!(
        kinds(&rows),
        [
            Kind::Prompt,
            Kind::AgentMessage,
            Kind::ToolCall,
            Kind::ToolCallUpdate,
            Kind::Plan,
            Kind::Usage,
            Kind::TurnEnd,
        ]
    );
    assert_eq!(
        rows[1].payload,
        json!({"text": "I ran the tests, and they pass.", "message_id": "m1"}),
        "three chunks of one message make one row"
    );
    assert_eq!(rows[2].payload["toolCallId"], "call-1");
    assert_eq!(rows[3].payload["status"], "completed");
    assert_eq!(
        rows[4].payload["entries"][0],
        json!({"content": "Run the tests", "priority": "high", "status": "completed"})
    );
    assert_eq!(rows[5].payload["used"], 1200);
    assert_eq!(rows[6].payload, json!({"stop_reason": "max_tokens"}));

    assert_eq!(record.usage.context_used, Some(1200));
    assert_eq!(record.usage.context_size, Some(200_000));
    assert_eq!(record.usage.cost_amount, Some(0.25));
    assert_eq!(record.usage.cost_currency.as_deref(), Some("USD"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn two_prompts_sent_while_a_turn_runs_go_as_one_prompt_when_the_turn_ends(pool: SqlitePool) {
    let world = world(pool).await;
    let (decisions, gate) = GatedDecisions::new();
    let place = DuplexPlace::new(Script::default().turn(asks_permission()));
    let sessions = world.coding_sessions(place.clone(), decisions);
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world
        .wait_for_state(&session.id, State::NeedsDecision)
        .await;

    for text in ["Also add a test.", "And update the docs."] {
        let outcome = sessions
            .prompt(&world.workspace_id, &session.id, text.to_string())
            .await
            .unwrap();
        assert_eq!(outcome, PromptOutcome::Queued);
    }
    gate.notify_one();
    // The queued prompts start a second turn when the first ends, so the
    // session is idle for a moment between the two turns. Wait for the
    // end of the second turn.
    world.wait_for_rows(&session.id, Kind::TurnEnd, 2).await;
    world.wait_for_state(&session.id, State::Idle).await;

    assert_eq!(
        prompts(&place.harness()),
        [
            "Fix the login bug.",
            "Also add a test.\n\nAnd update the docs."
        ]
    );
    let rows = world.rows(&session.id).await;
    let sent: Vec<&Value> = rows_of(&rows, Kind::Prompt)
        .into_iter()
        .map(|row| &row.payload["text"])
        .collect();
    assert_eq!(
        sent,
        [
            "Fix the login bug.",
            "Also add a test.\n\nAnd update the docs."
        ]
    );

    let outcome = sessions
        .prompt(&world.workspace_id, &session.id, "One more.".to_string())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PromptOutcome::Sent,
        "an idle session sends at once"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_permission_request_waits_in_needs_decision_and_gets_reject_once(pool: SqlitePool) {
    let world = world(pool).await;
    let (decisions, gate) = GatedDecisions::new();
    let place = DuplexPlace::new(Script::default().turn(asks_permission()));
    let sessions = world.coding_sessions(place.clone(), decisions);
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    world
        .wait_for_state(&session.id, State::NeedsDecision)
        .await;
    let rows = world.wait_for_row(&session.id, Kind::Permission).await;
    let asked = &rows_of(&rows, Kind::Permission)[0].payload;
    assert_eq!(asked["waits_for"], "person");
    assert_eq!(asked["tool_call_id"], "call-1");
    assert_eq!(asked["title"], "Run cargo test");

    gate.notify_one();
    let rows = world.wait_for_row(&session.id, Kind::TurnEnd).await;
    let decided = &rows_of(&rows, Kind::Decision)[0].payload;
    assert_eq!(decided["decision"], "reject_once");
    assert_eq!(decided["decider"], "person");
    assert_eq!(decided["ask_id"], asked["ask_id"]);
    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
    );
    world.wait_for_state(&session.id, State::Idle).await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_question_gets_cancel(pool: SqlitePool) {
    let world = world(pool).await;
    let ask = Ask::form(
        "Which branch?",
        acp::ElicitationSchema::new().string("branch", true),
    );
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)),
    );
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    let rows = world.wait_for_row(&session.id, Kind::TurnEnd).await;

    let asked = &rows_of(&rows, Kind::Question)[0].payload;
    assert_eq!(asked["waits_for"], "person");
    assert_eq!(asked["message"], "Which branch?");
    assert_eq!(rows_of(&rows, Kind::Answer)[0].payload["answer"], "cancel");
    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"action": "cancel"})
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_withdrawn_permission_writes_a_withdrawn_decision(pool: SqlitePool) {
    let world = world(pool).await;
    let (decisions, _gate) = GatedDecisions::new();
    let ask = Ask::permission(run_tests(), options()).withdrawn();
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)),
    );
    let sessions = world.coding_sessions(place.clone(), decisions);
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world
        .wait_for_state(&session.id, State::NeedsDecision)
        .await;

    place.harness().withdraw_ask();

    let rows = world.wait_for_row(&session.id, Kind::Decision).await;
    assert_eq!(
        rows_of(&rows, Kind::Decision)[0].payload["decision"],
        "withdrawn"
    );
    world.wait_for_state(&session.id, State::Idle).await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn cancel_ends_the_turn_with_cancelled_and_empties_the_queue(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let queued = sessions
        .prompt(&world.workspace_id, &session.id, "Also this.".to_string())
        .await
        .unwrap();
    assert_eq!(queued, PromptOutcome::Queued);

    sessions
        .cancel(&world.workspace_id, &session.id)
        .await
        .unwrap();

    let rows = world.wait_for_row(&session.id, Kind::TurnEnd).await;
    assert_eq!(
        rows_of(&rows, Kind::TurnEnd)[0].payload,
        json!({"stop_reason": "cancelled"})
    );
    world.wait_for_state(&session.id, State::Idle).await;
    assert_eq!(
        prompts(&place.harness()),
        ["Fix the login bug."],
        "the cancel emptied the queue"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn close_closes_the_stream_and_the_exit_that_follows_changes_nothing(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    sessions
        .close(&world.workspace_id, &session.id, CloseReason::Closed)
        .await
        .unwrap();

    let record = world.record(&session.id).await;
    assert_eq!(record.state, State::Closed);
    assert_eq!(record.end_reason.as_deref(), Some("closed"));
    assert!(record.ended_at.is_some());
    let deadline = tokio::time::Instant::now() + WAIT;
    while !place.dropped.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the stream is not closed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        place.harness().params("session/cancel").len(),
        1,
        "close cancels the turn that runs"
    );

    place.exit(SessionExit::new(Some(0), String::new()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(world.record(&session.id).await, record);
    let refused = sessions
        .prompt(&world.workspace_id, &session.id, "More.".to_string())
        .await;
    assert!(
        matches!(refused, Err(SessionError::NotOpen(State::Closed))),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_stop_of_the_person_closes_the_session_with_the_end_reason_stopped(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    sessions
        .close(&world.workspace_id, &session.id, CloseReason::Stopped)
        .await
        .unwrap();

    let record = world.record(&session.id).await;
    assert_eq!(record.state, State::Closed);
    assert_eq!(record.end_reason.as_deref(), Some("stopped"));
    let again = sessions
        .close(&world.workspace_id, &session.id, CloseReason::Stopped)
        .await;
    assert!(
        matches!(again, Err(SessionError::NotOpen(State::Closed))),
        "{again:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_open_answer_not_found_fails_the_session(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::refusing(OpenFailure {
        code: OpenFailureCode::NotFound,
        message: "npx is not on PATH".to_string(),
    });
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));

    let failed = sessions.start(world.new_session(&world.run_id)).await;

    let Err(StartFailure::Open {
        session_id,
        failure,
    }) = failed
    else {
        panic!("the start fails to open: {failed:?}");
    };
    assert_eq!(failure.code, OpenFailureCode::NotFound);
    let record = world.record(&session_id).await;
    assert_eq!(record.state, State::Failed);
    assert_eq!(record.end_reason.as_deref(), Some("not_found"));
    assert!(record.ended_at.is_some());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_session_new_that_needs_a_sign_in_fails_the_session_and_reports_the_harness(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().new_session_auth_required());
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));

    let failed = sessions.start(world.new_session(&world.run_id)).await;

    let Err(failure) = failed else {
        panic!("the start fails: {failed:?}");
    };
    let StartFailure::SignInRequired { session_id, .. } = &failure else {
        panic!("the start needs a sign-in: {failure:?}");
    };
    assert_eq!(
        failure.to_string(),
        "Claude Code is not signed in on your Air. Ask the user to sign in: Settings › Hosts › Air."
    );
    let record = world.record(session_id).await;
    assert_eq!(record.state, State::Failed);
    assert_eq!(record.end_reason.as_deref(), Some("sign_in_required"));
    assert!(world.needs_sign_in().await);
    assert_eq!(world.sign_in_changes().await, [world.sign_in_change(true)]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_later_session_new_that_succeeds_clears_the_report(pool: SqlitePool) {
    let world = world(pool).await;
    let signed_out = DuplexPlace::new(Script::default().new_session_auth_required());
    world
        .coding_sessions(signed_out, Arc::new(RefuseDecisions))
        .start(world.new_session(&world.run_id))
        .await
        .expect_err("the harness needs a sign-in");
    assert!(world.needs_sign_in().await);

    let signed_in = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let run_id = world.run(Some(world.channel_id.clone()), None).await;
    world
        .coding_sessions(signed_in, Arc::new(RefuseDecisions))
        .start(world.new_session(&run_id))
        .await
        .expect("the session starts");

    assert!(!world.needs_sign_in().await);
    assert_eq!(
        world.sign_in_changes().await,
        [world.sign_in_change(true), world.sign_in_change(false)]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_prompt_that_needs_a_sign_in_in_a_later_turn_fails_the_session_and_reports_it(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let place = DuplexPlace::new(
        Script::default()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn))
            .turn(Turn::auth_required()),
    );
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world.wait_for_state(&session.id, State::Idle).await;
    assert!(!world.needs_sign_in().await);

    sessions
        .prompt(
            &world.workspace_id,
            &session.id,
            "Run the tests.".to_string(),
        )
        .await
        .unwrap();

    let record = world.wait_for_state(&session.id, State::Failed).await;
    assert_eq!(record.end_reason.as_deref(), Some("sign_in_required"));
    assert!(world.needs_sign_in().await);
    assert_eq!(world.sign_in_changes().await, [world.sign_in_change(true)]);
    let ended = world
        .rules
        .wait_for_batches("coding_session.ended", 1)
        .await;
    assert_eq!(ended[0].events[0].metadata["reason"], "sign_in_required");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_exit_that_nobody_asked_for_fails_the_session_with_its_exit_code(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    assert!(place.exit(SessionExit::new(
        Some(3),
        "panic: out of memory".to_string()
    )));

    let record = world.wait_for_state(&session.id, State::Failed).await;
    assert_eq!(record.end_reason.as_deref(), Some("harness_exited"));
    let detail = record.end_detail.expect("an end detail");
    assert!(detail.contains("exit code 3"), "{detail}");
    assert!(detail.contains("panic: out of memory"), "{detail}");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn get_with_the_id_of_another_workspaces_session_answers_none(pool: SqlitePool) {
    let world = world(pool.clone()).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let person = pagis_storage_sqlite::seed_org_and_administrator(&pool, "Other", now_ms())
        .await
        .unwrap();
    let other = workspace(&pool, person.id).await;

    assert_eq!(sessions.get(&other, &session.id).await.unwrap(), None);
    assert_eq!(
        sessions
            .get(&world.workspace_id, &session.id)
            .await
            .unwrap()
            .map(|record| record.id),
        Some(session.id.clone())
    );
    let refused = sessions
        .prompt(&other, &session.id, "Hello.".to_string())
        .await;
    assert!(
        matches!(refused, Err(SessionError::NotFound)),
        "{refused:?}"
    );
}

/// A permission of `kind` for the tool call `call-1`.
fn tool_call(
    kind: acp::ToolKind,
    locations: &[&str],
    command: Option<&str>,
) -> acp::ToolCallUpdate {
    let mut fields = acp::ToolCallUpdateFields::new()
        .title("A tool")
        .kind(kind)
        .locations(
            locations
                .iter()
                .map(|path| acp::ToolCallLocation::new(*path))
                .collect::<Vec<_>>(),
        );
    if let Some(command) = command {
        fields = fields.raw_input(json!({ "command": command }));
    }
    acp::ToolCallUpdate::new("call-1", fields)
}

fn every_option() -> Vec<acp::PermissionOption> {
    [
        ("allow-always", acp::PermissionOptionKind::AllowAlways),
        ("allow-once", acp::PermissionOptionKind::AllowOnce),
        ("reject-always", acp::PermissionOptionKind::RejectAlways),
        ("reject-once", acp::PermissionOptionKind::RejectOnce),
    ]
    .into_iter()
    .map(|(id, kind)| acp::PermissionOption::new(id, id, kind))
    .collect()
}

fn edit_inside() -> acp::ToolCallUpdate {
    tool_call(
        acp::ToolKind::Edit,
        &[&format!("{WORKTREE_DIRECTORY}/src/login.rs")],
        None,
    )
}

/// An allow selects the `allow_once` option, even when the harness offers
/// `allow_always` first. The session does not wait, and the audit fact
/// names the scope.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn policy_allows_an_edit_inside_the_directory_once_with_the_scope_as_decider(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let ask = Ask::permission(edit_inside(), every_option());
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)),
    );
    let sessions = world.coding_sessions(place.clone(), world.policy());
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    let rows = world.wait_for_row(&session.id, Kind::TurnEnd).await;

    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
    );
    let asked = &rows_of(&rows, Kind::Permission)[0].payload;
    assert_eq!(asked["waits_for"], Value::Null);
    assert_eq!(asked["kind"], "edit");
    let decided = &rows_of(&rows, Kind::Decision)[0].payload;
    assert_eq!(decided["decision"], "allow_once");
    assert_eq!(decided["decider"], "scope");
    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].agent_id.as_ref(), Some(&world.agent_id));
    assert_eq!(facts[0].run_id, None);
    assert_eq!(
        facts[0].payload,
        json!({
            "session_id": session.id,
            "agent_id": world.agent_id,
            "host_id": world.host_id,
            "tool_call_id": "call-1",
            "tool_kind": "edit",
            "command": null,
            "locations": [format!("{WORKTREE_DIRECTORY}/src/login.rs")],
            "grant_revision": null,
            "decider": "scope",
            "outcome": "allowed",
            "option_kind": "allow_once",
        })
    );
    world.wait_for_state(&session.id, State::Idle).await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_allow_with_no_allow_once_option_answers_cancelled(pool: SqlitePool) {
    let world = world(pool).await;
    let options = vec![
        acp::PermissionOption::new(
            "allow-always",
            "Always",
            acp::PermissionOptionKind::AllowAlways,
        ),
        acp::PermissionOption::new("reject-once", "No", acp::PermissionOptionKind::RejectOnce),
    ];
    let ask = Ask::permission(edit_inside(), options);
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)),
    );
    let sessions = world.coding_sessions(place.clone(), world.policy());
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    world.wait_for_row(&session.id, Kind::TurnEnd).await;

    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "cancelled"}})
    );
    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "scope");
    assert_eq!(facts[0].payload["outcome"], "cancelled");
    assert_eq!(facts[0].payload["option_kind"], Value::Null);
}

/// A command that no rule allows waits for the Person on an approval
/// card in the session's Thread. A cancel answers it `cancelled`, as ACP
/// requires, expires its Request, and its audit fact has no decider.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_permission_that_policy_does_not_allow_asks_the_person_until_a_cancel_expires_it(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let ask = Ask::permission(
        tool_call(acp::ToolKind::Execute, &[], Some("rm -rf /")),
        every_option(),
    );
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![]).asks(ask)));
    let sessions = world.coding_sessions(place.clone(), world.policy());
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    world
        .wait_for_state(&session.id, State::NeedsDecision)
        .await;
    let rows = world.wait_for_row(&session.id, Kind::Permission).await;
    assert_eq!(
        rows_of(&rows, Kind::Permission)[0].payload["waits_for"],
        "person"
    );
    assert!(place.harness().answers().is_empty(), "the permission waits");
    let requests = SqliteRequestStore::new(world.pool.clone());
    let pending = requests
        .list_by_state(
            &world.workspace_id,
            RequestState::Pending,
            Some(Request::HARNESS_PERMISSION_KIND),
        )
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].run_id, None);
    assert_eq!(pending[0].payload["command"], "rm -rf /");
    let thread = SqliteMessageStore::new(world.pool.clone())
        .list_thread(&world.workspace_id, &session.root_message_id)
        .await
        .unwrap();
    let card = thread.last().expect("the card");
    assert_eq!(card.author_kind, AuthorKind::System);
    assert_eq!(
        card.parent_message_id.as_ref(),
        Some(&session.root_message_id)
    );
    assert_eq!(
        card.blocks,
        [Block::approval_card(
            pending[0].id.as_str(),
            "Claude Code wants to run a command",
            "rm -rf /"
        )]
    );

    sessions
        .cancel(&world.workspace_id, &session.id)
        .await
        .unwrap();

    let rows = world.wait_for_row(&session.id, Kind::TurnEnd).await;
    assert_eq!(
        rows_of(&rows, Kind::Decision)[0].payload["decision"],
        "withdrawn"
    );
    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "cancelled"}})
    );
    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["command"], "rm -rf /");
    assert_eq!(facts[0].payload["tool_kind"], "execute");
    assert_eq!(facts[0].payload["decider"], Value::Null);
    assert_eq!(facts[0].payload["outcome"], "expired");
    let expired = requests
        .get(&world.workspace_id, &pending[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.state, RequestState::Expired);
    world.wait_for_state(&session.id, State::Idle).await;
}

/// The effective mode is the narrower of the session's mode and the
/// widest mode of the live host Grant, so an `auto` session on a Grant
/// that allows `agent` asks the Agent.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_host_grant_narrows_the_mode_of_the_session(pool: SqlitePool) {
    let world = world(pool).await;
    world.host_grant(SessionApprovalMode::Agent, &[]).await;
    let ask = Ask::permission(
        tool_call(acp::ToolKind::Execute, &[], Some("cargo test")),
        every_option(),
    );
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![]).asks(ask)));
    let sessions = world.coding_sessions(place.clone(), world.policy());
    let mut new = world.new_session(&world.run_id);
    new.approval_mode = SessionApprovalMode::Auto;
    let session = sessions.start(new).await.unwrap();

    world
        .wait_for_state(&session.id, State::NeedsDecision)
        .await;
    let rows = world.wait_for_row(&session.id, Kind::Permission).await;
    assert_eq!(
        rows_of(&rows, Kind::Permission)[0].payload["waits_for"],
        "agent"
    );
}

/// An `auto` session on a Grant that allows `auto` allows each request.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_auto_session_on_an_auto_grant_allows_with_auto_as_decider(pool: SqlitePool) {
    let world = world(pool).await;
    world.host_grant(SessionApprovalMode::Auto, &[]).await;
    let ask = Ask::permission(
        tool_call(acp::ToolKind::Execute, &[], Some("rm -rf /")),
        every_option(),
    );
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)),
    );
    let sessions = world.coding_sessions(place.clone(), world.policy());
    let mut new = world.new_session(&world.run_id);
    new.approval_mode = SessionApprovalMode::Auto;
    let session = sessions.start(new).await.unwrap();

    world.wait_for_row(&session.id, Kind::TurnEnd).await;

    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "auto");
    assert_eq!(facts[0].payload["grant_revision"], 1);
    assert_eq!(
        place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_exit_on_the_session_socket_fails_the_session_and_an_exit_after_close_changes_nothing(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let hosts = Arc::new(HostSessions::new());
    let (daemon_end, client_app_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn({
        let hosts = hosts.clone();
        let (workspace_id, host_id) = (world.workspace_id.clone(), world.host_id.clone());
        async move {
            hosts
                .serve(workspace_id, host_id, daemon_end.compat())
                .await;
        }
    });
    let script = Script::default()
        .turn(Turn::until_cancel(vec![]))
        .turn(Turn::until_cancel(vec![]));
    tokio::spawn(serve_client_app(
        client_app_end.compat(),
        script,
        WORKTREE_DIRECTORY.to_string(),
    ));
    let deadline = tokio::time::Instant::now() + WAIT;
    while !hosts.is_open(&world.host_id) {
        assert!(tokio::time::Instant::now() < deadline, "no session socket");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let sessions = world.coding_sessions(hosts.clone(), Arc::new(RefuseDecisions));

    let exited = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    assert_eq!(
        exited.working_directory.as_deref(),
        Some(WORKTREE_DIRECTORY)
    );
    hosts.exited(
        &world.workspace_id,
        &world.host_id,
        &exited.id,
        SessionExit::new(Some(137), "killed".to_string()),
    );
    let record = world.wait_for_state(&exited.id, State::Failed).await;
    assert_eq!(record.end_reason.as_deref(), Some("harness_exited"));
    assert!(record.end_detail.unwrap().contains("exit code 137"));

    let closed = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    sessions
        .close(&world.workspace_id, &closed.id, CloseReason::Closed)
        .await
        .unwrap();
    hosts.exited(
        &world.workspace_id,
        &world.host_id,
        &closed.id,
        SessionExit::new(Some(0), String::new()),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let record = world.record(&closed.id).await;
    assert_eq!(record.state, State::Closed);
    assert_eq!(record.end_reason.as_deref(), Some("closed"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_end_of_a_turn_raises_turn_ended_with_the_row_of_the_end(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(
        Script::default().turn(Turn::new(vec![message("Done.")], acp::StopReason::EndTurn)),
    );
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));

    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    let batches = world
        .rules
        .wait_for_batches("coding_session.turn_ended", 1)
        .await;
    assert!(
        matches!(&world.rules.calls()[0], RuleCall::Create(id) if *id == session.id),
        "the rule is made before any news: {:?}",
        world.rules.calls()
    );
    let rows = world.rows(&session.id).await;
    let end = rows_of(&rows, Kind::TurnEnd)[0];
    let batch = &batches[0];
    assert_eq!(batch.workspace_id, world.workspace_id);
    assert_eq!(batch.agent_id.as_ref(), Some(&world.agent_id));
    assert_eq!(batch.source.coding_session_id(), Some(&session.id));
    let event = &batch.events[0];
    assert_eq!(
        event.provider_event_id,
        format!("{}:{}", session.id, end.seq)
    );
    assert_eq!(event.metadata["stop_reason"], "end_turn");
    assert_eq!(event.metadata["seq"], end.seq);
    assert_eq!(event.metadata["harness"], "claude");
    assert_eq!(event.metadata["machine"], "Air");
    assert_eq!(event.metadata["title"], "Fix the login");
    assert!(
        !event.metadata.to_string().contains("Done."),
        "the metadata holds no harness text"
    );
    assert!(world.rules.batches("coding_session.ended").is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_permission_that_waits_raises_needs_decision(pool: SqlitePool) {
    let world = world(pool).await;
    let (decisions, _gate) = GatedDecisions::new();
    let place = DuplexPlace::new(Script::default().turn(asks_permission()));
    let sessions = world.coding_sessions(place, decisions);
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    let batches = world
        .rules
        .wait_for_batches("coding_session.needs_decision", 1)
        .await;

    let rows = world.rows(&session.id).await;
    let asked = rows_of(&rows, Kind::Permission)[0];
    let event = &batches[0].events[0];
    assert_eq!(event.metadata["decision_kind"], "permission");
    assert_eq!(event.metadata["seq"], asked.seq);
    assert_eq!(
        event.provider_event_id,
        format!("{}:{}", session.id, asked.seq)
    );
    assert!(world.rules.batches("coding_session.turn_ended").is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn close_raises_ended_and_then_ends_the_rules(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place, Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    sessions
        .close(&world.workspace_id, &session.id, CloseReason::Stopped)
        .await
        .unwrap();

    let calls = world.rules.calls();
    let [.., RuleCall::Ingest(ended), RuleCall::End(ended_id)] = calls.as_slice() else {
        panic!("close raises the end and then ends the rules: {calls:?}");
    };
    assert_eq!(*ended_id, session.id);
    assert_eq!(ended.event_kind, "coding_session.ended");
    let event = &ended.events[0];
    assert_eq!(
        event.provider_event_id,
        format!("{}:ended:closed", session.id)
    );
    assert_eq!(event.metadata["state"], "closed");
    assert_eq!(event.metadata["reason"], "stopped");
}

/// A session with no task, such as a session of the daemon before a
/// restart, closes from its record. That close also raises the end and
/// then ends the rules.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn close_of_a_session_with_no_task_raises_ended_and_then_ends_the_rules(pool: SqlitePool) {
    let world = world(pool).await;
    let before = world.coding_sessions(
        DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![]))),
        Arc::new(RefuseDecisions),
    );
    let session = before
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let after = world.coding_sessions(
        DuplexPlace::new(Script::default()),
        Arc::new(RefuseDecisions),
    );

    after
        .close(&world.workspace_id, &session.id, CloseReason::Closed)
        .await
        .unwrap();

    assert_eq!(world.record(&session.id).await.state, State::Closed);
    let calls = world.rules.calls();
    let [.., RuleCall::Ingest(ended), RuleCall::End(ended_id)] = calls.as_slice() else {
        panic!("close raises the end and then ends the rules: {calls:?}");
    };
    assert_eq!(*ended_id, session.id);
    assert_eq!(ended.event_kind, "coding_session.ended");
    let event = &ended.events[0];
    assert_eq!(
        event.provider_event_id,
        format!("{}:ended:closed", session.id)
    );
    assert_eq!(event.metadata["machine"], "Air");
    assert_eq!(event.metadata["reason"], "closed");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_harness_that_exits_raises_ended_and_then_ends_the_rules(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    assert!(place.exit(SessionExit::new(Some(3), "panic".to_string())));

    let batches = world
        .rules
        .wait_for_batches("coding_session.ended", 1)
        .await;
    let event = &batches[0].events[0];
    assert_eq!(event.metadata["state"], "failed");
    assert_eq!(event.metadata["reason"], "harness_exited");
    let deadline = tokio::time::Instant::now() + WAIT;
    while !matches!(world.rules.calls().last(), Some(RuleCall::End(id)) if *id == session.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the rules do not end: {:?}",
            world.rules.calls()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_start_whose_session_rule_is_not_made_fails_before_the_stream_opens(pool: SqlitePool) {
    let world = world(pool).await;
    world.rules.refuse_create.store(true, Ordering::SeqCst);
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));

    let failed = sessions.start(world.new_session(&world.run_id)).await;

    let Err(StartFailure::Rules { session_id, .. }) = failed else {
        panic!("the start fails: {failed:?}");
    };
    assert!(place.requests.lock().unwrap().is_empty(), "no stream opens");
    let record = world.record(&session_id).await;
    assert_eq!(record.state, State::Failed);
    assert_eq!(
        record.end_reason.as_deref(),
        Some("temporarily_unavailable")
    );
    let calls = world.rules.calls();
    assert!(
        matches!(calls.as_slice(), [RuleCall::End(id)] if *id == session_id),
        "a part of the rule that was made ends, and no news goes out: {calls:?}"
    );
}

impl World {
    /// A second machine of the Person.
    async fn other_host(&self) -> HostId {
        SqliteHostStore::new(self.pool.clone())
            .register(&self.workspace_id, "Mini", "macos", &[], now_ms())
            .await
            .unwrap()
            .id
    }

    /// A session record in `state` with no task, written through the
    /// store, as a daemon before a restart left it.
    async fn stored(&self, state: State) -> CodingSession {
        let at = now_ms();
        let terminal = state.is_terminal();
        let message_id = MessageId::generate();
        let session = CodingSession {
            id: CodingSessionId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            harness_id: "claude".to_string(),
            harness_version: "1.0.0".to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(self.host_id.clone()),
            directory: DIRECTORY.to_string(),
            working_directory: Some(WORKTREE_DIRECTORY.to_string()),
            worktree_branch: Some("pagis/fix-login".to_string()),
            approval_mode: SessionApprovalMode::Person,
            title: format!("A {} session", state.as_str()),
            state,
            end_reason: terminal.then(|| "closed".to_string()),
            end_detail: None,
            acp_session_id: Some(NEW_SESSION_ID.to_string()),
            channel_id: self.channel_id.clone(),
            root_message_id: message_id.clone(),
            message_id,
            run_id: self.run_id.clone(),
            usage: Default::default(),
            created_at: at,
            updated_at: at,
            ended_at: terminal.then_some(at),
        };
        self.sessions.insert(&session).await.unwrap();
        session
    }

    /// The interruptions that the rules heard, by session.
    fn interruptions(&self) -> Vec<(String, Value)> {
        self.rules
            .batches("coding_session.ended")
            .into_iter()
            .flat_map(|batch| batch.events)
            .filter(|event| event.metadata["state"] == "interrupted")
            .map(|event| (event.provider_event_id, event.metadata))
            .collect()
    }

    /// Asserts that the session is `interrupted` with no end reason, and
    /// that the rules heard its interruption with `reason` once.
    async fn assert_interrupted(&self, id: &CodingSessionId, reason: &str) {
        let record = self.wait_for_state(id, State::Interrupted).await;
        assert_eq!(record.end_reason, None, "an interruption is no end");
        assert_eq!(record.ended_at, None);
        // The news goes after the write of the record.
        let heard_of = || -> Vec<_> {
            self.interruptions()
                .into_iter()
                .filter(|(_, metadata)| metadata["coding_session_id"] == id.as_str())
                .collect()
        };
        let deadline = tokio::time::Instant::now() + WAIT;
        while heard_of().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "no interruption of {id} in {:?}",
                self.rules.calls()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let heard = heard_of();
        assert_eq!(heard.len(), 1, "one interruption of {id}: {heard:?}");
        let (event_id, metadata) = &heard[0];
        assert_eq!(metadata["reason"], reason);
        assert_eq!(
            *event_id,
            format!("{id}:ended:interrupted:{}", record.updated_at)
        );
        assert!(
            !self
                .rules
                .calls()
                .iter()
                .any(|call| matches!(call, RuleCall::End(ended) if ended == id)),
            "the Session Rule of an interrupted session stays"
        );
    }
}

/// A turn that ends at once, so the session is `idle`.
fn ends_at_once() -> Turn {
    Turn::new(vec![], acp::StopReason::EndTurn)
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_departure_of_the_host_interrupts_its_sessions_and_no_session_of_another_host(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let other_host = world.other_host().await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let connection = world.presence.connect(&world.host_id);
    let working = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let idle_place = DuplexPlace::new(Script::default().turn(ends_at_once()));
    let idle_sessions = world.coding_sessions(idle_place, Arc::new(RefuseDecisions));
    let idle = idle_sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world.wait_for_state(&idle.id, State::Idle).await;
    let elsewhere = sessions
        .start(NewCodingSession {
            host_id: other_host,
            ..world.new_session(&world.run_id)
        })
        .await
        .unwrap();

    drop(connection);

    world.assert_interrupted(&working.id, "host_lost").await;
    world.assert_interrupted(&idle.id, "host_lost").await;
    assert!(
        place.dropped.load(Ordering::SeqCst),
        "the stream of an interrupted session closes"
    );
    assert_eq!(world.record(&elsewhere.id).await.state, State::Working);
    assert_eq!(
        sessions
            .prompt(&world.workspace_id, &working.id, "Go on.".to_string())
            .await
            .unwrap_err()
            .to_string(),
        "the Coding Session is interrupted",
        "an interrupted session has no task"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_lost_session_socket_interrupts_its_sessions_and_no_session_of_another_host(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let other_host = world.other_host().await;
    let hosts = Arc::new(HostSessions::new());
    let serve = |host_id: HostId, script: Script| {
        let (daemon_end, client_app_end) = tokio::io::duplex(64 * 1024);
        tokio::spawn({
            let hosts = hosts.clone();
            let workspace_id = world.workspace_id.clone();
            async move {
                hosts
                    .serve(workspace_id, host_id, daemon_end.compat())
                    .await;
            }
        });
        tokio::spawn(serve_client_app(
            client_app_end.compat(),
            script,
            WORKTREE_DIRECTORY.to_string(),
        ))
    };
    // The harness of each stream plays the turns of the script.
    let client_app = serve(
        world.host_id.clone(),
        Script::default().turn(Turn::until_cancel(vec![])),
    );
    let _other_client_app = serve(
        other_host.clone(),
        Script::default().turn(Turn::until_cancel(vec![])),
    );
    let deadline = tokio::time::Instant::now() + WAIT;
    while !(hosts.is_open(&world.host_id) && hosts.is_open(&other_host)) {
        assert!(tokio::time::Instant::now() < deadline, "no session socket");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let sessions = world.coding_sessions(hosts.clone(), Arc::new(RefuseDecisions));
    let working = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    let elsewhere = sessions
        .start(NewCodingSession {
            host_id: other_host,
            ..world.new_session(&world.run_id)
        })
        .await
        .unwrap();

    // The Client App goes away, and its end of the socket with it.
    client_app.abort();

    world.assert_interrupted(&working.id, "host_lost").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(world.record(&elsewhere.id).await.state, State::Working);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_lost_session_socket_interrupts_an_idle_session(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(ends_at_once()));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let idle = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world.wait_for_state(&idle.id, State::Idle).await;

    // The exit of a stream ends with an error when its socket ends.
    drop(place.exit.lock().unwrap().take());

    world.assert_interrupted(&idle.id, "host_lost").await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn interrupt_all_interrupts_every_open_session_and_leaves_the_ended_ones(pool: SqlitePool) {
    let world = world(pool).await;
    let open = [
        world.stored(State::Starting).await,
        world.stored(State::Working).await,
        world.stored(State::NeedsDecision).await,
        world.stored(State::Idle).await,
    ];
    let closed = world.stored(State::Closed).await;
    let failed = world.stored(State::Failed).await;
    let interrupted = world.stored(State::Interrupted).await;
    let sessions = world.coding_sessions(
        DuplexPlace::new(Script::default()),
        Arc::new(RefuseDecisions),
    );

    let count = sessions
        .interrupt_all(InterruptReason::DaemonRestart)
        .await
        .unwrap();

    assert_eq!(count, open.len());
    for session in &open {
        world
            .assert_interrupted(&session.id, "daemon_restart")
            .await;
    }
    assert_eq!(world.record(&closed.id).await, closed);
    assert_eq!(world.record(&failed.id).await, failed);
    assert_eq!(world.record(&interrupted.id).await, interrupted);
    assert_eq!(world.interruptions().len(), open.len());
}

/// A live session that a `interrupt_all` interrupts closes its stream.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn interrupt_all_closes_the_stream_of_a_live_session(pool: SqlitePool) {
    let world = world(pool).await;
    let place = DuplexPlace::new(Script::default().turn(Turn::until_cancel(vec![])));
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let working = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();

    let count = sessions
        .interrupt_all(InterruptReason::DaemonRestart)
        .await
        .unwrap();

    assert_eq!(count, 1);
    world
        .assert_interrupted(&working.id, "daemon_restart")
        .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !place.dropped.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < deadline, "the stream stays");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// An idle session of `script` that the departure of its Host
/// interrupted, with its place and its runtime.
async fn interrupted_session(
    world: &World,
    script: Script,
) -> (Arc<DuplexPlace>, CodingSessions, CodingSession) {
    let place = DuplexPlace::new(script);
    let sessions = world.coding_sessions(place.clone(), Arc::new(RefuseDecisions));
    let connection = world.presence.connect(&world.host_id);
    let session = sessions
        .start(world.new_session(&world.run_id))
        .await
        .unwrap();
    world.wait_for_state(&session.id, State::Idle).await;
    drop(connection);
    world.wait_for_state(&session.id, State::Interrupted).await;
    (place, sessions, session)
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_reopens_the_stream_in_the_working_directory_and_the_session_is_idle(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let (place, sessions, session) = interrupted_session(
        &world,
        Script::default()
            .resume()
            .turn(ends_at_once())
            .turn(Turn::new(
                vec![message("Resumed.")],
                acp::StopReason::EndTurn,
            )),
    )
    .await;

    sessions
        .resume(&world.workspace_id, &session.id)
        .await
        .expect("the session resumes");

    let record = world.record(&session.id).await;
    assert_eq!(record.state, State::Idle);
    assert_eq!(record.end_reason, None);
    let requests = place.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "the resume opens a second stream");
    let (_, host_id, request) = &requests[1];
    assert_eq!(*host_id, world.host_id);
    assert_eq!(request.cwd, WORKTREE_DIRECTORY);
    assert_eq!(request.worktree, None, "the worktree exists");
    let (command, args) =
        pagis_core::harness::launch_command(pagis_core::harness::entry("claude").unwrap());
    assert_eq!(request.command, command);
    assert_eq!(request.args, args);
    let harness = place.harness();
    let resumed = harness.params("session/resume");
    assert_eq!(resumed[0]["sessionId"], NEW_SESSION_ID);
    assert_eq!(resumed[0]["cwd"], WORKTREE_DIRECTORY);

    // The task runs again: a prompt starts a turn, and its end makes
    // the session idle.
    let outcome = sessions
        .prompt(&world.workspace_id, &session.id, "Go on.".to_string())
        .await
        .unwrap();
    assert_eq!(outcome, PromptOutcome::Sent);
    world.wait_for_rows(&session.id, Kind::TurnEnd, 2).await;
    world.wait_for_state(&session.id, State::Idle).await;
    assert_eq!(prompts(&harness), ["Go on."]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_load_drops_the_replayed_history(pool: SqlitePool) {
    let world = world(pool).await;
    let (place, sessions, session) = interrupted_session(
        &world,
        Script::default()
            .load()
            .history(vec![message("An old message."), message("Another.")])
            .turn(ends_at_once()),
    )
    .await;
    let before = world.rows(&session.id).await;

    sessions
        .resume(&world.workspace_id, &session.id)
        .await
        .expect("the session resumes");

    assert_eq!(
        place.harness().params("session/load")[0]["sessionId"],
        NEW_SESSION_ID
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(world.rows(&session.id).await, before, "no new row");
    assert_eq!(world.record(&session.id).await.state, State::Idle);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_a_harness_that_cannot_restore_answers_cannot_resume(pool: SqlitePool) {
    let world = world(pool).await;
    let (place, sessions, session) =
        interrupted_session(&world, Script::default().turn(ends_at_once())).await;
    place.dropped.store(false, Ordering::SeqCst);

    let failure = sessions
        .resume(&world.workspace_id, &session.id)
        .await
        .expect_err("the harness cannot resume");

    assert!(
        matches!(&failure, ResumeFailure::CannotResume { harness } if harness == "Claude Code"),
        "{failure:?}"
    );
    assert_eq!(
        failure.to_string(),
        "Claude Code cannot resume a session. Close it and start a new one."
    );
    assert_eq!(world.record(&session.id).await.state, State::Interrupted);
    let deadline = tokio::time::Instant::now() + WAIT;
    while !place.dropped.load(Ordering::SeqCst) {
        assert!(tokio::time::Instant::now() < deadline, "the stream stays");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The session has no task, so its close moves it to `closed` at once.
    sessions
        .close(&world.workspace_id, &session.id, CloseReason::Closed)
        .await
        .unwrap();
    assert_eq!(world.record(&session.id).await.state, State::Closed);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_no_session_socket_answers_host_not_connected(pool: SqlitePool) {
    let world = world(pool).await;
    let session = world.stored(State::Interrupted).await;
    let sessions = world.coding_sessions(Arc::new(HostSessions::new()), Arc::new(RefuseDecisions));

    let failure = sessions
        .resume(&world.workspace_id, &session.id)
        .await
        .expect_err("no Host is present");

    assert!(
        matches!(&failure, ResumeFailure::HostNotConnected { machine } if machine == "Air"),
        "{failure:?}"
    );
    assert_eq!(failure.to_string(), not_connected_message("Air"));
    assert_eq!(world.record(&session.id).await, session);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_of_a_session_that_is_not_interrupted_answers_its_state(pool: SqlitePool) {
    let world = world(pool).await;
    let idle = world.stored(State::Idle).await;
    let sessions = world.coding_sessions(
        DuplexPlace::new(Script::default().resume()),
        Arc::new(RefuseDecisions),
    );

    let failure = sessions
        .resume(&world.workspace_id, &idle.id)
        .await
        .expect_err("an idle session does not resume");

    assert!(
        matches!(failure, ResumeFailure::NotInterrupted(State::Idle)),
        "{failure:?}"
    );
}

// The `agent` mode: the supervising Agent decides a Harness Permission
// or gives it to the Person (ADR-0033).

const NOTE: &str = "The build directory is generated, so a delete is safe.";

/// A Coding Session in the `agent` mode on a host Grant whose widest mode
/// is `agent`, with the tools of its Agent.
struct AgentSession {
    place: Arc<DuplexPlace>,
    tools: CodingToolRuntime,
    session: CodingSession,
    grant: Grant,
}

fn rm_rf_build() -> Ask {
    Ask::permission(
        tool_call(acp::ToolKind::Execute, &[], Some("rm -rf build")),
        every_option(),
    )
}

impl World {
    /// Starts a session in the `agent` mode from `run_id`, whose harness
    /// asks to run `rm -rf build` and ends its turn when it has the
    /// answer, and waits until the permission waits for the Agent.
    async fn agent_session(&self, run_id: &RunId) -> AgentSession {
        let grant = self.host_grant(SessionApprovalMode::Agent, &[]).await;
        let place = DuplexPlace::new(
            Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(rm_rf_build())),
        );
        let sessions = Arc::new(self.coding_sessions(place.clone(), self.policy()));
        let grants = Arc::new(SqliteGrantStore::new(self.pool.clone()));
        let tools = CodingToolRuntime::new(
            Arc::clone(&sessions),
            Arc::new(CodingSessionStarts::new(
                self.sessions.clone(),
                grants.clone(),
            )),
            self.sessions.clone(),
            Arc::new(SqliteHostStore::new(self.pool.clone())),
            grants,
            self.agent_asks.clone(),
        );
        let mut new = self.new_session(run_id);
        new.approval_mode = SessionApprovalMode::Agent;
        let session = sessions.start(new).await.unwrap();
        self.wait_for_state(&session.id, State::NeedsDecision).await;
        let rows = self.wait_for_row(&session.id, Kind::Permission).await;
        assert_eq!(
            rows_of(&rows, Kind::Permission)[0].payload["waits_for"],
            "agent"
        );
        AgentSession {
            place,
            tools,
            session,
            grant,
        }
    }

    /// One call of `tool` by the Agent in `run_id`.
    async fn call_as(
        &self,
        tools: &CodingToolRuntime,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        tool: CoreTool,
        arguments: Value,
    ) -> ToolResult {
        let tool_name = match tool {
            CoreTool::CodingSessionDecide => "coding_session_decide",
            CoreTool::CodingSessionEscalate => "coding_session_escalate",
            _ => "coding_session_read",
        };
        tools
            .execute(AuthorizedCall {
                workspace_id: workspace_id.clone(),
                agent_id: agent_id.clone(),
                run_id: self.run_id.clone(),
                tool_name: tool_name.to_string(),
                source_version: "test".to_string(),
                route: ToolRoute::Core { tool },
                arguments,
                selected_connection: None,
                grant_id: None,
                grant_revision: None,
                call_timeout: None,
                tool_call_id: None,
                host: None,
                approved_by_rule: false,
            })
            .await
    }

    async fn decide(&self, agent: &AgentSession, decision: &str) -> ToolResult {
        self.call_as(
            &agent.tools,
            &self.workspace_id,
            &self.agent_id,
            CoreTool::CodingSessionDecide,
            json!({"session": agent.session.id.as_str(), "decision": decision, "note": NOTE}),
        )
        .await
    }

    async fn escalate(&self, agent: &AgentSession, note: &str) -> ToolResult {
        self.call_as(
            &agent.tools,
            &self.workspace_id,
            &self.agent_id,
            CoreTool::CodingSessionEscalate,
            json!({"session": agent.session.id.as_str(), "note": note}),
        )
        .await
    }

    /// Waits until a Harness Permission waits for the Person, and answers
    /// its Request.
    async fn wait_for_card(&self) -> Request {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let pending = SqliteRequestStore::new(self.pool.clone())
                .list_by_state(
                    &self.workspace_id,
                    RequestState::Pending,
                    Some(Request::HARNESS_PERMISSION_KIND),
                )
                .await
                .unwrap();
            if let Some(request) = pending.into_iter().next() {
                return request;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no Harness Permission waits for the Person"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Ends a Run of the Agent and reports it on the bus, as the agent
    /// loop does.
    async fn end_run(&self, run_id: &RunId) {
        let runs = SqliteRunStore::new(self.pool.clone());
        let mut run = runs
            .get(&self.workspace_id, run_id)
            .await
            .unwrap()
            .expect("the Run");
        run.state = RunState::Completed;
        run.ended_at = Some(now_ms());
        runs.update(&run).await.unwrap();
        self.bus
            .publish(NewEvent {
                workspace_id: self.workspace_id.clone(),
                event_type: "run.state_changed".to_string(),
                agent_id: Some(self.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload: json!({"from": "running", "to": "completed", "reason": "completed"}),
            })
            .await
            .unwrap();
    }
}

fn success(result: &ToolResult) -> &str {
    assert!(!result.is_error, "{result:?}");
    &result.content
}

fn code(result: &ToolResult) -> &str {
    assert!(result.is_error, "{result:?}");
    result.code.as_deref().expect("an error has a code")
}

fn reject_once() -> Value {
    json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
}

/// An allow answers the harness with its `allow_once` option. The audit
/// fact and the decision row name the Agent, its Run and its note, and
/// the host Grant gets no rule.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn decide_allow_answers_allow_once_and_names_the_agent_its_run_and_its_note(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let agent = world.agent_session(&world.run_id).await;

    let decided = world.decide(&agent, "allow").await;

    assert_eq!(
        success(&decided),
        "The harness does this one action once. The next request asks again."
    );
    let rows = world.wait_for_row(&agent.session.id, Kind::TurnEnd).await;
    assert_eq!(
        agent.place.harness().answers()[0].as_ref().unwrap(),
        &json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
    );
    let decision = rows_of(&rows, Kind::Decision)[0];
    assert_eq!(decision.payload["decision"], "allow_once");
    assert_eq!(decision.payload["decider"], "agent");
    assert_eq!(decision.payload["note"], NOTE);
    assert_eq!(decision.payload["run_id"], world.run_id.as_str());
    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "agent");
    assert_eq!(facts[0].payload["outcome"], "allowed");
    assert_eq!(facts[0].payload["option_kind"], "allow_once");
    assert_eq!(facts[0].payload["command"], "rm -rf build");
    assert_eq!(facts[0].payload["note"], NOTE);
    assert_eq!(facts[0].payload["run_id"], world.run_id.as_str());
    let grant = SqliteGrantStore::new(world.pool.clone())
        .live_for_resource(
            &world.workspace_id,
            &world.agent_id,
            Grant::HOST_KIND,
            world.host_id.as_str(),
        )
        .await
        .unwrap()
        .expect("the host Grant");
    assert_eq!(grant.revision, agent.grant.revision);
    assert_eq!(grant.scope, agent.grant.scope, "a decision writes no rule");
    world.wait_for_state(&agent.session.id, State::Idle).await;
    assert_eq!(
        code(&world.decide(&agent, "allow").await),
        "no_pending_decision"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn decide_deny_answers_reject_once(pool: SqlitePool) {
    let world = world(pool).await;
    let agent = world.agent_session(&world.run_id).await;

    let decided = world.decide(&agent, "deny").await;

    assert_eq!(success(&decided), "The harness does not do it.");
    world.wait_for_row(&agent.session.id, Kind::TurnEnd).await;
    assert_eq!(
        agent.place.harness().answers()[0].as_ref().unwrap(),
        &reject_once()
    );
    let facts = world.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "agent");
    assert_eq!(facts[0].payload["outcome"], "rejected");
}

/// The checks of a decision, in their order: the session is the Agent's
/// own, in its Workspace; a permission waits for the Agent.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn decide_reads_another_agents_session_as_absent_and_answers_no_pending_decision_when_nothing_waits(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let agent = world.agent_session(&world.run_id).await;
    let arguments =
        json!({"session": agent.session.id.as_str(), "decision": "allow", "note": NOTE});

    let another_agent = world
        .call_as(
            &agent.tools,
            &world.workspace_id,
            &AgentId::generate(),
            CoreTool::CodingSessionDecide,
            arguments.clone(),
        )
        .await;
    let another_workspace = world
        .call_as(
            &agent.tools,
            &WorkspaceId::generate(),
            &world.agent_id,
            CoreTool::CodingSessionDecide,
            arguments,
        )
        .await;

    assert_eq!(code(&another_agent), "session_not_found");
    assert_eq!(code(&another_workspace), "session_not_found");
    assert!(
        agent.place.harness().answers().is_empty(),
        "the permission waits"
    );
    let idle = world.stored(State::Idle).await;
    let nothing_waits = world
        .call_as(
            &agent.tools,
            &world.workspace_id,
            &world.agent_id,
            CoreTool::CodingSessionDecide,
            json!({"session": idle.id.as_str(), "decision": "allow", "note": NOTE}),
        )
        .await;
    assert_eq!(code(&nothing_waits), "no_pending_decision");
}

/// A Grant revision that narrows the widest mode to `person` during the
/// wait gives the decision to the Person: the decide answers
/// `escalated`, the card is posted, and the harness has no answer.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_narrower_grant_during_the_wait_makes_decide_answer_escalated_and_asks_the_person(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let agent = world.agent_session(&world.run_id).await;
    let scope = agent
        .grant
        .with_session_approval_mode(SessionApprovalMode::Person);
    SqliteGrantStore::new(world.pool.clone())
        .set_scope(&world.workspace_id, &agent.grant.id, &scope)
        .await
        .unwrap();

    let decided = world.decide(&agent, "allow").await;

    assert_eq!(code(&decided), "escalated");
    assert!(
        decided
            .content
            .contains("The widest mode on this machine changed. The person decides on the card."),
        "{decided:?}"
    );
    let request = world.wait_for_card().await;
    assert_eq!(request.payload["command"], "rm -rf build");
    assert_eq!(request.payload["note"], Value::Null);
    let rows = world
        .wait_for_rows(&agent.session.id, Kind::Permission, 2)
        .await;
    let decision = rows_of(&rows, Kind::Decision)[0];
    assert_eq!(decision.payload["decision"], "escalated");
    assert_eq!(decision.payload["decider"], Value::Null);
    assert_eq!(
        rows_of(&rows, Kind::Permission)[1].payload["waits_for"],
        "person"
    );
    assert!(agent.place.harness().answers().is_empty());
    assert_eq!(
        world.record(&agent.session.id).await.state,
        State::NeedsDecision
    );
}

/// An escalation returns at once. The daemon posts one approval card that
/// holds the Agent's note, and the session then waits for the Person.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn escalate_posts_one_card_with_the_note_and_returns_at_once(pool: SqlitePool) {
    let world = world(pool).await;
    let agent = world.agent_session(&world.run_id).await;
    let question = "Delete the build directory? I did not make it.";

    let escalated = world.escalate(&agent, question).await;

    assert_eq!(
        success(&escalated),
        "The person decides on the card in the session's Thread."
    );
    let request = world.wait_for_card().await;
    assert_eq!(request.payload["note"], question);
    let rows = world
        .wait_for_rows(&agent.session.id, Kind::Permission, 2)
        .await;
    let decision = rows_of(&rows, Kind::Decision)[0];
    assert_eq!(decision.payload["decision"], "escalated");
    assert_eq!(decision.payload["decider"], "agent");
    assert_eq!(decision.payload["note"], question);
    assert_eq!(decision.payload["run_id"], world.run_id.as_str());
    assert_eq!(
        rows_of(&rows, Kind::Permission)[1].payload["waits_for"],
        "person"
    );
    let cards: Vec<_> = SqliteMessageStore::new(world.pool.clone())
        .list_thread(&world.workspace_id, &agent.session.root_message_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.author_kind == AuthorKind::System && message.run_id.is_none())
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        code(&world.decide(&agent, "allow").await),
        "no_pending_decision"
    );
    assert!(agent.place.harness().answers().is_empty());
}

/// The decision does not wait forever on the Agent. A Run of the Agent in
/// the session's Thread that was active when the permission came does
/// not count; a Run that started after it and ends with no decision
/// gives the permission to the Person with the daemon's note.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_run_in_the_thread_that_ends_without_a_decision_escalates_the_permission(
    pool: SqlitePool,
) {
    let world = world(pool).await;
    let (starting_run, root) = world.run_in_thread().await;
    let agent = world.agent_session(&starting_run).await;
    assert_eq!(agent.session.root_message_id, root);

    world.end_run(&starting_run).await;
    let elsewhere = world.run(Some(world.channel_id.clone()), None).await;
    world.end_run(&elsewhere).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        SqliteRequestStore::new(world.pool.clone())
            .list_by_state(&world.workspace_id, RequestState::Pending, None)
            .await
            .unwrap()
            .is_empty(),
        "a Run that was active, or a Run outside the Thread, does not escalate"
    );

    let woken = world.run(Some(world.channel_id.clone()), Some(root)).await;
    world.end_run(&woken).await;

    let request = world.wait_for_card().await;
    assert_eq!(
        request.payload["note"],
        "The sprite ended its turn without a decision."
    );
    let rows = world
        .wait_for_rows(&agent.session.id, Kind::Permission, 2)
        .await;
    let decision = rows_of(&rows, Kind::Decision)[0];
    assert_eq!(decision.payload["decision"], "escalated");
    assert_eq!(
        decision.payload["note"],
        "The sprite ended its turn without a decision."
    );
    assert_eq!(
        code(&world.decide(&agent, "allow").await),
        "no_pending_decision"
    );
}
