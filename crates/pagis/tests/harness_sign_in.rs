//! Full-daemon tests of the Harness Sign-In (ADR-0033).
//!
//! The Person starts a sign-in through REST, and the daemon sends the
//! `harness_sign_in` frame to the fake Host on its Host socket. For an ACP
//! terminal method, the daemon first asks the harness for its methods
//! over the Host's session socket, whose Client App end runs the fake
//! Coding Harness of `pagis_coding`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use pagis_broker::HarnessSignIn;
use pagis_coding::fake::{Script, acp, serve_client_app};
use pagis_core::{HostId, harness, now_ms};
use pagis_testkit::{HostAnswer, HostClient, TestDaemon, TwoTenants};
use serde_json::{Value, json};
use tokio_util::compat::TokioAsyncReadCompatExt;

const WAIT: Duration = Duration::from_secs(10);

async fn sign_in(
    daemon: &TestDaemon,
    cookie: &str,
    host_id: &str,
    harness_id: &str,
    method: &str,
) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&json!({ "method": method }))
        .send()
        .await
        .expect("the daemon answers");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// The first sign-in that reaches `host`, once it arrives.
async fn first_sign_in(host: &HostClient) -> HarnessSignIn {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(sign_in) = host.sign_ins().into_iter().next() {
            return sign_in;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no harness_sign_in reached the Host"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[tokio::test]
async fn the_sign_in_sends_the_catalog_command_to_the_host() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:codex"],
        HostAnswer::ok(),
    )
    .await;

    let (status, body) = sign_in(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        "codex",
        "subscription",
    )
    .await;

    assert_eq!(status, 202, "{body}");
    let sign_in = first_sign_in(&host).await;
    assert_eq!(body["id"], sign_in.id.as_str());
    assert_eq!(sign_in.harness, "codex");
    assert_eq!(sign_in.name, "Codex");
    assert_eq!(sign_in.command, "npx");
    assert_eq!(
        sign_in.args,
        strings(&["--yes", "@openai/codex@0.159.1", "login"])
    );
    assert!(sign_in.env.is_empty());
}

/// Claude Code signs in with the ACP terminal method that its own
/// `initialize` gives on the Host.
#[tokio::test]
async fn a_terminal_method_comes_from_the_harness_on_the_host() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let script = Script::default().auth_method(acp::AuthMethod::Terminal(
        acp::AuthMethodTerminal::new("claude-ai-login", "Use Claude subscription")
            .args(vec!["--cli".to_owned()])
            .env(HashMap::from([("CLAUDE_LOGIN".to_owned(), "1".to_owned())])),
    ));
    open_session_socket(&daemon, &HostId::from(host.host_id().to_string()), script).await;

    let (status, body) = sign_in(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        "claude",
        "subscription",
    )
    .await;

    assert_eq!(status, 202, "{body}");
    let sign_in = first_sign_in(&host).await;
    assert_eq!(sign_in.name, "Claude Code");
    assert_eq!(sign_in.command, "npx");
    assert_eq!(
        sign_in.args,
        strings(&[
            "--yes",
            "@agentclientprotocol/claude-agent-acp@0.87.0",
            "--cli"
        ])
    );
    assert_eq!(
        sign_in.env,
        BTreeMap::from([("CLAUDE_LOGIN".to_string(), "1".to_string())])
    );
}

#[tokio::test]
async fn an_absent_host_answers_409_with_the_words_of_a_host_action() {
    let daemon = TestDaemon::start().await;
    let host = daemon
        .stores()
        .hosts
        .register(
            &daemon.workspace_id,
            "Air",
            "macos",
            &["harness:codex".to_string()],
            now_ms(),
        )
        .await
        .expect("write the Host");

    let (status, body) = sign_in(
        &daemon,
        daemon.cookie(),
        host.id.as_str(),
        "codex",
        "subscription",
    )
    .await;

    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "host_not_connected");
    assert_eq!(
        body["error"]["message"],
        pagis_broker::not_connected_message("Air")
    );
}

#[tokio::test]
async fn an_undeclared_harness_or_method_answers_422_and_sends_nothing() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:copilot"],
        HostAnswer::ok(),
    )
    .await;

    let (undeclared, body) = sign_in(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        "codex",
        "subscription",
    )
    .await;
    assert_eq!(undeclared, 422, "{body}");
    assert_eq!(body["error"]["code"], "undeclared_harness");

    // The catalog holds no API key sign-in for Copilot.
    let (method, body) = sign_in(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        "copilot",
        "api_key",
    )
    .await;
    assert_eq!(method, 422, "{body}");
    assert_eq!(body["error"]["code"], "method_not_offered");

    assert!(host.sign_ins().is_empty());
}

#[tokio::test]
async fn the_harness_list_names_each_catalog_harness_and_its_sign_in_methods() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/harnesses", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the daemon answers");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("a JSON answer");

    let items = body["items"].as_array().expect("the harnesses");
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    let catalog: Vec<&str> = harness::catalog().iter().map(|entry| entry.id).collect();
    assert_eq!(ids, catalog);
    assert_eq!(
        items[0],
        json!({
            "id": "claude",
            "name": "Claude Code",
            "sign_in_methods": [
                { "method": "subscription", "label": "Subscription" },
                { "method": "api_key", "label": "API key" },
            ],
        })
    );
    let copilot = items.iter().find(|item| item["id"] == "copilot").unwrap();
    assert_eq!(
        copilot["sign_in_methods"],
        json!([{ "method": "subscription", "label": "Subscription" }])
    );
}

/// A Host id is not a secret. Person B's sign-in on person A's Host reads
/// the Host as absent, and A's machine receives nothing.
#[tokio::test]
async fn person_b_cannot_start_a_sign_in_on_person_as_host() {
    let world = TwoTenants::start().await;
    let a_host = HostClient::connect_as(
        &world.daemon,
        &world.a.cookie,
        "Air",
        "macos",
        &["shell", "harness:codex"],
        HostAnswer::ok(),
    )
    .await;

    let (status, body) = sign_in(
        &world.daemon,
        &world.b.cookie,
        a_host.host_id(),
        "codex",
        "subscription",
    )
    .await;

    assert_eq!(status, 404, "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(a_host.sign_ins().is_empty());
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
        "/Users/bo".to_string(),
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
