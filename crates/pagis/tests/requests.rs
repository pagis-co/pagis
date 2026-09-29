//! Full-daemon Request tests: a scripted host_shell tool
//! call produces a `tool_action` row and card, the decision REST
//! endpoint wakes the parked run (approve executes, deny injects the
//! denial), a second decision is 409, and a restart fails the parked
//! run with the agent notified on the next trigger.
//!
//! A `form` row round-trips its submitted values, and values the row's
//! own field schema refuses are a 422 that leaves the run parked.
//! `ask_user` mints that row from an agent's own call: the block
//! that views it posts into the run's channel, the run parks, and the
//! submitted values come back as the tool result.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_testkit::{HostAnswer, HostClient, Script, ScriptedBrain, TestDaemon, TestDaemonOptions};

/// A local installation with the Pagis client running: the one machine a
/// host command runs on here. A host action runs on a present
/// client and never in the daemon, so every test that drives `host_shell`
/// holds one of these.
async fn connected_host(daemon: &TestDaemon) -> HostClient {
    HostClient::connect(daemon, "Air", "macos", &["shell"], HostAnswer::Echo).await
}
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
}

/// A brain scripted to request one host command in its first turn.
fn brain_requesting(command: &str) -> Arc<ScriptedBrain> {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": command }),
    ));
    brain
}

async fn send_json(socket: &mut Socket, frame: serde_json::Value) {
    socket
        .send(Message::text(frame.to_string()))
        .await
        .expect("ws send");
}

/// The next text frame as JSON, within a timeout.
async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("frame is JSON");
        }
    }
}

/// The next frame of one type, skipping others.
async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// Authenticate and consume `ready`. The firehose needs no channel
/// subscription.
async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    send_json(&mut socket, serde_json::json!({ "type": "auth" })).await;
    assert_eq!(next_frame(&mut socket).await["type"], "ready");
    socket
}

async fn rest_send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

async fn timeline(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}

async fn get_request(daemon: &TestDaemon, request_id: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}/api/v1/requests/{request_id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn decide(daemon: &TestDaemon, request_id: &str, decision: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "decision": decision }))
        .send()
        .await
        .unwrap()
}

/// Trigger the scripted host_shell call and read the request id from
/// the card once the run parks. Returns (request_id, card message).
async fn park_run(
    daemon: &TestDaemon,
    socket: &mut Socket,
    text: &str,
) -> (String, serde_json::Value) {
    rest_send(daemon, "p-park", text).await;
    let requested = next_frame_of(socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let card_event = next_frame_of(socket, "message.completed").await;
    loop {
        let state_changed = next_frame_of(socket, "run.state_changed").await;
        if state_changed["payload"]["payload"]["to"] == "waiting_for_approval" {
            break;
        }
    }
    (request_id, card_event)
}

#[tokio::test]
async fn approve_runs_the_command_and_the_model_continues() {
    let brain = brain_requesting("echo approved-path");
    brain.push(Script::reply(&["It printed."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let (request_id, _) = park_run(&daemon, &mut socket, "run echo for me").await;

    // The card block references the request; state lives in the row.
    let items = timeline(&daemon).await;
    let card = items
        .iter()
        .find(|m| m["blocks"][0]["type"] == "approval_card")
        .expect("card message in the timeline");
    assert_eq!(card["blocks"][0]["request_id"], request_id.as_str());
    assert_eq!(card["blocks"][0]["body"], "echo approved-path");
    assert!(
        card["blocks"][0]["state"].is_null(),
        "state is never in the block"
    );
    assert_eq!(get_request(&daemon, &request_id).await["state"], "pending");

    let response = decide(&daemon, &request_id, "approved").await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], "approved");

    // The run resumes, executes, and the second turn completes.
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let items = timeline(&daemon).await;
    assert_eq!(items[0]["text_content"], "It printed.");
    // The tool turn and the resumed reply are the only model requests.
    let requests = brain.requests();
    assert_eq!(requests.len(), 2);
    let result = requests[1].messages.last().unwrap();
    assert!(result.text.contains("approved-path"), "{}", result.text);
    assert_eq!(get_request(&daemon, &request_id).await["state"], "approved");
}

#[tokio::test]
async fn deny_returns_the_denial_to_the_model() {
    let brain = brain_requesting("rm -rf /tmp/everything");
    brain.push(Script::reply(&["Okay, I will not."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let (request_id, _) = park_run(&daemon, &mut socket, "clean up").await;

    let response = decide(&daemon, &request_id, "denied").await;
    assert_eq!(response.status(), 200);

    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = brain.requests();
    assert_eq!(
        requests[1].messages.last().unwrap().text,
        "user denied this action"
    );
    assert_eq!(
        timeline(&daemon).await[0]["text_content"],
        "Okay, I will not."
    );
    assert_eq!(get_request(&daemon, &request_id).await["state"], "denied");
}

#[tokio::test]
async fn deciding_twice_conflicts_with_409() {
    let brain = brain_requesting("echo once");
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let (request_id, _) = park_run(&daemon, &mut socket, "run it").await;

    assert_eq!(decide(&daemon, &request_id, "approved").await.status(), 200);

    let second = decide(&daemon, &request_id, "denied").await;
    assert_eq!(second.status(), 409);
    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(body["error"]["code"], "conflict");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("approved"),
        "the conflict names the current state"
    );
}

#[tokio::test]
async fn bad_decisions_are_rejected() {
    let daemon = TestDaemon::start().await;

    // An unknown request is 404.
    let missing = decide(&daemon, "01UNKNOWNREQUEST", "approved").await;
    assert_eq!(missing.status(), 404);

    // `always` widens the grant, so it never pairs with a denial.
    let brain = brain_requesting("echo hi");
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;
    let (request_id, _) = park_run(&daemon, &mut socket, "run it").await;
    let always_denied = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "decision": "denied", "scope": "always" }))
        .send()
        .await
        .unwrap();
    assert_eq!(always_denied.status(), 422);
    let bad = decide(&daemon, &request_id, "maybe").await;
    assert_eq!(bad.status(), 422);
    assert_eq!(
        get_request(&daemon, &request_id).await["state"],
        "pending",
        "rejected decisions leave the request pending"
    );
}

#[tokio::test]
async fn a_restart_while_parked_fails_the_run_and_notifies_on_the_next_trigger() {
    let brain = brain_requesting("echo never-runs");
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let (request_id, card_event) = park_run(&daemon, &mut socket, "run it").await;
    let run_id = card_event["payload"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    drop(socket);

    let next_brain = Arc::new(ScriptedBrain::default());
    next_brain.push(Script::reply(&["Sorry about that."]));
    let daemon = daemon.restart(options(&next_brain)).await;

    // The parked run failed and its request expired.
    use pagis_core::{RunId, RunState, RunStore};
    let run = pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &RunId::from(run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.as_deref(), Some(pagis_agent::RESTART_ERROR));
    assert_eq!(get_request(&daemon, &request_id).await["state"], "expired");

    // An expired request is not decidable.
    assert_eq!(decide(&daemon, &request_id, "approved").await.status(), 409);

    // The restart note is in the timeline, and the next trigger's
    // context carries it to the agent.
    let items = timeline(&daemon).await;
    let note = items
        .iter()
        .find(|m| m["author_kind"] == "system")
        .expect("system restart note");
    assert!(note["text_content"].as_str().unwrap().contains("restart"));

    let mut socket = firehose(&daemon).await;
    rest_send(&daemon, "p-after", "did that run?").await;
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = next_brain.requests();
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|m| m.text.contains("restart")),
        "the next trigger's context carries the restart note"
    );
}

#[tokio::test]
async fn a_message_instead_of_a_decision_continues_the_conversation() {
    let brain = brain_requesting("echo never-runs");
    brain.push(Script::reply(&["Okay, the time is now."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let (request_id, _) = park_run(&daemon, &mut socket, "run it").await;

    // The user replies to the card instead of deciding.
    rest_send(&daemon, "p-reply", "forget that, tell me the time").await;

    let superseded = next_frame_of(&mut socket, "request.superseded").await;
    assert_eq!(
        superseded["payload"]["payload"]["request_id"],
        request_id.as_str()
    );
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }

    // The request is settled and no longer decidable.
    assert_eq!(
        get_request(&daemon, &request_id).await["state"],
        "superseded"
    );
    assert_eq!(decide(&daemon, &request_id, "approved").await.status(), 409);

    // The run answered the message; the command never ran.
    let requests = brain.requests();
    let transcript = &requests[1].messages;
    assert!(
        transcript[transcript.len() - 2]
            .text
            .contains("sent a message instead"),
    );
    assert_eq!(
        transcript.last().unwrap().text,
        "User: forget that, tell me the time"
    );
    assert_eq!(
        timeline(&daemon).await[0]["text_content"],
        "Okay, the time is now."
    );
}

/// Write a second `form` row on the parked run, the way
/// `ask_user` writes one. It covers the decision path on its own,
/// without a second scripted run.
async fn seed_form(daemon: &TestDaemon, run_id: &str) -> String {
    use pagis_core::{AgentId, AgentStore, RequestStore, RunId, WorkspaceId};

    let agents = pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone());
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let workspace_id: WorkspaceId = agents
        .get(&daemon.workspace_id, &agent_id)
        .await
        .unwrap()
        .unwrap()
        .workspace_id
        .clone();
    let request = pagis_testkit::fixture::pending_form_request(
        &workspace_id,
        &agent_id,
        &RunId::from(run_id.to_string()),
    );
    pagis_storage_sqlite::SqliteRequestStore::new(daemon.pool().clone())
        .create(&request)
        .await
        .unwrap();
    request.id.to_string()
}

async fn submit(
    daemon: &TestDaemon,
    request_id: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn list_requests(daemon: &TestDaemon, query: &str) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!("{}/api/v1/requests?{query}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}

async fn run_state(daemon: &TestDaemon, run_id: &str) -> String {
    use pagis_core::{RunId, RunStore};
    pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &RunId::from(run_id.to_string()))
        .await
        .unwrap()
        .unwrap()
        .state
        .as_str()
        .to_string()
}

#[tokio::test]
async fn a_form_submission_round_trips_its_values() {
    let brain = brain_requesting("echo parked");
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;
    let (_, card_event) = park_run(&daemon, &mut socket, "run it").await;
    let run_id = card_event["payload"]["run_id"].as_str().unwrap();
    let form_id = seed_form(&daemon, run_id).await;

    let values = serde_json::json!({"who": "Ada", "room": "b"});
    let response = submit(
        &daemon,
        &form_id,
        serde_json::json!({"decision": "approved", "values": values}),
    )
    .await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], "approved");
    assert_eq!(body["values"], values);
    let row = get_request(&daemon, &form_id).await;
    assert_eq!(row["kind"], "form");
    assert_eq!(row["values"], values);
    // The field schema stays on the row; the block never carries state.
    assert_eq!(row["payload"]["fields"][0]["key"], "who");
}

#[tokio::test]
async fn values_the_row_schema_refuses_are_422_and_leave_the_run_parked() {
    let brain = brain_requesting("echo parked");
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;
    let (_, card_event) = park_run(&daemon, &mut socket, "run it").await;
    let run_id = card_event["payload"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let form_id = seed_form(&daemon, &run_id).await;

    // A select value outside the row's own options. A client that
    // posts back a mutated block is checked against the row.
    let mutated = submit(
        &daemon,
        &form_id,
        serde_json::json!({"decision": "approved", "values": {"who": "Ada", "room": "z"}}),
    )
    .await;
    assert_eq!(mutated.status(), 422);
    let body: serde_json::Value = mutated.json().await.unwrap();
    assert_eq!(body["error"]["code"], "validation");

    // A missing required field is the same answer.
    let incomplete = submit(
        &daemon,
        &form_id,
        serde_json::json!({"decision": "approved", "values": {"room": "a"}}),
    )
    .await;
    assert_eq!(incomplete.status(), 422);

    assert_eq!(get_request(&daemon, &form_id).await["state"], "pending");
    assert_eq!(run_state(&daemon, &run_id).await, "waiting_for_approval");

    // A dismissal takes no values, and a decided row keeps none.
    assert_eq!(
        submit(
            &daemon,
            &form_id,
            serde_json::json!({"decision": "denied", "values": {"who": "Ada"}})
        )
        .await
        .status(),
        422
    );
    assert_eq!(decide(&daemon, &form_id, "denied").await.status(), 200);
    assert!(get_request(&daemon, &form_id).await["values"].is_null());
}

#[tokio::test]
async fn the_pending_listing_narrows_by_state_and_kind() {
    let brain = brain_requesting("echo parked");
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;
    let (action_id, card_event) = park_run(&daemon, &mut socket, "run it").await;
    let run_id = card_event["payload"]["run_id"].as_str().unwrap();
    let form_id = seed_form(&daemon, run_id).await;

    let pending = list_requests(&daemon, "state=pending").await;
    let ids: Vec<&str> = pending.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&action_id.as_str()) && ids.contains(&form_id.as_str()));

    let forms = list_requests(&daemon, "state=pending&kind=form").await;
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0]["id"], form_id.as_str());

    // The default state is pending, and a decided row leaves it.
    assert_eq!(decide(&daemon, &action_id, "denied").await.status(), 200);
    let still_pending = list_requests(&daemon, "").await;
    assert_eq!(still_pending.len(), 1);
    assert_eq!(still_pending[0]["id"], form_id.as_str());
    assert_eq!(list_requests(&daemon, "state=denied").await.len(), 1);

    // An unreadable state is a 422, not an empty page.
    let bad = reqwest::Client::new()
        .get(format!("{}/api/v1/requests?state=maybe", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
}

/// End to end: the agent asks, the user answers through the
/// decision endpoint, and the answer is the tool result.
#[tokio::test]
async fn ask_user_posts_a_form_and_returns_the_answer_to_the_run() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "ask_user",
        serde_json::json!({
            "title": "Which room?",
            "fields": [{
                "key": "room", "label": "Room", "kind": "select", "required": true,
                "options": [{"value": "a", "label": "Room A"}, {"value": "b", "label": "Room B"}]
            }],
            "submit_label": "Book"
        }),
    ));
    brain.push(Script::reply(&["Booked Room B."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    rest_send(&daemon, "p-ask", "book a room").await;
    let requested = next_frame_of(&mut socket, "request.created").await;
    assert_eq!(requested["payload"]["payload"]["kind"], "form");
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    loop {
        let state_changed = next_frame_of(&mut socket, "run.state_changed").await;
        if state_changed["payload"]["payload"]["to"] == "waiting_for_approval" {
            break;
        }
    }

    // The form block views the row and carries no state of its own.
    let form = timeline(&daemon)
        .await
        .into_iter()
        .find(|m| m["blocks"][0]["type"] == "form")
        .expect("the question posted into the run's channel");
    assert_eq!(form["blocks"][0]["request_id"], request_id.as_str());
    assert_eq!(form["blocks"][0]["title"], "Which room?");
    assert_eq!(form["blocks"][0]["submit_label"], "Book");
    assert!(form["blocks"][0]["state"].is_null());

    // A value outside the row's own options never wakes the run.
    assert_eq!(
        submit(
            &daemon,
            &request_id,
            serde_json::json!({"decision": "approved", "values": {"room": "z"}})
        )
        .await
        .status(),
        422
    );

    let response = submit(
        &daemon,
        &request_id,
        serde_json::json!({"decision": "approved", "values": {"room": "b"}}),
    )
    .await;
    assert_eq!(response.status(), 200);

    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let transcript = &brain.requests()[1].messages;
    assert_eq!(transcript.last().unwrap().text, r#"{"room":"b"}"#);
    assert_eq!(timeline(&daemon).await[0]["text_content"], "Booked Room B.");
}
