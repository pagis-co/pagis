//! Full-daemon tests of Coding Sessions (ADR-0033).
//!
//! An Agent calls `coding_session_start`, the Person approves the card,
//! and the daemon starts the harness on the Person's machine over its
//! session socket. The REST routes read the records and the transcripts
//! that the stores hold. The Client App's end of the socket runs the fake
//! Coding Harness of `pagis_coding` on each stream in place of the
//! process: through `serve_client_app`, or through the fake Client App of
//! `pagis_broker` when a test reads the open requests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use pagis_broker::fake::FakeClientApp;
use pagis_coding::NewCodingSession;
use pagis_coding::fake::{FakeHarness, Script, Turn, serve_client_app};
use pagis_core::{
    AgentId, AuthorKind, Block, ChannelId, CodingSession, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, HostId, Message, MessageId, NewCodingSessionEvent, RunId,
    SessionApprovalMode, harness, now_ms,
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
    let host_id = host(daemon).await;
    open_session_socket(
        daemon,
        &host_id,
        Script::default().turn(Turn::until_cancel(vec![])),
    )
    .await;
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
    let mut arguments = json!({
        "harness": "claude",
        "directory": DIRECTORY,
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
    brain
        .requests()
        .last()
        .expect("the model read the tool result")
        .messages
        .last()
        .expect("the tool result")
        .text
        .clone()
}

async fn send(daemon: &TestDaemon, cookie: &str, channel_id: &str, text: &str) {
    let sent = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&json!({"pending_id": "p-1", "text": text}))
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
    let daemon = daemon_with(&brain).await;
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
    // Person B's Agent, in a DM with B, on the model aliases of B's
    // Workspace.
    let stores = daemon.stores();
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
    let mut firehose = daemon.event_socket(&tenants.b.cookie).await;

    send(
        daemon,
        &tenants.b.cookie,
        channel.id.as_str(),
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
