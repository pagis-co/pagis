//! Full-daemon allow-rule tests: an "always" approve writes the
//! derived rules into the agent's host grant and the same command class
//! then runs silently, a non-matching subcommand still asks, revoke
//! re-gates instantly, and the grants REST serves the settings page and
//! refuses a rule for a program that runs other programs. A stored rule
//! for such a program blocks no later "Always allow".

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_core::{Grant, GrantId, GrantStore};
use pagis_storage_sqlite::SqliteGrantStore;
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

async fn send_json(socket: &mut Socket, frame: serde_json::Value) {
    socket
        .send(Message::text(frame.to_string()))
        .await
        .expect("ws send");
}

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

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// Every frame up to and including the run's `completed` transition.
async fn frames_until_completed(socket: &mut Socket) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(socket).await;
        let completed = frame["type"] == "run.state_changed"
            && frame["payload"]["payload"]["to"] == "completed";
        frames.push(frame);
        if completed {
            return frames;
        }
    }
}

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

async fn decide(daemon: &TestDaemon, request_id: &str, decision: &str, scope: Option<&str>) {
    let mut body = serde_json::json!({ "decision": decision });
    if let Some(scope) = scope {
        body["scope"] = serde_json::json!(scope);
    }
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

async fn list_grants(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!("{}/api/v1/grants", daemon.base_url))
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

async fn put_rules(daemon: &TestDaemon, grant_id: &str, allow: &[&str]) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/grants/{grant_id}/rules",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "allow": allow }))
        .send()
        .await
        .unwrap()
}

async fn revoke(daemon: &TestDaemon, grant_id: &str) -> reqwest::Response {
    reqwest::Client::new()
        .delete(format!("{}/api/v1/grants/{grant_id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

/// Trigger the scripted host_shell call and read the request id from
/// the card once the run parks.
async fn park_run(daemon: &TestDaemon, socket: &mut Socket, pending_id: &str) -> String {
    rest_send(daemon, pending_id, "run it").await;
    let requested = next_frame_of(socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    loop {
        let state_changed = next_frame_of(socket, "run.state_changed").await;
        if state_changed["payload"]["payload"]["to"] == "waiting_for_approval" {
            break;
        }
    }
    request_id
}

#[tokio::test]
async fn always_allow_runs_the_command_class_silently_next_time() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo first" }),
    ));
    brain.push(Script::reply(&["Done one."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;

    // The request payload carries the daemon-derived proposal.
    let request = get_request(&daemon, &request_id).await;
    assert_eq!(request["payload"]["arguments"]["command"], "echo first");
    assert_eq!(
        request["payload"]["proposed_rules"],
        serde_json::json!(["echo"])
    );

    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;

    // "Always" wrote the rule into a live host grant.
    let grants = list_grants(&daemon).await;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["agent_id"], daemon.agent_id.as_str());
    assert_eq!(grants[0]["resource_kind"], "host");
    assert_eq!(grants[0]["allow"], serde_json::json!(["echo"]));

    // The same command class now runs without a request.
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo second" }),
    ));
    brain.push(Script::reply(&["Done two."]));
    rest_send(&daemon, "p-2", "again").await;
    let frames = frames_until_completed(&mut socket).await;
    assert!(
        frames.iter().all(|f| f["type"] != "request.created"),
        "an allowed command must not ask"
    );
    let executed = frames
        .iter()
        .find(|f| f["type"] == "tool.completed")
        .expect("tool.completed frame");
    assert_eq!(
        executed["payload"]["payload"]["authorization"],
        "allow_rule"
    );
    // The last work turn saw the tool result.
    let requests = brain.requests();
    let result = requests[requests.len() - 1]
        .messages
        .last()
        .unwrap()
        .clone();
    assert!(result.text.contains("second"), "{}", result.text);
    // The dispatch tells the client which command a rule approved, so the
    // client runs that one under `/bin/sh` and the card-approved one in
    // the person's own shell.
    let dispatched = host.dispatched();
    let approvals: Vec<(&str, bool)> = dispatched
        .iter()
        .map(|dispatch| (dispatch.command.as_str(), dispatch.approved_by_rule))
        .collect();
    assert_eq!(approvals, [("echo first", false), ("echo second", true)]);
}

#[tokio::test]
async fn a_non_matching_subcommand_still_asks() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo safe" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;

    // `echo` is allowed, but the compound's second part is not.
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo ok && rm -rf /tmp/x" }),
    ));
    brain.push(Script::reply(&["Okay, I will not."]));
    let request_id = park_run(&daemon, &mut socket, "p-2").await;
    let request = get_request(&daemon, &request_id).await;
    assert_eq!(
        request["payload"]["arguments"]["command"],
        "echo ok && rm -rf /tmp/x"
    );
    decide(&daemon, &request_id, "denied", None).await;
    frames_until_completed(&mut socket).await;
}

#[tokio::test]
async fn revoke_re_gates_instantly() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo one" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;

    let grants = list_grants(&daemon).await;
    let grant_id = grants[0]["id"].as_str().unwrap().to_string();
    assert_eq!(revoke(&daemon, &grant_id).await.status(), 204);
    assert_eq!(list_grants(&daemon).await.len(), 0);

    // A second revoke conflicts, and the same command asks again.
    assert_eq!(revoke(&daemon, &grant_id).await.status(), 409);
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo one" }),
    ));
    brain.push(Script::reply(&["Asked again."]));
    let request_id = park_run(&daemon, &mut socket, "p-2").await;
    decide(&daemon, &request_id, "denied", None).await;
    frames_until_completed(&mut socket).await;
}

#[tokio::test]
async fn approve_once_bootstraps_an_empty_grant_that_still_gates() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo once" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", None).await;
    frames_until_completed(&mut socket).await;

    // The bootstrap grant exists with no rules ...
    let grants = list_grants(&daemon).await;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["allow"], serde_json::json!([]));

    // ... so the same command asks again.
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo once" }),
    ));
    brain.push(Script::reply(&["Done again."]));
    let request_id = park_run(&daemon, &mut socket, "p-2").await;
    decide(&daemon, &request_id, "approved", None).await;
    frames_until_completed(&mut socket).await;
}

#[tokio::test]
async fn the_settings_page_edits_rules_through_the_grants_rest() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo hi" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;
    let grant_id = list_grants(&daemon).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Edit replaces the rule list, trimmed and deduplicated.
    let edited = put_rules(&daemon, &grant_id, &["git status ", "echo", "echo"]).await;
    assert_eq!(edited.status(), 200);
    let body: serde_json::Value = edited.json().await.unwrap();
    assert_eq!(body["allow"], serde_json::json!(["git status", "echo"]));
    assert_eq!(body["revision"], 2);
    assert_eq!(
        list_grants(&daemon).await[0]["allow"],
        serde_json::json!(["git status", "echo"])
    );

    // An empty rule, an unknown grant, and an edit after revoke reject.
    assert_eq!(put_rules(&daemon, &grant_id, &["  "]).await.status(), 422);
    assert_eq!(
        put_rules(&daemon, "01UNKNOWNGRANT", &["echo"])
            .await
            .status(),
        404
    );
    assert_eq!(revoke(&daemon, &grant_id).await.status(), 204);
    assert_eq!(put_rules(&daemon, &grant_id, &["echo"]).await.status(), 409);
}

/// A rule for a program that runs other programs approves every
/// program, so the settings page cannot save one. The grant keeps the
/// rules it had.
#[tokio::test]
async fn the_settings_page_refuses_a_rule_for_a_program_that_runs_other_programs() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo hi" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let _host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;
    let grant_id = list_grants(&daemon).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for allow in [&["env"][..], &["bash"], &["git status", "bash -c"]] {
        let refused = put_rules(&daemon, &grant_id, allow).await;
        assert_eq!(refused.status(), 422, "{allow:?}");
        let body: serde_json::Value = refused.json().await.unwrap();
        assert!(
            body.to_string().contains("runs other programs"),
            "{allow:?}: {body}"
        );
    }
    let grants = list_grants(&daemon).await;
    assert_eq!(grants[0]["allow"], serde_json::json!(["echo"]));
    assert_eq!(grants[0]["revision"], 1);
}

/// A stored rule for a program that runs other programs does not block
/// the next "Always allow" on the same grant. The decision is stored,
/// the run wakes, the new rule is written beside the stored one, and
/// the stored rule approves nothing.
#[tokio::test]
async fn a_stored_runner_rule_does_not_block_an_always_allow() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo hi" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let host = connected_host(&daemon).await;
    let mut socket = firehose(&daemon).await;

    let request_id = park_run(&daemon, &mut socket, "p-1").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    frames_until_completed(&mut socket).await;
    let grant_id = list_grants(&daemon).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The settings page refuses this rule, so the test writes it
    // through the store.
    assert!(
        SqliteGrantStore::new(daemon.pool().clone())
            .set_scope(
                &daemon.workspace_id,
                &GrantId::from(grant_id.clone()),
                &Grant::allow_scope(&["echo".to_string(), "env".to_string()]),
            )
            .await
            .unwrap()
    );

    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "git status" }),
    ));
    brain.push(Script::reply(&["Done two."]));
    let request_id = park_run(&daemon, &mut socket, "p-2").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    let frames = frames_until_completed(&mut socket).await;
    assert!(
        frames.iter().any(|frame| frame["type"] == "request.decided"
            && frame["payload"]["payload"]["request_id"] == request_id.as_str()),
        "the decision wakes the run"
    );
    assert_eq!(
        list_grants(&daemon).await[0]["allow"],
        serde_json::json!(["echo", "env", "git status"])
    );

    // The stored rule approves nothing: the command asks, and nothing
    // reaches the machine while it asks.
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "env git status" }),
    ));
    brain.push(Script::reply(&["Okay, I will not."]));
    let request_id = park_run(&daemon, &mut socket, "p-3").await;
    assert!(!host.commands().contains(&"env git status".to_string()));
    decide(&daemon, &request_id, "denied", None).await;
    frames_until_completed(&mut socket).await;
    assert_eq!(host.commands(), ["echo hi", "git status"]);
}
