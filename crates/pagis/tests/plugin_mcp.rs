//! The Plugin MCP host inside the daemon (ADR-0017): install the
//! fixture Plugin over the REST path, read the frozen catalog after a
//! restart that starts no server, call a Plugin tool through the run
//! loop, stop a Plugin tool with an uninstall, and read the two host
//! endpoints.
//!
//! The fixture MCP server runs in this test process over streamable
//! HTTP on the loopback interface, which is what the daemon's own
//! transport rule allows without TLS. The stdio transport and every
//! lifecycle rule are covered beside the fixture binary, in
//! `pagis-mcp-fixture`; what is here is the daemon around the host.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The fixture MCP server on a loopback port of its own. `stop` closes
/// the port, which is how a test reaches "no server answers".
struct FixtureServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    calls: Arc<AtomicUsize>,
}

impl FixtureServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("the address").port();
        let calls = Arc::new(AtomicUsize::new(0));
        let router = pagis_mcp_fixture::counting_http_router(Arc::clone(&calls));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { port, task, calls }
    }

    /// The tool calls that reached the server.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn stop(self) {
        self.task.abort();
    }
}

/// The stored fixture Plugin package as the uncompressed tar an upload
/// carries.
fn package() -> Vec<u8> {
    let source = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("../../fixtures/plugins/fixture-http");
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", &source)
        .expect("the upload tar");
    builder.into_inner().expect("the upload tar")
}

async fn upload(daemon: &TestDaemon) -> String {
    let part = reqwest::multipart::Part::bytes(package())
        .file_name("fixture-http.tar".to_string())
        .mime_str("application/x-tar")
        .expect("the part");
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/artifacts", daemon.base_url))
        .header("cookie", daemon.cookie())
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("JSON");
    body["id"].as_str().expect("the artifact id").to_string()
}

/// Install the fixture Plugin against the server on `port`. The
/// install freezes the tool list, so the server must answer here.
async fn install(daemon: &TestDaemon, port: u16) -> serde_json::Value {
    let artifact_id = upload(daemon).await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact_id},
            "bindings": [
                {"field": "endpoint", "kind": "value", "value": port.to_string()},
                {"field": "token", "kind": "secret", "secret": "the-token"},
            ],
        }))
        .send()
        .await
        .expect("install");
    assert_eq!(response.status().as_u16(), 201);
    response.json().await.expect("JSON")
}

async fn grant(daemon: &TestDaemon, plugin_id: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/grants",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "agent_id": daemon.agent_id }))
        .send()
        .await
        .expect("grant");
    assert_eq!(response.status().as_u16(), 201);
}

/// Uninstall the Plugin through the route the Administration Interface
/// calls.
async fn uninstall(daemon: &TestDaemon, plugin_id: &str) {
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/plugins/{plugin_id}",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("uninstall");
    assert_eq!(response.status().as_u16(), 204);
}

async fn get(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .error_for_status()
        .expect("success")
        .json()
        .await
        .expect("JSON")
}

/// The frozen tool of one qualified name, out of a Plugin detail.
fn frozen<'a>(detail: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    detail["frozen_tools"]
        .as_array()
        .expect("the frozen tools")
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("the frozen manifest carries {name}: {detail:#}"))
}

#[tokio::test]
async fn an_install_freezes_what_the_server_offers_and_names_the_effect_classes() {
    let server = FixtureServer::start().await;
    let daemon = TestDaemon::start().await;

    let installed = install(&daemon, server.port).await;

    assert_eq!(installed["name"], "fixturehttp");
    assert_eq!(installed["state"], "enabled");
    assert_eq!(installed["tools_changed"], false);
    // The package declares `echo` free; every tool it says nothing
    // about is `Host` (ADR-0017).
    assert_eq!(frozen(&installed, "fixturehttp__echo")["effect"], "free");
    assert_eq!(frozen(&installed, "fixturehttp__blob")["effect"], "host");
    assert_eq!(frozen(&installed, "fixturehttp__blob")["server"], "fixture");
    assert_eq!(
        frozen(&installed, "fixturehttp__blob")["description"],
        "Answer with a string of the given length."
    );
}

#[tokio::test]
async fn the_frozen_catalog_survives_a_restart_that_starts_no_server() {
    let server = FixtureServer::start().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    grant(&daemon, &plugin_id).await;

    // Nothing answers on the port. The catalog is a record,
    // so the daemon still offers the Plugin's tools at boot.
    server.stop();
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Nothing to do."]));
    let daemon = daemon
        .restart(TestDaemonOptions {
            brain: Arc::clone(&brain) as _,
            ..TestDaemonOptions::default()
        })
        .await;

    let detail = get(&daemon, &format!("/api/v1/plugins/{plugin_id}")).await;
    assert_eq!(detail["state"], "enabled");
    assert_eq!(detail["tools_changed"], false);
    assert_eq!(frozen(&detail, "fixturehttp__echo")["effect"], "free");
    assert_eq!(
        detail["frozen_tools"].as_array().expect("the tools").len(),
        9,
        "the whole frozen list reads back without a server"
    );

    // The boot put the frozen manifest into the broker, so a Run of
    // the granted Agent is offered the tools with no server behind
    // them.
    let mut socket = firehose(&daemon).await;
    send(&daemon, "p-1", "hello").await;
    frames_until_completed(&mut socket).await;
    let offered: Vec<String> = brain.requests()[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .filter(|name| name.starts_with("fixturehttp__"))
        .collect();
    assert!(
        offered.contains(&"fixturehttp__echo".to_string())
            && offered.contains(&"fixturehttp__blob".to_string()),
        "{offered:?}"
    );
}

#[tokio::test]
async fn a_host_class_plugin_tool_waits_for_a_card_and_then_answers_the_run() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    // `blob` is undeclared, so it is `Host`, and its answer is longer
    // than the broker's output cap.
    brain.push(Script::tool_call(
        &[],
        "fixturehttp__blob",
        serde_json::json!({ "size": 150_000 }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    grant(&daemon, &plugin_id).await;
    let mut socket = firehose(&daemon).await;

    send(&daemon, "p-1", "read the blob").await;
    let requested = next_frame_of(&mut socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .expect("the request id")
        .to_string();

    // The card names the Plugin, and its "always" rule is the one
    // tool's own qualified name.
    let request = get(&daemon, &format!("/api/v1/requests/{request_id}")).await;
    assert_eq!(request["payload"]["plugin_id"], plugin_id);
    assert_eq!(
        request["payload"]["proposed_rules"],
        serde_json::json!(["fixturehttp__blob"])
    );

    decide(&daemon, &request_id, "approved").await;
    let frames = frames_until_completed(&mut socket).await;
    let executed = frames
        .iter()
        .find(|frame| frame["type"] == "tool.completed")
        .expect("the tool ran");
    assert_eq!(executed["payload"]["payload"]["outcome"], "completed");

    // The Run saw the capped answer, not 150,000 characters of it.
    let requests = brain.requests();
    let result = requests[requests.len() - 1]
        .messages
        .last()
        .expect("the tool result reached the model")
        .text
        .clone();
    assert!(
        result.contains("[the result is 150000 characters; the first 100000 are shown]"),
        "the broker caps every route; the Run saw {} characters",
        result.chars().count()
    );
}

#[tokio::test]
async fn a_free_plugin_tool_runs_without_a_card() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "fixturehttp__echo",
        serde_json::json!({ "text": "over the daemon" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let installed = install(&daemon, server.port).await;
    grant(&daemon, installed["id"].as_str().expect("the id")).await;
    let mut socket = firehose(&daemon).await;

    send(&daemon, "p-1", "echo it").await;
    let frames = frames_until_completed(&mut socket).await;

    assert!(
        frames
            .iter()
            .all(|frame| frame["type"] != "request.created"),
        "a free Plugin tool asks nobody"
    );
    let requests = brain.requests();
    let result = requests[requests.len() - 1]
        .messages
        .last()
        .expect("the tool result")
        .text
        .clone();
    assert!(result.contains("over the daemon"), "{result}");
    assert_eq!(server.calls(), 1, "the call reached the server once");
}

#[tokio::test]
async fn a_plugin_tool_is_absent_from_a_run_that_holds_no_grant() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "fixturehttp__echo",
        serde_json::json!({ "text": "no grant" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    install(&daemon, server.port).await;
    let mut socket = firehose(&daemon).await;

    send(&daemon, "p-1", "echo it").await;
    frames_until_completed(&mut socket).await;

    let offered = brain.requests()[0]
        .tools
        .iter()
        .any(|tool| tool.name.starts_with("fixturehttp__"));
    assert!(!offered, "an ungranted Plugin has no tool in the snapshot");
}

/// A Plugin uninstall is the stop for a bad Plugin (ADR-0005). The
/// uninstall revokes the Plugin Grant and the broker reads that Grant
/// live, so a Run whose Capability Snapshot still holds the Plugin Tool
/// gets `permission_revoked`, and the call does not reach the server.
#[tokio::test]
async fn an_uninstall_stops_a_plugin_tool_that_an_old_snapshot_holds() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    // `echo` is free, so the call goes to dispatch with no card.
    brain.push(Script::gate_tool_call(
        "fixturehttp__echo",
        serde_json::json!({ "text": "after the uninstall" }),
        Arc::clone(&entered),
        Arc::clone(&release),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    grant(&daemon, &plugin_id).await;
    let mut socket = firehose(&daemon).await;

    send(&daemon, "p-1", "echo it").await;
    // The turn started, so the Run holds its Capability Snapshot.
    entered.notified().await;
    assert!(
        brain.requests()[0]
            .tools
            .iter()
            .any(|tool| tool.name == "fixturehttp__echo"),
        "the snapshot holds the Plugin Tool"
    );
    uninstall(&daemon, &plugin_id).await;
    release.notify_one();
    let frames = frames_until_completed(&mut socket).await;

    let completed = tool_completed(&frames, "fixturehttp__echo");
    assert_eq!(completed["outcome"], "rejected", "{completed:#}");
    assert_eq!(
        completed["error_code"], "permission_revoked",
        "{completed:#}"
    );
    assert!(
        last_message(&brain).contains("permission_revoked"),
        "the Run reads the refusal"
    );
    assert_eq!(server.calls(), 0, "the Plugin Tool did not execute");
}

/// A Plugin Tool call that waits for an Approval does not run when the
/// Person approves it after the uninstall: the broker reads the Plugin
/// Grant again when the Approval resumes the call (ADR-0005).
#[tokio::test]
async fn an_uninstall_stops_a_plugin_tool_call_that_waits_for_an_approval() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    // `blob` is undeclared, so it is `Host` and waits for a card.
    brain.push(Script::tool_call(
        &[],
        "fixturehttp__blob",
        serde_json::json!({ "size": 10 }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    grant(&daemon, &plugin_id).await;
    let mut socket = firehose(&daemon).await;

    send(&daemon, "p-1", "read the blob").await;
    let requested = next_frame_of(&mut socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .expect("the request id")
        .to_string();
    uninstall(&daemon, &plugin_id).await;
    decide(&daemon, &request_id, "approved").await;
    let frames = frames_until_completed(&mut socket).await;

    let completed = tool_completed(&frames, "fixturehttp__blob");
    assert_eq!(completed["outcome"], "rejected", "{completed:#}");
    assert_eq!(
        completed["error_code"], "permission_revoked",
        "{completed:#}"
    );
    assert_eq!(
        completed["request_id"],
        request_id.as_str(),
        "{completed:#}"
    );
    assert!(
        last_message(&brain).contains("permission_revoked"),
        "the Run reads the refusal"
    );
    assert_eq!(server.calls(), 0, "the Plugin Tool did not execute");
}

/// The payload of the `tool.completed` frame of one qualified name.
fn tool_completed<'a>(frames: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    frames
        .iter()
        .filter(|frame| frame["type"] == "tool.completed")
        .map(|frame| &frame["payload"]["payload"])
        .find(|payload| payload["qualified_name"] == name)
        .unwrap_or_else(|| panic!("no tool.completed frame for {name}: {frames:#?}"))
}

/// The text of the last message the model read: the tool result.
fn last_message(brain: &ScriptedBrain) -> String {
    let requests = brain.requests();
    requests[requests.len() - 1]
        .messages
        .last()
        .expect("the tool result reached the model")
        .text
        .clone()
}

#[tokio::test]
async fn the_start_endpoint_and_the_log_endpoint_serve_the_desk() {
    let server = FixtureServer::start().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();

    let started = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/start",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("start");
    assert_eq!(started.status().as_u16(), 200);
    let detail: serde_json::Value = started.json().await.expect("JSON");
    assert_eq!(detail["state"], "enabled");
    assert_eq!(frozen(&detail, "fixturehttp__echo")["effect"], "free");
    // The desk names the server that holds a process now.
    assert_eq!(detail["servers"][0]["running"], true);

    let log = get(&daemon, &format!("/api/v1/plugins/{plugin_id}/log")).await;
    assert!(log["text"].is_string(), "the log reads as text");

    // Neither endpoint answers through the id of a Plugin that is not
    // installed.
    let missing = pagis_core::PluginId::generate();
    assert_eq!(
        status(&daemon, &format!("/api/v1/plugins/{missing}/log")).await,
        404
    );
}

#[tokio::test]
async fn a_start_with_no_server_behind_it_is_refused() {
    let server = FixtureServer::start().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    server.stop();

    let refused = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/start",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("start");

    assert_eq!(refused.status().as_u16(), 422);
}

async fn status(daemon: &TestDaemon, path: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .status()
        .as_u16()
}

async fn send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .expect("send");
    assert_eq!(response.status().as_u16(), 201);
}

async fn decide(daemon: &TestDaemon, request_id: &str, decision: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "decision": decision }))
        .send()
        .await
        .expect("decide");
    assert_eq!(response.status().as_u16(), 200);
}

async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("ws send");
    assert_eq!(next_frame(&mut socket).await["type"], "ready");
    socket
}

async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket is open")
            .expect("the frame reads");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("the frame is JSON");
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

/// A Plugin is the Org's install, and every tenant runs it
/// (ADR-0017).
///
/// The administrator installs once for the Org, the manifest reaches
/// every tenant's registry, and a member's sprite calls the tool in the
/// member's own Plugin Computer.
#[tokio::test]
async fn an_administrators_install_reaches_a_members_sprite() {
    let server = FixtureServer::start().await;
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "fixturehttp__echo",
        serde_json::json!({ "text": "from the member" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;

    // Person B: a member of the same Org, with a Workspace of their own.
    let org_id = pagis_core::UserStore::get(daemon.stores().users.as_ref(), &daemon.user_id)
        .await
        .expect("read the administrator")
        .expect("the boot seeds one person")
        .org_id;
    let member = pagis_core::User {
        email: Some("bo@example.com".to_string()),
        name: Some("Bo".to_string()),
        ..pagis_core::User::new(org_id, pagis_core::UserRole::Member, pagis_core::now_ms())
    };
    daemon
        .stores()
        .users
        .create(&member)
        .await
        .expect("write the member");
    let workspace_b = pagis_server::provisioning::WorkspaceSeed::from(daemon.stores())
        .run(
            &member.id,
            "Bo's Workspace",
            "UTC",
            pagis_server::provisioning::Onboarding::Done,
            pagis_core::now_ms(),
        )
        .await
        .expect("seed the member's Workspace");
    let cookie_b = daemon.cookie_for(&member.id).await;
    let sprite_b =
        pagis_core::AgentStore::list_by_workspace(daemon.stores().agents.as_ref(), &workspace_b.id)
            .await
            .expect("the member's agents")
            .remove(0);
    let dm_b = pagis_core::ChannelStore::find_user_dm(
        daemon.stores().channels.as_ref(),
        &workspace_b.id,
        &sprite_b.id,
    )
    .await
    .expect("the member's DM")
    .expect("the seed makes one");

    // The administrator installs the Plugin once, for the Org.
    let installed = install(&daemon, server.port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();

    // The member reads the same Plugin: the Org owns which plugins are
    // available.
    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/plugins", daemon.base_url))
        .header("cookie", &cookie_b)
        .send()
        .await
        .expect("GET")
        .json()
        .await
        .expect("JSON");
    assert_eq!(
        listed["items"][0]["id"].as_str(),
        Some(plugin_id.as_str()),
        "{listed:#}"
    );

    // The member grants it to their own sprite, and the tool answers in
    // the member's own run.
    let granted = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/grants",
            daemon.base_url
        ))
        .header("cookie", &cookie_b)
        .json(&serde_json::json!({ "agent_id": sprite_b.id.as_str() }))
        .send()
        .await
        .expect("grant");
    assert_eq!(granted.status().as_u16(), 201);

    let posted = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url,
            dm_b.id.as_str()
        ))
        .header("cookie", &cookie_b)
        .json(&serde_json::json!({ "pending_id": "p-b", "text": "echo it" }))
        .send()
        .await
        .expect("send");
    assert_eq!(posted.status().as_u16(), 201);

    // The snapshot of the member's run offers the Plugin's tool, and the
    // tool answered it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let requests = brain.requests();
        let answered = requests.iter().any(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text.contains("from the member"))
        });
        if answered {
            let offered = requests[0]
                .tools
                .iter()
                .any(|tool| tool.name == "fixturehttp__echo");
            assert!(offered, "the member's snapshot carries no Plugin tool");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the member's run never called the Plugin tool: {:?}",
            brain.requests().len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    server.stop();
}
