//! Full-daemon tests of the Coding Session routes and events (ADR-0033).
//!
//! The REST routes read the records and the transcripts that the stores
//! hold. The stop and the events run the daemon's own session runtime
//! over a session socket whose Client App end runs the fake harness.

use std::time::Duration;

use futures::StreamExt;
use pagis_coding::NewCodingSession;
use pagis_coding::fake::{Script, Turn, serve_client_app};
use pagis_core::{
    AgentId, AuthorKind, Block, ChannelId, CodingSession, CodingSessionEventKind,
    CodingSessionState, HostId, Message, MessageId, NewCodingSessionEvent, RunId,
    SessionApprovalMode, now_ms,
};
use pagis_testkit::{Socket, TestDaemon, fixture};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_util::compat::TokioAsyncReadCompatExt;

const WAIT: Duration = Duration::from_secs(10);
const DIRECTORY: &str = "/Users/bo/code/app";

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
