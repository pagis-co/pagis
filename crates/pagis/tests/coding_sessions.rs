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
use pagis_coding::fake::{Ask, FakeHarness, Script, Turn, acp, serve_client_app};
use pagis_coding::{CloseReason, NewCodingSession, PERMISSION_DECIDED_EVENT, PromptOutcome};
use pagis_core::{
    AgentId, AuthorKind, Block, ChannelId, CodingSession, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, Event, EventSource, HostId, Message, MessageId, NewCodingSessionEvent, Run,
    RunId, RunOrigin, RunState, SessionApprovalMode, TriggerKind, WakeupState, harness, now_ms,
};
use pagis_testkit::{
    HostAnswer, HostClient, ScriptedBrain, SessionClient, Socket, TestDaemon, TestDaemonOptions,
    TwoTenants, fixture,
};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_util::compat::TokioAsyncReadCompatExt;

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
            host_id,
            harness_id: "claude".to_string(),
            directory: DIRECTORY.to_string(),
            worktree: None,
            approval_mode: SessionApprovalMode::Person,
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
    brain.push(start_call(None));
    brain.push(pagis_testkit::Script::reply(&["Started."]));
    let daemon = daemon_for_test_runs(&brain).await;
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
/// `rm -rf /` waits until a cancel answers it `cancelled`. Each decision
/// writes one audit fact.
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
    assert_eq!(facts[2].payload["outcome"], "cancelled");
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
