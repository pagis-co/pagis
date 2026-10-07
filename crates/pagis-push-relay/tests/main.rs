//! The Push Relay over real HTTP: the router on a loopback port with an
//! in-memory SQLite, and the binary with a SQLite file. The deployment of
//! `deploy/push-relay/` as Docker Compose reads it.

mod apns;
mod deployment;
mod fcm;
mod push;

use std::io::{BufRead, BufReader};
use std::net::{IpAddr, SocketAddr};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pagis_push_relay::{
    Clock, Delivery, Message, PublicOrigin, Registration, Transport, Transports, TrustedProxy,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// The public origin of the binary in its tests.
const PUBLIC_ORIGIN: &str = "https://push.example.test";

/// The generator of P-256 as an uncompressed point: a valid public key.
/// It is the public key of the scalar 1, [`VAPID_SECRET`].
const GENERATOR: [u8; 65] = [
    0x04, 0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40,
    0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2,
    0x96, 0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e,
    0x16, 0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51,
    0xf5,
];

/// The scalar 1: the VAPID secret key whose public key is [`GENERATOR`].
const VAPID_SECRET: [u8; 32] = {
    let mut scalar = [0u8; 32];
    scalar[31] = 1;
    scalar
};

const APNS_TOKEN: &str = "6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f";
const FCM_TOKEN: &str = "dX3k9:APA91bH-q_7rT2";

/// A transport that keeps each message it gets and answers the delivery
/// that the test sets.
struct FakeTransport {
    answer: Mutex<Delivery>,
    sent: Mutex<Vec<(Registration, Message)>>,
}

impl FakeTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(Delivery::Delivered),
            sent: Mutex::new(Vec::new()),
        })
    }

    fn answer(&self, delivery: Delivery) {
        *self.answer.lock().unwrap() = delivery;
    }

    fn sent(&self) -> Vec<(Registration, Message)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Transport for FakeTransport {
    async fn send(&self, registration: &Registration, message: &Message) -> Delivery {
        self.sent
            .lock()
            .unwrap()
            .push((registration.clone(), message.clone()));
        self.answer.lock().unwrap().clone()
    }
}

/// A clock that reads the system time until a test sets it.
#[derive(Default)]
struct TestClock(Mutex<Option<SystemTime>>);

impl TestClock {
    fn set(&self, now: SystemTime) {
        *self.0.lock().unwrap() = Some(now);
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        self.0.lock().unwrap().unwrap_or_else(SystemTime::now)
    }
}

/// Which platforms the relay of a test serves.
#[derive(Clone, Copy)]
enum Serves {
    Both,
    IosOnly,
}

/// A relay that serves on a loopback port, and the pool behind it. Its
/// public origin is the loopback origin, so a Web Push of `pagis-push`
/// to its endpoint carries the `aud` that the relay checks.
struct Relay {
    origin: String,
    pool: SqlitePool,
    client: reqwest::Client,
    transport: Arc<FakeTransport>,
    clock: Arc<TestClock>,
}

impl Relay {
    async fn start(proxy: TrustedProxy) -> Self {
        Self::start_serving(proxy, Serves::Both).await
    }

    async fn start_serving(proxy: TrustedProxy, serves: Serves) -> Self {
        let pool = pagis_push_relay::connect_memory()
            .await
            .expect("an in-memory SQLite");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("the bound address");
        let origin = format!("http://{address}");
        let public_origin = PublicOrigin::parse(&origin).expect("the loopback origin");
        let transport = FakeTransport::new();
        let transports = match serves {
            Serves::Both => Transports::default()
                .with_ios(transport.clone())
                .with_android(transport.clone()),
            Serves::IosOnly => Transports::default().with_ios(transport.clone()),
        };
        let clock = Arc::new(TestClock::default());
        let router = pagis_push_relay::router(
            pool.clone(),
            public_origin,
            proxy,
            transports,
            clock.clone(),
        );
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        Self {
            origin,
            pool,
            client: reqwest::Client::new(),
            transport,
            clock,
        }
    }

    async fn register(&self, body: Value) -> reqwest::Response {
        self.register_from(body, None).await
    }

    async fn register_from(&self, body: Value, forwarded_for: Option<&str>) -> reqwest::Response {
        let mut request = self
            .client
            .post(format!("{}/v1/registrations", self.origin))
            .json(&body);
        if let Some(forwarded_for) = forwarded_for {
            request = request.header("x-forwarded-for", forwarded_for);
        }
        request.send().await.expect("the relay answers")
    }

    /// Register an iOS installation and answer `{id, secret, endpoint}`.
    async fn registered(&self) -> Value {
        let response = self.register(ios_body()).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        response.json().await.expect("a JSON body")
    }

    async fn token_of(&self, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT token FROM registrations WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .expect("read the registration")
    }
}

fn vapid_key() -> String {
    URL_SAFE_NO_PAD.encode(GENERATOR)
}

fn ios_body() -> Value {
    json!({
        "platform": "ios",
        "environment": "sandbox",
        "token": APNS_TOKEN,
        "vapid_key": vapid_key(),
    })
}

fn android_body() -> Value {
    json!({
        "platform": "android",
        "token": FCM_TOKEN,
        "vapid_key": vapid_key(),
    })
}

fn decoded_len(value: &str) -> usize {
    URL_SAFE_NO_PAD
        .decode(value)
        .unwrap_or_else(|error| panic!("{value:?} is not base64url: {error}"))
        .len()
}

#[tokio::test]
async fn a_registration_answers_an_id_a_secret_and_an_endpoint_on_the_public_origin() {
    let relay = Relay::start(TrustedProxy::none()).await;

    let response = relay.register(ios_body()).await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await.expect("a JSON body");
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");
    let endpoint = body["endpoint"].as_str().expect("an endpoint");
    assert_eq!(decoded_len(id), 16, "the id is 128 bits");
    assert_eq!(decoded_len(secret), 32, "the secret is 256 bits");
    assert_eq!(endpoint, format!("{}/v1/push/{id}", relay.origin));
    assert!(!endpoint.contains(APNS_TOKEN));
}

#[tokio::test]
async fn an_android_registration_needs_no_environment() {
    let relay = Relay::start(TrustedProxy::none()).await;

    let response = relay.register(android_body()).await;

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn the_store_holds_the_sha256_of_the_secret_and_not_the_secret() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");

    let (secret_hash, token, vapid): (Vec<u8>, String, Vec<u8>) =
        sqlx::query_as("SELECT secret_hash, token, vapid_key FROM registrations WHERE id = ?")
            .bind(id)
            .fetch_one(&relay.pool)
            .await
            .expect("the registration row");

    assert_eq!(secret_hash, Sha256::digest(secret.as_bytes()).to_vec());
    assert_eq!(token, APNS_TOKEN);
    assert_eq!(vapid, GENERATOR);
    let holds_secret: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registrations \
         WHERE instr(CAST(secret_hash AS TEXT), ?) > 0 OR instr(token, ?) > 0",
    )
    .bind(secret)
    .bind(secret)
    .fetch_one(&relay.pool)
    .await
    .expect("search the table");
    assert_eq!(holds_secret, 0);
}

#[tokio::test]
async fn put_with_the_secret_changes_the_token_and_keeps_the_endpoint() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");

    let response = relay
        .client
        .put(format!("{}/v1/registrations/{id}", relay.origin))
        .bearer_auth(secret)
        .json(&json!({ "token": "ffff0000" }))
        .send()
        .await
        .expect("the relay answers");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(relay.token_of(id).await.as_deref(), Some("ffff0000"));
    let id_now: Option<String> = sqlx::query_scalar("SELECT id FROM registrations")
        .fetch_optional(&relay.pool)
        .await
        .expect("read the registration");
    assert_eq!(id_now.as_deref(), Some(id), "the endpoint keeps its id");
}

#[tokio::test]
async fn put_with_a_wrong_or_missing_secret_or_an_unknown_id_answers_not_found() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");
    let other = relay.registered().await;
    let other_secret = other["secret"].as_str().expect("a secret");
    let unknown_id = URL_SAFE_NO_PAD.encode([7u8; 16]);
    let url = |id: &str| format!("{}/v1/registrations/{id}", relay.origin);
    let change = json!({ "token": "ffff0000" });

    let wrong = relay
        .client
        .put(url(id))
        .bearer_auth(other_secret)
        .json(&change)
        .send()
        .await
        .expect("the relay answers");
    let missing = relay
        .client
        .put(url(id))
        .json(&change)
        .send()
        .await
        .expect("the relay answers");
    let unknown = relay
        .client
        .put(url(&unknown_id))
        .bearer_auth(secret)
        .json(&change)
        .send()
        .await
        .expect("the relay answers");

    assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let wrong_body: Value = wrong.json().await.expect("a JSON body");
    let unknown_body: Value = unknown.json().await.expect("a JSON body");
    assert_eq!(
        wrong_body, unknown_body,
        "the answers do not tell them apart"
    );
    assert_eq!(relay.token_of(id).await.as_deref(), Some(APNS_TOKEN));
}

#[tokio::test]
async fn put_with_a_bad_token_answers_unprocessable() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");

    let response = relay
        .client
        .put(format!("{}/v1/registrations/{id}", relay.origin))
        .bearer_auth(secret)
        .json(&json!({ "token": "not a token" }))
        .send()
        .await
        .expect("the relay answers");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(relay.token_of(id).await.as_deref(), Some(APNS_TOKEN));
}

#[tokio::test]
async fn delete_with_the_secret_removes_the_row() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id");
    let secret = body["secret"].as_str().expect("a secret");
    let url = format!("{}/v1/registrations/{id}", relay.origin);

    let wrong = relay
        .client
        .delete(&url)
        .bearer_auth("not-the-secret")
        .send()
        .await
        .expect("the relay answers");
    assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
    assert!(
        relay.token_of(id).await.is_some(),
        "a wrong secret removes nothing"
    );

    let response = relay
        .client
        .delete(&url)
        .bearer_auth(secret)
        .send()
        .await
        .expect("the relay answers");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(relay.token_of(id).await, None);
    let again = relay
        .client
        .delete(&url)
        .bearer_auth(secret)
        .send()
        .await
        .expect("the relay answers");
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

/// Send `body` and answer the message of its `422`.
async fn refusal(relay: &Relay, body: Value) -> String {
    let response = relay.register(body.clone()).await;
    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "{body} is refused"
    );
    let answer: Value = response.json().await.expect("a JSON body");
    answer["error"]["message"]
        .as_str()
        .expect("an error message")
        .to_string()
}

fn with(mut body: Value, field: &str, value: Value) -> Value {
    body[field] = value;
    body
}

#[tokio::test]
async fn a_bad_vapid_key_answers_unprocessable_and_names_the_field() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let mut off_the_curve = GENERATOR;
    off_the_curve[64] ^= 1;
    let mut compressed = vec![0x03];
    compressed.extend_from_slice(&GENERATOR[1..33]);

    for vapid_key in [
        json!("not base64url!"),
        json!(URL_SAFE_NO_PAD.encode(off_the_curve)),
        json!(URL_SAFE_NO_PAD.encode(compressed)),
        json!(URL_SAFE_NO_PAD.encode([4u8; 65])),
        json!(42),
        Value::Null,
    ] {
        let message = refusal(&relay, with(ios_body(), "vapid_key", vapid_key)).await;
        assert!(message.contains("vapid_key"), "{message}");
    }
}

#[tokio::test]
async fn an_unknown_platform_answers_unprocessable_and_names_the_field() {
    let relay = Relay::start(TrustedProxy::none()).await;

    for platform in [json!("windows"), json!(""), Value::Null] {
        let message = refusal(&relay, with(ios_body(), "platform", platform)).await;
        assert!(message.contains("platform"), "{message}");
    }
}

#[tokio::test]
async fn an_environment_is_required_for_ios_and_refused_for_android() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let mut no_environment = ios_body();
    no_environment
        .as_object_mut()
        .expect("an object")
        .remove("environment");

    for body in [
        no_environment,
        with(ios_body(), "environment", json!("staging")),
        with(android_body(), "environment", json!("production")),
    ] {
        let message = refusal(&relay, body).await;
        assert!(message.contains("environment"), "{message}");
    }
}

#[tokio::test]
async fn a_bad_token_answers_unprocessable_and_names_the_field() {
    let relay = Relay::start(TrustedProxy::none()).await;

    for token in [
        json!(""),
        json!("abc/def"),
        json!("abc def"),
        json!("x".repeat(4097)),
        json!(7),
    ] {
        let message = refusal(&relay, with(ios_body(), "token", token)).await;
        assert!(message.contains("token"), "{message}");
    }
    let longest = relay
        .register(with(ios_body(), "token", json!("x".repeat(4096))))
        .await;
    assert_eq!(longest.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn a_body_that_is_not_a_json_object_answers_unprocessable() {
    let relay = Relay::start(TrustedProxy::none()).await;

    let response = relay
        .client
        .post(format!("{}/v1/registrations", relay.origin))
        .body("platform=ios")
        .send()
        .await
        .expect("the relay answers");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_platform_with_no_transport_answers_unprocessable_and_names_the_platform() {
    let relay = Relay::start_serving(TrustedProxy::none(), Serves::IosOnly).await;

    let message = refusal(&relay, android_body()).await;
    let ios = relay.register(ios_body()).await;

    assert!(message.contains("android"), "{message}");
    assert_eq!(ios.status(), StatusCode::CREATED);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM registrations")
        .fetch_one(&relay.pool)
        .await
        .expect("count the registrations");
    assert_eq!(rows, 1, "the refused registration is not kept");
}

const PROXY: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

#[tokio::test]
async fn the_21st_registration_from_one_address_in_one_hour_answers_too_many_requests() {
    let relay = Relay::start(TrustedProxy::at(PROXY)).await;
    for _ in 0..20 {
        let response = relay
            .register_from(android_body(), Some("203.0.113.1"))
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let refused = relay
        .register_from(android_body(), Some("203.0.113.1"))
        .await;
    let other = relay
        .register_from(android_body(), Some("203.0.113.2"))
        .await;

    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = refused
        .headers()
        .get("retry-after")
        .expect("a Retry-After header")
        .to_str()
        .expect("ASCII")
        .parse()
        .expect("seconds");
    assert!((1..=3600).contains(&retry_after), "{retry_after}");
    assert_eq!(other.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn the_limit_counts_the_last_forwarded_address_of_the_trusted_proxy() {
    let relay = Relay::start(TrustedProxy::at(PROXY)).await;
    for n in 0..20 {
        let forwarded = format!("198.51.100.{n}, 203.0.113.9");
        let response = relay.register_from(android_body(), Some(&forwarded)).await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let refused = relay
        .register_from(android_body(), Some("198.51.100.99, 203.0.113.9"))
        .await;

    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn forwarded_for_from_a_peer_that_is_not_the_trusted_proxy_does_not_count() {
    let not_the_peer = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
    for proxy in [TrustedProxy::none(), TrustedProxy::at(not_the_peer)] {
        let relay = Relay::start(proxy).await;
        for n in 0..20 {
            let forwarded = format!("203.0.113.{n}");
            let response = relay.register_from(android_body(), Some(&forwarded)).await;
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let refused = relay
            .register_from(android_body(), Some("203.0.113.200"))
            .await;

        assert_eq!(
            refused.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "every request counts against the peer"
        );
    }
}

/// Lines that the binary prints on its standard output.
fn output_lines(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().expect("the relay's stdout");
    let (lines, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}

/// The origin in the line that says where the relay listens.
fn listening_origin(lines: &mpsc::Receiver<String>) -> String {
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(30))
            .expect("the relay says where it listens");
        if let Some((_, origin)) = line.split_once("listens on ") {
            return origin.trim().to_string();
        }
    }
}

#[tokio::test]
async fn the_binary_opens_its_sqlite_file_and_answers_health() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("relay.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pagis-push-relay"))
        .env_clear()
        .env("PUSH_RELAY_PUBLIC_ORIGIN", PUBLIC_ORIGIN)
        .env("PUSH_RELAY_DATABASE", &database)
        .env("PUSH_RELAY_BIND", "127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the relay starts");
    let lines = output_lines(&mut child);
    let origin = listening_origin(&lines);

    let response = reqwest::get(format!("{origin}/v1/health")).await;
    let logged = (0..10)
        .map_while(|_| lines.recv_timeout(Duration::from_secs(10)).ok())
        .find(|line| line.contains("route=/v1/health"));

    let _ = child.kill();
    let _ = child.wait();
    let response = response.expect("the relay answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("a JSON body");
    assert_eq!(body, json!({ "version": env!("CARGO_PKG_VERSION") }));
    assert!(database.exists(), "the relay made its SQLite file");
    let file = SqlitePool::connect(&format!("sqlite://{}", database.display()))
        .await
        .expect("open the relay's SQLite file");
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&file)
        .await
        .expect("read the journal mode");
    assert_eq!(journal_mode, "wal");
    let logged = logged.expect("a log line for the request");
    assert!(logged.contains("status=200"), "{logged}");
}

#[test]
fn the_binary_stops_at_start_and_names_a_missing_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_pagis-push-relay"))
        .env_clear()
        .env("PUSH_RELAY_DATABASE", "/nonexistent/relay.sqlite")
        .stdin(Stdio::null())
        .output()
        .expect("the relay runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PUSH_RELAY_PUBLIC_ORIGIN"), "{stderr}");
}

/// The binary with the required variables and `extra`, on a free port
/// with a SQLite file in `directory`.
fn relay_command(directory: &std::path::Path, extra: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pagis-push-relay"));
    command
        .env_clear()
        .env("PUSH_RELAY_PUBLIC_ORIGIN", PUBLIC_ORIGIN)
        .env("PUSH_RELAY_DATABASE", directory.join("relay.sqlite"))
        .env("PUSH_RELAY_BIND", "127.0.0.1:0")
        .envs(extra.iter().copied())
        .stdin(Stdio::null());
    command
}

/// The four APNs variables, with the key in `key_path`.
fn apns_variables(key_path: &str) -> [(&'static str, &str); 4] {
    [
        ("PUSH_RELAY_APNS_KEY_PATH", key_path),
        ("PUSH_RELAY_APNS_KEY_ID", "ABC123DEFG"),
        ("PUSH_RELAY_APNS_TEAM_ID", "DEF123GHIJ"),
        ("PUSH_RELAY_APNS_TOPIC", "co.pagis.mobile"),
    ]
}

#[test]
fn a_partial_apns_setting_stops_the_binary_and_names_the_missing_variable() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let all = apns_variables("/run/secrets/apns.p8");
    let partial = [all[0], all[1], all[3]];

    let output = relay_command(directory.path(), &partial)
        .output()
        .expect("the relay runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PUSH_RELAY_APNS_TEAM_ID"), "{stderr}");
    assert!(!directory.path().join("relay.sqlite").exists());
}

#[test]
fn an_apns_key_that_does_not_parse_stops_the_binary() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let key_path = directory.path().join("apns.p8");
    std::fs::write(&key_path, "this is not a PEM key").expect("write the file");
    let key_path = key_path.to_str().expect("a UTF-8 path");

    let output = relay_command(directory.path(), &apns_variables(key_path))
        .output()
        .expect("the relay runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PUSH_RELAY_APNS_KEY_PATH"), "{stderr}");
    assert!(stderr.contains(key_path), "{stderr}");
    assert!(!directory.path().join("relay.sqlite").exists());
}

#[tokio::test]
async fn the_binary_with_an_apns_key_serves_ios_and_no_other_platform() {
    use p256::pkcs8::{EncodePrivateKey, LineEnding};

    let directory = tempfile::tempdir().expect("a temporary directory");
    let key_path = directory.path().join("apns.p8");
    let key = p256::SecretKey::from_slice(&[3; 32]).expect("a P-256 scalar");
    let pem = key.to_pkcs8_pem(LineEnding::LF).expect("a PKCS#8 PEM");
    std::fs::write(&key_path, pem.as_bytes()).expect("write the key");
    let mut child = relay_command(
        directory.path(),
        &apns_variables(key_path.to_str().expect("a UTF-8 path")),
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .expect("the relay starts");
    let lines = output_lines(&mut child);
    let origin = listening_origin(&lines);
    let client = reqwest::Client::new();
    let register = |body: Value| {
        client
            .post(format!("{origin}/v1/registrations"))
            .json(&body)
            .send()
    };

    let ios = register(ios_body()).await;
    let android = register(android_body()).await;

    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        ios.expect("the relay answers").status(),
        StatusCode::CREATED
    );
    assert_eq!(
        android.expect("the relay answers").status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn a_partial_fcm_setting_stops_the_binary_and_names_the_missing_variable() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    let output = relay_command(
        directory.path(),
        &[("PUSH_RELAY_FCM_PROJECT_ID", "pagis-mobile")],
    )
    .output()
    .expect("the relay runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PUSH_RELAY_FCM_CREDENTIALS_PATH"),
        "{stderr}"
    );
    assert!(!directory.path().join("relay.sqlite").exists());
}

#[test]
fn fcm_credentials_that_do_not_parse_stop_the_binary() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let credentials = directory.path().join("fcm.json");
    std::fs::write(&credentials, r#"{"type": "service_account"}"#).expect("write the file");
    let credentials = credentials.to_str().expect("a UTF-8 path");

    let output = relay_command(
        directory.path(),
        &[
            ("PUSH_RELAY_FCM_CREDENTIALS_PATH", credentials),
            ("PUSH_RELAY_FCM_PROJECT_ID", "pagis-mobile"),
        ],
    )
    .output()
    .expect("the relay runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PUSH_RELAY_FCM_CREDENTIALS_PATH"),
        "{stderr}"
    );
    assert!(stderr.contains(credentials), "{stderr}");
    assert!(!directory.path().join("relay.sqlite").exists());
}
