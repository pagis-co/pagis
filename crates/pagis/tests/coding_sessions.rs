//! Full-daemon tests of Coding Sessions (ADR-0033).
//!
//! An Agent calls `coding_session_start`, the Person approves the card,
//! and the daemon starts the harness on the Person's machine over its
//! session socket. Pagis policy then answers each Harness Permission. The
//! REST routes read the records and the transcripts that the stores hold.
//! The Client App's end of the socket runs the fake Coding Harness of
//! `pagis_coding` on each stream in place of the process: through
//! `serve_client_app`, or through the fake Client App of `pagis_broker`
//! when a test reads the open requests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use pagis_agent::{Brain, BrainError, TurnRequest, TurnRole, TurnStream};
use pagis_broker::fake::FakeClientApp;
use pagis_coding::fake::{Ask, FakeHarness, NEW_SESSION_ID, Script, Turn, acp, serve_client_app};
use pagis_coding::{CloseReason, NewCodingSession, PERMISSION_DECIDED_EVENT, Place, PromptOutcome};
use pagis_core::{
    AgentId, AuthorKind, Block, ChannelId, CodingSession, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, Event, EventSource, HostId, KnownBlock, Message, MessageId,
    NewCodingSessionEvent, Request, RequestId, RequestState, Run, RunId, RunOrigin, RunState,
    SessionApprovalMode, TriggerKind, WakeupState, harness, now_ms,
};
use pagis_testkit::{
    HostAnswer, HostClient, ScriptedBrain, SessionClient, Socket, TestDaemon, TestDaemonOptions,
    TwoTenants, fixture,
};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const WAIT: Duration = Duration::from_secs(10);
const DIRECTORY: &str = "/Users/bo/code/app";
const WORKTREE: &str = "/Users/bo/.pagis-worktrees/app/pagis-fix-the-login";
const PROMPT: &str = "Fix the login bug.";

/// A Host of the daemon's own person that can start Claude Code.
async fn host(daemon: &TestDaemon) -> HostId {
    daemon
        .stores()
        .hosts
        .register(
            &daemon.workspace_id,
            "Air",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .expect("write the Host")
        .id
}

/// A queued Run in the seeded DM channel, which a Coding Session names.
async fn run(daemon: &TestDaemon) -> RunId {
    run_in(daemon, None).await
}

/// A queued Run in the seeded DM channel, in the Thread of `root` when
/// it names one.
async fn run_in(daemon: &TestDaemon, root: Option<MessageId>) -> RunId {
    let mut run = fixture::queued_run(
        &daemon.workspace_id,
        &AgentId::from(daemon.agent_id.clone()),
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    run.root_message_id = root;
    daemon
        .stores()
        .runs
        .create(&run)
        .await
        .expect("write the Run");
    run.id
}

/// A Coding Session record of Claude Code on `host_id`, written
/// through the stores.
async fn record(daemon: &TestDaemon, host_id: &HostId) -> CodingSession {
    let session = fixture::coding_session(
        &daemon.workspace_id,
        &AgentId::from(daemon.agent_id.clone()),
        &run(daemon).await,
        &ChannelId::from(daemon.dm_channel_id.clone()),
        host_id,
    );
    daemon
        .stores()
        .coding_sessions
        .insert(&session)
        .await
        .expect("write the Coding Session");
    session
}

async fn get(daemon: &TestDaemon, path: &str) -> Value {
    let response = reqwest::Client::new()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the daemon answers");
    assert_eq!(response.status(), 200, "GET {path}");
    response.json().await.expect("a JSON answer")
}

async fn stop(daemon: &TestDaemon, session: &CodingSession) -> u16 {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/coding-sessions/{}/stop",
            daemon.base_url, session.id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the daemon answers")
        .status()
        .as_u16()
}

/// The session socket of `host_id`, whose Client App end runs the fake
/// harness of `script` on each stream.
async fn open_session_socket(daemon: &TestDaemon, host_id: &HostId, script: Script) {
    let (daemon_end, client_app_end) = tokio::io::duplex(64 * 1024);
    let host_sessions = daemon.host_sessions.clone();
    let (workspace_id, served_host) = (daemon.workspace_id.clone(), host_id.clone());
    tokio::spawn(async move {
        host_sessions
            .serve(workspace_id, served_host, daemon_end.compat())
            .await;
    });
    tokio::spawn(serve_client_app(
        client_app_end.compat(),
        script,
        DIRECTORY.to_string(),
    ));
    let deadline = tokio::time::Instant::now() + WAIT;
    while !daemon.host_sessions.is_open(host_id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session socket is not open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A Coding Session that the daemon's runtime starts, with a turn that
/// runs until a cancel.
async fn start_session(daemon: &TestDaemon) -> CodingSession {
    let run_id = run(daemon).await;
    start_session_from(daemon, run_id).await
}

/// A Coding Session that `run_id` starts, with a turn that runs until a
/// cancel.
async fn start_session_from(daemon: &TestDaemon, run_id: RunId) -> CodingSession {
    start_session_with(
        daemon,
        run_id,
        Script::default().turn(Turn::until_cancel(vec![])),
    )
    .await
}

/// A Coding Session that `run_id` starts, whose harness plays `script`.
async fn start_session_with(daemon: &TestDaemon, run_id: RunId, script: Script) -> CodingSession {
    let host_id = host(daemon).await;
    open_session_socket(daemon, &host_id, script).await;
    daemon
        .coding_sessions
        .start(NewCodingSession {
            workspace_id: daemon.workspace_id.clone(),
            agent_id: AgentId::from(daemon.agent_id.clone()),
            run_id,
            place: Place::Host(host_id),
            harness_id: "claude".to_string(),
            directory: DIRECTORY.to_string(),
            worktree: None,
            approval_mode: SessionApprovalMode::Person,
            harness_mode: None,
            title: "Fix the login bug".to_string(),
            prompt: "Fix the login bug.".to_string(),
        })
        .await
        .expect("the session starts")
}

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> Value {
    loop {
        let frame = tokio::time::timeout(WAIT, socket.next())
            .await
            .unwrap_or_else(|_| panic!("no {frame_type} frame in time"))
            .expect("the socket is open")
            .expect("a frame");
        let Frame::Text(text) = frame else { continue };
        let value: Value = serde_json::from_str(&text).expect("a JSON frame");
        if value["type"] == frame_type {
            return value;
        }
    }
}

#[tokio::test]
async fn the_list_names_the_harness_and_the_machine_and_the_transcript_comes_in_pages() {
    let daemon = TestDaemon::start().await;
    let host_id = host(&daemon).await;
    let session = record(&daemon, &host_id).await;
    for step in 1..=250 {
        daemon
            .stores()
            .coding_sessions
            .append_event(
                &daemon.workspace_id,
                &session.id,
                NewCodingSessionEvent {
                    at: now_ms(),
                    kind: CodingSessionEventKind::ToolCall,
                    payload: json!({"toolCallId": format!("call-{step}"), "title": format!("Run step {step}")}),
                },
            )
            .await
            .expect("write a transcript row");
    }

    let page = get(&daemon, "/api/v1/coding-sessions").await;
    let items = page["items"].as_array().expect("a page of sessions");
    assert_eq!(items.len(), 1);
    let listed = &items[0];
    assert_eq!(listed["id"], session.id.as_str());
    assert_eq!(listed["harness_name"], "Claude Code");
    assert_eq!(listed["machine_name"], "Air");
    assert_eq!(listed["state"], "idle");
    assert_eq!(listed["last_activity"], "Run step 250");
    assert_eq!(listed["pending"], Value::Null);

    let transcript_path = format!("/api/v1/coding-sessions/{}/transcript", session.id);
    let first = get(&daemon, &transcript_path).await;
    let rows = first["items"].as_array().expect("a page of rows");
    assert_eq!(rows.len(), 200);
    assert_eq!(rows[0]["seq"], 1);
    assert_eq!(rows[0]["kind"], "tool_call");
    assert_eq!(rows[0]["payload"]["title"], "Run step 1");
    assert_eq!(first["next_after"], 200);

    let second = get(&daemon, &format!("{transcript_path}?after=200")).await;
    let rows = second["items"].as_array().expect("a page of rows");
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[0]["seq"], 201);
    assert_eq!(second["next_after"], Value::Null);

    let one = get(&daemon, &format!("/api/v1/coding-sessions/{}", session.id)).await;
    assert_eq!(one["title"], "Fix the failing test");
    assert_eq!(one["machine_name"], "Air");
}

#[tokio::test]
async fn a_list_with_a_state_that_does_not_parse_answers_422() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/coding-sessions?state=asleep",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the daemon answers");

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn a_stop_of_a_terminal_session_answers_409() {
    let daemon = TestDaemon::start().await;
    let host_id = host(&daemon).await;
    let mut session = record(&daemon, &host_id).await;
    session.state = CodingSessionState::Closed;
    session.end_reason = Some("closed".to_string());
    session.ended_at = Some(now_ms());
    daemon
        .stores()
        .coding_sessions
        .update(&session)
        .await
        .expect("close the Coding Session");

    assert_eq!(stop(&daemon, &session).await, 409);
}

#[tokio::test]
async fn a_stop_closes_a_running_session_with_the_end_reason_stopped() {
    let daemon = TestDaemon::start().await;
    let session = start_session(&daemon).await;
    assert_eq!(session.state, CodingSessionState::Working);

    assert_eq!(stop(&daemon, &session).await, 204);

    let stopped = get(&daemon, &format!("/api/v1/coding-sessions/{}", session.id)).await;
    assert_eq!(stopped["state"], "closed");
    assert_eq!(stopped["end_reason"], "stopped");
    assert_eq!(stop(&daemon, &session).await, 409, "a second stop");
}

#[tokio::test]
async fn a_transcript_row_reaches_the_event_socket_as_a_frame_that_names_the_session() {
    let daemon = TestDaemon::start().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;

    let session = start_session(&daemon).await;

    let frame = next_frame_of(&mut socket, "coding_session.transcript").await;
    assert_eq!(
        frame["payload"]["payload"],
        json!({"coding_session_id": session.id.as_str(), "seq": 1, "kind": "prompt"})
    );
    let changed = next_frame_of(&mut socket, "coding_session.changed").await;
    assert_eq!(
        changed["payload"]["payload"]["coding_session_id"],
        session.id.as_str()
    );
}

/// The message that holds the block of `session`.
async fn session_message(daemon: &TestDaemon, session: &CodingSession) -> Message {
    daemon
        .stores()
        .messages
        .get(&daemon.workspace_id, &session.message_id)
        .await
        .expect("read the message")
        .expect("the daemon posts the block of the session")
}

/// The one block of the session's message: the daemon-made
/// `coding_session` block with copies of the display fields.
fn assert_session_block(message: &Message, session: &CodingSession) {
    assert_eq!(
        message.blocks,
        [Block::coding_session(
            session.id.as_str(),
            "Claude Code",
            "Air",
            DIRECTORY,
            "Fix the login bug",
        )]
    );
    assert_eq!(message.author_kind, AuthorKind::System);
    assert_eq!(message.run_id.as_ref(), Some(&session.run_id));
    assert_eq!(
        message.text_content,
        format!("Coding session \"Fix the login bug\": Claude Code on Air in {DIRECTORY}")
    );
}

#[tokio::test]
async fn a_session_from_a_run_in_a_thread_posts_its_block_as_a_reply_in_that_thread() {
    let daemon = TestDaemon::start().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    let root = fixture::user_message(
        &daemon.workspace_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
        "Fix the login bug.",
    );
    daemon
        .stores()
        .messages
        .insert(&root)
        .await
        .expect("write the root message");
    let run_id = run_in(&daemon, Some(root.id.clone())).await;

    let session = start_session_from(&daemon, run_id).await;

    assert_eq!(session.root_message_id, root.id);
    let message = session_message(&daemon, &session).await;
    assert_eq!(message.channel_id, session.channel_id);
    assert_eq!(message.parent_message_id.as_ref(), Some(&root.id));
    assert_session_block(&message, &session);
    let completed = next_frame_of(&mut socket, "message.completed").await;
    assert_eq!(
        completed["payload"]["payload"]["message_id"],
        session.message_id.as_str()
    );
    assert_eq!(
        completed["payload"]["payload"]["parent_message_id"],
        root.id.as_str()
    );
}

#[tokio::test]
async fn a_session_from_a_run_with_no_thread_posts_the_root_of_its_own_thread() {
    let daemon = TestDaemon::start().await;

    let session = start_session(&daemon).await;

    let message = session_message(&daemon, &session).await;
    assert_eq!(message.id, session.root_message_id);
    assert_eq!(message.parent_message_id, None);
    assert_eq!(message.channel_id, session.channel_id);
    assert_session_block(&message, &session);
}

fn start_call(machine: Option<&str>) -> pagis_testkit::Script {
    start_call_in(DIRECTORY, machine)
}

/// A start in `mode`, or in the default mode.
fn start_call_in_mode(mode: Option<&str>) -> pagis_testkit::Script {
    let mut arguments = json!({
        "harness": "claude",
        "directory": DIRECTORY,
        "title": "Fix the login",
        "prompt": PROMPT,
    });
    if let Some(mode) = mode {
        arguments["mode"] = mode.into();
    }
    pagis_testkit::Script::tool_call(&[], "coding_session_start", arguments)
}

fn start_call_in(directory: &str, machine: Option<&str>) -> pagis_testkit::Script {
    let mut arguments = json!({
        "harness": "claude",
        "directory": directory,
        "title": "Fix the login",
        "prompt": PROMPT,
    });
    if let Some(machine) = machine {
        arguments["machine"] = machine.into();
    }
    pagis_testkit::Script::tool_call(&[], "coding_session_start", arguments)
}

async fn daemon_with(brain: &Arc<ScriptedBrain>) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    })
    .await
}

/// A daemon on `brain` whose Wake-ups of a Coding Session read no script.
/// The end of a turn and a decision that waits wake the Agent (ADR-0033).
/// A Wake-up Run that took the next script of the shared queue would take
/// it from the Run that the test starts, so it finds no script and fails.
async fn daemon_for_test_runs(brain: &Arc<ScriptedBrain>) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::new(TestRunsOnly(Arc::clone(brain))),
        ..TestDaemonOptions::default()
    })
    .await
}

/// The scripted brain of the Runs that a test starts: the briefing of a
/// Coding Session event gets no script.
struct TestRunsOnly(Arc<ScriptedBrain>);

#[async_trait]
impl Brain for TestRunsOnly {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        if request.system.contains("Event kind: coding_session.") {
            return Err(BrainError::new(
                "no script for a Wake-up of a Coding Session",
            ));
        }
        self.0.turn(request).await
    }
}

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The tool result that the model read, once the Run completes.
async fn the_tool_result(firehose: &mut Socket, brain: &ScriptedBrain) -> String {
    loop {
        let changed = next_frame_of(firehose, "run.state_changed").await;
        if changed["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    // The end of a turn wakes the Agent, so the last request, or the last
    // message of a request, can be a briefing. The tool result is the
    // newest tool message.
    brain
        .requests()
        .iter()
        .rev()
        .flat_map(|request| request.messages.iter().rev())
        .find(|message| message.role == TurnRole::Tool)
        .expect("the model read the tool result")
        .text
        .clone()
}

/// Sends `text` as the Person. The text is the pending id too, so each
/// text of one test is a new message.
async fn send(daemon: &TestDaemon, cookie: &str, channel_id: &str, text: &str) {
    let sent = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&json!({"pending_id": text, "text": text}))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 201);
}

/// The whole path: the Agent calls the tool, the Person approves the
/// card that names the harness and the machine, the Client App gets the
/// launch command of the Harness Catalog, the harness gets the first
/// prompt, and the Agent reads a session that works. The approval wrote
/// the host Grant of the machine.
#[tokio::test]
async fn an_agent_starts_a_coding_session_on_a_host_after_the_person_approves_its_card() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(start_call(None));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let daemon = daemon_for_test_runs(&brain).await;
    let harnesses: Arc<Mutex<Vec<FakeHarness>>> = Arc::default();
    let client_app = FakeClientApp::running(WORKTREE, {
        let harnesses = Arc::clone(&harnesses);
        move |stream| {
            let (read, write) = futures::io::AsyncReadExt::split(stream);
            let script = Script::default().turn(Turn::until_cancel(vec![]));
            harnesses
                .lock()
                .unwrap()
                .push(FakeHarness::serve(script, write, read));
        }
    });
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let _sessions = SessionClient::connect(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        Arc::clone(&client_app),
    )
    .await
    .expect("the session socket opens");
    wait_until("the session socket is not open", || {
        daemon.host_sessions.is_open(&host_id)
    })
    .await;
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    send(
        &daemon,
        daemon.cookie(),
        &daemon.dm_channel_id,
        "fix the login",
    )
    .await;

    let created = next_frame_of(&mut firehose, "request.created").await;
    let request_id = created["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let card = get(&daemon, &format!("/api/v1/requests/{request_id}")).await;
    assert_eq!(card["kind"], "tool_action");
    assert_eq!(
        card["payload"]["action_title"],
        "Start Claude Code on your Air"
    );
    assert_eq!(card["payload"]["host_id"], host.host_id());
    assert!(
        client_app.requests().is_empty(),
        "nothing starts before the approval"
    );

    let decided = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({"decision": "approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(decided.status(), 200);

    let result = the_tool_result(&mut firehose, &brain).await;
    let result: Value = serde_json::from_str(&result)
        .unwrap_or_else(|error| panic!("the tool result is JSON ({error}): {result}"));
    assert_eq!(result["state"], "working");
    assert_eq!(result["directory"], WORKTREE);
    assert_eq!(result["branch"], "pagis/fix-the-login");
    let session_id = CodingSessionId::from(result["session_id"].as_str().unwrap().to_string());
    let record = daemon
        .stores()
        .coding_sessions
        .get(&daemon.workspace_id, &session_id)
        .await
        .unwrap()
        .expect("the session record");
    assert_eq!(record.state, CodingSessionState::Working);
    assert_eq!(record.host_id.as_ref(), Some(&host_id));

    let requests = client_app.requests();
    assert_eq!(requests.len(), 1);
    let (command, args) = harness::launch_command(harness::entry("claude").unwrap());
    assert_eq!(requests[0].command, command);
    assert_eq!(requests[0].args, args);
    assert_eq!(requests[0].cwd, DIRECTORY);
    let worktree = requests[0].worktree.as_ref().expect("a worktree");
    assert_eq!(worktree.repo, DIRECTORY);
    assert_eq!(worktree.branch, "pagis/fix-the-login");

    let harness = harnesses.lock().unwrap()[0].clone();
    let prompts = harness.params("session/prompt");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].to_string().contains(PROMPT), "{prompts:?}");

    let grants = get(&daemon, "/api/v1/grants").await;
    let items = grants["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{grants:#}");
    assert_eq!(items[0]["resource_kind"], "host");
    assert_eq!(items[0]["resource_id"], host.host_id());
}

/// Person B's Agent, in a DM with B, on the model aliases of B's
/// Workspace. It answers the DM.
async fn person_bs_agent(tenants: &TwoTenants) -> ChannelId {
    let stores = tenants.daemon.stores();
    pagis_server::provisioning::WorkspaceSeed::from(stores)
        .ensure_aliases(&tenants.b.workspace_id)
        .await
        .unwrap();
    let agent = fixture::agent(&tenants.b.workspace_id);
    stores.agents.create(&agent).await.unwrap();
    let channel = fixture::channel(&tenants.b.workspace_id);
    stores.channels.create(&channel).await.unwrap();
    for participant in [
        fixture::agent_participant(&tenants.b.workspace_id, &channel.id, &agent.id),
        fixture::user_participant(&tenants.b.workspace_id, &channel.id),
    ] {
        stores.participants.create(&participant).await.unwrap();
    }
    channel.id
}

/// Person B's Agent names person A's machine. The Host read names B's
/// Workspace, so A's machine is no candidate, and the answer names B's
/// own machines. Nothing reaches A's machine, and B is asked nothing.
#[tokio::test]
async fn person_bs_agent_that_names_person_as_machine_gets_machine_not_found() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(start_call(Some("Air")));
    brain.push(pagis_testkit::Script::reply(&["No such computer."]));
    let tenants = TwoTenants::on(daemon_with(&brain).await).await;
    let daemon = &tenants.daemon;
    let a_client_app = FakeClientApp::opening(WORKTREE);
    let a_host = HostClient::connect_as(
        daemon,
        &tenants.a.cookie,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let _a_sessions = SessionClient::connect(
        daemon,
        &tenants.a.cookie,
        a_host.host_id(),
        Arc::clone(&a_client_app),
    )
    .await
    .expect("person A's session socket opens");
    let _b_host = HostClient::connect_as(
        daemon,
        &tenants.b.cookie,
        "Mini",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let channel_id = person_bs_agent(&tenants).await;
    let stores = daemon.stores();
    let mut firehose = daemon.event_socket(&tenants.b.cookie).await;

    send(
        daemon,
        &tenants.b.cookie,
        channel_id.as_str(),
        "fix it on Air",
    )
    .await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(result.contains("Mini"), "{result}");
    assert!(result.contains("\"Air\""), "{result}");
    let fact = stores
        .events
        .list_by_types(&tenants.b.workspace_id, &["tool.completed"], None, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.payload["name"] == "coding_session_start")
        .expect("the start is in the audit log");
    assert_eq!(fact.payload["error_code"], "machine_not_found");
    assert!(a_client_app.requests().is_empty());
    let pending = stores
        .requests
        .list_by_state(
            &tenants.b.workspace_id,
            pagis_core::RequestState::Pending,
            None,
        )
        .await
        .unwrap();
    assert!(pending.is_empty(), "{pending:?}");
}

/// A Coding Session that an Agent started on a Host, after the Person
/// approved its card, with the harness of `script`.
struct Started {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    _host: HostClient,
    _sessions: SessionClient,
    harnesses: Arc<Mutex<Vec<FakeHarness>>>,
    session_id: CodingSessionId,
    /// The host Grant that the approval wrote.
    grant_id: String,
}

async fn start_approved_session(script: Script) -> Started {
    start_session_after(script, json!({"decision": "approved"})).await
}

/// A Coding Session that an Agent started on a Host after the Person
/// sent `decision` for its card.
async fn start_session_after(script: Script, decision: Value) -> Started {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon_brain = Arc::new(TestRunsOnly(Arc::clone(&brain)));
    start_session_in_mode(script, decision, brain, daemon_brain, None).await
}

/// A Coding Session in `mode` that an Agent started on a Host after the
/// Person sent `decision` for its card. `brain` scripts the Runs of the
/// test, and the daemon thinks with `daemon_brain`. A mode sets the
/// widest mode of the Agent on the machine first, as the Access tab does.
async fn start_session_in_mode(
    script: Script,
    decision: Value,
    brain: Arc<ScriptedBrain>,
    daemon_brain: Arc<dyn Brain>,
    mode: Option<&str>,
) -> Started {
    brain.push(start_call_in_mode(mode));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: daemon_brain,
        ..TestDaemonOptions::default()
    })
    .await;
    let harnesses: Arc<Mutex<Vec<FakeHarness>>> = Arc::default();
    let client_app = FakeClientApp::running(WORKTREE, {
        let harnesses = Arc::clone(&harnesses);
        move |stream| {
            let (read, write) = futures::io::AsyncReadExt::split(stream);
            harnesses
                .lock()
                .unwrap()
                .push(FakeHarness::serve(script.clone(), write, read));
        }
    });
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let sessions = SessionClient::connect(&daemon, daemon.cookie(), host.host_id(), client_app)
        .await
        .expect("the session socket opens");
    wait_until("the session socket is not open", || {
        daemon.host_sessions.is_open(&host_id)
    })
    .await;
    if let Some(mode) = mode {
        // The first write makes the host Grant.
        assert_eq!(
            set_widest_mode(&daemon, host.host_id(), mode).await,
            201,
            "the widest mode is set"
        );
    }
    let mut firehose = daemon.event_socket(daemon.cookie()).await;
    send(
        &daemon,
        daemon.cookie(),
        &daemon.dm_channel_id,
        "fix the login",
    )
    .await;
    let created = next_frame_of(&mut firehose, "request.created").await;
    let request_id = created["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let decided = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&decision)
        .send()
        .await
        .unwrap();
    assert_eq!(decided.status(), 200);
    let result: Value =
        serde_json::from_str(&the_tool_result(&mut firehose, &brain).await).unwrap();
    let session_id = CodingSessionId::from(result["session_id"].as_str().unwrap().to_string());
    let grants = get(&daemon, "/api/v1/grants").await;
    let grant_id = grants["items"][0]["id"].as_str().unwrap().to_string();
    Started {
        daemon,
        brain,
        _host: host,
        _sessions: sessions,
        harnesses,
        session_id,
        grant_id,
    }
}

impl Started {
    fn harness(&self) -> FakeHarness {
        self.harnesses.lock().unwrap()[0].clone()
    }

    /// The answers that the harness got to its permission requests.
    fn answers(&self) -> Vec<Value> {
        self.harness()
            .answers()
            .into_iter()
            .map(|answer| answer.expect("an answer and no error"))
            .collect()
    }

    /// Replaces the Allow Rules of the host Grant, as the Access tab does.
    async fn set_rules(&self, rules: &[&str]) {
        let set = reqwest::Client::new()
            .put(format!(
                "{}/api/v1/grants/{}/rules",
                self.daemon.base_url, self.grant_id
            ))
            .header("cookie", self.daemon.cookie())
            .json(&json!({ "allow": rules }))
            .send()
            .await
            .unwrap();
        assert_eq!(set.status(), 200);
    }

    async fn prompt(&self, text: &str) {
        self.daemon
            .coding_sessions
            .prompt(
                &self.daemon.workspace_id,
                &self.session_id,
                text.to_string(),
            )
            .await
            .unwrap();
    }

    async fn state(&self) -> CodingSessionState {
        self.daemon
            .stores()
            .coding_sessions
            .get(&self.daemon.workspace_id, &self.session_id)
            .await
            .unwrap()
            .expect("the session record")
            .state
    }

    async fn wait_for_state(&self, state: CodingSessionState) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let now = self.state().await;
            if now == state {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the session is {now:?}, not {state:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the bus holds `count` audit facts of Harness
    /// Permissions, and answers them, oldest first.
    async fn wait_for_facts(&self, count: usize) -> Vec<Event> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let mut facts = self
                .daemon
                .stores()
                .events
                .list_by_types(
                    &self.daemon.workspace_id,
                    &[PERMISSION_DECIDED_EVENT],
                    None,
                    100,
                )
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn permission(kind: acp::ToolKind, locations: &[&str], command: Option<&str>) -> Ask {
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
    let options = [
        ("allow-always", acp::PermissionOptionKind::AllowAlways),
        ("allow-once", acp::PermissionOptionKind::AllowOnce),
        ("reject-once", acp::PermissionOptionKind::RejectOnce),
    ]
    .into_iter()
    .map(|(id, kind)| acp::PermissionOption::new(id, id, kind))
    .collect();
    Ask::permission(acp::ToolCallUpdate::new("call-1", fields), options)
}

fn git_status() -> Ask {
    permission(acp::ToolKind::Execute, &[], Some("git status"))
}

fn ends_after(ask: Ask) -> Turn {
    Turn::new(vec![], acp::StopReason::EndTurn).asks(ask)
}

fn allow_once() -> Value {
    json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
}

fn cancelled() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

/// An edit inside the session's directory passes on its scope, an
/// `execute` that a Host Allow Rule matches passes on the rule, and
/// `rm -rf /` waits for the Person until a cancel answers it `cancelled`
/// and expires its Request. Each decision writes one audit fact.
#[tokio::test]
async fn pagis_policy_allows_by_scope_and_by_rule_and_a_cancel_ends_what_waits() {
    let edit = permission(
        acp::ToolKind::Edit,
        &[&format!("{WORKTREE}/src/login.rs")],
        None,
    );
    let remove_all = permission(acp::ToolKind::Execute, &[], Some("rm -rf /"));
    let started = start_approved_session(
        Script::default()
            .turn(ends_after(edit))
            .turn(ends_after(git_status()))
            .turn(Turn::until_cancel(vec![]).asks(remove_all)),
    )
    .await;

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "scope");
    assert_eq!(facts[0].payload["outcome"], "allowed");
    assert_eq!(facts[0].payload["option_kind"], "allow_once");
    assert_eq!(facts[0].payload["session_id"], started.session_id.as_str());
    assert_eq!(facts[0].run_id, None);
    started.wait_for_state(CodingSessionState::Idle).await;

    started.set_rules(&["git status"]).await;
    started.prompt("Check the tree.").await;
    let facts = started.wait_for_facts(2).await;
    assert_eq!(facts[1].payload["decider"], "rule");
    assert_eq!(facts[1].payload["command"], "git status");
    assert_eq!(facts[1].payload["tool_kind"], "execute");
    assert_eq!(facts[1].payload["outcome"], "allowed");
    assert!(facts[1].payload["grant_revision"].as_i64().unwrap() > 1);
    started.wait_for_state(CodingSessionState::Idle).await;

    started.prompt("Clean up.").await;
    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;
    assert_eq!(started.answers(), [allow_once(), allow_once()]);

    started
        .daemon
        .coding_sessions
        .cancel(&started.daemon.workspace_id, &started.session_id)
        .await
        .unwrap();

    let facts = started.wait_for_facts(3).await;
    assert_eq!(facts[2].payload["command"], "rm -rf /");
    assert_eq!(facts[2].payload["outcome"], "expired");
    assert_eq!(facts[2].payload["decider"], Value::Null);
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once(), allow_once(), cancelled()]);
}

/// The policy reads the live host Grant at each permission, so a
/// revision that removes the rule makes the next same command wait.
#[tokio::test]
async fn a_grant_revision_that_removes_the_rule_makes_the_next_same_command_wait() {
    let started = start_approved_session(
        Script::default()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn))
            .turn(ends_after(git_status()))
            .turn(Turn::until_cancel(vec![]).asks(git_status())),
    )
    .await;
    started.wait_for_state(CodingSessionState::Idle).await;
    started.set_rules(&["git status"]).await;
    started.prompt("Check the tree.").await;
    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "rule");
    started.wait_for_state(CodingSessionState::Idle).await;

    started.set_rules(&[]).await;
    started.prompt("Check it again.").await;

    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;
    assert_eq!(started.answers(), [allow_once()]);
    assert_eq!(
        started.wait_for_facts(1).await.len(),
        1,
        "a waiting permission has no fact"
    );
}

/// A queued Run in the Thread of a new message of the Person in the
/// seeded DM channel, and the root of that Thread.
async fn run_in_a_thread(daemon: &TestDaemon) -> (RunId, MessageId) {
    let root = fixture::user_message(
        &daemon.workspace_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
        "Fix the login bug.",
    );
    daemon
        .stores()
        .messages
        .insert(&root)
        .await
        .expect("write the root message");
    (run_in(daemon, Some(root.id.clone())).await, root.id)
}

/// The Runs that a Wake-up of an Incoming Event started, oldest first,
/// once `count` of them reached `state`.
async fn wait_for_event_runs(daemon: &TestDaemon, state: RunState, count: usize) -> Vec<Run> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let mut runs: Vec<Run> = daemon
            .stores()
            .runs
            .list(&daemon.workspace_id, None, None, &[], None, 100)
            .await
            .expect("read the Runs")
            .into_iter()
            .filter(|run| run.trigger_kind == TriggerKind::Event)
            .collect();
        if runs.iter().filter(|run| run.state == state).count() >= count {
            runs.sort_by_key(|run| run.created_at);
            return runs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fewer than {count} event Runs are {state:?}: {runs:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The text inside the untrusted envelope of a Coding Session in the
/// briefing of the event Run of `event_kind`.
fn session_envelope(brain: &ScriptedBrain, session: &CodingSession, event_kind: &str) -> String {
    let request = brain
        .requests()
        .into_iter()
        .find(|request| {
            request
                .system
                .contains(&format!("Event kind: {event_kind}"))
        })
        .unwrap_or_else(|| panic!("no briefing of {event_kind}"));
    let source = format!("source=coding_session:{}", session.id);
    let begin = request
        .system
        .find(&format!("[BEGIN UNTRUSTED {source}"))
        .expect("the envelope of the session");
    let end = request
        .system
        .find(&format!("[END UNTRUSTED {source}"))
        .expect("the end of the envelope");
    assert!(
        !request.system.contains("Sender trust:"),
        "a session is no mail"
    );
    request.system[begin..end].to_string()
}

/// The Session Rule of a session, in the given states.
async fn session_rules(
    daemon: &TestDaemon,
    session: &CodingSession,
    states: &[&str],
) -> Vec<pagis_core::EventSubscription> {
    daemon
        .stores()
        .subscriptions
        .list_for_source(
            &daemon.workspace_id,
            &EventSource::coding_session(session.id.clone()),
            states,
        )
        .await
        .expect("read the Session Rule")
}

#[tokio::test]
async fn a_turn_end_and_the_end_of_the_session_wake_the_agent_in_the_thread_of_the_session() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(pagis_testkit::Script::reply(&["The turn ended."]));
    brain.push(pagis_testkit::Script::reply(&["The session ended."]));
    let daemon = daemon_with(&brain).await;
    let (run_id, root) = run_in_a_thread(&daemon).await;

    let session = start_session_with(
        &daemon,
        run_id,
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn)),
    )
    .await;
    assert_eq!(session_rules(&daemon, &session, &["active"]).await.len(), 3);

    let runs = wait_for_event_runs(&daemon, RunState::Completed, 1).await;
    let woken = &runs[0];
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    assert_eq!(woken.agent_id, agent_id);
    assert_eq!(woken.channel_id.as_ref(), Some(&channel_id));
    assert_eq!(woken.root_message_id.as_ref(), Some(&root));
    assert_eq!(woken.hop_count, 0);
    assert_eq!(
        woken.origin,
        Some(RunOrigin {
            agent_id: agent_id.clone(),
            channel_id: channel_id.clone(),
            root_message_id: Some(root.clone()),
        }),
        "the Thread of the session is the Origin"
    );
    let rows = session_envelope(&brain, &session, "coding_session.turn_ended");
    assert!(rows.contains(session.id.as_str()), "{rows}");
    assert!(rows.contains("stop_reason: end_turn"), "{rows}");
    assert!(rows.contains("machine: Air"), "{rows}");

    daemon
        .coding_sessions
        .close(&daemon.workspace_id, &session.id, CloseReason::Closed)
        .await
        .expect("close the session");

    let runs = wait_for_event_runs(&daemon, RunState::Completed, 2).await;
    assert_eq!(runs[1].root_message_id.as_ref(), Some(&root));
    let rows = session_envelope(&brain, &session, "coding_session.ended");
    assert!(rows.contains("state: closed"), "{rows}");
    assert!(rows.contains("reason: closed"), "{rows}");
    assert!(
        session_rules(&daemon, &session, &["active"])
            .await
            .is_empty()
    );
    assert_eq!(
        session_rules(&daemon, &session, &["archived"]).await.len(),
        3,
        "the end archives the Session Rule"
    );
}

/// A Person asks at the top level of the conversation, so the block of
/// the session is the root of the session's Thread. The supervision
/// stays in that Thread, and the end of the session wakes the Agent at
/// the top level, so its report shows where the Person asked.
#[tokio::test]
async fn a_session_from_the_top_level_ends_with_the_report_of_the_agent_at_the_top_level() {
    const SUPERVISION: &str = "The turn ended, and the checks pass.";
    const REPORT: &str = "Claude Code fixed the login bug, and the tests pass.";
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(pagis_testkit::Script::reply(&[SUPERVISION]));
    brain.push(pagis_testkit::Script::reply(&[REPORT]));
    let daemon = daemon_with(&brain).await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());

    let session = start_session_with(
        &daemon,
        run(&daemon).await,
        Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn)),
    )
    .await;
    assert_eq!(session.root_message_id, session.message_id);

    let runs = wait_for_event_runs(&daemon, RunState::Completed, 1).await;
    assert_eq!(
        runs[0].root_message_id.as_ref(),
        Some(&session.root_message_id),
        "the supervision stays in the session's Thread"
    );

    daemon
        .coding_sessions
        .close(&daemon.workspace_id, &session.id, CloseReason::Closed)
        .await
        .expect("close the session");

    let runs = wait_for_event_runs(&daemon, RunState::Completed, 2).await;
    let ended = &runs[1];
    assert_eq!(
        ended.root_message_id, None,
        "the end wakes the Agent at the top level"
    );
    assert_eq!(
        ended.origin,
        Some(RunOrigin {
            agent_id: agent_id.clone(),
            channel_id: channel_id.clone(),
            root_message_id: None,
        })
    );
    let briefing = brain
        .requests()
        .into_iter()
        .find(|request| request.system.contains("Event kind: coding_session.ended"))
        .expect("the briefing of the end")
        .system;
    assert!(
        briefing.contains("report to the user"),
        "the instruction asks for the report: {briefing}"
    );

    let top_level: Vec<Message> = daemon
        .stores()
        .messages
        .list_top_level(&daemon.workspace_id, &channel_id, None, 50)
        .await
        .expect("read the top level")
        .into_iter()
        .map(|entry| entry.message)
        .collect();
    let report = top_level
        .iter()
        .find(|message| message.text_content == REPORT)
        .unwrap_or_else(|| panic!("no report at the top level: {top_level:?}"));
    assert_eq!(report.author_kind, AuthorKind::Agent);
    assert_eq!(report.author_agent_id.as_ref(), Some(&agent_id));
    let thread = daemon
        .stores()
        .messages
        .list_thread(&daemon.workspace_id, &session.root_message_id)
        .await
        .expect("read the session's Thread");
    assert!(
        thread
            .iter()
            .any(|message| message.text_content == SUPERVISION),
        "{thread:?}"
    );
    assert!(
        top_level
            .iter()
            .all(|message| message.text_content != SUPERVISION),
        "the supervision is not at the top level"
    );
}

#[tokio::test]
async fn two_turn_ends_while_the_woken_run_is_active_make_one_pending_wakeup() {
    let brain = Arc::new(ScriptedBrain::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    brain.push(pagis_testkit::Script::gate_reply(
        &["Reading the session."],
        Arc::clone(&entered),
        Arc::clone(&release),
    ));
    let daemon = daemon_with(&brain).await;
    let ends = || Turn::new(vec![], acp::StopReason::EndTurn);
    let session = start_session_with(
        &daemon,
        run(&daemon).await,
        Script::default().turn(ends()).turn(ends()).turn(ends()),
    )
    .await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .expect("the first turn end wakes the Agent");

    for (text, turns) in [("Go on.", 2), ("And finish.", 3)] {
        let outcome = daemon
            .coding_sessions
            .prompt(&daemon.workspace_id, &session.id, text.to_string())
            .await
            .expect("send a prompt");
        assert_eq!(outcome, PromptOutcome::Sent);
        wait_for_turn_ends(&daemon, &session, turns).await;
    }

    let rule = session_rules(&daemon, &session, &["active"])
        .await
        .into_iter()
        .find(|rule| rule.event_kind == "coding_session.turn_ended")
        .expect("the turn_ended rule");
    let deadline = tokio::time::Instant::now() + WAIT;
    let wakeups = loop {
        let wakeups = daemon
            .stores()
            .subscriptions
            .list_wakeups(&daemon.workspace_id, &rule.id, None, 10)
            .await
            .expect("read the Wake-ups");
        if wakeups
            .iter()
            .map(|wakeup| wakeup.source_count)
            .sum::<u32>()
            == 3
        {
            break wakeups;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the three turn ends do not reach the rule: {wakeups:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let pending: Vec<_> = wakeups
        .iter()
        .filter(|wakeup| wakeup.state == WakeupState::Pending)
        .collect();
    assert_eq!(wakeups.len(), 2, "{wakeups:?}");
    assert_eq!(pending.len(), 1, "{wakeups:?}");
    assert_eq!(pending[0].source_count, 2);
    let event_runs = daemon
        .stores()
        .runs
        .list(&daemon.workspace_id, None, None, &[], None, 100)
        .await
        .expect("read the Runs")
        .into_iter()
        .filter(|run| run.trigger_kind == TriggerKind::Event)
        .count();
    assert_eq!(event_runs, 1, "the woken Run is the only one");
    release.notify_one();
}

/// Waits until the transcript holds `count` ends of a turn and the
/// session is `idle`.
async fn wait_for_turn_ends(daemon: &TestDaemon, session: &CodingSession, count: usize) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let rows = daemon
            .stores()
            .coding_sessions
            .list_events(&daemon.workspace_id, &session.id, None, 1_000)
            .await
            .expect("read the transcript");
        let ends = rows
            .iter()
            .filter(|row| row.kind == CodingSessionEventKind::TurnEnd)
            .count();
        let record = daemon
            .coding_sessions
            .get(&daemon.workspace_id, &session.id)
            .await
            .expect("read the session")
            .expect("the session");
        if ends >= count && record.state == CodingSessionState::Idle {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{ends} turn ends, the session is {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// "Always allow" on a start card writes a session Allow Rule onto the
/// host Grant. A later start of the same harness in a directory under it
/// runs with no card, and a write of the command rules in Settings keeps
/// the session rule.
#[tokio::test]
async fn an_always_approve_of_a_start_lets_a_later_start_under_its_directory_run_with_no_card() {
    let started = start_session_after(
        Script::default().turn(Turn::until_cancel(vec![])),
        json!({"decision": "approved", "scope": "always"}),
    )
    .await;
    let daemon = &started.daemon;
    let grants = get(daemon, "/api/v1/grants").await;
    let session_rules = json!([{"harness": "claude", "directory": DIRECTORY}]);
    assert_eq!(grants["items"][0]["sessions"], session_rules, "{grants:#}");
    assert_eq!(grants["items"][0]["allow"], json!([]));

    started.set_rules(&["git status"]).await;

    let grants = get(daemon, "/api/v1/grants").await;
    assert_eq!(grants["items"][0]["allow"], json!(["git status"]));
    assert_eq!(grants["items"][0]["sessions"], session_rules, "{grants:#}");

    started
        .brain
        .push(start_call_in(&format!("{DIRECTORY}/web"), None));
    started
        .brain
        .push(pagis_testkit::Script::reply(&["Started again."]));
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    send(
        daemon,
        daemon.cookie(),
        &daemon.dm_channel_id,
        "fix the web app",
    )
    .await;

    let result = the_tool_result(&mut firehose, &started.brain).await;
    let result: Value = serde_json::from_str(&result)
        .unwrap_or_else(|error| panic!("the tool result is JSON ({error}): {result}"));
    assert_eq!(result["state"], "working");
    assert_ne!(result["session_id"], started.session_id.as_str());
    let pending = daemon
        .stores()
        .requests
        .list_by_state(
            &daemon.workspace_id,
            pagis_core::RequestState::Pending,
            None,
        )
        .await
        .unwrap();
    assert!(pending.is_empty(), "{pending:?}");
    assert_eq!(started.harnesses.lock().unwrap().len(), 2);
}

/// The error code of the last call of `tool` in the audit log of a
/// Workspace.
async fn error_code_of(
    daemon: &TestDaemon,
    workspace_id: &pagis_core::WorkspaceId,
    tool: &str,
) -> Value {
    daemon
        .stores()
        .events
        .list_by_types(workspace_id, &["tool.completed"], None, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.payload["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} is in the audit log"))
        .payload["error_code"]
        .clone()
}

/// Sends the Person's `text` and answers the tool result that the model
/// read in the Run that it starts.
async fn tool_result_after(started: &Started, text: &str) -> String {
    let daemon = &started.daemon;
    let mut firehose = daemon.event_socket(daemon.cookie()).await;
    send(daemon, daemon.cookie(), &daemon.dm_channel_id, text).await;
    the_tool_result(&mut firehose, &started.brain).await
}

/// One call of `tool` on the started session, then a reply.
fn session_call(started: &Started, tool: &str, mut arguments: Value) {
    arguments["session"] = started.session_id.as_str().into();
    started
        .brain
        .push(pagis_testkit::Script::tool_call(&[], tool, arguments));
    started.brain.push(pagis_testkit::Script::reply(&["Done."]));
}

impl Started {
    /// Waits until the transcript holds `count` ends of a turn.
    async fn wait_for_turn_ends(&self, count: usize) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let ends = self
                .daemon
                .stores()
                .coding_sessions
                .list_events(&self.daemon.workspace_id, &self.session_id, None, 1_000)
                .await
                .unwrap()
                .into_iter()
                .filter(|row| row.kind == CodingSessionEventKind::TurnEnd)
                .count();
            if ends >= count {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{ends} ends of a turn, not {count}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The Agent sends a prompt while a turn runs, so the prompt waits. The
/// first turn ends, the prompt starts the second, and a read after it
/// ends holds the agent message of the harness inside the envelope of
/// the session.
#[tokio::test]
async fn an_agent_sends_a_prompt_while_a_turn_runs_and_reads_the_message_after_it_ends() {
    let waits = permission(acp::ToolKind::Execute, &[], Some("rm -rf /")).withdrawn();
    let answer = acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("The login works now.")),
    ));
    let started = start_approved_session(
        Script::default()
            .turn(ends_after(waits))
            .turn(Turn::new(vec![answer], acp::StopReason::EndTurn)),
    )
    .await;
    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;

    session_call(
        &started,
        "coding_session_send",
        json!({"prompt": "Add a test."}),
    );
    let sent = tool_result_after(&started, "add a test").await;

    assert_eq!(sent, "A turn runs. Your prompt goes when it ends.");
    started.harness().withdraw_ask();
    started.wait_for_turn_ends(2).await;
    started.wait_for_state(CodingSessionState::Idle).await;
    let prompts = started.harness().params("session/prompt");
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].to_string().contains("Add a test."),
        "{prompts:?}"
    );

    session_call(&started, "coding_session_read", json!({}));
    let read = tool_result_after(&started, "how is it going").await;

    let read: Value = serde_json::from_str(&read)
        .unwrap_or_else(|error| panic!("the tool result is JSON ({error}): {read}"));
    assert_eq!(read["state"], "idle");
    assert_eq!(read["machine"], "Air");
    let output = read["harness_output"].as_str().unwrap();
    let begin = format!(
        "[BEGIN UNTRUSTED source=coding_session:{}]",
        started.session_id
    );
    assert!(output.starts_with(&begin), "{output}");
    assert!(output.contains("The login works now."), "{output}");
}

/// Person B's Agent names the id of person A's session. The read names
/// B's Workspace, so the session reads as absent.
#[tokio::test]
async fn person_bs_agent_that_reads_person_as_session_gets_session_not_found() {
    let brain = Arc::new(ScriptedBrain::default());
    let tenants = TwoTenants::on(daemon_with(&brain).await).await;
    let daemon = &tenants.daemon;
    let a_session = tenants.a_id("coding_session_id").to_string();
    brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_read",
        json!({ "session": a_session }),
    ));
    brain.push(pagis_testkit::Script::reply(&["No such session."]));
    // B's machine declares a harness, so B's Agent holds the tool.
    daemon
        .stores()
        .hosts
        .register(
            &tenants.b.workspace_id,
            "Mini",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .unwrap();
    let channel_id = person_bs_agent(&tenants).await;
    let mut firehose = daemon.event_socket(&tenants.b.cookie).await;

    send(
        daemon,
        &tenants.b.cookie,
        channel_id.as_str(),
        "read that session",
    )
    .await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(result.contains("session_not_found"), "{result}");
    assert!(!result.contains("Fix the failing test"), "{result}");
    assert_eq!(
        error_code_of(daemon, &tenants.b.workspace_id, "coding_session_read").await,
        "session_not_found"
    );
}

fn cargo_test() -> Ask {
    permission(acp::ToolKind::Execute, &[], Some("cargo test"))
}

fn reject_once() -> Value {
    json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
}

/// Sends `decision` for a Request as the person of `cookie`, and
/// answers the status.
async fn decide(daemon: &TestDaemon, cookie: &str, request_id: &RequestId, decision: Value) -> u16 {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&decision)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

impl Started {
    /// Waits until a Harness Permission waits for the Person, and
    /// answers its Request.
    async fn wait_for_card(&self) -> Request {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let pending = self
                .daemon
                .stores()
                .requests
                .list_by_state(
                    &self.daemon.workspace_id,
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the Request is in `state`.
    async fn wait_for_request(&self, request_id: &RequestId, state: RequestState) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let request = self
                .daemon
                .stores()
                .requests
                .get(&self.daemon.workspace_id, request_id)
                .await
                .unwrap()
                .expect("the Request");
            if request.state == state {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Request is {:?}, not {state:?}",
                request.state
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn record(&self) -> CodingSession {
        self.daemon
            .stores()
            .coding_sessions
            .get(&self.daemon.workspace_id, &self.session_id)
            .await
            .unwrap()
            .expect("the session record")
    }

    /// The approval cards in the session's Thread.
    async fn cards(&self) -> Vec<Message> {
        let record = self.record().await;
        self.daemon
            .stores()
            .messages
            .list_thread(&self.daemon.workspace_id, &record.root_message_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|message| {
                message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::Known(KnownBlock::ApprovalCard { .. })))
            })
            .collect()
    }

    async fn decide(&self, request: &Request, decision: Value) -> u16 {
        decide(&self.daemon, self.daemon.cookie(), &request.id, decision).await
    }
}

/// In the `person` mode, an `execute` that no rule allows posts one
/// approval card in the session's Thread, as a System message. "Approve
/// once" answers the harness with its `allow_once` option, and the audit
/// fact names the Person.
#[tokio::test]
async fn a_command_that_policy_does_not_allow_asks_the_person_and_approve_once_allows_it_once() {
    let started = start_approved_session(Script::default().turn(ends_after(cargo_test()))).await;

    let request = started.wait_for_card().await;
    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;
    let record = started.record().await;
    assert_eq!(request.run_id, None);
    assert_eq!(request.agent_id.as_str(), started.daemon.agent_id);
    assert_eq!(request.payload["session_id"], started.session_id.as_str());
    assert_eq!(request.payload["host_name"], "Air");
    assert_eq!(request.payload["directory"], WORKTREE);
    assert_eq!(request.payload["command"], "cargo test");
    assert_eq!(request.payload["proposed_rules"], json!(["cargo test"]));
    assert_eq!(
        request.payload["root_message_id"],
        record.root_message_id.as_str()
    );
    let cards = started.cards().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].author_kind, AuthorKind::System);
    assert_eq!(cards[0].run_id, None);
    assert_eq!(
        cards[0].blocks,
        [Block::approval_card(
            request.id.as_str(),
            "Claude Code wants to run a command",
            "cargo test"
        )]
    );
    let begin = format!(
        "[BEGIN UNTRUSTED source=coding_session:{}]",
        started.session_id
    );
    assert!(
        cards[0].text_content.contains(&begin),
        "{}",
        cards[0].text_content
    );
    assert!(started.answers().is_empty(), "the permission waits");

    assert_eq!(
        started
            .decide(&request, json!({"decision": "approved"}))
            .await,
        200
    );

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "person");
    assert_eq!(facts[0].payload["outcome"], "allowed");
    assert_eq!(facts[0].payload["option_kind"], "allow_once");
    assert_eq!(facts[0].payload["scope"], "once");
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once()]);
    let decisions: Vec<_> = started
        .daemon
        .stores()
        .coding_sessions
        .list_events(
            &started.daemon.workspace_id,
            &started.session_id,
            None,
            1_000,
        )
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == CodingSessionEventKind::Decision)
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].payload["decision"], "allow_once");
    assert_eq!(decisions[0].payload["decider"], "person");
}

/// "Always allow" writes the rule `cargo test` onto the host Grant of the
/// machine and answers `allow_once`. The next `cargo test` of the session
/// passes on the rule, with no card.
#[tokio::test]
async fn always_allow_writes_the_rule_onto_the_host_grant_and_the_next_same_command_needs_no_card()
{
    let started = start_approved_session(
        Script::default()
            .turn(ends_after(cargo_test()))
            .turn(ends_after(cargo_test())),
    )
    .await;
    let request = started.wait_for_card().await;

    assert_eq!(
        started
            .decide(&request, json!({"decision": "approved", "scope": "always"}))
            .await,
        200
    );

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "person");
    assert_eq!(facts[0].payload["scope"], "always");
    let grants = get(&started.daemon, "/api/v1/grants").await;
    assert_eq!(grants["items"].as_array().unwrap().len(), 1, "{grants:#}");
    assert_eq!(grants["items"][0]["allow"], json!(["cargo test"]));
    started.wait_for_state(CodingSessionState::Idle).await;

    started.prompt("Run the tests again.").await;

    let facts = started.wait_for_facts(2).await;
    assert_eq!(facts[1].payload["decider"], "rule");
    assert_eq!(facts[1].payload["outcome"], "allowed");
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once(), allow_once()]);
    assert_eq!(started.cards().await.len(), 1);
}

/// "Deny" answers `reject_once`. A cancel of the turn expires the
/// Request of the next card, and the harness gets `cancelled`. A decision
/// on the expired Request is refused.
#[tokio::test]
async fn deny_answers_reject_once_and_a_cancel_of_the_turn_expires_the_request() {
    let started = start_approved_session(
        Script::default()
            .turn(ends_after(cargo_test()))
            .turn(Turn::until_cancel(vec![]).asks(cargo_test())),
    )
    .await;
    let denied = started.wait_for_card().await;

    assert_eq!(
        started.decide(&denied, json!({"decision": "denied"})).await,
        200
    );

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "person");
    assert_eq!(facts[0].payload["outcome"], "rejected");
    assert_eq!(facts[0].payload["option_kind"], "reject_once");
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [reject_once()]);

    started.prompt("Try again.").await;
    let waiting = started.wait_for_card().await;
    assert_ne!(waiting.id, denied.id);
    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;
    started
        .daemon
        .coding_sessions
        .cancel(&started.daemon.workspace_id, &started.session_id)
        .await
        .unwrap();

    started
        .wait_for_request(&waiting.id, RequestState::Expired)
        .await;
    let facts = started.wait_for_facts(2).await;
    assert_eq!(facts[1].payload["outcome"], "expired");
    assert_eq!(facts[1].payload["decider"], Value::Null);
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [reject_once(), cancelled()]);
    assert_eq!(
        started
            .decide(&waiting, json!({"decision": "approved"}))
            .await,
        409
    );
}

/// A restart ends every ACP connection, so the boot expires each pending
/// Harness Permission. A pending Request of a Run that did not park stays
/// as it is.
#[tokio::test]
async fn a_restart_expires_a_pending_harness_permission() {
    let daemon = TestDaemon::start().await;
    let host_id = host(&daemon).await;
    let session = record(&daemon, &host_id).await;
    let permission = Request {
        id: RequestId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        agent_id: AgentId::from(daemon.agent_id.clone()),
        run_id: None,
        kind: Request::HARNESS_PERMISSION_KIND.to_string(),
        payload: json!({"session_id": session.id, "command": "cargo test"}),
        state: RequestState::Pending,
        values: None,
        decided_at: None,
        created_at: now_ms(),
    };
    let stores = daemon.stores();
    stores.requests.create(&permission).await.unwrap();
    let form = fixture::pending_form_request(
        &daemon.workspace_id,
        &AgentId::from(daemon.agent_id.clone()),
        &session.run_id,
    );
    stores.requests.create(&form).await.unwrap();

    let daemon = daemon.restart(TestDaemonOptions::default()).await;

    let stores = daemon.stores();
    let read = |id: RequestId| {
        let requests = stores.requests.clone();
        let workspace_id = daemon.workspace_id.clone();
        async move { requests.get(&workspace_id, &id).await.unwrap().unwrap() }
    };
    assert_eq!(read(permission.id).await.state, RequestState::Expired);
    assert_eq!(read(form.id).await.state, RequestState::Pending);
}

/// Person B posts a decision on person A's Harness Permission. The read
/// names B's Workspace, so the Request reads as absent, and A's harness
/// gets no answer.
#[tokio::test]
async fn person_bs_decision_on_person_as_harness_permission_gets_404() {
    let started = start_approved_session(
        Script::default().turn(Turn::until_cancel(vec![]).asks(cargo_test())),
    )
    .await;
    let request = started.wait_for_card().await;
    let Started {
        daemon,
        _host: _a_host,
        _sessions: _a_sessions,
        harnesses,
        ..
    } = started;
    let tenants = TwoTenants::on(daemon).await;

    let status = decide(
        &tenants.daemon,
        &tenants.b.cookie,
        &request.id,
        json!({"decision": "approved"}),
    )
    .await;

    assert_eq!(status, 404);
    let still = tenants
        .daemon
        .stores()
        .requests
        .get(&tenants.a.workspace_id, &request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.state, RequestState::Pending);
    assert!(harnesses.lock().unwrap()[0].answers().is_empty());
}

/// The Needs-You Queue that `cookie` reads.
async fn needs_you(daemon: &TestDaemon, cookie: &str) -> Vec<Value> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/needs-you", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .expect("the daemon answers");
    assert_eq!(response.status(), 200);
    let queue: Value = response.json().await.expect("a JSON answer");
    queue["items"].as_array().expect("the items").clone()
}

/// The item of the queue with `item_id`, if the queue holds it.
fn item_of(items: &[Value], item_id: &str) -> Option<Value> {
    items.iter().find(|item| item["id"] == item_id).cloned()
}

/// In the `person` mode, a Harness Permission is an Approval of the
/// Needs-You Queue that opens the session's Thread. A decision with no
/// `scope`, as the service worker sends it from a Notification, answers
/// the harness with `allow_once`, and the item leaves the queue.
#[tokio::test]
async fn a_harness_permission_is_a_needs_you_item_that_a_notification_answers_once() {
    let started = start_approved_session(Script::default().turn(ends_after(cargo_test()))).await;
    let mut firehose = started.daemon.event_socket(started.daemon.cookie()).await;
    let request = started.wait_for_card().await;
    let record = started.record().await;
    let item_id = format!("request:{}", request.id);

    let items = needs_you(&started.daemon, started.daemon.cookie()).await;

    let item = item_of(&items, &item_id).expect("the queue holds the Harness Permission");
    assert_eq!(item["kind"], "approval");
    assert_eq!(item["request_kind"], Request::HARNESS_PERMISSION_KIND);
    assert_eq!(
        item["url"],
        format!("/c/{}/t/{}", record.channel_id, record.root_message_id)
    );

    assert_eq!(
        started
            .decide(&request, json!({"decision": "approved"}))
            .await,
        200
    );

    loop {
        let removed = next_frame_of(&mut firehose, "needs_you.removed").await;
        if removed["payload"]["payload"]["item_id"] == item_id.as_str() {
            break;
        }
    }
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once()]);
    let items = needs_you(&started.daemon, started.daemon.cookie()).await;
    assert_eq!(item_of(&items, &item_id), None);
}

/// Person B's Needs-You Queue holds nothing of person A's Harness
/// Permission.
#[tokio::test]
async fn person_bs_queue_holds_nothing_of_person_as_harness_permission() {
    let started = start_approved_session(
        Script::default().turn(Turn::until_cancel(vec![]).asks(cargo_test())),
    )
    .await;
    let request = started.wait_for_card().await;
    let item_id = format!("request:{}", request.id);
    let Started {
        daemon,
        _host: _a_host,
        _sessions: _a_sessions,
        ..
    } = started;
    let tenants = TwoTenants::on(daemon).await;

    let a_items = needs_you(&tenants.daemon, &tenants.a.cookie).await;
    let b_items = needs_you(&tenants.daemon, &tenants.b.cookie).await;

    assert!(item_of(&a_items, &item_id).is_some(), "A reads {a_items:?}");
    assert_eq!(b_items, Vec::<Value>::new());
}

/// Waits until the record of `session_id` is in `state`.
async fn wait_for_session_state(
    daemon: &TestDaemon,
    session_id: &CodingSessionId,
    state: CodingSessionState,
) -> CodingSession {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let record = daemon
            .stores()
            .coding_sessions
            .get(&daemon.workspace_id, session_id)
            .await
            .unwrap()
            .expect("the session record");
        if record.state == state {
            return record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session is {:?}, not {state:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The Client App goes away with its Host socket and its session socket,
/// and the session is interrupted. The Client App comes back, the Agent
/// calls `coding_session_resume`, and the harness resumes its own session
/// in the working directory. The session is then idle.
#[tokio::test]
async fn a_lost_host_interrupts_the_session_and_the_agent_resumes_it_when_the_host_is_back() {
    let script = Script::default()
        .resume()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn));
    let started = start_approved_session(script.clone()).await;
    started.wait_for_state(CodingSessionState::Idle).await;
    let Started {
        daemon,
        brain,
        _host: host,
        _sessions: sessions,
        harnesses,
        session_id,
        grant_id,
    } = started;
    let host_id = HostId::from(host.host_id().to_string());

    drop(sessions);
    drop(host);

    let record =
        wait_for_session_state(&daemon, &session_id, CodingSessionState::Interrupted).await;
    assert_eq!(record.end_reason, None);
    // The interruption wakes the Agent at the top level, where the Person
    // asked. The brain of the test has no script for it, so that Run
    // fails, and then the Person writes there. The end of the turn
    // before woke the Agent in the session's Thread.
    let woken = wait_for_event_runs(&daemon, RunState::Failed, 2).await;
    let places: Vec<_> = woken
        .iter()
        .map(|run| run.root_message_id.is_some())
        .collect();
    assert_eq!(places, [true, false], "{woken:?}");
    let client_app = FakeClientApp::running(WORKTREE, {
        let harnesses = Arc::clone(&harnesses);
        move |stream| {
            let (read, write) = futures::io::AsyncReadExt::split(stream);
            harnesses
                .lock()
                .unwrap()
                .push(FakeHarness::serve(script.clone(), write, read));
        }
    });
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    assert_eq!(host.host_id(), host_id.as_str(), "the same machine is back");
    let sessions = SessionClient::connect(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        Arc::clone(&client_app),
    )
    .await
    .expect("the session socket opens");
    wait_until("the session socket is not open", || {
        daemon.host_sessions.is_open(&host_id)
    })
    .await;
    let started = Started {
        daemon,
        brain,
        _host: host,
        _sessions: sessions,
        harnesses,
        session_id,
        grant_id,
    };

    session_call(&started, "coding_session_resume", json!({}));
    let resumed = tool_result_after(&started, "resume the session").await;

    assert_eq!(
        resumed,
        "The session resumed. It is idle and takes a new prompt."
    );
    started.wait_for_state(CodingSessionState::Idle).await;
    let request = &client_app.requests()[0];
    assert_eq!(request.session_id, started.session_id);
    assert_eq!(request.cwd, WORKTREE, "the session resumes in its worktree");
    assert_eq!(request.worktree, None, "the worktree exists");
    let harness = started.harnesses.lock().unwrap()[1].clone();
    let resume = harness.params("session/resume");
    assert_eq!(resume.len(), 1, "{:?}", harness.received());
    assert_eq!(resume[0]["sessionId"], NEW_SESSION_ID);
    assert_eq!(resume[0]["cwd"], WORKTREE);
}

/// A restart ends every ACP connection, so the session that was working
/// is interrupted.
#[tokio::test]
async fn a_restart_leaves_a_working_session_interrupted() {
    let daemon = TestDaemon::start().await;
    let session = start_session(&daemon).await;
    assert_eq!(session.state, CodingSessionState::Working);

    let daemon = daemon.restart(TestDaemonOptions::default()).await;

    let record =
        wait_for_session_state(&daemon, &session.id, CodingSessionState::Interrupted).await;
    assert_eq!(record.end_reason, None);
    assert_eq!(record.ended_at, None);
}

/// Allows or stops the Unattended Modes of the daemon's Agent on a
/// machine, as the Access tab does, and answers the status.
async fn set_unattended_modes(daemon: &TestDaemon, host_id: &HostId, allowed: bool) -> u16 {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/agents/{}/hosts/{host_id}/unattended-modes",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({ "allowed": allowed }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// pi never asks, so Pagis cannot apply a narrower Grant to it: the
/// session closes, and the end wakes the Agent in the session's Thread.
#[tokio::test]
async fn the_person_who_stops_allowing_unattended_modes_closes_a_pi_session_and_the_agent_wakes() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(pagis_testkit::Script::reply(&["The session ended."]));
    let daemon = daemon_with(&brain).await;
    let (run_id, root) = run_in_a_thread(&daemon).await;
    let host_id = host(&daemon).await;
    assert_eq!(set_unattended_modes(&daemon, &host_id, true).await, 201);
    open_session_socket(
        &daemon,
        &host_id,
        Script::default().turn(Turn::until_cancel(vec![])),
    )
    .await;
    let session = daemon
        .coding_sessions
        .start(NewCodingSession {
            workspace_id: daemon.workspace_id.clone(),
            agent_id: AgentId::from(daemon.agent_id.clone()),
            run_id,
            place: Place::Host(host_id.clone()),
            harness_id: "pi".to_string(),
            directory: DIRECTORY.to_string(),
            worktree: None,
            approval_mode: SessionApprovalMode::Person,
            harness_mode: None,
            title: "Fix the login bug".to_string(),
            prompt: PROMPT.to_string(),
        })
        .await
        .expect("the session starts");

    assert_eq!(set_unattended_modes(&daemon, &host_id, false).await, 200);

    let record = wait_for_session_state(&daemon, &session.id, CodingSessionState::Closed).await;
    assert_eq!(record.end_reason.as_deref(), Some("approval_mode_narrowed"));
    let runs = wait_for_event_runs(&daemon, RunState::Completed, 1).await;
    assert_eq!(runs[0].root_message_id.as_ref(), Some(&root));
    let rows = session_envelope(&brain, &session, "coding_session.ended");
    assert!(rows.contains("state: closed"), "{rows}");
    assert!(rows.contains("reason: approval_mode_narrowed"), "{rows}");
}

/// The Agent picks a Harness Mode that acts without asking where the
/// Person allows Unattended Modes: the card names the mode before the
/// harness runs, and the session reads as unattended.
#[tokio::test]
async fn an_agent_starts_a_session_in_bypass_permissions_where_the_person_allows_unattended_modes()
{
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_start",
        json!({
            "harness": "claude",
            "directory": DIRECTORY,
            "title": "Fix the login",
            "prompt": PROMPT,
            "harness_mode": "bypassPermissions",
        }),
    ));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let daemon = daemon_for_test_runs(&brain).await;
    let modes: Vec<(&str, &str)> = harness::entry("claude")
        .unwrap()
        .modes
        .iter()
        .map(|mode| (mode.id, mode.name))
        .collect();
    let script = Script::default()
        .modes("default", &modes)
        .turn(Turn::until_cancel(vec![]));
    let client_app = FakeClientApp::running(WORKTREE, move |stream| {
        let (read, write) = futures::io::AsyncReadExt::split(stream);
        FakeHarness::serve(script.clone(), write, read);
    });
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let _sessions = SessionClient::connect(&daemon, daemon.cookie(), host.host_id(), client_app)
        .await
        .expect("the session socket opens");
    wait_until("the session socket is not open", || {
        daemon.host_sessions.is_open(&host_id)
    })
    .await;
    assert_eq!(set_unattended_modes(&daemon, &host_id, true).await, 201);
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    send(
        &daemon,
        daemon.cookie(),
        &daemon.dm_channel_id,
        "fix the login",
    )
    .await;

    let created = next_frame_of(&mut firehose, "request.created").await;
    let request_id = created["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let card = get(&daemon, &format!("/api/v1/requests/{request_id}")).await;
    let body = card["payload"]["body"].as_str().unwrap_or_default();
    assert!(
        body.contains("\nHarness mode: Bypass permissions (acts without asking)\n"),
        "{body}"
    );
    let decided = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({"decision": "approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(decided.status(), 200);
    let result = the_tool_result(&mut firehose, &brain).await;
    let result: Value = serde_json::from_str(&result)
        .unwrap_or_else(|error| panic!("the tool result is JSON ({error}): {result}"));
    let session_id = result["session_id"].as_str().expect("a started session");

    let session = get(&daemon, &format!("/api/v1/coding-sessions/{session_id}")).await;
    assert_eq!(session["harness_mode"], "bypassPermissions");
    assert_eq!(session["harness_mode_name"], "Bypass permissions");
    assert_eq!(session["unattended"], true);
    assert_eq!(session["harness_modes"].as_array().map(Vec::len), Some(5));
    assert_eq!(session["state"], "working");
}

// The `agent` mode: the supervising Agent decides a Harness Permission
// or gives it to the Person (ADR-0033).

const NOTE: &str = "The build directory is generated, so a delete is safe.";

/// Sets the widest Session Approval Mode of the daemon's Agent on a
/// machine, as the Access tab does, and answers the status.
async fn set_widest_mode(daemon: &TestDaemon, host_id: &str, mode: &str) -> u16 {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/agents/{}/hosts/{host_id}/session-approval-mode",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({ "mode": mode }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

fn rm_rf_build() -> Ask {
    permission(acp::ToolKind::Execute, &[], Some("rm -rf build"))
}

/// The brain of a test in the `agent` mode. The Runs that the test starts
/// take the scripts of `test`. The Wake-up of a decision that waits says
/// so on `entered`, and takes the scripts of `decisions` once the test
/// opens the gate. Each other Wake-up of a Coding Session gets no script.
struct DecisionWakeups {
    test: Arc<ScriptedBrain>,
    decisions: Arc<ScriptedBrain>,
    entered: Arc<tokio::sync::Notify>,
    gate: tokio::sync::watch::Receiver<bool>,
}

#[async_trait]
impl Brain for DecisionWakeups {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        if request
            .system
            .contains("Event kind: coding_session.needs_decision")
        {
            self.entered.notify_one();
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
            return self.decisions.turn(request).await;
        }
        if request.system.contains("Event kind: coding_session.") {
            return Err(BrainError::new(
                "no script for a Wake-up of a Coding Session",
            ));
        }
        self.test.turn(request).await
    }
}

/// A Coding Session in the `agent` mode whose permission waits for the
/// Agent, and the brain of the Wake-up of that decision.
struct AgentMode {
    started: Started,
    decisions: Arc<ScriptedBrain>,
    entered: Arc<tokio::sync::Notify>,
    gate: tokio::sync::watch::Sender<bool>,
}

/// Starts a session in the `agent` mode on a machine whose widest mode
/// is `agent`, and waits until its first permission waits.
async fn start_agent_mode_session(script: Script) -> AgentMode {
    let brain = Arc::new(ScriptedBrain::default());
    let decisions = Arc::new(ScriptedBrain::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let (gate, opened) = tokio::sync::watch::channel(false);
    let daemon_brain = Arc::new(DecisionWakeups {
        test: Arc::clone(&brain),
        decisions: Arc::clone(&decisions),
        entered: Arc::clone(&entered),
        gate: opened,
    });
    let started = start_session_in_mode(
        script,
        json!({"decision": "approved"}),
        brain,
        daemon_brain,
        Some("agent"),
    )
    .await;
    started
        .wait_for_state(CodingSessionState::NeedsDecision)
        .await;
    AgentMode {
        started,
        decisions,
        entered,
        gate,
    }
}

impl AgentMode {
    /// Waits until the decision wakes the Agent and its Run asks the
    /// brain.
    async fn woken(&self) {
        tokio::time::timeout(WAIT, self.entered.notified())
            .await
            .expect("the decision wakes the Agent");
    }

    /// Waits until the session's Thread holds one approval card, and
    /// answers the cards.
    async fn wait_for_card_message(&self) -> Vec<Message> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let cards = self.started.cards().await;
            if !cards.is_empty() {
                return cards;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no approval card in the session's Thread"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Lets the woken Run take `scripts`.
    fn think(&self, scripts: Vec<pagis_testkit::Script>) {
        for script in scripts {
            self.decisions.push(script);
        }
        self.gate.send_replace(true);
    }

    fn call(&self, tool: &str, mut arguments: Value) -> pagis_testkit::Script {
        arguments["session"] = self.started.session_id.as_str().into();
        pagis_testkit::Script::tool_call(&[], tool, arguments)
    }

    /// Waits until the transcript holds `count` decision rows, and
    /// answers them, oldest first.
    async fn decision_rows(&self, count: usize) -> Vec<Value> {
        let daemon = &self.started.daemon;
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let rows: Vec<Value> = daemon
                .stores()
                .coding_sessions
                .list_events(&daemon.workspace_id, &self.started.session_id, None, 1_000)
                .await
                .unwrap()
                .into_iter()
                .filter(|row| row.kind == CodingSessionEventKind::Decision)
                .map(|row| row.payload)
                .collect();
            if rows.len() >= count {
                return rows;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} decision rows, not {count}",
                rows.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// In the `agent` mode an `execute` of `rm -rf build` wakes the Agent in
/// the session's Thread. Its allow answers the harness with `allow_once`,
/// with no card. The audit fact names the Agent, the woken Run and the
/// note, and the host Grant gets no rule.
#[tokio::test]
async fn in_the_agent_mode_a_command_wakes_the_agent_and_its_allow_answers_the_harness_once() {
    let agent = start_agent_mode_session(Script::default().turn(ends_after(rm_rf_build()))).await;
    let started = &agent.started;
    let daemon = &started.daemon;

    agent.woken().await;
    agent.think(vec![
        agent.call(
            "coding_session_decide",
            json!({"decision": "allow", "note": NOTE}),
        ),
        pagis_testkit::Script::reply(&["I allowed it."]),
    ]);

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "agent");
    assert_eq!(facts[0].payload["outcome"], "allowed");
    assert_eq!(facts[0].payload["option_kind"], "allow_once");
    assert_eq!(facts[0].payload["command"], "rm -rf build");
    assert_eq!(facts[0].payload["note"], NOTE);
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once()]);
    let record = started.record().await;
    let woken = wait_for_event_runs(daemon, RunState::Completed, 1).await;
    let woken = woken
        .iter()
        .find(|run| run.state == RunState::Completed)
        .expect("the woken Run");
    assert_eq!(
        woken.root_message_id.as_ref(),
        Some(&record.root_message_id)
    );
    assert_eq!(facts[0].payload["run_id"], woken.id.as_str());
    let rows = session_envelope(&agent.decisions, &record, "coding_session.needs_decision");
    assert!(rows.contains("decision_kind: permission"), "{rows}");
    assert!(started.cards().await.is_empty(), "the Agent decided");
    let grants = get(daemon, "/api/v1/grants").await;
    assert_eq!(grants["items"].as_array().unwrap().len(), 1);
    assert_eq!(grants["items"][0]["allow"], json!([]));
    assert_eq!(grants["items"][0]["sessions"], json!([]));
    assert_eq!(grants["items"][0]["session_approval_mode"], "agent");
    let decisions = agent.decision_rows(1).await;
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["decision"], "allow_once");
    assert_eq!(decisions[0]["decider"], "agent");
}

/// `coding_session_escalate` posts one approval card that shows the
/// Agent's note. The Person's "Approve once" answers the harness, and the
/// audit fact names the Person.
#[tokio::test]
async fn an_escalation_posts_one_card_with_the_note_and_the_person_approves_it_once() {
    let question = "Delete the build directory? I did not make it.";
    let agent = start_agent_mode_session(Script::default().turn(ends_after(rm_rf_build()))).await;
    let started = &agent.started;

    agent.woken().await;
    agent.think(vec![
        agent.call("coding_session_escalate", json!({"note": question})),
        pagis_testkit::Script::reply(&["I asked you on the card."]),
    ]);

    let request = started.wait_for_card().await;
    assert_eq!(request.payload["note"], question);
    assert_eq!(request.payload["command"], "rm -rf build");
    assert_eq!(agent.wait_for_card_message().await.len(), 1);
    assert!(started.answers().is_empty(), "the permission waits");

    assert_eq!(
        started
            .decide(&request, json!({"decision": "approved"}))
            .await,
        200
    );

    let facts = started.wait_for_facts(1).await;
    assert_eq!(facts[0].payload["decider"], "person");
    assert_eq!(facts[0].payload["outcome"], "allowed");
    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [allow_once()]);
    let decisions = agent.decision_rows(2).await;
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0]["decision"], "escalated");
    assert_eq!(decisions[0]["decider"], "agent");
    assert_eq!(decisions[0]["note"], question);
    assert_eq!(decisions[1]["decision"], "allow_once");
    assert_eq!(decisions[1]["decider"], "person");
}

/// A woken Run that ends with no decision gives the permission to the
/// Person, on a card with the daemon's note.
#[tokio::test]
async fn a_woken_run_that_ends_without_a_decision_escalates_the_permission() {
    let agent = start_agent_mode_session(Script::default().turn(ends_after(rm_rf_build()))).await;
    let started = &agent.started;

    agent.woken().await;
    agent.think(vec![pagis_testkit::Script::reply(&[
        "I will look at it later.",
    ])]);

    let request = started.wait_for_card().await;
    assert_eq!(
        request.payload["note"],
        "The sprite ended its turn without a decision."
    );
    assert_eq!(agent.wait_for_card_message().await.len(), 1);
    assert!(started.answers().is_empty(), "the permission waits");
    let decisions = agent.decision_rows(1).await;
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["decision"], "escalated");
    assert_eq!(decisions[0]["decider"], Value::Null);
    assert_eq!(
        started.state().await,
        CodingSessionState::NeedsDecision,
        "the session waits for the Person"
    );
}

/// A Grant revision that narrows the widest mode to `person` while the
/// permission waits makes `coding_session_decide` answer `escalated`, and
/// the Person decides on the card.
#[tokio::test]
async fn a_narrower_widest_mode_during_the_wait_makes_decide_answer_escalated_and_posts_the_card() {
    let agent = start_agent_mode_session(Script::default().turn(ends_after(rm_rf_build()))).await;
    let started = &agent.started;
    let daemon = &started.daemon;

    agent.woken().await;
    assert_eq!(
        set_widest_mode(daemon, started._host.host_id(), "person").await,
        200
    );
    agent.think(vec![
        agent.call(
            "coding_session_decide",
            json!({"decision": "allow", "note": NOTE}),
        ),
        pagis_testkit::Script::reply(&["The user decides."]),
    ]);

    let request = started.wait_for_card().await;
    assert_eq!(request.payload["note"], Value::Null);
    wait_for_event_runs(daemon, RunState::Completed, 1).await;
    assert_eq!(
        error_code_of(daemon, &daemon.workspace_id, "coding_session_decide").await,
        "escalated"
    );
    assert!(started.answers().is_empty(), "the Agent did not decide");
    assert!(
        started.wait_for_facts(0).await.is_empty(),
        "no audit fact before the Person decides"
    );
}

/// Person B's Agent names the id of person A's session. The session reads
/// as absent, and nothing reaches A's harness.
#[tokio::test]
async fn person_bs_agent_that_decides_on_person_as_session_gets_session_not_found() {
    let brain = Arc::new(ScriptedBrain::default());
    let tenants = TwoTenants::on(daemon_with(&brain).await).await;
    let daemon = &tenants.daemon;
    let a_session = tenants.a_id("coding_session_id").to_string();
    brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_decide",
        json!({"session": a_session, "decision": "allow", "note": NOTE}),
    ));
    brain.push(pagis_testkit::Script::reply(&["No such session."]));
    let b_host = daemon
        .stores()
        .hosts
        .register(
            &tenants.b.workspace_id,
            "Mini",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .unwrap();
    let channel_id = person_bs_agent(&tenants).await;
    // B lets B's Agent decide on B's machine, so the Agent holds the tool.
    let b_agent = daemon
        .stores()
        .agents
        .list_by_workspace(&tenants.b.workspace_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("B's Agent");
    daemon
        .stores()
        .grants
        .create(&pagis_core::Grant {
            id: pagis_core::GrantId::generate(),
            workspace_id: tenants.b.workspace_id.clone(),
            agent_id: b_agent.id,
            resource_kind: pagis_core::Grant::HOST_KIND.to_string(),
            resource_id: Some(b_host.id.to_string()),
            scope: json!({"allow": [], "session_approval_mode": "agent"}),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        })
        .await
        .unwrap();
    let mut firehose = daemon.event_socket(&tenants.b.cookie).await;

    send(
        daemon,
        &tenants.b.cookie,
        channel_id.as_str(),
        "allow that command",
    )
    .await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(result.contains("session_not_found"), "{result}");
    assert_eq!(
        error_code_of(daemon, &tenants.b.workspace_id, "coding_session_decide").await,
        "session_not_found"
    );
}

// A question of the harness goes to the supervising Agent (ADR-0033).

fn asks_for_the_branch() -> Ask {
    Ask::form(
        "Which branch?",
        acp::ElicitationSchema::new()
            .string("branch", true)
            .boolean("push", false),
    )
}

/// The error codes of the calls of `coding_session_answer`, oldest first.
/// A call that succeeded has none.
async fn answer_codes(daemon: &TestDaemon) -> Vec<Value> {
    let mut calls: Vec<Event> = daemon
        .stores()
        .events
        .list_by_types(&daemon.workspace_id, &["tool.completed"], None, 50)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.payload["name"] == "coding_session_answer")
        .collect();
    calls.sort_by_key(|event| event.seq);
    calls
        .into_iter()
        .map(|event| event.payload["error_code"].clone())
        .collect()
}

/// In the `agent` mode a form question wakes the Agent in the session's
/// Thread with a `coding_session.needs_decision` Wake-up that holds no
/// harness text. The Agent reads the message and the form with
/// `coding_session_read`. Values that lack a required property get a tool
/// error and the question still waits. Values that match the form answer `accept` with
/// them.
#[tokio::test]
async fn in_the_agent_mode_a_form_question_wakes_the_agent_and_its_answer_accepts_the_values() {
    let agent = start_agent_mode_session(
        Script::default()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch())),
    )
    .await;
    let started = &agent.started;
    let daemon = &started.daemon;

    agent.woken().await;
    agent.think(vec![
        agent.call("coding_session_read", json!({})),
        agent.call("coding_session_answer", json!({"values": {"push": true}})),
        agent.call(
            "coding_session_answer",
            json!({"values": {"branch": "main", "push": true}}),
        ),
        pagis_testkit::Script::reply(&["I answered: main."]),
    ]);

    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(
        started.answers(),
        [json!({"action": "accept", "content": {"branch": "main", "push": true}})]
    );
    wait_for_event_runs(daemon, RunState::Completed, 1).await;
    assert_eq!(
        answer_codes(daemon).await,
        [json!("invalid_values"), Value::Null]
    );
    let record = started.record().await;
    let rows = session_envelope(&agent.decisions, &record, "coding_session.needs_decision");
    assert!(rows.contains("decision_kind: question"), "{rows}");
    assert!(
        !rows.contains("Which branch?"),
        "the event holds no harness text: {rows}"
    );
    let source = format!("[BEGIN UNTRUSTED source=coding_session:{}", record.id);
    let read = agent
        .decisions
        .requests()
        .iter()
        .flat_map(|request| request.messages.clone())
        .find(|message| message.role == TurnRole::Tool && message.text.contains(&source))
        .expect("the woken Run read the session")
        .text;
    let inside = &read[read.find(&source).unwrap()..];
    assert!(inside.contains("Which branch?"), "{read}");
    assert!(inside.contains("push"), "the read holds the form: {read}");
}

/// A woken Run that ends without an answer makes the daemon answer the
/// question `cancel`, and the transcript says that the Agent did not
/// answer.
#[tokio::test]
async fn a_woken_run_that_ends_without_an_answer_makes_the_daemon_answer_cancel() {
    let agent = start_agent_mode_session(
        Script::default()
            .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch())),
    )
    .await;
    let started = &agent.started;
    let daemon = &started.daemon;

    agent.woken().await;
    agent.think(vec![pagis_testkit::Script::reply(&[
        "I will look at it later.",
    ])]);

    started.wait_for_state(CodingSessionState::Idle).await;
    assert_eq!(started.answers(), [json!({"action": "cancel"})]);
    let answers: Vec<Value> = daemon
        .stores()
        .coding_sessions
        .list_events(&daemon.workspace_id, &started.session_id, None, 1_000)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == CodingSessionEventKind::Answer)
        .map(|row| row.payload)
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["answer"], "cancel");
    assert_eq!(
        answers[0]["note"],
        "The sprite ended its turn without an answer."
    );
}

/// Person B's Agent names the id of person A's session. The session reads
/// as absent.
#[tokio::test]
async fn person_bs_agent_that_answers_person_as_session_gets_session_not_found() {
    let brain = Arc::new(ScriptedBrain::default());
    let tenants = TwoTenants::on(daemon_with(&brain).await).await;
    let daemon = &tenants.daemon;
    let a_session = tenants.a_id("coding_session_id").to_string();
    brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_answer",
        json!({"session": a_session, "values": {"branch": "main"}}),
    ));
    brain.push(pagis_testkit::Script::reply(&["No such session."]));
    // B's machine declares a harness, so B's Agent holds the tool.
    daemon
        .stores()
        .hosts
        .register(
            &tenants.b.workspace_id,
            "Mini",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .unwrap();
    let channel_id = person_bs_agent(&tenants).await;
    let mut firehose = daemon.event_socket(&tenants.b.cookie).await;

    send(daemon, &tenants.b.cookie, channel_id.as_str(), "answer it").await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(result.contains("session_not_found"), "{result}");
    assert_eq!(
        error_code_of(daemon, &tenants.b.workspace_id, "coding_session_answer").await,
        "session_not_found"
    );
}

// A Claude Code session in the Agent's own Computer: the container is the
// sandbox, so the start has no card and the harness acts without asking
// (ADR-0033).

/// The working directory of a session in the Computer, on the Agent's own
/// volume.
const COMPUTER_DIRECTORY: &str = "/data/agent/app";

/// The Org's Anthropic key, which the harness never holds.
const ANTHROPIC_KEY: &str = "sk-ant-org-key";

fn computer_start_call(directory: &str) -> pagis_testkit::Script {
    computer_start_of("claude", directory)
}

fn computer_start_of(harness: &str, directory: &str) -> pagis_testkit::Script {
    pagis_testkit::Script::tool_call(
        &[],
        "computer_coding_session_start",
        json!({
            "harness": harness,
            "directory": directory,
            "title": "Fix the login",
            "prompt": PROMPT,
        }),
    )
}

/// The modes of the Claude Code adapter, as the Harness Catalog names
/// them.
fn claude_modes() -> Vec<(&'static str, &'static str)> {
    harness::entry("claude")
        .unwrap()
        .modes
        .iter()
        .map(|mode| (mode.id, mode.name))
        .collect()
}

/// A daemon whose Agent has a Computer, with a fake provider behind the
/// Harness Model Endpoint.
struct InComputer {
    daemon: TestDaemon,
    runtime: Arc<pagis_computer::fake::FakeComputerRuntime>,
    harness: Arc<Mutex<Option<FakeHarness>>>,
    _provider: wiremock::MockServer,
}

/// Starts a daemon that thinks with `daemon_brain` and holds the
/// Anthropic key when `with_key` is true.
async fn computer_daemon(daemon_brain: Arc<dyn Brain>, with_key: bool) -> InComputer {
    let keys: &[(&'static str, &'static str)] = if with_key {
        &[("ANTHROPIC_API_KEY", ANTHROPIC_KEY)]
    } else {
        &[]
    };
    computer_daemon_with(daemon_brain, keys).await
}

/// Starts a daemon that thinks with `daemon_brain` and holds the Org
/// keys of `keys`, as pairs of the variable and the key.
async fn computer_daemon_with(
    daemon_brain: Arc<dyn Brain>,
    keys: &[(&'static str, &'static str)],
) -> InComputer {
    let provider = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/messages/count_tokens"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 12})),
        )
        .mount(&provider)
        .await;
    let runtime = Arc::new(pagis_computer::fake::FakeComputerRuntime::with_image());
    let keys = keys.to_vec();
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: daemon_brain,
        computer: Arc::clone(&runtime) as _,
        keys: pagis_testkit::test_provider_keys(keys),
        provider_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    InComputer {
        daemon,
        runtime,
        harness: Arc::default(),
        _provider: provider,
    }
}

impl InComputer {
    /// Serves the fake harness of `script` on the server end of the next
    /// streaming exec, as `claude-agent-acp` does in the container.
    fn serve_harness(&self, script: Script) {
        let runtime = Arc::clone(&self.runtime);
        let harness = Arc::clone(&self.harness);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + WAIT;
            let end = loop {
                if let Some(end) = runtime.take_server_end() {
                    break end;
                }
                if tokio::time::Instant::now() > deadline {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let (read, write) = tokio::io::split(end);
            *harness.lock().unwrap() = Some(FakeHarness::serve(
                script,
                write.compat_write(),
                read.compat(),
            ));
        });
    }

    fn harness(&self) -> FakeHarness {
        self.harness
            .lock()
            .unwrap()
            .clone()
            .expect("the harness runs")
    }

    /// Sends `text` as the Person, and answers the tool result that the
    /// model read once the Run completes.
    async fn tool_result_after(&self, brain: &ScriptedBrain, text: &str) -> String {
        let mut firehose = self.daemon.event_socket(self.daemon.cookie()).await;
        send(
            &self.daemon,
            self.daemon.cookie(),
            &self.daemon.dm_channel_id,
            text,
        )
        .await;
        the_tool_result(&mut firehose, brain).await
    }

    /// The token of the session from the environment of its exec.
    fn token(&self) -> String {
        let streams = self.runtime.exec_streams();
        streams[0]
            .env
            .iter()
            .find_map(|entry| entry.strip_prefix("ANTHROPIC_AUTH_TOKEN="))
            .expect("the harness holds a token")
            .to_string()
    }

    /// The status of a token count that `token` sends to the Harness
    /// Model Endpoint, as Claude Code sends it.
    async fn count_tokens(&self, token: &str) -> u16 {
        reqwest::Client::new()
            .post(format!(
                "http://{}/anthropic/v1/messages/count_tokens",
                self.daemon.model_addr
            ))
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body(r#"{"model":"claude-sonnet-4-5","messages":[{"role":"user","content":"hi"}]}"#)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    async fn pending_requests(&self) -> Vec<Request> {
        self.daemon
            .stores()
            .requests
            .list_by_state(&self.daemon.workspace_id, RequestState::Pending, None)
            .await
            .unwrap()
    }
}

/// The session id in the JSON tool result of a start.
fn started_session(result: &str) -> CodingSessionId {
    let result: Value = serde_json::from_str(result)
        .unwrap_or_else(|error| panic!("the tool result is JSON ({error}): {result}"));
    CodingSessionId::from(
        result["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("a started session: {result}"))
            .to_string(),
    )
}

/// The whole path in the Computer: the Agent starts Claude Code with no
/// card, the harness runs as `agent` in the directory with a token of the
/// session and no provider key, it is in `bypassPermissions` before the
/// first prompt, and its tool call asks nobody. The token reaches the
/// Harness Model Endpoint while the session is open and stops at its
/// close.
#[tokio::test]
async fn an_agent_starts_claude_code_in_its_computer_where_the_harness_acts_without_asking() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_call(COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let computer = computer_daemon(Arc::new(TestRunsOnly(Arc::clone(&brain))), true).await;
    let daemon = &computer.daemon;
    computer.serve_harness(
        Script::default()
            .modes("default", &claude_modes())
            .turn(Turn::new(
                vec![acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new("call-1", "Run the tests")
                        .kind(acp::ToolKind::Execute)
                        .status(acp::ToolCallStatus::Completed),
                )],
                acp::StopReason::EndTurn,
            )),
    );

    let result = computer
        .tool_result_after(&brain, "fix the login in your computer")
        .await;

    let session_id = started_session(&result);
    wait_for_session_state(daemon, &session_id, CodingSessionState::Idle).await;
    let session = get(daemon, &format!("/api/v1/coding-sessions/{session_id}")).await;
    assert_eq!(session["place"], "computer");
    assert_eq!(session["harness_mode"], "bypassPermissions");
    assert_eq!(session["unattended"], true);
    let harness = computer.harness();
    assert_eq!(
        harness.params("session/set_mode")[0]["modeId"],
        "bypassPermissions"
    );
    let methods: Vec<_> = harness
        .received()
        .into_iter()
        .map(|received| received.method)
        .filter(|method| method == "session/set_mode" || method == "session/prompt")
        .collect();
    assert_eq!(methods, ["session/set_mode", "session/prompt"]);
    assert!(harness.answers().is_empty(), "the harness asked nothing");
    assert!(computer.pending_requests().await.is_empty());
    let streams = computer.runtime.exec_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].argv, ["claude-agent-acp"]);
    assert_eq!(streams[0].user, "agent");
    assert_eq!(streams[0].cwd, COMPUTER_DIRECTORY);
    assert!(
        !streams[0]
            .env
            .iter()
            .any(|entry| entry.contains(ANTHROPIC_KEY)),
        "{:?}",
        streams[0].env
    );
    let token = computer.token();
    assert_eq!(computer.count_tokens(&token).await, 200);

    brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_close",
        json!({"session": session_id.as_str()}),
    ));
    brain.push(pagis_testkit::Script::reply(&["Closed."]));
    computer.tool_result_after(&brain, "close it").await;
    wait_for_session_state(daemon, &session_id, CodingSessionState::Closed).await;
    assert_eq!(computer.count_tokens(&token).await, 401);
}

/// A permission that Claude Code still asks in `bypassPermissions` goes
/// to the supervising Agent: the session waits for its decision, and its
/// allow answers the harness once, with no card.
#[tokio::test]
async fn a_permission_that_claude_code_asks_in_the_computer_waits_for_the_agent() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_call(COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let decisions = Arc::new(ScriptedBrain::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let (gate, opened) = tokio::sync::watch::channel(false);
    let computer = computer_daemon(
        Arc::new(DecisionWakeups {
            test: Arc::clone(&brain),
            decisions: Arc::clone(&decisions),
            entered: Arc::clone(&entered),
            gate: opened,
        }),
        true,
    )
    .await;
    let daemon = &computer.daemon;
    computer.serve_harness(
        Script::default()
            .modes("default", &claude_modes())
            .turn(ends_after(rm_rf_build())),
    );

    let result = computer
        .tool_result_after(&brain, "clean the build in your computer")
        .await;

    let session_id = started_session(&result);
    wait_for_session_state(daemon, &session_id, CodingSessionState::NeedsDecision).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .expect("the decision wakes the Agent");
    decisions.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_decide",
        json!({"session": session_id.as_str(), "decision": "allow", "note": NOTE}),
    ));
    decisions.push(pagis_testkit::Script::reply(&["I allowed it."]));
    gate.send_replace(true);
    wait_for_session_state(daemon, &session_id, CodingSessionState::Idle).await;
    let answers: Vec<Value> = computer
        .harness()
        .answers()
        .into_iter()
        .map(|answer| answer.expect("an answer and no error"))
        .collect();
    assert_eq!(answers, [allow_once()]);
    assert!(computer.pending_requests().await.is_empty());
}

/// Claude Code in the Computer spends the installation's Anthropic key.
/// With no key, the start is a tool error that names the cause, and no
/// exec starts.
#[tokio::test]
async fn with_no_anthropic_key_the_start_in_the_computer_is_a_tool_error_and_no_exec_starts() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_call(COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["No key."]));
    let computer = computer_daemon(Arc::new(TestRunsOnly(Arc::clone(&brain))), false).await;

    let result = computer.tool_result_after(&brain, "fix the login").await;

    assert!(result.contains("no_provider_key"), "{result}");
    assert!(result.contains("Anthropic key"), "{result}");
    assert!(computer.runtime.exec_streams().is_empty());
    assert!(computer.runtime.execs().is_empty());
}

/// The working directory of a session in the Computer is on the Agent's
/// own volume.
#[tokio::test]
async fn a_directory_outside_the_agents_home_is_a_tool_error() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_call("/etc"));
    brain.push(pagis_testkit::Script::reply(&["Bad directory."]));
    let computer = computer_daemon(Arc::new(TestRunsOnly(Arc::clone(&brain))), true).await;

    let result = computer.tool_result_after(&brain, "fix the login").await;

    assert!(result.contains("bad_directory"), "{result}");
    assert!(result.contains("/data/agent"), "{result}");
    assert!(computer.runtime.exec_streams().is_empty());
}

// Codex, OpenCode and pi in the Agent's own Computer: each takes the first
// route of the Harness Model Endpoint whose provider holds an Org key, and
// its own configuration points it there (ADR-0033).

const OPENAI_KEY: &str = "sk-openai-org-key";
const OPENROUTER_KEY: &str = "sk-or-org-key";

/// The modes of a harness, as the Harness Catalog names them.
fn catalog_modes(harness_id: &str) -> Vec<(&'static str, &'static str)> {
    harness::entry(harness_id)
        .unwrap()
        .modes
        .iter()
        .map(|mode| (mode.id, mode.name))
        .collect()
}

/// A turn that ends at once.
fn quiet_turn() -> Turn {
    Turn::new(Vec::new(), acp::StopReason::EndTurn)
}

impl InComputer {
    /// The one exec of a harness so far.
    fn harness_exec(&self) -> pagis_computer::ExecRequest {
        let streams = self.runtime.exec_streams();
        assert_eq!(streams.len(), 1, "{streams:?}");
        streams[0].clone()
    }

    /// The value of `name` in the environment of the harness's exec.
    fn exec_variable(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        self.harness_exec()
            .env
            .iter()
            .find_map(|entry| entry.strip_prefix(&prefix).map(str::to_string))
    }

    /// The files that one shell command wrote into `directory` as uid
    /// `agent`, from the tar on its stdin, by name.
    fn uploaded(&self, directory: &str) -> std::collections::BTreeMap<String, String> {
        let command = format!("mkdir -p '{directory}' && tar -x -C '{directory}'");
        let writes: Vec<pagis_computer::ExecRequest> = self
            .runtime
            .execs()
            .into_iter()
            .filter(|exec| exec.argv.last() == Some(&command))
            .collect();
        assert_eq!(writes.len(), 1, "the writes into {directory}");
        assert_eq!(writes[0].user, "agent");
        let tar = writes[0].stdin.clone().expect("the tar on stdin");
        let mut archive = tar::Archive::new(tar.as_slice());
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().into_owned();
                let mut contents = String::new();
                std::io::Read::read_to_string(&mut entry, &mut contents).unwrap();
                (name, contents)
            })
            .collect()
    }

    /// Whether a shell command wrote a configuration directory.
    fn wrote_a_configuration(&self) -> bool {
        self.runtime.execs().iter().any(|exec| {
            exec.argv
                .last()
                .is_some_and(|command| command.contains("tar -x"))
        })
    }

    /// The base URL of the Harness Model Endpoint as the Computer reaches
    /// it.
    fn endpoint(&self) -> String {
        pagis_computer::model_endpoint(self.daemon.model_addr.port())
    }

    /// The session token of the exec. The environment holds no Org key
    /// of `keys`, and no file of `files` holds the token.
    fn token_only_in_the_environment(
        &self,
        keys: &[&str],
        files: &std::collections::BTreeMap<String, String>,
    ) -> String {
        let exec = self.harness_exec();
        for key in keys {
            assert!(
                !exec.env.iter().any(|entry| entry.contains(key)),
                "{:?}",
                exec.env
            );
        }
        let token = self
            .exec_variable("PAGIS_MODEL_TOKEN")
            .expect("the exec holds the token in PAGIS_MODEL_TOKEN");
        assert!(!token.is_empty());
        for (name, contents) in files {
            assert!(!contents.contains(&token), "{name} holds the token");
        }
        token
    }
}

/// Codex in the Computer with only the Org's OpenAI key: the daemon writes
/// the provider `pagis` at `/openai/v1` into `config.toml` in the
/// session's own `CODEX_HOME`, starts `codex-acp` there with the token in
/// `PAGIS_MODEL_TOKEN`, and puts the harness in `agent-full-access` before
/// the first prompt.
#[tokio::test]
async fn an_agent_starts_codex_in_its_computer_on_the_openai_route_in_full_access() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_of("codex", COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let computer = computer_daemon_with(
        Arc::new(TestRunsOnly(Arc::clone(&brain))),
        &[("OPENAI_API_KEY", OPENAI_KEY)],
    )
    .await;
    computer.serve_harness(
        Script::default()
            .modes("read-only", &catalog_modes("codex"))
            .turn(quiet_turn()),
    );

    let result = computer
        .tool_result_after(&brain, "fix the login with codex")
        .await;

    let session_id = started_session(&result);
    wait_for_session_state(&computer.daemon, &session_id, CodingSessionState::Idle).await;
    let harness = computer.harness();
    let methods: Vec<_> = harness
        .received()
        .into_iter()
        .map(|received| received.method)
        .filter(|method| method == "session/set_mode" || method == "session/prompt")
        .collect();
    assert_eq!(methods, ["session/set_mode", "session/prompt"]);
    assert_eq!(
        harness.params("session/set_mode")[0]["modeId"],
        "agent-full-access"
    );
    let exec = computer.harness_exec();
    assert_eq!(exec.argv, ["codex-acp"]);
    assert_eq!(exec.user, "agent");
    assert_eq!(exec.cwd, COMPUTER_DIRECTORY);
    let codex_home = format!("/data/agent/.pagis/coding/{session_id}/codex");
    assert_eq!(
        computer.exec_variable("CODEX_HOME"),
        Some(codex_home.clone())
    );
    let files = computer.uploaded(&codex_home);
    computer.token_only_in_the_environment(&[OPENAI_KEY], &files);
    let config: toml::Table = toml::from_str(&files["config.toml"]).unwrap();
    assert_eq!(config["model_provider"].as_str(), Some("pagis"));
    let provider = &config["model_providers"]["pagis"];
    assert_eq!(
        provider["base_url"].as_str(),
        Some(format!("{}/openai/v1", computer.endpoint()).as_str())
    );
    assert_eq!(provider["env_key"].as_str(), Some("PAGIS_MODEL_TOKEN"));
    assert_eq!(provider["wire_api"].as_str(), Some("responses"));
}

/// OpenCode in the Computer with only the Org's OpenRouter key: its inline
/// configuration gives its own `openrouter` provider the endpoint's
/// `/openrouter/v1` and the key from `PAGIS_MODEL_TOKEN`, and enables no
/// other provider. OpenCode lists no mode, so the start sets none.
#[tokio::test]
async fn an_agent_starts_opencode_in_its_computer_on_the_openrouter_route() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_of("opencode", COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let computer = computer_daemon_with(
        Arc::new(TestRunsOnly(Arc::clone(&brain))),
        &[("OPENROUTER_API_KEY", OPENROUTER_KEY)],
    )
    .await;
    computer.serve_harness(Script::default().turn(quiet_turn()));

    let result = computer
        .tool_result_after(&brain, "fix the login with opencode")
        .await;

    let session_id = started_session(&result);
    wait_for_session_state(&computer.daemon, &session_id, CodingSessionState::Idle).await;
    assert!(computer.harness().params("session/set_mode").is_empty());
    let exec = computer.harness_exec();
    assert_eq!(exec.argv, ["opencode", "acp"]);
    assert_eq!(exec.user, "agent");
    assert_eq!(exec.cwd, COMPUTER_DIRECTORY);
    assert!(!computer.wrote_a_configuration());
    let content = computer
        .exec_variable("OPENCODE_CONFIG_CONTENT")
        .expect("the inline configuration");
    let token = computer.token_only_in_the_environment(
        &[OPENROUTER_KEY],
        &[("OPENCODE_CONFIG_CONTENT".to_string(), content.clone())].into(),
    );
    assert!(!content.contains(&token));
    let config: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(config["enabled_providers"], json!(["openrouter"]));
    assert_eq!(
        config["provider"],
        json!({"openrouter": {"options": {
            "baseURL": format!("{}/openrouter/v1", computer.endpoint()),
            "apiKey": "{env:PAGIS_MODEL_TOKEN}",
        }}})
    );
}

/// pi in the Computer with only the Org's Anthropic key: the daemon writes
/// one provider `pagis` at the endpoint's `/anthropic` with the first
/// Anthropic candidate of the Agent's model alias into `models.json`, and
/// that model as the default into `settings.json`, in the session's own
/// `PI_CODING_AGENT_DIR`. pi never asks, so the start sets no mode.
#[tokio::test]
async fn an_agent_starts_pi_in_its_computer_on_the_anthropic_route_with_its_own_model() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_of("pi", COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let computer = computer_daemon_with(
        Arc::new(TestRunsOnly(Arc::clone(&brain))),
        &[("ANTHROPIC_API_KEY", ANTHROPIC_KEY)],
    )
    .await;
    let daemon = &computer.daemon;
    let alias = daemon
        .stores()
        .agents
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .model_alias;
    daemon
        .stores()
        .model_aliases
        .update_candidates(
            &daemon.workspace_id,
            &alias,
            &[
                "openai/gpt-6-luna".to_string(),
                "anthropic/claude-sonnet-5-5".to_string(),
                "anthropic/claude-opus-5".to_string(),
            ],
            now_ms(),
        )
        .await
        .unwrap();
    computer.serve_harness(Script::default().turn(quiet_turn()));

    let result = computer
        .tool_result_after(&brain, "fix the login with pi")
        .await;

    let session_id = started_session(&result);
    wait_for_session_state(daemon, &session_id, CodingSessionState::Idle).await;
    assert!(computer.harness().params("session/set_mode").is_empty());
    let exec = computer.harness_exec();
    assert_eq!(exec.argv, ["pi-acp"]);
    assert_eq!(exec.user, "agent");
    assert_eq!(exec.cwd, COMPUTER_DIRECTORY);
    let agent_dir = format!("/data/agent/.pagis/coding/{session_id}/pi");
    assert_eq!(
        computer.exec_variable("PI_CODING_AGENT_DIR"),
        Some(agent_dir.clone())
    );
    let files = computer.uploaded(&agent_dir);
    let token = computer.token_only_in_the_environment(&[ANTHROPIC_KEY], &files);
    let models: Value = serde_json::from_str(&files["models.json"]).unwrap();
    assert_eq!(
        models,
        json!({"providers": {"pagis": {
            "baseUrl": format!("{}/anthropic", computer.endpoint()),
            "api": "anthropic-messages",
            "apiKey": "$PAGIS_MODEL_TOKEN",
            "models": [{"id": "claude-sonnet-5-5"}],
        }}})
    );
    let settings: Value = serde_json::from_str(&files["settings.json"]).unwrap();
    assert_eq!(
        settings,
        json!({"defaultProvider": "pagis", "defaultModel": "claude-sonnet-5-5"})
    );
    // pi sends the token as `x-api-key`, and the endpoint takes it.
    let status = reqwest::Client::new()
        .post(format!(
            "http://{}/anthropic/v1/messages/count_tokens",
            daemon.model_addr
        ))
        .header("x-api-key", &token)
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-sonnet-5-5","messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200);
}

/// Codex spends only the Org's OpenAI or OpenRouter key. With only an
/// Anthropic key, no route of Codex has a key: the start is a tool error
/// that names the cause, and no exec starts.
#[tokio::test]
async fn with_no_key_on_any_route_of_codex_the_start_is_a_tool_error_and_no_exec_starts() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(computer_start_of("codex", COMPUTER_DIRECTORY));
    brain.push(pagis_testkit::Script::reply(&["No key."]));
    let computer = computer_daemon_with(
        Arc::new(TestRunsOnly(Arc::clone(&brain))),
        &[("ANTHROPIC_API_KEY", ANTHROPIC_KEY)],
    )
    .await;

    let result = computer.tool_result_after(&brain, "fix the login").await;

    assert!(result.contains("no_provider_key"), "{result}");
    assert!(result.contains("OpenAI or OpenRouter key"), "{result}");
    assert!(computer.runtime.exec_streams().is_empty());
    assert!(computer.runtime.execs().is_empty());
}
