//! Full-daemon connect tests (ADR-0012): the settings path that
//! puts a Google Connection in the table, takes it to `connected`, and
//! hands a granted Agent an account that answers. One scripted `gog`
//! stands in for the provider, so no process and no browser run.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pagis_core::GrantId;
use pagis_google::{GogCommand, GogRunner, ProcessFailure, ProcessOutput};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A `gog` that records what it was asked and answers from a script.
/// The exits are the ones that the pinned `gog` documents.
#[derive(Default)]
struct FakeGog {
    commands: Mutex<Vec<Vec<String>>>,
    stdin: Mutex<Vec<Vec<u8>>>,
    /// The exit every `auth add` returns; 0 unless a test says else.
    authorize_status: Mutex<i32>,
    /// The exit every tool call returns.
    call_status: Mutex<i32>,
    /// A process failure returned before a provider exit can be read.
    call_failure: Mutex<Option<ProcessFailure>>,
}

impl FakeGog {
    fn commands(&self) -> Vec<Vec<String>> {
        self.commands.lock().unwrap().clone()
    }

    fn stdin(&self) -> Vec<Vec<u8>> {
        self.stdin.lock().unwrap().clone()
    }

    fn refuse_authorization(&self, status: i32) {
        *self.authorize_status.lock().unwrap() = status;
    }

    fn refuse_calls(&self, status: i32) {
        *self.call_status.lock().unwrap() = status;
    }

    fn fail_calls(&self, failure: ProcessFailure) {
        *self.call_failure.lock().unwrap() = Some(failure);
    }
}

#[async_trait]
impl GogRunner for FakeGog {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        let args = command.args().to_vec();
        self.commands.lock().unwrap().push(args.clone());
        if let Some(input) = command.stdin() {
            self.stdin.lock().unwrap().push(input.to_vec());
        }
        let subcommand: Vec<&str> = args.iter().map(String::as_str).collect();
        if subcommand.contains(&"auth") && subcommand.contains(&"add") {
            return Ok(ProcessOutput {
                status: Some(*self.authorize_status.lock().unwrap()),
                stdout: b"{}".to_vec(),
            });
        }
        if subcommand.contains(&"credentials") {
            return Ok(ProcessOutput {
                status: Some(0),
                stdout: b"{}".to_vec(),
            });
        }
        if let Some(failure) = *self.call_failure.lock().unwrap() {
            return Err(failure);
        }
        Ok(ProcessOutput {
            status: Some(*self.call_status.lock().unwrap()),
            stdout: br#"{"messages":[{"id":"m1","subject":"hello"}]}"#.to_vec(),
        })
    }
}

#[tokio::test]
async fn an_expired_google_token_publishes_the_connection_repair_state() {
    let h = boot().await;
    let (_, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let id = created["id"].as_str().unwrap().to_string();
    authorize(&h.daemon, &id, &["gmail_read"]).await;
    grant(&h.daemon, &id, &["gmail_read"]).await;
    let mut socket = firehose(&h.daemon).await;
    h.gog.fail_calls(ProcessFailure::ReauthRequired);

    let response = client()
        .put(format!(
            "{}/api/v1/connections/{id}/sync",
            h.daemon.base_url
        ))
        .header("cookie", h.daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": h.daemon.agent_id,
            "enabled": true,
            "filter": pagis_google::gmail_filter::default_filter(),
            "since": 0,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let changed = loop {
        let frame = next_frame_of(&mut socket, "connection.changed").await;
        if frame["payload"]["payload"]["status"] == "reauth_required" {
            break frame;
        }
    };
    assert_eq!(changed["payload"]["payload"]["connection_id"], id.as_str());
    assert_eq!(
        list_connections(&h.daemon).await["items"][0]["status"],
        "reauth_required"
    );
}

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    gog: Arc<FakeGog>,
}

async fn boot() -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let gog = Arc::new(FakeGog::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        gog: Some(Arc::clone(&gog) as _),
        authorize_timeout: Duration::from_secs(5),
        ..TestDaemonOptions::default()
    })
    .await;
    Harness { daemon, brain, gog }
}

/// A local installation in Remote Access: People on other machines reach
/// it through the Funnel on this machine. With Remote Access off, the
/// daemon refuses every forwarded request outright.
async fn boot_in_remote_access() -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let gog = Arc::new(FakeGog::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        gog: Some(Arc::clone(&gog) as _),
        authorize_timeout: Duration::from_secs(5),
        public_origin: "https://pagis.owner.example".to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        remote_access: true,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness { daemon, brain, gog }
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// The Desktop client secret the user types in the connect flow. No
/// interface the daemon retains may echo it back.
const CLIENT_SECRET: &str = "GOCSPX-connect-flow-secret";

async fn create_connection(
    daemon: &TestDaemon,
    alias: &str,
    account: &str,
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client()
        .post(format!("{}/api/v1/settings/connections", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "provider": "google",
            "alias": alias,
            "display_name": "Work Google",
            "fields": {
                "account": account,
                "client_id": "1234.apps.googleusercontent.com",
                "client_secret": CLIENT_SECRET,
            },
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response.json().await.unwrap_or(serde_json::json!({})),
    )
}

/// The whole answer of the authorize route: the connection, and
/// the address to send the person to when the flow is brokered.
async fn authorize_answer(
    daemon: &TestDaemon,
    connection_id: &str,
    capabilities: &[&str],
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client()
        .post(format!(
            "{}/api/v1/settings/connections/{connection_id}/authorize",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "capabilities": capabilities }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response.json().await.unwrap_or(serde_json::json!({})),
    )
}

/// The connection the authorize route answered, for the paths that
/// finish inside the request.
async fn authorize(
    daemon: &TestDaemon,
    connection_id: &str,
    capabilities: &[&str],
) -> (reqwest::StatusCode, serde_json::Value) {
    let (status, answer) = authorize_answer(daemon, connection_id, capabilities).await;
    // An error body carries no connection, so the caller reads it whole.
    match answer.get("connection").cloned() {
        Some(connection) if !connection.is_null() => (status, connection),
        _ => (status, answer),
    }
}

async fn list_connections(daemon: &TestDaemon) -> serde_json::Value {
    client()
        .get(format!("{}/api/v1/settings/connections", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn grant(daemon: &TestDaemon, connection_id: &str, capabilities: &[&str]) -> GrantId {
    let response = client()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": connection_id,
            "capabilities": capabilities,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    GrantId::from(body["id"].as_str().unwrap().to_string())
}

async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
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
    next_frame_of(&mut socket, "ready").await;
    socket
}

async fn frames_until_settled(socket: &mut Socket) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(socket).await;
        let settled = frame["type"] == "run.state_changed"
            && (frame["payload"]["payload"]["to"] == "completed"
                || frame["payload"]["payload"]["to"] == "failed");
        frames.push(frame);
        if settled {
            return frames;
        }
    }
}

async fn send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = client()
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

fn tool_fact<'a>(frames: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    frames
        .iter()
        .find(|frame| {
            frame["type"] == "tool.completed" && frame["payload"]["payload"]["name"] == name
        })
        .unwrap_or_else(|| panic!("no tool.completed fact for {name}"))
}

/// The Provider Catalog (ADR-0012): the picker draws the list
/// the daemon serves, and holds none of its own. A person picks only
/// what a person connects: the carrier account and the mail domain are
/// the installation's, and the Administration Interface sets them up.
#[tokio::test]
async fn the_daemon_serves_the_provider_catalog_the_picker_draws() {
    let h = boot().await;

    let response = client()
        .get(format!(
            "{}/api/v1/settings/connections/providers",
            h.daemon.base_url
        ))
        .header("cookie", h.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let page: serde_json::Value = response.json().await.unwrap();
    let items = page["items"].as_array().unwrap();
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["google"]);

    let google = &items[0];
    assert_eq!(google["kind"], "oauth");
    assert_eq!(
        google["capabilities"],
        serde_json::json!(["mail", "calendar"])
    );
    assert_eq!(google["max_instances"], serde_json::Value::Null);

    // A Connection names what it gives, so the card follows that and
    // not the provider name.
    let (status, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    assert_eq!(status, 201);
    assert_eq!(
        created["capabilities"],
        serde_json::json!(["mail", "calendar"])
    );
}

#[tokio::test]
async fn connecting_google_records_the_account_and_reaches_connected() {
    let h = boot().await;

    let (status, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    assert_eq!(status, 201);
    assert_eq!(created["status"], "disconnected");
    assert_eq!(created["auth_mode"], "byo");
    assert_eq!(created["account"], "alice@example.com");
    assert_eq!(created["alias"], "work");
    assert_eq!(created["authorized_capabilities"], serde_json::json!([]));

    let id = created["id"].as_str().unwrap();
    let (status, connected) = authorize(&h.daemon, id, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(connected["status"], "connected");
    assert_eq!(
        connected["authorized_capabilities"],
        serde_json::json!(["gmail_read", "calendar_read"])
    );

    // The card reads the same on a fresh list.
    let listed = list_connections(&h.daemon).await;
    assert_eq!(listed["items"][0]["status"], "connected");
    assert_eq!(listed["items"][0]["account"], "alice@example.com");
    assert_eq!(listed["items"][0]["auth_mode"], "byo");
    assert_eq!(
        listed["items"][0]["authorized_capabilities"],
        serde_json::json!(["gmail_read", "calendar_read"])
    );

    // The provider saw the client install, then the loopback exchange
    // on 127.0.0.1 with the read-only profile a Connection starts at.
    let commands = h.gog.commands();
    assert_eq!(commands.len(), 2);
    assert!(commands[0].contains(&"credentials".to_string()));
    assert!(commands[1].contains(&"127.0.0.1:0".to_string()));
    assert!(commands[1].contains(&"--readonly".to_string()));
}

/// A `byo` Google consent runs on the daemon host, so a request that
/// came through a proxy starts none. On a local installation in Remote
/// Access, a person on another machine reaches the daemon that way. The answer names the fix: the Installation OAuth Client.
#[tokio::test]
async fn a_byo_google_connection_through_a_proxy_is_refused_and_names_the_fix() {
    let h = boot_in_remote_access().await;

    let response = client()
        .post(format!("{}/api/v1/settings/connections", h.daemon.base_url))
        .header("cookie", h.daemon.cookie())
        .header("x-forwarded-for", "203.0.113.7")
        .json(&serde_json::json!({
            "provider": "google",
            "alias": "work",
            "display_name": "Work Google",
            "fields": {
                "account": "alice@example.com",
                "client_id": "1234.apps.googleusercontent.com",
                "client_secret": CLIENT_SECRET,
            },
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(
            "an administrator sets up the Google OAuth client in the Administration Interface"
        ),
        "{message}"
    );
    assert_eq!(
        list_connections(&h.daemon).await["items"],
        serde_json::json!([])
    );
    assert!(h.gog.commands().is_empty(), "gog never ran");
}

/// The consent of a `byo` Connection made at the machine follows the
/// same rule when the person asks again from elsewhere.
#[tokio::test]
async fn authorizing_a_byo_google_connection_through_a_proxy_is_refused() {
    let h = boot_in_remote_access().await;
    let (status, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    assert_eq!(status, 201);
    let id = created["id"].as_str().unwrap();

    let response = client()
        .post(format!(
            "{}/api/v1/settings/connections/{id}/authorize",
            h.daemon.base_url
        ))
        .header("cookie", h.daemon.cookie())
        .header("x-forwarded-for", "203.0.113.7")
        .json(&serde_json::json!({ "capabilities": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("the Google OAuth client in the Administration Interface"),
        "{body}"
    );
    assert_eq!(
        list_connections(&h.daemon).await["items"][0]["status"],
        "disconnected"
    );
    // Only the client install ran; no loopback exchange started.
    assert_eq!(h.gog.commands().len(), 1);
}

#[tokio::test]
async fn a_granted_agent_reaches_the_account_the_user_connected() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;
    let (_, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let id = created["id"].as_str().unwrap().to_string();
    authorize(&h.daemon, &id, &[]).await;
    grant(&h.daemon, &id, &["gmail_read"]).await;

    h.brain.push(Script::tool_call(
        &[],
        pagis_broker::MAIL_SEARCH,
        serde_json::json!({"mailbox": "work", "query": "is:unread"}),
    ));
    h.brain.push(Script::reply(&["Done."]));
    send(&h.daemon, "p1", "check my mail").await;
    let frames = frames_until_settled(&mut socket).await;

    let fact = tool_fact(&frames, pagis_broker::MAIL_SEARCH);
    assert_eq!(fact["payload"]["payload"]["outcome"], "completed");
    assert_eq!(fact["payload"]["payload"]["selected_connection"], id);
    // The call carries the account and client the connect flow bound.
    let call = h.gog.commands().pop().expect("a tool call");
    assert!(
        call.windows(2)
            .any(|pair| pair == ["--account", "alice@example.com"])
    );
    assert!(call.windows(2).any(|pair| pair == ["--client", "work"]));
}

#[tokio::test]
async fn access_revoked_at_google_surfaces_on_the_record_and_the_next_call() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;
    let (_, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let id = created["id"].as_str().unwrap().to_string();
    authorize(&h.daemon, &id, &[]).await;
    grant(&h.daemon, &id, &["gmail_read"]).await;

    // The exit the pinned `gog` uses once the user revoked access.
    h.gog.refuse_calls(4);
    h.brain.push(Script::tool_call(
        &[],
        pagis_broker::MAIL_SEARCH,
        serde_json::json!({"mailbox": "work", "query": "is:unread"}),
    ));
    h.brain.push(Script::reply(&["I cannot reach it."]));
    send(&h.daemon, "p1", "check my mail").await;
    let frames = frames_until_settled(&mut socket).await;
    assert_eq!(
        tool_fact(&frames, pagis_broker::MAIL_SEARCH)["payload"]["payload"]["error_code"],
        "reauth_required"
    );

    let listed = list_connections(&h.daemon).await;
    assert_eq!(listed["items"][0]["status"], "reauth_required");

    // A record at `reauth_required` offers no tools, so the next run
    // never reaches Google at all.
    let before = h.gog.commands().len();
    h.brain.push(Script::tool_call(
        &[],
        pagis_broker::MAIL_SEARCH,
        serde_json::json!({"mailbox": "work", "query": "is:unread"}),
    ));
    h.brain.push(Script::reply(&["Still no."]));
    send(&h.daemon, "p2", "try again").await;
    frames_until_settled(&mut socket).await;
    assert_eq!(h.gog.commands().len(), before, "no second provider call");

    // Reauthorizing from the card repairs the record.
    let (status, repaired) = authorize(&h.daemon, &id, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(repaired["status"], "connected");
}

#[tokio::test]
async fn two_accounts_get_distinct_aliases_and_a_duplicate_is_refused() {
    let h = boot().await;
    let (first, _) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let (second, _) = create_connection(&h.daemon, "personal", "alice@gmail.com").await;
    assert_eq!(first, 201);
    assert_eq!(second, 201);

    let (duplicate, body) = create_connection(&h.daemon, "work", "carol@example.com").await;
    assert_eq!(duplicate, 409);
    assert_eq!(body["error"]["code"], "conflict");
    assert_eq!(
        list_connections(&h.daemon).await["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn an_exchange_google_refuses_leaves_the_record_ready_to_retry() {
    let h = boot().await;
    let (_, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let id = created["id"].as_str().unwrap().to_string();

    h.gog.refuse_authorization(6);
    let (status, body) = authorize(&h.daemon, &id, &[]).await;
    assert_eq!(status, 422);
    assert!(
        !body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(CLIENT_SECRET),
        "a refusal never quotes what the user typed"
    );

    let listed = list_connections(&h.daemon).await;
    assert_eq!(listed["items"][0]["status"], "disconnected");
    // The binding survives, so the user retries from the same card.
    assert_eq!(listed["items"][0]["account"], "alice@example.com");
}

#[tokio::test]
async fn no_secret_reaches_an_interface_the_daemon_retains() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;
    let (_, created) = create_connection(&h.daemon, "work", "alice@example.com").await;
    let id = created["id"].as_str().unwrap().to_string();
    authorize(&h.daemon, &id, &[]).await;

    // The two documents the API hands back.
    assert!(!created.to_string().contains(CLIENT_SECRET));
    assert!(
        !list_connections(&h.daemon)
            .await
            .to_string()
            .contains(CLIENT_SECRET)
    );

    // The event feed, which the audit log keeps.
    let created_event = next_frame_of(&mut socket, "connection.created").await;
    let changed_event = next_frame_of(&mut socket, "connection.changed").await;
    assert!(!created_event.to_string().contains(CLIENT_SECRET));
    assert!(!changed_event.to_string().contains(CLIENT_SECRET));
    assert_eq!(changed_event["payload"]["payload"]["status"], "connected");

    // The stored row, which the provider reads on every call.
    let config: String = sqlx::query_scalar("SELECT config FROM connections WHERE id = ?")
        .bind(&id)
        .fetch_one(h.daemon.pool())
        .await
        .unwrap();
    assert!(!config.contains(CLIENT_SECRET));
    assert!(!config.contains("1234.apps.googleusercontent.com"));

    // And no argument list: the client reached `gog` over stdin only.
    for command in h.gog.commands() {
        assert!(!command.join(" ").contains(CLIENT_SECRET));
    }
    let stdin = h.gog.stdin();
    assert_eq!(stdin.len(), 1, "one client install, one stdin document");
    assert!(
        String::from_utf8(stdin[0].clone())
            .unwrap()
            .contains(CLIENT_SECRET)
    );
}

#[tokio::test]
async fn a_provider_pagis_does_not_connect_is_refused() {
    let h = boot().await;
    let response = client()
        .post(format!("{}/api/v1/settings/connections", h.daemon.base_url))
        .header("cookie", h.daemon.cookie())
        .json(&serde_json::json!({
            "provider": "slack",
            "alias": "team",
            "display_name": "Slack",
            "fields": {
                "account": "alice@example.com",
                "client_id": "id",
                "client_secret": "secret",
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    assert!(h.gog.commands().is_empty());
}

/// The brokered Google flow through the daemon's own HTTP surface
/// (ADR-0012). An Administrator sets up the Installation OAuth Client.
/// The authorize route answers the start route on the Public Origin. The
/// start route binds the `state` to the browser of the initiating Person
/// with a transaction cookie (RFC 9700, section 2.1.1). The public
/// callback route connects the Connection only for that browser, while
/// the Session that started the authorization is live, and only for the
/// Google account of the Connection. A fake Google answers the token
/// calls, so nothing here reaches Google.
mod brokered {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use pagis_core::{ConnectionId, SealedSecret, User, UserRole, Workspace, WorkspaceId, now_ms};
    use pagis_server::SESSION_COOKIE;
    use pagis_testkit::evaluation::FixtureClock;
    use pagis_testkit::{TestDaemon, TestDaemonOptions};
    use reqwest::header::{LOCATION, SET_COOKIE};

    use super::FakeGog;

    /// The client id of the Installation OAuth Client.
    const CLIENT_ID: &str = "installation.apps.googleusercontent.com";
    /// The Google account of the Connection, as the Person typed it.
    const ACCOUNT: &str = "alice@example.com";
    /// Where the fake sends a browser to consent. Nothing answers there,
    /// and no test follows the redirect.
    const GOOGLE: &str = "https://accounts.example.test/authorize";
    /// The transaction cookie where the Public Origin is `https:`.
    const HOST_ONLY_COOKIE: &str = "__Host-pagis_google_authorization";
    /// The transaction cookie where the Public Origin is plain `http:` on
    /// loopback, as on a Local Installation.
    const LOOPBACK_COOKIE: &str = "pagis_google_authorization";
    const GMAIL_READ: &str = "https://www.googleapis.com/auth/gmail.readonly";
    const GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
    /// What Google grants for `openid email`.
    const IDENTITY: &str = "openid https://www.googleapis.com/auth/userinfo.email";
    /// The page of a callback that connected nothing.
    const REFUSED: &str = "did not complete";

    /// A fake Google token endpoint. It answers each code exchange with an
    /// access token, a refresh token named after the code, the `scope`
    /// value that the test sets and an ID token for the Google account
    /// that the test sets.
    struct FakeGoogle {
        exchanges: Mutex<Vec<String>>,
        /// The `email` and `email_verified` claims of the ID token.
        consent: Mutex<(String, bool)>,
        /// The capability scopes of the `scope` value.
        granted: Mutex<Vec<&'static str>>,
    }

    impl Default for FakeGoogle {
        fn default() -> Self {
            Self {
                exchanges: Mutex::default(),
                consent: Mutex::new((ACCOUNT.to_string(), true)),
                granted: Mutex::new(vec![GMAIL_READ]),
            }
        }
    }

    impl FakeGoogle {
        fn exchanges(&self) -> Vec<String> {
            self.exchanges.lock().unwrap().clone()
        }

        /// The Google account that the person picks at the consent
        /// screen, and whether Google verified its address.
        fn consent_as(&self, email: &str, verified: bool) {
            *self.consent.lock().unwrap() = (email.to_string(), verified);
        }

        /// The capability scopes that Google answers as granted.
        fn grant(&self, scopes: &[&'static str]) {
            *self.granted.lock().unwrap() = scopes.to_vec();
        }
    }

    async fn token(
        State(google): State<Arc<FakeGoogle>>,
        body: String,
    ) -> axum::Json<serde_json::Value> {
        google.exchanges.lock().unwrap().push(body.clone());
        let code = body
            .split('&')
            .find_map(|pair| pair.strip_prefix("code="))
            .unwrap_or_default()
            .to_string();
        let (email, verified) = google.consent.lock().unwrap().clone();
        let scope = std::iter::once(IDENTITY)
            .chain(google.granted.lock().unwrap().iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        axum::Json(serde_json::json!({
            "access_token": "ya29.access",
            "refresh_token": format!("1//refresh-for-{code}"),
            "expires_in": 3599,
            "scope": scope,
            "token_type": "Bearer",
            "id_token": id_token(&serde_json::json!({
                "iss": "https://accounts.google.com",
                "aud": CLIENT_ID,
                "sub": "110169484474386276334",
                "email": email,
                "email_verified": verified,
            })),
        }))
    }

    /// An ID token as the token endpoint of Google answers it. The daemon
    /// reads its claims from a TLS answer and checks no signature, so the
    /// signature is a placeholder.
    fn id_token(claims: &serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "RS256", "kid": "fake", "typ": "JWT" });
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            URL_SAFE_NO_PAD.encode("not-a-signature"),
        )
    }

    async fn serve(google: Arc<FakeGoogle>) -> SocketAddr {
        let app = Router::new()
            .route("/token", post(token))
            .with_state(google);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// A browser that follows no redirect, so a test reads each 303.
    fn browser() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// One installation that holds the Installation OAuth Client, and one
    /// `brokered` Google Connection of the seeded Person.
    struct Brokered {
        daemon: TestDaemon,
        google: Arc<FakeGoogle>,
        gog: Arc<FakeGog>,
        /// The answer of the Administrator's setup of the client.
        registered: serde_json::Value,
        connection_id: String,
        /// The name of the transaction cookie at the Public Origin.
        cookie_name: &'static str,
    }

    /// A Local Installation with Remote Access off: its Public Origin is
    /// loopback, and it has one Person.
    async fn brokered() -> Brokered {
        brokered_with(TestDaemonOptions::default()).await
    }

    /// A Local Installation in Remote Access: the Funnel answers on an
    /// `https:` Public Origin and reaches the daemon from this machine.
    /// People on other machines use it.
    async fn brokered_in_remote_access() -> Brokered {
        brokered_with(TestDaemonOptions {
            public_origin: "https://pagis.owner.example".to_string(),
            trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
            remote_access: true,
            ..TestDaemonOptions::default()
        })
        .await
    }

    async fn brokered_with(options: TestDaemonOptions) -> Brokered {
        let google = Arc::new(FakeGoogle::default());
        let addr = serve(Arc::clone(&google)).await;
        let gog = Arc::new(FakeGog::default());
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            gog: Some(Arc::clone(&gog) as _),
            google_oauth: Some(Arc::new(pagis_google::GoogleOAuth::with_endpoints(
                GOOGLE,
                &format!("http://{addr}/token"),
            ))),
            ..options
        })
        .await;
        let (status, registered) = daemon
            .set_up_provider(
                "google",
                "oauth-client",
                serde_json::json!({
                    "client_id": CLIENT_ID,
                    "client_secret": "GOCSPX-installation",
                }),
            )
            .await;
        assert_eq!(status, 200, "{registered}");
        Brokered {
            connection_id: create_brokered_connection(&daemon, "google").await,
            cookie_name: match daemon.public_origin.starts_with("https://") {
                true => HOST_ONLY_COOKIE,
                false => LOOPBACK_COOKIE,
            },
            daemon,
            google,
            gog,
            registered,
        }
    }

    /// Create a Google Connection of [`ACCOUNT`] under `alias` as the
    /// seeded Person, and answer its id. The installation holds the
    /// Installation OAuth Client, so the Connection is `brokered`.
    async fn create_brokered_connection(daemon: &TestDaemon, alias: &str) -> String {
        let created: serde_json::Value = browser()
            .post(format!("{}/api/v1/settings/connections", daemon.base_url))
            .header("cookie", daemon.cookie())
            .json(&serde_json::json!({
                "provider": "google",
                "alias": alias,
                "display_name": "Work Google",
                "fields": { "account": ACCOUNT },
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(created["auth_mode"], "brokered", "{created}");
        created["id"].as_str().unwrap().to_string()
    }

    impl Brokered {
        /// Authorize the Connection as the Session of `session`, and
        /// answer the address that the Product App opens.
        async fn authorize(&self, session: &str, capabilities: &[&str]) -> String {
            self.authorize_on(&self.connection_id, session, capabilities)
                .await
        }

        /// [`Brokered::authorize`] for the Connection `connection_id`.
        async fn authorize_on(
            &self,
            connection_id: &str,
            session: &str,
            capabilities: &[&str],
        ) -> String {
            let response = browser()
                .post(format!(
                    "{}/api/v1/settings/connections/{connection_id}/authorize",
                    self.daemon.base_url
                ))
                .header("cookie", session)
                .json(&serde_json::json!({ "capabilities": capabilities }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let answer: serde_json::Value = response.json().await.unwrap();
            assert_eq!(answer["connection"]["status"], "connecting");
            answer["authorization_url"]
                .as_str()
                .expect("a brokered Connection answers an address")
                .to_string()
        }

        /// Open the start route in a browser that holds the Session
        /// `session`, or no Session at all. A Public Origin that is not
        /// the listener of the test is a proxy in front of it, so the
        /// request goes to the listener with the same path.
        async fn open_start(&self, url: &str, session: Option<&str>) -> reqwest::Response {
            let path = url
                .strip_prefix(&self.daemon.public_origin)
                .filter(|path| path.starts_with("/api/v1/connections/google/start?"))
                .unwrap_or_else(|| {
                    panic!(
                        "the authorize request answered {url}, not the start route on the \
                         Public Origin"
                    )
                });
            let request = browser().get(format!("{}{path}", self.daemon.base_url));
            match session {
                Some(cookie) => request.header("cookie", cookie),
                None => request,
            }
            .send()
            .await
            .unwrap()
        }

        /// Authorize as the Session `session` and open the start route in
        /// the same browser. Answer the `state`, and the transaction
        /// cookie as that browser sends it back.
        async fn begin_as(&self, session: &str, capabilities: &[&str]) -> (String, String) {
            self.begin_on(&self.connection_id, session, capabilities)
                .await
        }

        /// [`Brokered::begin_as`] for the Connection `connection_id`.
        async fn begin_on(
            &self,
            connection_id: &str,
            session: &str,
            capabilities: &[&str],
        ) -> (String, String) {
            let url = self
                .authorize_on(connection_id, session, capabilities)
                .await;
            let started = self.open_start(&url, Some(session)).await;
            assert_eq!(started.status(), 303, "the start route refused its Person");
            let transaction = set_cookie(&started, self.cookie_name)
                .expect("the start route sets the transaction cookie");
            (state_of(&location(&started)), cookie_pair(&transaction))
        }

        /// [`Brokered::begin_as`] as the seeded Person.
        async fn begin(&self, capabilities: &[&str]) -> (String, String) {
            self.begin_as(self.daemon.cookie(), capabilities).await
        }

        /// The redirect of Google, as a browser brings it to the callback.
        /// The Session cookie is `SameSite=Strict`, so a browser never
        /// sends it on this cross-site navigation. `cookies` is all that
        /// the browser sends.
        async fn callback(&self, state: &str, code: &str, cookies: Option<&str>) -> Landed {
            let request = browser().get(format!(
                "{}/api/v1/connections/google/callback?state={state}&code={code}",
                self.daemon.base_url
            ));
            let response = match cookies {
                Some(cookies) => request.header("cookie", cookies),
                None => request,
            }
            .send()
            .await
            .unwrap();
            let status = response.status().as_u16();
            let clears_the_cookie = set_cookie(&response, self.cookie_name).is_some_and(|value| {
                value.starts_with(&format!("{}=;", self.cookie_name)) && value.contains("Max-Age=0")
            });
            Landed {
                status,
                clears_the_cookie,
                page: response.text().await.unwrap(),
            }
        }

        /// The Connection as the connection list shows it.
        async fn connection(&self) -> serde_json::Value {
            let listed: serde_json::Value = browser()
                .get(format!(
                    "{}/api/v1/settings/connections",
                    self.daemon.base_url
                ))
                .header("cookie", self.daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            listed["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == self.connection_id.as_str())
                .cloned()
                .expect("the Connection is listed")
        }

        /// The sealed refresh token that the Connection holds.
        async fn refresh_token(&self) -> Option<SealedSecret> {
            self.daemon
                .stores()
                .connections
                .refresh_token(
                    &self.daemon.workspace_id,
                    &ConnectionId::from(self.connection_id.clone()),
                )
                .await
                .unwrap()
        }
    }

    /// What the callback answered a browser.
    struct Landed {
        status: u16,
        /// The answer expires the transaction cookie of the browser.
        clears_the_cookie: bool,
        page: String,
    }

    /// The `state` query parameter of an address.
    fn state_of(url: &str) -> String {
        url.split(['?', '&'])
            .find_map(|pair| pair.strip_prefix("state="))
            .expect("the address carries a state")
            .to_string()
    }

    fn location(response: &reqwest::Response) -> String {
        response
            .headers()
            .get(LOCATION)
            .expect("the answer is a redirect")
            .to_str()
            .unwrap()
            .to_string()
    }

    /// The whole `Set-Cookie` value that one answer sets for `name`.
    fn set_cookie(response: &reqwest::Response, name: &str) -> Option<String> {
        response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| {
                value
                    .split(';')
                    .next()
                    .and_then(|pair| pair.split_once('='))
                    .is_some_and(|(key, _)| key.trim() == name)
            })
            .map(str::to_string)
    }

    /// The `name=value` pair of a `Set-Cookie` value, as a browser sends
    /// it back.
    fn cookie_pair(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().trim().to_string()
    }

    /// Trade the Client Credential for a Session of the Client App, as
    /// the Client App does on this machine. Answer its `Cookie` header.
    async fn client_app_session(daemon: &TestDaemon) -> String {
        let traded = browser()
            .post(format!("{}/api/v1/sessions/client", daemon.base_url))
            .json(&serde_json::json!({
                "credential": daemon.client_credential(),
                "client_name": "owner-mac",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(traded.status(), 200);
        cookie_pair(&set_cookie(&traded, SESSION_COOKIE).unwrap())
    }

    /// A Member of the same Org with a Workspace and a Session of their
    /// own. Answer the `Cookie` header of that Session.
    async fn another_person(daemon: &TestDaemon) -> String {
        let now = now_ms();
        let org = daemon.stores().orgs.list().await.unwrap().remove(0);
        let grace = User {
            email: Some("grace@example.com".to_string()),
            name: Some("Grace".to_string()),
            ..User::new(org.id, UserRole::Member, now)
        };
        daemon.stores().users.create(&grace).await.unwrap();
        daemon
            .stores()
            .workspaces
            .create(&Workspace {
                id: WorkspaceId::generate(),
                user_id: grace.id.clone(),
                name: "Grace".to_string(),
                timezone: "UTC".to_string(),
                created_at: now,
                onboarded_at: Some(now),
                chief_of_staff_agent_id: None,
                report_schedule_id: None,
                home_exit_host_id: None,
            })
            .await
            .unwrap();
        daemon.cookie_for(&grace.id).await
    }

    /// The forwarded consent. A Member starts an authorization for a
    /// Connection whose account is somebody else's and sends the address
    /// to that person. That person consents in a browser that holds no
    /// transaction cookie. The daemon redeems no code, stores no refresh
    /// token and does not connect the Connection.
    #[tokio::test]
    async fn a_callback_without_the_transaction_cookie_connects_nothing() {
        let h = brokered().await;
        let url = h.authorize(h.daemon.cookie(), &["gmail_read"]).await;

        let landed = h.callback(&state_of(&url), "victims-code", None).await;

        assert_ne!(
            h.connection().await["status"],
            "connected",
            "a browser without the transaction cookie connected the Connection"
        );
        assert!(
            h.refresh_token().await.is_none(),
            "a refresh token was stored"
        );
        assert!(
            h.google.exchanges().is_empty(),
            "the daemon redeemed the code of a browser it did not bind"
        );
        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
    }

    /// A browser without the binding carries a code that the daemon must
    /// never redeem, so that callback spends the `state`. The initiating
    /// browser cannot finish it afterwards with the code of the other
    /// browser.
    #[tokio::test]
    async fn a_callback_without_the_transaction_cookie_spends_the_state() {
        let h = brokered().await;
        let (state, transaction) = h.begin(&["gmail_read"]).await;

        let forwarded = h.callback(&state, "victims-code", None).await;
        let late = h.callback(&state, "victims-code", Some(&transaction)).await;

        assert!(forwarded.page.contains(REFUSED), "{}", forwarded.page);
        assert!(late.page.contains(REFUSED), "{}", late.page);
        assert!(h.google.exchanges().is_empty());
        assert!(h.refresh_token().await.is_none());
    }

    /// The cookie of one `state` binds no other `state`.
    #[tokio::test]
    async fn a_transaction_cookie_of_another_state_binds_nothing() {
        let h = brokered().await;
        let (state, _) = h.begin(&["gmail_read"]).await;
        let other = format!(
            "{}={}",
            h.cookie_name,
            pagis_server::hash_secret("another-state")
        );

        let landed = h.callback(&state, "alices-code", Some(&other)).await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
        assert!(h.google.exchanges().is_empty());
        assert!(h.refresh_token().await.is_none());
    }

    /// In Remote Access the start route requires a Session of the
    /// Person who started the authorization. A browser with no Session
    /// goes to sign in. Another Person of the Org, who opens a forwarded
    /// start address, gets no transaction cookie and no Google address.
    #[tokio::test]
    async fn in_remote_access_the_start_route_requires_a_session_of_the_initiating_person() {
        let h = brokered_in_remote_access().await;
        let url = h.authorize(h.daemon.cookie(), &["gmail_read"]).await;
        let grace = another_person(&h.daemon).await;

        let unsigned = h.open_start(&url, None).await;
        let refused = h.open_start(&url, Some(&grace)).await;

        assert_eq!(unsigned.status(), 303);
        assert_eq!(
            location(&unsigned),
            format!("/connections/google/start?state={}", state_of(&url))
        );
        assert!(set_cookie(&unsigned, h.cookie_name).is_none());
        assert_eq!(refused.status(), 403);
        assert!(set_cookie(&refused, h.cookie_name).is_none());
        assert!(refused.headers().get(LOCATION).is_none());
        // The refusals spend nothing: the initiating Person goes on.
        let started = h.open_start(&url, Some(h.daemon.cookie())).await;
        assert_eq!(started.status(), 303);
        assert!(location(&started).starts_with(GOOGLE));
    }

    /// On a Local Installation with Remote Access off, the Client
    /// App opens the start route in the system browser, which holds no
    /// Session. That installation has one Person, so the start route asks
    /// for no sign-in: it sets the transaction cookie and sends the
    /// browser to Google, and the flow finishes.
    #[tokio::test]
    async fn on_a_single_person_installation_the_system_browser_goes_to_google_without_a_session() {
        let h = brokered().await;
        let client_app = client_app_session(&h.daemon).await;
        let url = h.authorize(&client_app, &["gmail_read"]).await;
        let state = state_of(&url);

        let started = h.open_start(&url, None).await;

        assert_eq!(started.status(), 303);
        let google = location(&started);
        assert!(google.starts_with(&format!("{GOOGLE}?")), "{google}");
        assert_eq!(state_of(&google), state);
        let transaction = cookie_pair(
            &set_cookie(&started, LOOPBACK_COOKIE)
                .expect("the start route sets the transaction cookie"),
        );
        let landed = h.callback(&state, "owners-code", Some(&transaction)).await;
        assert!(
            landed.page.contains("Google is connected"),
            "{}",
            landed.page
        );
        assert!(landed.clears_the_cookie);
        assert_eq!(h.connection().await["status"], "connected");
        assert!(h.refresh_token().await.is_some());
    }

    /// On that installation the callback still requires the transaction
    /// cookie. A browser without it connects nothing and spends the
    /// `state`, so the browser that went to Google cannot finish it later
    /// with the code of the other browser.
    #[tokio::test]
    async fn on_a_single_person_installation_a_callback_without_the_transaction_cookie_still_connects_nothing()
     {
        let h = brokered().await;
        let url = h.authorize(h.daemon.cookie(), &["gmail_read"]).await;
        let started = h.open_start(&url, None).await;
        assert_eq!(started.status(), 303, "the start route asked for a sign-in");
        let transaction = cookie_pair(
            &set_cookie(&started, LOOPBACK_COOKIE)
                .expect("the start route sets the transaction cookie"),
        );
        let state = state_of(&location(&started));

        let forwarded = h.callback(&state, "victims-code", None).await;
        let late = h.callback(&state, "victims-code", Some(&transaction)).await;

        assert!(forwarded.page.contains(REFUSED), "{}", forwarded.page);
        assert!(forwarded.clears_the_cookie);
        assert!(late.page.contains(REFUSED), "{}", late.page);
        assert_ne!(h.connection().await["status"], "connected");
        assert!(
            h.refresh_token().await.is_none(),
            "a refresh token was stored"
        );
        assert!(h.google.exchanges().is_empty());
    }

    /// On that installation the pending authorization still holds the
    /// Session of the Client App. A sign-out in the Client App between
    /// start and callback ends the authorization.
    #[tokio::test]
    async fn on_a_single_person_installation_a_sign_out_in_the_client_app_ends_the_authorization() {
        let h = brokered().await;
        let client_app = client_app_session(&h.daemon).await;
        let url = h.authorize(&client_app, &["gmail_read"]).await;
        let started = h.open_start(&url, None).await;
        let transaction = cookie_pair(
            &set_cookie(&started, LOOPBACK_COOKIE)
                .expect("the start route sets the transaction cookie"),
        );
        h.daemon.sign_out(&client_app).await;

        let landed = h
            .callback(&state_of(&url), "owners-code", Some(&transaction))
            .await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
        assert!(h.google.exchanges().is_empty());
        assert!(h.refresh_token().await.is_none());
    }

    /// On an `https:` Public Origin, where a Server and a Local
    /// Installation in Remote Access are, the start route sets the
    /// transaction cookie with the `__Host-` prefix and sends the browser
    /// to Google. The cookie holds the hash of the `state` and not the
    /// `state` itself, and it lives as long as the authorization waits.
    /// The callback reads it back.
    #[tokio::test]
    async fn the_start_route_sets_the_transaction_cookie_and_redirects_to_google() {
        let h = brokered_in_remote_access().await;
        let url = h.authorize(h.daemon.cookie(), &["gmail_read"]).await;
        let state = state_of(&url);

        let started = h.open_start(&url, Some(h.daemon.cookie())).await;

        assert_eq!(started.status(), 303);
        let google = location(&started);
        assert!(google.starts_with(&format!("{GOOGLE}?")), "{google}");
        assert_eq!(state_of(&google), state);
        assert!(
            google.contains(
                "scope=openid%20email%20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fgmail.readonly"
            ),
            "{google}"
        );
        assert!(
            google.contains("login_hint=alice%40example.com"),
            "{google}"
        );
        assert!(
            google.contains(
                "redirect_uri=https%3A%2F%2Fpagis.owner.example%2Fapi%2Fv1%2Fconnections%2Fgoogle%2Fcallback"
            ),
            "{google}"
        );
        let cookie = set_cookie(&started, HOST_ONLY_COOKIE)
            .expect("the start route sets the transaction cookie");
        let attributes: Vec<&str> = cookie.split(';').map(str::trim).collect();
        assert_eq!(
            attributes[0],
            format!("{HOST_ONLY_COOKIE}={}", pagis_server::hash_secret(&state))
        );
        for attribute in [
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            "Path=/",
            "Max-Age=600",
        ] {
            assert!(
                attributes.contains(&attribute),
                "{cookie} has no {attribute}"
            );
        }
        assert!(!cookie.to_lowercase().contains("domain"), "{cookie}");
        assert!(
            !cookie.contains(&state),
            "the cookie carries the state itself"
        );
        assert!(h.google.exchanges().is_empty(), "nobody consented yet");

        let landed = h
            .callback(&state, "alices-code", Some(&cookie_pair(&cookie)))
            .await;

        assert!(
            landed.page.contains("Google is connected"),
            "{}",
            landed.page
        );
        assert!(landed.clears_the_cookie);
        assert_eq!(h.connection().await["status"], "connected");
    }

    /// A Local Installation serves its Public Origin over plain `http:` on
    /// loopback. A browser refuses a `Secure` cookie from there, as it
    /// does the Session cookie, and the `__Host-` prefix needs `Secure`.
    /// So the cookie has neither and keeps every other attribute.
    #[tokio::test]
    async fn on_a_loopback_origin_the_transaction_cookie_leaves_out_secure() {
        let h = brokered().await;
        assert!(h.daemon.public_origin.starts_with("http://127.0.0.1:"));
        let url = h.authorize(h.daemon.cookie(), &["gmail_read"]).await;
        let state = state_of(&url);

        let started = h.open_start(&url, Some(h.daemon.cookie())).await;

        assert_eq!(started.status(), 303);
        assert!(set_cookie(&started, HOST_ONLY_COOKIE).is_none());
        let cookie = set_cookie(&started, LOOPBACK_COOKIE)
            .expect("the start route sets the transaction cookie");
        let attributes: Vec<&str> = cookie.split(';').map(str::trim).collect();
        assert_eq!(
            attributes[0],
            format!("{LOOPBACK_COOKIE}={}", pagis_server::hash_secret(&state))
        );
        for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=600"] {
            assert!(
                attributes.contains(&attribute),
                "{cookie} has no {attribute}"
            );
        }
        assert!(!attributes.contains(&"Secure"), "{cookie}");
        assert!(!cookie.to_lowercase().contains("domain"), "{cookie}");
    }

    /// The whole flow for the initiating Person. The Administrator sees
    /// the redirect URI to paste into the Google console, the form asks
    /// for the account alone, and the callback that brings the
    /// transaction cookie seals the refresh token and connects the
    /// Connection. Google compares addresses without regard to case, and
    /// so does the daemon.
    #[tokio::test]
    async fn the_initiating_browser_finishes_the_consent_and_reaches_connected() {
        let h = brokered().await;
        let facts = h.registered["parts"][0]["facts"].as_array().unwrap();
        let fact = |label: &str| {
            facts
                .iter()
                .find(|fact| fact["label"] == label)
                .map(|fact| fact["value"].clone())
                .unwrap_or_else(|| panic!("no {label} in {}", h.registered))
        };
        assert_eq!(fact("Client ID"), CLIENT_ID);
        assert_eq!(
            fact("Redirect URI"),
            format!(
                "{}/api/v1/connections/google/callback",
                h.daemon.public_origin
            )
        );
        h.google.consent_as("Alice@Example.com", true);

        let (state, transaction) = h.begin(&["gmail_read"]).await;
        assert!(h.gog.commands().is_empty(), "the brokered flow runs no gog");
        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert_eq!(landed.status, 200);
        assert!(
            landed.page.contains("Google is connected"),
            "{}",
            landed.page
        );
        // The page names no account and no Workspace.
        assert!(
            !landed.page.to_lowercase().contains(ACCOUNT),
            "{}",
            landed.page
        );
        assert!(landed.clears_the_cookie);
        let posted = h.google.exchanges()[0].clone();
        assert!(posted.contains("grant_type=authorization_code"), "{posted}");
        assert!(posted.contains("code=alices-code"), "{posted}");
        assert!(posted.contains("code_verifier="), "{posted}");
        let connection = h.connection().await;
        assert_eq!(connection["status"], "connected");
        assert_eq!(connection["auth_mode"], "brokered");
        assert_eq!(
            connection["authorized_capabilities"],
            serde_json::json!(["gmail_read"])
        );
        assert!(h.refresh_token().await.is_some());
        // No interface shows the tokens.
        let body = connection.to_string();
        assert!(!body.contains("1//refresh-for-alices-code"), "{body}");
        assert!(!body.contains("ya29."), "{body}");
    }

    /// `login_hint` is a hint: the person at the consent screen can pick
    /// any Google account. The daemon reads the ID token and keeps the
    /// token only for the account of the Connection.
    #[tokio::test]
    async fn a_consent_from_another_google_account_stores_no_token() {
        let h = brokered().await;
        h.google.consent_as("mallory@example.com", true);
        let (state, transaction) = h.begin(&["gmail_read"]).await;

        let landed = h
            .callback(&state, "mallorys-code", Some(&transaction))
            .await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
        assert!(
            h.refresh_token().await.is_none(),
            "a refresh token was stored"
        );
        assert_eq!(h.connection().await["status"], "disconnected");
    }

    /// An address that Google did not verify proves no account.
    #[tokio::test]
    async fn a_consent_from_an_unverified_address_stores_no_token() {
        let h = brokered().await;
        h.google.consent_as(ACCOUNT, false);
        let (state, transaction) = h.begin(&["gmail_read"]).await;

        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(
            h.refresh_token().await.is_none(),
            "a refresh token was stored"
        );
        assert_eq!(h.connection().await["status"], "disconnected");
    }

    /// A person can clear a box on the consent screen, and
    /// `include_granted_scopes=true` returns what the account granted this
    /// client before. The Connection records only what it asked for and
    /// Google granted.
    #[tokio::test]
    async fn only_the_requested_capabilities_that_google_granted_are_recorded() {
        let h = brokered().await;
        h.google.grant(&[GMAIL_READ, GMAIL_SEND]);
        let (state, transaction) = h.begin(&["gmail_read", "calendar_read"]).await;

        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert!(
            landed.page.contains("Google is connected"),
            "{}",
            landed.page
        );
        let connection = h.connection().await;
        assert_eq!(connection["status"], "connected");
        assert_eq!(
            connection["authorized_capabilities"],
            serde_json::json!(["gmail_read"])
        );
    }

    /// A result that grants none of the requested capabilities is
    /// refused, and the daemon keeps no token of it.
    #[tokio::test]
    async fn a_result_that_grants_no_requested_capability_stores_no_token() {
        let h = brokered().await;
        h.google.grant(&[GMAIL_SEND]);
        let (state, transaction) = h.begin(&["gmail_read", "calendar_read"]).await;

        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(
            h.refresh_token().await.is_none(),
            "a refresh token was stored"
        );
        let connection = h.connection().await;
        assert_eq!(connection["status"], "disconnected");
        assert_eq!(connection["authorized_capabilities"], serde_json::json!([]));
    }

    /// A `state` is spent on first use. A replay of the redirect reaches
    /// no Google call and keeps the token of the first callback.
    #[tokio::test]
    async fn a_replayed_state_stores_no_token_and_clears_the_cookie() {
        let h = brokered().await;
        let (state, transaction) = h.begin(&["gmail_read"]).await;
        let first = h.callback(&state, "alices-code", Some(&transaction)).await;
        assert!(first.page.contains("Google is connected"), "{}", first.page);
        let kept = h
            .refresh_token()
            .await
            .expect("the first callback stores a token");

        let replayed = h
            .callback(&state, "replayed-code", Some(&transaction))
            .await;

        assert!(replayed.page.contains(REFUSED), "{}", replayed.page);
        assert!(replayed.clears_the_cookie);
        assert_eq!(h.google.exchanges().len(), 1, "the replay reached Google");
        assert_eq!(
            h.refresh_token().await,
            Some(kept),
            "the replay replaced the token"
        );
    }

    /// An authorization waits ten minutes. A redirect after that finds
    /// nothing.
    #[tokio::test]
    async fn an_expired_state_stores_no_token_and_clears_the_cookie() {
        let now = now_ms();
        let clock = FixtureClock::at(now);
        let h = brokered_with(TestDaemonOptions {
            clock: Arc::new(clock.clone()),
            ..TestDaemonOptions::default()
        })
        .await;
        let (state, transaction) = h.begin(&["gmail_read"]).await;
        clock.advance_to(now + pagis_connect::AUTHORIZE_WINDOW_MS + 60_000);

        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
        assert!(h.google.exchanges().is_empty());
        assert!(h.refresh_token().await.is_none());
    }

    /// The callback checks the initiating Person through the Session that
    /// started the authorization, because the Session cookie does not
    /// come with the redirect of Google. A sign-out ends that Session and
    /// the authorization with it.
    #[tokio::test]
    async fn a_sign_out_between_start_and_callback_stores_no_token_and_clears_the_cookie() {
        let h = brokered().await;
        let session = h.daemon.cookie_for(&h.daemon.user_id).await;
        let (state, transaction) = h.begin_as(&session, &["gmail_read"]).await;
        h.daemon.sign_out(&session).await;

        let landed = h.callback(&state, "alices-code", Some(&transaction)).await;

        assert!(landed.page.contains(REFUSED), "{}", landed.page);
        assert!(landed.clears_the_cookie);
        assert!(h.google.exchanges().is_empty());
        assert!(h.refresh_token().await.is_none());
    }

    /// In Remote Access the Client App opens the start route in the
    /// system browser, which holds no Session. The start route sends that
    /// browser to sign in at a Product App address that comes back to the
    /// start route, and the Person who started the authorization finishes
    /// it there. The authorization holds the Session of the Client App.
    #[tokio::test]
    async fn in_remote_access_the_client_app_person_signs_in_in_the_system_browser_and_finishes() {
        let h = brokered_in_remote_access().await;
        let base = h.daemon.base_url.clone();
        h.daemon
            .stores()
            .users
            .set_email_and_password(
                &h.daemon.user_id,
                "owner@example.com",
                &pagis_server::hash_password("a good password").unwrap(),
                now_ms(),
            )
            .await
            .unwrap();
        let client_app = client_app_session(&h.daemon).await;
        let url = h.authorize(&client_app, &["gmail_read"]).await;
        let state = state_of(&url);

        // The system browser holds no Session yet.
        let unsigned = h.open_start(&url, None).await;
        assert_eq!(unsigned.status(), 303);
        assert!(set_cookie(&unsigned, h.cookie_name).is_none());
        assert_eq!(
            location(&unsigned),
            format!("/connections/google/start?state={state}")
        );
        // The Person signs in there as the initiating Person, and the
        // Product App at that address comes back to the start route.
        let signed_in = browser()
            .post(format!("{base}/api/v1/sessions"))
            .json(&serde_json::json!({
                "email": "owner@example.com",
                "password": "a good password",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(signed_in.status(), 200);
        let system_browser = cookie_pair(&set_cookie(&signed_in, SESSION_COOKIE).unwrap());
        let started = h.open_start(&url, Some(&system_browser)).await;
        assert_eq!(started.status(), 303);
        assert!(location(&started).starts_with(GOOGLE));
        let transaction = cookie_pair(&set_cookie(&started, h.cookie_name).unwrap());

        let landed = h.callback(&state, "owners-code", Some(&transaction)).await;

        assert!(
            landed.page.contains("Google is connected"),
            "{}",
            landed.page
        );
        assert_eq!(h.connection().await["status"], "connected");
        assert!(h.refresh_token().await.is_some());
    }

    /// The Tenant Data Key of one Workspace, which the Vault and the
    /// Google broker both use. The daemon runs on a `secrets.enc` in a
    /// directory of the test, so a test reads the file that a restart
    /// reads.
    mod tenant_data_key {
        use std::sync::Condvar;
        use std::time::Duration;

        use pagis_core::{DataKey, SecretError, SecretStore, data_key_name};

        use super::*;

        /// The Installation Key that seals the `secrets.enc` of a test.
        const INSTALLATION_KEY: [u8; 32] = [7; 32];

        /// The secret store of the daemon: `secrets.enc` in a directory
        /// of the test. It counts the loads of the Tenant Data Key of one
        /// Workspace. Each load answers only after a second load arrives
        /// or a quarter second passes, so two loads that start together
        /// overlap every time, whatever the timing of the requests.
        struct Watched {
            file: pagis::EncryptedFileSecretStore,
            watched: Mutex<Option<String>>,
            loads: Mutex<usize>,
            arrived: Condvar,
        }

        impl Watched {
            fn new(dir: &std::path::Path) -> Self {
                Self {
                    file: pagis::EncryptedFileSecretStore::new(
                        dir.join("secrets.enc"),
                        INSTALLATION_KEY,
                    ),
                    watched: Mutex::new(None),
                    loads: Mutex::new(0),
                    arrived: Condvar::new(),
                }
            }

            /// Count and hold the loads of the key of `workspace_id`.
            fn watch(&self, workspace_id: &WorkspaceId) {
                *self.watched.lock().unwrap() = Some(data_key_name(workspace_id));
            }

            fn loads(&self) -> usize {
                *self.loads.lock().unwrap()
            }

            fn load<T>(&self, name: &str, read: impl FnOnce() -> T) -> T {
                let value = read();
                if self.watched.lock().unwrap().as_deref() == Some(name) {
                    let mut loads = self.loads.lock().unwrap();
                    *loads += 1;
                    self.arrived.notify_all();
                    let wait = Duration::from_millis(250);
                    drop(
                        self.arrived
                            .wait_timeout_while(loads, wait, |loads| *loads < 2)
                            .unwrap(),
                    );
                }
                value
            }
        }

        impl SecretStore for Watched {
            fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
                self.load(name, || self.file.get(name))
            }

            fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
                self.file.set(name, value)
            }

            fn get_or_insert(&self, name: &str, value: &str) -> Result<String, SecretError> {
                self.load(name, || self.file.get_or_insert(name, value))
            }

            fn delete(&self, name: &str) -> Result<(), SecretError> {
                self.file.delete(name)
            }
        }

        /// A daemon on the store `secrets`, whose Tenant Data Key of the
        /// seeded Workspace the store watches.
        async fn watched(secrets: &Arc<Watched>) -> Brokered {
            let h = brokered_with(TestDaemonOptions {
                secrets: Arc::clone(secrets) as _,
                ..TestDaemonOptions::default()
            })
            .await;
            secrets.watch(&h.daemon.workspace_id);
            h
        }

        /// Save the Credential `ada-{index}` through the Vault, and answer
        /// the status.
        async fn save_credential(daemon: &TestDaemon, index: usize) -> u16 {
            browser()
                .post(format!("{}/api/v1/settings/credentials", daemon.base_url))
                .header("cookie", daemon.cookie())
                .json(&serde_json::json!({
                    "domain": "example.com",
                    "username": format!("ada-{index}"),
                    "login_url": "https://accounts.example.com/signin",
                    "secret": format!("secret-{index}"),
                }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }

        /// Many first uses of one Workspace's Tenant Data Key start
        /// together: Credential saves through the Vault, and consents
        /// through the Google broker. All callers seal with one key, and
        /// `secrets.enc` holds that key, so each sealed secret opens after
        /// a restart.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn concurrent_first_uses_through_the_vault_and_the_broker_seal_with_one_stored_key() {
            const SAVES: usize = 6;
            const CONSENTS: usize = 3;
            let dir = tempfile::tempdir().unwrap();
            let secrets = Arc::new(Watched::new(dir.path()));
            let h = watched(&secrets).await;
            let mut consents = Vec::new();
            for index in 0..CONSENTS {
                let connection_id = match index {
                    0 => h.connection_id.clone(),
                    _ => create_brokered_connection(&h.daemon, &format!("google-{index}")).await,
                };
                let (state, transaction) = h
                    .begin_on(&connection_id, h.daemon.cookie(), &["gmail_read"])
                    .await;
                consents.push((connection_id, state, transaction, format!("code-{index}")));
            }

            let saves = (0..SAVES).map(|index| save_credential(&h.daemon, index));
            let finishes = consents
                .iter()
                .map(|(_, state, transaction, code)| h.callback(state, code, Some(transaction)));
            let (saved, landed) = tokio::join!(
                futures::future::join_all(saves),
                futures::future::join_all(finishes)
            );

            assert!(saved.iter().all(|status| *status == 201), "{saved:?}");
            for landed in &landed {
                assert!(
                    landed.page.contains("Google is connected"),
                    "{}",
                    landed.page
                );
            }
            // The key as a restart reads it: from `secrets.enc`, with the
            // Installation Key.
            let workspace_id = &h.daemon.workspace_id;
            let file = pagis::EncryptedFileSecretStore::new(
                dir.path().join("secrets.enc"),
                INSTALLATION_KEY,
            );
            assert!(
                file.get(&data_key_name(workspace_id)).unwrap().is_some(),
                "secrets.enc holds no Tenant Data Key"
            );
            let key = DataKey::load(&file, workspace_id).unwrap();
            let credentials = h
                .daemon
                .stores()
                .credentials
                .list(workspace_id, None)
                .await
                .unwrap();
            assert_eq!(credentials.len(), SAVES);
            for credential in &credentials {
                let index = credential.username.trim_start_matches("ada-");
                assert_eq!(
                    key.open(&credential.secret),
                    Ok(format!("secret-{index}")),
                    "the Credential {} is sealed with a key that secrets.enc does not hold",
                    credential.username
                );
            }
            for (connection_id, _, _, code) in &consents {
                let sealed = h
                    .daemon
                    .stores()
                    .connections
                    .refresh_token(workspace_id, &ConnectionId::from(connection_id.clone()))
                    .await
                    .unwrap()
                    .expect("the consent stored a refresh token");
                assert_eq!(
                    key.open(&sealed),
                    Ok(format!("1//refresh-for-{code}")),
                    "the refresh token of the consent {code} is sealed with a key that \
                     secrets.enc does not hold"
                );
            }
        }

        /// The Vault and the Google broker share one holder of the Tenant
        /// Data Keys: the key that a Credential save loads is the key that
        /// the next consent seals with, and the store is not asked for it
        /// again.
        #[tokio::test]
        async fn the_vault_and_the_broker_share_one_holder_of_the_tenant_data_keys() {
            let dir = tempfile::tempdir().unwrap();
            let secrets = Arc::new(Watched::new(dir.path()));
            let h = watched(&secrets).await;

            assert_eq!(save_credential(&h.daemon, 0).await, 201);
            let (state, transaction) = h.begin(&["gmail_read"]).await;
            let landed = h.callback(&state, "code-0", Some(&transaction)).await;

            assert!(
                landed.page.contains("Google is connected"),
                "{}",
                landed.page
            );
            assert_eq!(
                secrets.loads(),
                1,
                "the Vault and the Google broker each loaded the Tenant Data Key"
            );
        }
    }
}
