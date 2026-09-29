//! Full-daemon vault tests (ADR-0013): the record binds a fill to
//! its address, the daemon owns the navigation and writes only into a
//! page it verified, through its browser channel and never as
//! keystrokes, the credential grant decides whether a card appears, and
//! no secret reaches the model.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_computer::fake::FakeComputerRuntime;
use pagis_computer::{FieldKind, FillField, InputHolder};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions, TwoTenants};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The RFC 6238 test seed.
const TOTP_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    computer: Arc<FakeComputerRuntime>,
    secrets: Arc<dyn pagis_core::SecretStore>,
}

async fn boot() -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let computer = Arc::new(FakeComputerRuntime::with_image());
    let secrets: Arc<dyn pagis_core::SecretStore> =
        Arc::new(pagis_core::MemorySecretStore::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        computer: Arc::clone(&computer) as _,
        secrets: Arc::clone(&secrets),
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        brain,
        computer,
        secrets,
    }
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

/// Every frame up to and including the run's `completed` transition.
async fn frames_until_completed(socket: &mut Socket) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(socket).await;
        let completed = frame["type"] == "run.state_changed"
            && frame["payload"]["payload"]["to"] == "completed";
        let failed =
            frame["type"] == "run.state_changed" && frame["payload"]["payload"]["to"] == "failed";
        frames.push(frame);
        if completed || failed {
            return frames;
        }
    }
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
        .unwrap();
    assert_eq!(response.status(), 201);
}

async fn add_credential(daemon: &TestDaemon, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/api/v1/settings/credentials", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn list_credentials(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!("{}/api/v1/settings/credentials", daemon.base_url))
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
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
}

/// A user credential the daemon can fill.
async fn seed_login(daemon: &TestDaemon, secret: &str, totp: Option<&str>) -> String {
    let response = add_credential(
        daemon,
        serde_json::json!({
            "domain": "example.com",
            "username": "alice@example.com",
            "login_url": "https://accounts.example.com/signin",
            "secret": secret,
            "totp_seed": totp,
        }),
    )
    .await;
    assert_eq!(response.status(), 201);
    response.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Run one vault tool call and settle its request card.
async fn call_tool(
    h: &Harness,
    socket: &mut Socket,
    pending_id: &str,
    tool: &str,
    arguments: serde_json::Value,
    decision: Option<(&str, Option<&str>)>,
) -> Vec<serde_json::Value> {
    h.brain.push(Script::tool_call(&[], tool, arguments));
    h.brain.push(Script::reply(&["Done."]));
    send(&h.daemon, pending_id, "go").await;
    if let Some((decision, scope)) = decision {
        let requested = next_frame_of(socket, "request.created").await;
        let request_id = requested["payload"]["payload"]["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        decide(&h.daemon, &request_id, decision, scope).await;
    }
    frames_until_completed(socket).await
}

fn tool_fact<'a>(frames: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    frames
        .iter()
        .find(|frame| {
            frame["type"] == "tool.completed" && frame["payload"]["payload"]["name"] == name
        })
        .unwrap_or_else(|| panic!("no tool.completed fact for {name}"))
}

// ---- Records ----

#[tokio::test]
async fn a_record_whose_login_url_domain_differs_is_rejected() {
    let h = boot().await;

    let refused = add_credential(
        &h.daemon,
        serde_json::json!({
            "domain": "example.com",
            "username": "alice",
            "login_url": "https://evil.test/login",
            "secret": "hunter2",
        }),
    )
    .await;

    assert_eq!(refused.status(), 422);
    assert!(list_credentials(&h.daemon).await.is_empty());

    // A subdomain of the record's own domain is the same site.
    let accepted = add_credential(
        &h.daemon,
        serde_json::json!({
            "domain": "example.com",
            "username": "alice",
            "login_url": "https://accounts.example.com/signin",
            "secret": "hunter2",
        }),
    )
    .await;
    assert_eq!(accepted.status(), 201);
}

/// A secret sent to an `http` address is readable on the network path,
/// so a record with one is refused when it is written, by a person and
/// by an Agent.
#[tokio::test]
async fn a_record_with_an_http_login_address_is_refused() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;

    let refused = add_credential(
        &h.daemon,
        serde_json::json!({
            "domain": "example.com",
            "username": "alice",
            "login_url": "http://accounts.example.com/signin",
            "secret": "hunter2",
        }),
    )
    .await;
    assert_eq!(refused.status(), 422);

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__create",
        serde_json::json!({
            "domain": "example.com",
            "username": "agent@example.com",
            "login_url": "http://example.com/signup",
        }),
        None,
    )
    .await;
    assert!(
        frames
            .iter()
            .all(|frame| frame["type"] != "request.created"),
        "a record that cannot be written must not raise a card"
    );
    assert!(list_credentials(&h.daemon).await.is_empty());
}

/// A record that holds an `http` login address fills nothing: the check
/// runs again before each fill, not only when the record is written.
#[tokio::test]
async fn a_stored_http_login_address_is_refused_before_the_fill() {
    let h = boot().await;
    // The first record makes the Workspace's data key.
    seed_login(&h.daemon, "correct-horse-battery", None).await;
    let id = store_http_login(&h).await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert!(
        h.computer.browser_opens().is_empty(),
        "an http address was opened"
    );
    assert_failed(&frames, "vault__fill", &id, "https");
}

/// Write a record with an `http` login address straight into the store,
/// past the write check, and answer its id.
async fn store_http_login(h: &Harness) -> String {
    use pagis_core::CredentialStore;
    let workspace_id = h.daemon.workspace_id.clone();
    let key = pagis_vault::DataKey::load(h.secrets.as_ref(), &workspace_id).unwrap();
    let credential = pagis_core::Credential {
        id: pagis_core::CredentialId::generate(),
        workspace_id,
        domain: "example.com".to_string(),
        username: "alice@example.com".to_string(),
        login_url: "http://accounts.example.com/signin".to_string(),
        secret: key.seal("plain-http-secret").unwrap(),
        totp_seed: None,
        recipe: String::new(),
        owner_agent_id: None,
        provenance: pagis_core::CredentialProvenance::UserSupplied,
        created_run_id: None,
        created_at: pagis_core::now_ms(),
    };
    pagis_storage_sqlite::SqliteCredentialStore::new(h.daemon.pool().clone())
        .create(&credential)
        .await
        .unwrap();
    credential.id.to_string()
}

#[tokio::test]
async fn settings_shows_a_saved_login_and_never_its_secret() {
    let h = boot().await;

    seed_login(&h.daemon, "correct-horse-battery", Some(TOTP_SEED)).await;

    let items = list_credentials(&h.daemon).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["domain"], "example.com");
    assert_eq!(items[0]["username"], "alice@example.com");
    assert_eq!(items[0]["provenance"], "user_supplied");
    assert_eq!(items[0]["has_totp"], true);
    let body = serde_json::to_string(&items).unwrap();
    assert!(!body.contains("correct-horse-battery"), "{body}");
    assert!(!body.contains(TOTP_SEED), "{body}");
}

// ---- The fill ----

/// Nothing reached a page and nothing reached the screen: the browser
/// took no fill, and the daemon sent no keystroke.
fn assert_nothing_written(h: &Harness) {
    let fills = h.computer.browser_fills();
    assert!(fills.is_empty(), "a fill wrote into a page: {fills:?}");
    let typed = h.computer.inputs();
    assert!(typed.is_empty(), "the daemon sent keystrokes: {typed:?}");
}

/// The text of the last tool result the model read.
fn last_tool_result(h: &Harness) -> String {
    let requests = h.brain.requests();
    requests[requests.len() - 1]
        .messages
        .last()
        .expect("a tool result")
        .text
        .clone()
}

/// The fill reported that it failed, and its audit fact records that
/// outcome with the record and the reason. The reason names `names`.
fn assert_failed(frames: &[serde_json::Value], tool: &str, id: &str, names: &str) {
    let fact = &tool_fact(frames, tool)["payload"]["payload"];
    assert_eq!(fact["ok"], false, "{fact}");
    assert_eq!(fact["error_code"], "failed", "{fact}");
    assert_eq!(fact["fill"], "failed", "{fact}");
    assert_eq!(fact["credential_id"], id, "{fact}");
    assert_eq!(fact["domain"], "example.com", "{fact}");
    let reason = fact["reason"].as_str().expect("the fact carries a reason");
    assert!(reason.contains(names), "{reason}");
}

#[tokio::test]
async fn a_fill_opens_the_records_address_in_its_own_tab_and_writes_into_verified_fields() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    // The daemon opened the record's own address, and the browser took
    // the username and the secret for the https origin the page is on.
    assert_eq!(
        h.computer.browser_opens(),
        vec!["https://accounts.example.com/signin".to_string()]
    );
    assert_eq!(
        h.computer.browser_fills(),
        vec![(
            "https://accounts.example.com".to_string(),
            vec![
                FillField::text("alice@example.com"),
                FillField::password("correct-horse-battery"),
            ]
        )]
    );
    // Not one keystroke went to the compositor, so the focused window
    // received nothing.
    assert!(h.computer.inputs().is_empty(), "{:?}", h.computer.inputs());
    // The switch came back, and the audit fact names the record and the
    // outcome.
    assert_eq!(
        h.computer.holder(&h.daemon.agent_id.clone().into()),
        InputHolder::Agent
    );
    let fact = tool_fact(&frames, "vault__fill");
    assert_eq!(fact["payload"]["payload"]["domain"], "example.com");
    assert_eq!(fact["payload"]["payload"]["credential_id"], id);
    assert_eq!(
        fact["payload"]["payload"]["login_url"],
        "https://accounts.example.com/signin"
    );
    assert_eq!(fact["payload"]["payload"]["fill"], "filled");
    assert!(fact["payload"]["payload"]["request_id"].is_string());
}

#[tokio::test]
async fn a_fill_whose_address_redirects_to_another_domain_writes_nothing() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    h.computer.redirect(
        "https://accounts.example.com/signin",
        "https://accounts.example.org/signin",
    );
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__fill", &id, "example.org");
    let result = last_tool_result(&h);
    assert!(result.contains("failed"), "{result}");
    assert!(result.contains("example.org"), "{result}");
    assert_eq!(
        h.computer.holder(&h.daemon.agent_id.clone().into()),
        InputHolder::Agent
    );
}

#[tokio::test]
async fn a_fill_that_lands_on_an_http_page_writes_nothing() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    h.computer.redirect(
        "https://accounts.example.com/signin",
        "http://accounts.example.com/signin",
    );
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__fill", &id, "https");
}

#[tokio::test]
async fn a_fill_whose_page_does_not_load_writes_nothing() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    h.computer
        .fail_browser_open("the page did not load in 30 seconds");
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__fill", &id, "did not load");
}

#[tokio::test]
async fn a_fill_that_a_page_check_refuses_reports_the_reason() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    h.computer
        .fail_browser_fill("the password field did not keep the focus");
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__fill", &id, "did not keep the focus");
}

#[tokio::test]
async fn no_secret_reaches_the_model_or_the_event_log() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    let seen = serde_json::to_string(&frames).unwrap();
    assert!(!seen.contains("correct-horse-battery"), "{seen}");
    for request in h.brain.requests() {
        let prompt = format!("{:?}", request.messages);
        assert!(!prompt.contains("correct-horse-battery"), "{prompt}");
        let tools = format!("{:?}", request.tools);
        assert!(!tools.contains("correct-horse-battery"), "{tools}");
    }
}

#[tokio::test]
async fn a_denied_fill_types_nothing_and_the_run_continues() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("denied", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert!(
        h.computer.browser_opens().is_empty(),
        "a denial opened a page"
    );
    assert_eq!(
        h.computer.holder(&h.daemon.agent_id.clone().into()),
        InputHolder::Agent,
        "the daemon never held the switch across the parked run"
    );
    let completed = frames.last().expect("a final frame").clone();
    assert_eq!(completed["payload"]["payload"]["to"], "completed");
}

#[tokio::test]
async fn always_allow_lets_the_next_fill_on_that_domain_run_free() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", None).await;
    let mut socket = firehose(&h.daemon).await;

    call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__fill",
        serde_json::json!({ "id": id }),
        Some(("approved", Some("always"))),
    )
    .await;

    // The same domain: no card.
    let frames = call_tool(
        &h,
        &mut socket,
        "p-2",
        "vault__fill",
        serde_json::json!({ "id": id }),
        None,
    )
    .await;
    assert!(
        frames
            .iter()
            .all(|frame| frame["type"] != "request.created"),
        "an allowed domain must not ask"
    );
    assert_eq!(
        tool_fact(&frames, "vault__fill")["payload"]["payload"]["authorization"],
        "allow_rule"
    );

    // Another domain still asks.
    let other = add_credential(
        &h.daemon,
        serde_json::json!({
            "domain": "other.test",
            "username": "alice",
            "login_url": "https://other.test/login",
            "secret": "another-secret",
        }),
    )
    .await
    .json::<serde_json::Value>()
    .await
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let frames = call_tool(
        &h,
        &mut socket,
        "p-3",
        "vault__fill",
        serde_json::json!({ "id": other }),
        Some(("approved", None)),
    )
    .await;
    assert_eq!(
        tool_fact(&frames, "vault__fill")["payload"]["payload"]["authorization"],
        "approved_once"
    );
}

// ---- Minting ----

#[tokio::test]
async fn create_mints_the_strongest_secret_the_recipe_permits_and_stores_it() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__create",
        serde_json::json!({
            "domain": "example.com",
            "username": "agent@example.com",
            "login_url": "https://example.com/signup",
            "rules": "maxlength: 8; required: digit",
        }),
        Some(("approved", None)),
    )
    .await;

    let items = list_credentials(&h.daemon).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["provenance"], "agent_minted");
    assert_eq!(items[0]["owner_agent_id"], h.daemon.agent_id.as_str());
    assert_eq!(
        tool_fact(&frames, "vault__create")["payload"]["payload"]["domain"],
        "example.com"
    );

    // The stored recipe is the one the site published, and the secret
    // it minted honours it. The secret is read here through the store,
    // which is the only place it exists.
    let (recipe, secret) = minted_secret(&h).await;
    assert_eq!(recipe, "maxlength: 8; required: digit");
    assert_eq!(secret.chars().count(), 8);
    assert!(secret.chars().any(|c| c.is_ascii_digit()), "{secret}");
}

#[tokio::test]
async fn create_refuses_a_login_url_on_another_domain_before_it_asks() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__create",
        serde_json::json!({
            "domain": "example.com",
            "username": "agent@example.com",
            "login_url": "https://evil.test/signup",
        }),
        None,
    )
    .await;

    assert!(
        frames
            .iter()
            .all(|frame| frame["type"] != "request.created"),
        "a record that cannot be written must not raise a card"
    );
    assert!(list_credentials(&h.daemon).await.is_empty());
}

// ---- One-time codes ----

#[tokio::test]
async fn a_totp_fill_writes_the_code_into_the_daemons_tab_and_never_returns_it() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", Some(TOTP_SEED)).await;
    // The Agent went on from the fill to the code step of the site.
    h.computer
        .set_browser_page("https://accounts.example.com/signin/challenge");
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__totp_fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    // A code fill does not navigate: it writes one six-digit code into
    // the page the daemon's tab shows, for the origin that page is on.
    assert!(
        h.computer.browser_opens().is_empty(),
        "a code fill navigated"
    );
    let fills = h.computer.browser_fills();
    assert_eq!(fills.len(), 1, "{fills:?}");
    let (origin, fields) = &fills[0];
    assert_eq!(origin, "https://accounts.example.com");
    assert_eq!(fields.len(), 1, "{fields:?}");
    assert_eq!(fields[0].kind, FieldKind::Text);
    let code = fields[0].text.clone();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
    assert!(h.computer.inputs().is_empty(), "{:?}", h.computer.inputs());
    assert_eq!(
        tool_fact(&frames, "vault__totp_fill")["payload"]["payload"]["fill"],
        "filled"
    );

    // The code never comes back through the tool result or the log.
    let seen = serde_json::to_string(&frames).unwrap();
    assert!(!seen.contains(code.as_str()), "{seen}");
    assert!(!seen.contains(TOTP_SEED), "{seen}");
    for request in h.brain.requests() {
        let prompt = format!("{:?}", request.messages);
        assert!(!prompt.contains(code.as_str()), "{prompt}");
    }
}

#[tokio::test]
async fn a_totp_fill_on_a_page_of_another_domain_writes_nothing() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", Some(TOTP_SEED)).await;
    h.computer
        .set_browser_page("https://accounts.example.org/challenge");
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__totp_fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__totp_fill", &id, "example.org");
}

#[tokio::test]
async fn a_totp_fill_with_no_login_tab_writes_nothing() {
    let h = boot().await;
    let id = seed_login(&h.daemon, "correct-horse-battery", Some(TOTP_SEED)).await;
    let mut socket = firehose(&h.daemon).await;

    let frames = call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__totp_fill",
        serde_json::json!({ "id": id }),
        Some(("approved", None)),
    )
    .await;

    assert_nothing_written(&h);
    assert_failed(&frames, "vault__totp_fill", &id, "no tab");
}

// ---- Listing, deleting and archival ----

#[tokio::test]
async fn list_returns_handles_and_never_a_secret() {
    let h = boot().await;
    seed_login(&h.daemon, "correct-horse-battery", None).await;
    let mut socket = firehose(&h.daemon).await;

    call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__list",
        serde_json::json!({ "domain": "example.com" }),
        None,
    )
    .await;

    let requests = h.brain.requests();
    let result = requests[requests.len() - 1]
        .messages
        .last()
        .unwrap()
        .clone();
    assert!(result.text.contains("example.com"), "{}", result.text);
    assert!(result.text.contains("alice@example.com"), "{}", result.text);
    assert!(
        !result.text.contains("correct-horse-battery"),
        "{}",
        result.text
    );
}

#[tokio::test]
async fn archiving_an_agent_leaves_its_minted_credentials_in_the_vault() {
    let h = boot().await;
    let mut socket = firehose(&h.daemon).await;
    call_tool(
        &h,
        &mut socket,
        "p-1",
        "vault__create",
        serde_json::json!({
            "domain": "example.com",
            "username": "agent@example.com",
            "login_url": "https://example.com/signup",
        }),
        Some(("approved", None)),
    )
    .await;
    assert_eq!(
        list_credentials(&h.daemon).await[0]["owner_agent_id"],
        h.daemon.agent_id.as_str()
    );

    let archived = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/agents/{}/archive",
            h.daemon.base_url, h.daemon.agent_id
        ))
        .header("cookie", h.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(archived.status(), 200);

    // The record stays, under the user: archival must not destroy
    // account access, because the vault has no export.
    let items = list_credentials(&h.daemon).await;
    assert_eq!(items.len(), 1);
    assert!(items[0]["owner_agent_id"].is_null());
    let (_, secret) = minted_secret(&h).await;
    assert!(!secret.is_empty(), "the secret is still openable");
}

/// The recipe and the secret of the single stored credential, opened
/// the way a fill does: through the data key the daemon minted
/// into the secret store. The test is the only place outside a fill
/// where a plaintext secret exists.
async fn minted_secret(h: &Harness) -> (String, String) {
    use pagis_core::{CredentialStore, WorkspaceStore};
    let store = pagis_storage_sqlite::SqliteCredentialStore::new(h.daemon.pool().clone());
    let workspace_id = pagis_storage_sqlite::SqliteWorkspaceStore::new(h.daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .remove(0)
        .id;
    let credential = store.list(&workspace_id, None).await.unwrap().remove(0);
    let key = pagis_vault::DataKey::load(h.secrets.as_ref(), &workspace_id).unwrap();
    (
        credential.recipe.clone(),
        key.open(&credential.secret).unwrap(),
    )
}

/// One tenant's Tenant Data Key does not open another tenant's
/// Credential. The vault seals with the key of the Workspace that
/// owns the record, so a row that is read across the tenant line is
/// still ciphertext to the reader.
#[tokio::test]
async fn one_tenants_data_key_does_not_open_the_others_credential() {
    use pagis_core::{CredentialStore, SecretStore};

    let secrets: Arc<dyn SecretStore> = Arc::new(pagis_core::MemorySecretStore::default());
    let world = TwoTenants::on(
        TestDaemon::start_with(TestDaemonOptions {
            computer: Arc::new(FakeComputerRuntime::with_image()) as _,
            secrets: Arc::clone(&secrets),
            ..TestDaemonOptions::default()
        })
        .await,
    )
    .await;

    for (person, secret) in [(&world.a, "A-only-secret"), (&world.b, "B-only-secret")] {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/settings/credentials",
                world.daemon.base_url
            ))
            .header("cookie", &person.cookie)
            .json(&serde_json::json!({
                "domain": "example.com",
                "username": "ada",
                "login_url": "https://accounts.example.com/signin",
                "secret": secret,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201, "the credential is stored");
    }

    let store = pagis_storage_sqlite::SqliteCredentialStore::new(world.daemon.pool().clone());
    let a_row = store
        .list(&world.a.workspace_id, None)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.username == "ada")
        .expect("A's credential");
    let b_row = store
        .list(&world.b.workspace_id, None)
        .await
        .unwrap()
        .remove(0);

    let a_key = pagis_vault::DataKey::load(secrets.as_ref(), &world.a.workspace_id).unwrap();
    let b_key = pagis_vault::DataKey::load(secrets.as_ref(), &world.b.workspace_id).unwrap();

    assert_eq!(a_key.open(&a_row.secret).unwrap(), "A-only-secret");
    assert_eq!(b_key.open(&b_row.secret).unwrap(), "B-only-secret");
    assert!(
        b_key.open(&a_row.secret).is_err(),
        "B's key opened A's credential"
    );
    assert!(
        a_key.open(&b_row.secret).is_err(),
        "A's key opened B's credential"
    );
    // The keys sit in `secrets.enc` under names that carry the
    // Workspace, and no unscoped name is present.
    assert!(
        secrets
            .get(&pagis_vault::data_key_name(&world.a.workspace_id))
            .unwrap()
            .is_some()
    );
    assert_eq!(secrets.get("vault_data_key").unwrap(), None);
}
