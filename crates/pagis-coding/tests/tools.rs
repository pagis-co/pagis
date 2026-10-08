//! The tools that an Agent drives its own Coding Sessions with (ADR-0033):
//! `coding_session_send`, `coding_session_read`, `coding_session_cancel`,
//! `coding_session_close`, `coding_session_list` and
//! `coding_session_resume`, against the SQLite
//! stores. A live session runs the fake harness behind the session socket
//! registry; a test that reads a transcript writes its rows itself.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CoreTool, HostSessions, ToolExecutor, ToolResult, ToolRoute};
use pagis_coding::fake::{Script, Turn, acp, serve_client_app};
use pagis_coding::{
    CodingSessionStarts, CodingSessions, CodingSessionsDeps, CodingToolRuntime, NewCodingSession,
    RefuseDecisions, SessionEvents, SessionRuleError, SessionRules,
};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, Channel, ChannelId, ChannelKind, ChannelStore,
    CodingSession, CodingSessionEventKind as Kind, CodingSessionId, CodingSessionPlace,
    CodingSessionState as State, CodingSessionStore, CodingSessionUsage, Event, EventBus, EventId,
    EventScope, EventStream, Grant, GrantId, GrantStore, HostId, HostStore, IngestBatch, MessageId,
    NewCodingSessionEvent, NewEvent, Run, RunId, RunState, RunStore, SessionApprovalMode,
    StoreError, SystemClock, TriggerKind, Workspace, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteChannelStore, SqliteCodingSessionStore, SqliteGrantStore,
    SqliteHostStore, SqliteMessageStore, SqliteRunStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(10);
const DIRECTORY: &str = "/Users/bo/code/app";

/// The rows that a Coding Session points at, the runtime of the
/// sessions, and the tools.
struct World {
    store: Arc<SqliteCodingSessionStore>,
    workspace_id: WorkspaceId,
    /// The Agent that calls the tools.
    agent_id: AgentId,
    /// Another Agent of the same Workspace.
    other_agent_id: AgentId,
    channel_id: ChannelId,
    host_id: HostId,
    run_id: RunId,
    sessions: Arc<CodingSessions>,
    tools: CodingToolRuntime,
    grants: Arc<SqliteGrantStore>,
}

/// A world whose machine runs the fake harness of `script` on each
/// session stream.
async fn world(pool: SqlitePool, script: Script) -> World {
    let person = pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", now_ms())
        .await
        .unwrap();
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: person.id,
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
    let agent_id = agent(&pool, &workspace.id, "Robin").await;
    let other_agent_id = agent(&pool, &workspace.id, "Sam").await;
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: workspace.id.clone(),
        kind: ChannelKind::Dm,
        title: Some("general".to_string()),
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteChannelStore::new(pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: agent_id.clone(),
        channel_id: Some(channel.id.clone()),
        root_message_id: None,
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
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .unwrap();
    let hosts_store = Arc::new(SqliteHostStore::new(pool.clone()));
    let host = hosts_store
        .register(
            &workspace.id,
            "Air",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .unwrap();
    let place = session_socket(&workspace.id, &host.id, script).await;
    let store = Arc::new(SqliteCodingSessionStore::new(pool.clone()));
    let sessions = Arc::new(CodingSessions::new(CodingSessionsDeps {
        sessions: store.clone(),
        runs: Arc::new(SqliteRunStore::new(pool.clone())),
        hosts: hosts_store.clone(),
        messages: Arc::new(SqliteMessageStore::new(pool.clone())),
        bus: Arc::new(SilentBus),
        place,
        decisions: Arc::new(RefuseDecisions),
        rules: Arc::new(SilentRules),
        events: Arc::new(SilentRules),
        clock: Arc::new(SystemClock),
        departures: pagis_broker::HostPresence::new().departures(),
        cancel: CancellationToken::new(),
    }));
    let tools = CodingToolRuntime::new(
        Arc::clone(&sessions),
        Arc::new(CodingSessionStarts::new(
            store.clone(),
            Arc::new(SqliteGrantStore::new(pool.clone())),
        )),
        store.clone(),
        hosts_store,
        Arc::new(SqliteGrantStore::new(pool.clone())),
    );
    let grants = Arc::new(SqliteGrantStore::new(pool.clone()));
    World {
        store,
        workspace_id: workspace.id,
        agent_id,
        other_agent_id,
        channel_id: channel.id,
        host_id: host.id,
        run_id: run.id,
        sessions,
        tools,
        grants,
    }
}

async fn agent(pool: &SqlitePool, workspace_id: &WorkspaceId, name: &str) -> AgentId {
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
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
    agent.id
}

/// The session socket of the machine, whose Client App end runs the
/// fake harness of `script` on each stream.
async fn session_socket(
    workspace_id: &WorkspaceId,
    host_id: &HostId,
    script: Script,
) -> Arc<HostSessions> {
    let hosts = Arc::new(HostSessions::new());
    let (daemon_end, client_app_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn({
        let hosts = hosts.clone();
        let (workspace_id, host_id) = (workspace_id.clone(), host_id.clone());
        async move {
            hosts
                .serve(workspace_id, host_id, daemon_end.compat())
                .await;
        }
    });
    tokio::spawn(serve_client_app(
        client_app_end.compat(),
        script,
        DIRECTORY.to_string(),
    ));
    let deadline = tokio::time::Instant::now() + WAIT;
    while !hosts.is_open(host_id) {
        assert!(tokio::time::Instant::now() < deadline, "no session socket");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    hosts
}

impl World {
    /// A session of the calling Agent that the runtime starts.
    async fn start(&self) -> CodingSession {
        self.sessions
            .start(NewCodingSession {
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                run_id: self.run_id.clone(),
                host_id: self.host_id.clone(),
                harness_id: "claude".to_string(),
                directory: DIRECTORY.to_string(),
                worktree: None,
                approval_mode: SessionApprovalMode::Person,
                title: "Fix the login".to_string(),
                prompt: "Fix the login bug.".to_string(),
            })
            .await
            .expect("the session starts")
    }

    /// A session record of `agent_id` in `state`, with no task, written
    /// through the store.
    async fn record(&self, agent_id: &AgentId, state: State, title: &str) -> CodingSession {
        let at = now_ms();
        let terminal = state.is_terminal();
        let session = CodingSession {
            id: CodingSessionId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: agent_id.clone(),
            harness_id: "claude".to_string(),
            harness_version: "0.87.0".to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(self.host_id.clone()),
            directory: DIRECTORY.to_string(),
            working_directory: Some(DIRECTORY.to_string()),
            worktree_branch: None,
            approval_mode: SessionApprovalMode::Person,
            title: title.to_string(),
            state,
            end_reason: terminal.then(|| "closed".to_string()),
            end_detail: None,
            acp_session_id: Some("acp-1".to_string()),
            channel_id: self.channel_id.clone(),
            root_message_id: MessageId::generate(),
            message_id: MessageId::generate(),
            run_id: self.run_id.clone(),
            usage: CodingSessionUsage::default(),
            created_at: at,
            updated_at: at,
            ended_at: terminal.then_some(at),
        };
        self.store.insert(&session).await.unwrap();
        session
    }

    async fn append(&self, session: &CodingSession, kind: Kind, payload: Value) {
        self.store
            .append_event(
                &self.workspace_id,
                &session.id,
                NewCodingSessionEvent {
                    at: now_ms(),
                    kind,
                    payload,
                },
            )
            .await
            .unwrap();
    }

    /// One call of the calling Agent.
    async fn call(&self, tool: CoreTool, name: &str, arguments: Value) -> ToolResult {
        self.tools
            .execute(AuthorizedCall {
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                run_id: self.run_id.clone(),
                tool_name: name.to_string(),
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

    async fn send(&self, session: &CodingSession, prompt: &str) -> ToolResult {
        self.call(
            CoreTool::CodingSessionSend,
            "coding_session_send",
            json!({"session": session.id.as_str(), "prompt": prompt}),
        )
        .await
    }

    async fn read(&self, session: &CodingSession) -> ToolResult {
        self.call(
            CoreTool::CodingSessionRead,
            "coding_session_read",
            json!({"session": session.id.as_str()}),
        )
        .await
    }

    async fn cancel(&self, session: &CodingSession) -> ToolResult {
        self.call(
            CoreTool::CodingSessionCancel,
            "coding_session_cancel",
            json!({"session": session.id.as_str()}),
        )
        .await
    }

    async fn close(&self, session: &CodingSession) -> ToolResult {
        self.call(
            CoreTool::CodingSessionClose,
            "coding_session_close",
            json!({"session": session.id.as_str()}),
        )
        .await
    }

    async fn resume(&self, session: &CodingSession) -> ToolResult {
        self.call(
            CoreTool::CodingSessionResume,
            "coding_session_resume",
            json!({"session": session.id.as_str()}),
        )
        .await
    }

    /// A live host Grant of the calling Agent on the machine.
    async fn host_grant(&self) {
        self.grants
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                resource_kind: Grant::HOST_KIND.to_string(),
                resource_id: Some(self.host_id.to_string()),
                scope: json!({"allow": []}),
                revision: 1,
                created_at: now_ms(),
                revoked_at: None,
            })
            .await
            .unwrap();
    }

    async fn list(&self) -> ToolResult {
        self.call(
            CoreTool::CodingSessionList,
            "coding_session_list",
            json!({}),
        )
        .await
    }

    async fn state(&self, session: &CodingSession) -> State {
        self.store
            .get(&self.workspace_id, &session.id)
            .await
            .unwrap()
            .expect("the session record")
            .state
    }

    async fn wait_for_state(&self, session: &CodingSession, state: State) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let now = self.state(session).await;
            if now == state {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the session is {now:?}, not {state:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn kinds(&self, session: &CodingSession) -> Vec<Kind> {
        self.store
            .list_events(&self.workspace_id, &session.id, None, 1_000)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.kind)
            .collect()
    }
}

/// A bus that drops each event.
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

/// The Session Rules and the `ingest` of the Trigger module, which these
/// tests do not read.
struct SilentRules;

#[async_trait]
impl SessionRules for SilentRules {
    async fn create(&self, _: &CodingSession) -> Result<(), SessionRuleError> {
        Ok(())
    }

    async fn end(&self, _: &CodingSession) -> Result<(), SessionRuleError> {
        Ok(())
    }
}

#[async_trait]
impl SessionEvents for SilentRules {
    async fn ingest(&self, _: IngestBatch) -> Result<(), SessionRuleError> {
        Ok(())
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

fn json_of(result: &ToolResult) -> Value {
    serde_json::from_str(success(result)).unwrap_or_else(|error| panic!("{error}: {result:?}"))
}

/// The text between the two markers of the one envelope of `text`, after
/// a check that the envelope names `source`.
fn enveloped<'a>(text: &'a str, source: &str) -> &'a str {
    let begin = format!("[BEGIN UNTRUSTED source={source}]\n");
    let end = format!("\n[END UNTRUSTED source={source}]");
    assert_eq!(text.matches("[BEGIN UNTRUSTED").count(), 1, "{text}");
    assert!(text.starts_with(&begin), "{text}");
    assert!(text.ends_with(&end), "{text}");
    &text[begin.len()..text.len() - end.len()]
}

fn message(id: &str, text: &str) -> Value {
    json!({"text": text, "message_id": id})
}

fn tool_call(id: &str, kind: &str, paths: &[&str]) -> Value {
    let locations: Vec<Value> = paths.iter().map(|path| json!({"path": path})).collect();
    json!({
        "toolCallId": id,
        "title": format!("A {kind}"),
        "kind": kind,
        "status": "pending",
        "locations": locations,
    })
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn send_in_idle_starts_a_turn_and_send_in_working_answers_that_the_prompt_waits(
    pool: SqlitePool,
) {
    let world = world(
        pool,
        Script::default()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn))
            .turn(Turn::until_cancel(vec![])),
    )
    .await;
    let session = world.start().await;
    world.wait_for_state(&session, State::Idle).await;

    let sent = world.send(&session, "Add a test.").await;

    assert_eq!(success(&sent), "The turn started.");
    world.wait_for_state(&session, State::Working).await;
    let queued = world.send(&session, "And update the docs.").await;
    assert_eq!(
        success(&queued),
        "A turn runs. Your prompt goes when it ends."
    );
    assert_eq!(
        world.kinds(&session).await,
        [Kind::Prompt, Kind::TurnEnd, Kind::Prompt],
        "the queued prompt waits for the end of the turn"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn send_to_a_session_that_is_not_open_answers_its_state(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    let closed = world.record(&world.agent_id, State::Closed, "Done").await;

    let refused = world.send(&closed, "More.").await;

    assert_eq!(code(&refused), "session_not_open");
    assert!(refused.content.contains("closed"), "{refused:?}");
}

/// The daemon's own fields are outside the envelope. The harness text is
/// inside one envelope: the pending decision, the last agent message,
/// the plan and the changed files, newest first. A file that two tool
/// calls touch appears once, and a read is no change.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn read_puts_the_harness_text_inside_one_envelope_and_the_state_and_usage_outside(
    pool: SqlitePool,
) {
    let world = world(pool, Script::default()).await;
    let mut session = world
        .record(&world.agent_id, State::NeedsDecision, "Fix the login")
        .await;
    session.usage = CodingSessionUsage {
        context_used: Some(1200),
        context_size: Some(200_000),
        cost_amount: Some(0.25),
        cost_currency: Some("USD".to_string()),
    };
    world.store.update(&session).await.unwrap();
    let login = format!("{DIRECTORY}/src/login.rs");
    let session_rs = format!("{DIRECTORY}/src/session.rs");
    let store_rs = format!("{DIRECTORY}/src/store.rs");
    for (kind, payload) in [
        (Kind::AgentMessage, message("m1", "Looking at the login.")),
        (Kind::ToolCall, tool_call("call-1", "edit", &[&login])),
        (
            Kind::ToolCall,
            tool_call("call-2", "read", &[&format!("{DIRECTORY}/README.md")]),
        ),
        (
            Kind::ToolCallUpdate,
            json!({"toolCallId": "call-1", "status": "completed"}),
        ),
        (Kind::ToolCall, tool_call("call-3", "edit", &[&session_rs])),
        // An update names no kind: the kind is the kind of its call.
        (
            Kind::ToolCallUpdate,
            json!({"toolCallId": "call-3", "locations": [{"path": store_rs}]}),
        ),
        (Kind::ToolCall, tool_call("call-4", "move", &[&login])),
        (
            Kind::Plan,
            json!({"entries": [{"content": "Run the tests", "status": "in_progress"}]}),
        ),
        (Kind::AgentMessage, message("m2", "The login works now.")),
        (
            Kind::Permission,
            json!({
                "ask_id": "7",
                "tool_call_id": "call-5",
                "title": "Run cargo test",
                "kind": "execute",
                "locations": [],
                "raw_input": {"command": "cargo test"},
                "options": ["allow_once", "reject_once"],
                "waits_for": "person",
            }),
        ),
    ] {
        world.append(&session, kind, payload).await;
    }

    let read = json_of(&world.read(&session).await);

    assert_eq!(read["session_id"], session.id.as_str());
    assert_eq!(read["harness"], "Claude Code");
    assert_eq!(read["machine"], "Air");
    assert_eq!(read["title"], "Fix the login");
    assert_eq!(read["state"], "needs_decision");
    assert_eq!(read["end_reason"], Value::Null);
    assert_eq!(read["usage"]["context_used"], 1200);
    assert_eq!(read["usage"]["context_size"], 200_000);
    assert_eq!(read["usage"]["cost_amount"], 0.25);
    assert_eq!(read["usage"]["cost_currency"], "USD");
    assert!(read["updated_at"].is_i64(), "{read:#}");
    let mut outside = read.clone();
    let output = outside["harness_output"].take();
    assert!(
        !outside.to_string().contains("The login works now."),
        "{outside:#}"
    );
    let source = format!("coding_session:{}", session.id);
    let inside: Value = serde_json::from_str(enveloped(output.as_str().unwrap(), &source)).unwrap();
    assert_eq!(inside["last_message"], "The login works now.");
    assert_eq!(
        inside["plan"],
        json!([{"content": "Run the tests", "status": "in_progress"}])
    );
    assert_eq!(inside["pending_decision"]["kind"], "permission");
    assert_eq!(inside["pending_decision"]["title"], "Run cargo test");
    assert_eq!(
        inside["pending_decision"]["options"],
        json!(["allow_once", "reject_once"])
    );
    assert_eq!(
        inside["changed_files"],
        json!([login, store_rs, session_rs])
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn read_cuts_a_long_agent_message_and_marks_the_cut(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    let session = world.record(&world.agent_id, State::Idle, "Long").await;
    world
        .append(
            &session,
            Kind::AgentMessage,
            message("m1", &"a".repeat(5_000)),
        )
        .await;

    let read = json_of(&world.read(&session).await);

    let source = format!("coding_session:{}", session.id);
    let inside: Value =
        serde_json::from_str(enveloped(read["harness_output"].as_str().unwrap(), &source)).unwrap();
    let last = inside["last_message"].as_str().unwrap();
    assert!(last.starts_with(&"a".repeat(4_000)), "{last}");
    assert!(!last.starts_with(&"a".repeat(4_001)), "{last}");
    assert!(last.ends_with("[1000 more characters]"), "{last}");
    assert_eq!(
        inside["pending_decision"],
        Value::Null,
        "an idle session waits for nothing"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn cancel_in_idle_changes_nothing(pool: SqlitePool) {
    let world = world(
        pool,
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn)),
    )
    .await;
    let session = world.start().await;
    world.wait_for_state(&session, State::Idle).await;
    let before = world.kinds(&session).await;

    let cancelled = world.cancel(&session).await;

    assert_eq!(success(&cancelled), "No turn runs.");
    assert_eq!(world.state(&session).await, State::Idle);
    assert_eq!(world.kinds(&session).await, before);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn cancel_in_working_ends_the_turn(pool: SqlitePool) {
    let world = world(pool, Script::default().turn(Turn::until_cancel(vec![]))).await;
    let session = world.start().await;

    let cancelled = world.cancel(&session).await;

    success(&cancelled);
    world.wait_for_state(&session, State::Idle).await;
    assert_eq!(world.kinds(&session).await, [Kind::Prompt, Kind::TurnEnd]);
}

/// An `interrupted` session has no stream, so its close moves it to
/// `closed` at once.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn close_of_an_interrupted_session_moves_it_to_closed(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    let session = world
        .record(&world.agent_id, State::Interrupted, "Lost")
        .await;

    let closed = world.close(&session).await;

    success(&closed);
    let record = world
        .store
        .get(&world.workspace_id, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.state, State::Closed);
    assert_eq!(record.end_reason.as_deref(), Some("closed"));
    assert!(record.ended_at.is_some());
    assert_eq!(code(&world.close(&session).await), "session_not_open");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn close_of_a_working_session_closes_it(pool: SqlitePool) {
    let world = world(pool, Script::default().turn(Turn::until_cancel(vec![]))).await;
    let session = world.start().await;

    success(&world.close(&session).await);

    assert_eq!(world.state(&session).await, State::Closed);
}

/// The open sessions come first, then the 20 newest that ended. A
/// session of another Agent is not in the list.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn list_holds_the_open_sessions_first_and_leaves_out_another_agents(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    world
        .record(&world.agent_id, State::Closed, "Oldest closed")
        .await;
    for index in 0..20 {
        world
            .record(&world.agent_id, State::Closed, &format!("Closed {index}"))
            .await;
    }
    let idle = world.record(&world.agent_id, State::Idle, "Idle").await;
    world.record(&world.agent_id, State::Failed, "Failed").await;
    let interrupted = world
        .record(&world.agent_id, State::Interrupted, "Interrupted")
        .await;
    world
        .record(&world.other_agent_id, State::Working, "Not yours")
        .await;

    let list = json_of(&world.list().await);

    let items = list["items"].as_array().unwrap();
    let titles: Vec<&str> = items
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles.len(), 22, "{titles:?}");
    assert_eq!(titles[..2], ["Interrupted", "Idle"]);
    assert_eq!(titles[2], "Failed");
    assert!(!titles.contains(&"Not yours"), "{titles:?}");
    assert!(!titles.contains(&"Oldest closed"), "{titles:?}");
    assert_eq!(items[0]["session_id"], interrupted.id.as_str());
    assert_eq!(items[1]["session_id"], idle.id.as_str());
    assert_eq!(items[1]["harness"], "Claude Code");
    assert_eq!(items[1]["machine"], "Air");
    assert_eq!(items[1]["directory"], DIRECTORY);
    assert_eq!(items[1]["state"], "idle");
    assert!(items[1]["updated_at"].is_i64(), "{list:#}");
}

/// A session of another Agent reads as absent, as an id that names no
/// session does.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn each_tool_answers_session_not_found_for_another_agents_session(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    let theirs = world
        .record(&world.other_agent_id, State::Idle, "Not yours")
        .await;
    let mut absent = theirs.clone();
    absent.id = CodingSessionId::generate();

    for session in [&theirs, &absent] {
        for result in [
            world.send(session, "Do it.").await,
            world.read(session).await,
            world.cancel(session).await,
            world.close(session).await,
        ] {
            assert_eq!(code(&result), "session_not_found", "{result:?}");
            assert!(!result.content.contains("Not yours"), "{result:?}");
        }
    }
    assert_eq!(world.state(&theirs).await, State::Idle);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn send_to_an_interrupted_session_names_the_resume(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    let lost = world
        .record(&world.agent_id, State::Interrupted, "Lost")
        .await;

    let refused = world.send(&lost, "More.").await;

    assert_eq!(code(&refused), "session_not_open");
    assert!(
        refused.content.contains("coding_session_resume"),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_a_live_host_grant_makes_the_session_idle(pool: SqlitePool) {
    let world = world(
        pool,
        Script::default()
            .resume()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn)),
    )
    .await;
    world.host_grant().await;
    let lost = world
        .record(&world.agent_id, State::Interrupted, "Lost")
        .await;

    let resumed = world.resume(&lost).await;

    assert_eq!(
        success(&resumed),
        "The session resumed. It is idle and takes a new prompt."
    );
    assert_eq!(world.state(&lost).await, State::Idle);
    assert_eq!(
        success(&world.send(&lost, "Go on.").await),
        "The turn started."
    );
    assert_eq!(
        code(&world.resume(&lost).await),
        "session_not_interrupted",
        "a session that runs does not resume"
    );
}

/// The Person revoked the host Grant, so the Agent no longer runs the
/// harness on that machine.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_no_live_host_grant_answers_permission_revoked(pool: SqlitePool) {
    let world = world(pool, Script::default().resume()).await;
    let lost = world
        .record(&world.agent_id, State::Interrupted, "Lost")
        .await;

    let refused = world.resume(&lost).await;

    assert_eq!(code(&refused), "permission_revoked");
    assert_eq!(world.state(&lost).await, State::Interrupted);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_of_a_session_of_another_agent_answers_session_not_found(pool: SqlitePool) {
    let world = world(pool, Script::default().resume()).await;
    world.host_grant().await;
    let theirs = world
        .record(&world.other_agent_id, State::Interrupted, "Theirs")
        .await;

    let refused = world.resume(&theirs).await;

    assert_eq!(code(&refused), "session_not_found");
    assert_eq!(world.state(&theirs).await, State::Interrupted);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn resume_with_a_harness_that_cannot_restore_answers_cannot_resume(pool: SqlitePool) {
    let world = world(pool, Script::default()).await;
    world.host_grant().await;
    let lost = world
        .record(&world.agent_id, State::Interrupted, "Lost")
        .await;

    let refused = world.resume(&lost).await;

    assert_eq!(code(&refused), "cannot_resume");
    assert!(
        refused
            .content
            .contains("Claude Code cannot resume a session. Close it and start a new one."),
        "{refused:?}"
    );
    assert_eq!(world.state(&lost).await, State::Interrupted);
}
