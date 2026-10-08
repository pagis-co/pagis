//! The APNs transport against a fake APNs: the request that it sends, the
//! key and the provider token of each APNs environment, and the delivery
//! of each answer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use pagis_push_relay::{
    ApnsBaseUrls, ApnsKey, ApnsSettings, ApnsTransport, Clock, Delivery, Environment, Message,
    Platform, Registration, Transport, Urgency,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DEVICE_TOKEN: &str = "6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f";
const DEVICE_PATH: &str =
    "/3/device/6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f6a1f0c9e2b7d4e8f";

/// 2026-10-06 12:00:00 UTC.
const NOON: u64 = 1_791_288_000;
const MINUTE: u64 = 60;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A clock that a test moves.
struct TestClock(Mutex<SystemTime>);

impl TestClock {
    fn at(seconds: u64) -> Arc<Self> {
        Arc::new(Self(Mutex::new(at(seconds))))
    }

    fn set(&self, seconds: u64) {
        *self.0.lock().unwrap() = at(seconds);
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

const PRODUCTION_KEY_ID: &str = "PRD123DEFG";
const SANDBOX_KEY_ID: &str = "SBX123DEFG";

/// The private scalar of the key of each environment.
const PRODUCTION_SCALAR: [u8; 32] = [3; 32];
const SANDBOX_SCALAR: [u8; 32] = [5; 32];

/// A fake APNs for each environment, and a transport that sends to them
/// with a key for each environment in a temporary `.p8` file.
struct Apns {
    production: MockServer,
    sandbox: MockServer,
    clock: Arc<TestClock>,
    transport: ApnsTransport,
    _key_files: Vec<tempfile::NamedTempFile>,
}

impl Apns {
    async fn start() -> Self {
        Self::start_with(&[Environment::Production, Environment::Sandbox]).await
    }

    /// The transport holds a key for each of `environments` only.
    async fn start_with(environments: &[Environment]) -> Self {
        let production = MockServer::start().await;
        let sandbox = MockServer::start().await;
        let base_urls = ApnsBaseUrls {
            production: production.uri(),
            sandbox: sandbox.uri(),
        };
        let clock = TestClock::at(NOON);
        let (transport, key_files) = transport(base_urls, clock.clone(), environments);
        Self {
            production,
            sandbox,
            clock,
            transport,
            _key_files: key_files,
        }
    }

    /// Both environments answer each request with `answer`.
    async fn answer(&self, answer: ResponseTemplate) {
        for server in [&self.production, &self.sandbox] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path(DEVICE_PATH))
                .respond_with(answer.clone())
                .mount(server)
                .await;
        }
    }

    async fn send(&self, environment: Environment) -> Delivery {
        self.transport
            .send(&registration(environment), &message())
            .await
    }

    /// The `iat` of the provider token of each request to the sandbox,
    /// in order.
    async fn issued_at(&self) -> Vec<u64> {
        issued_at(&self.sandbox).await
    }
}

/// The provider token of each request that `server` got, in order.
async fn provider_tokens(server: &MockServer) -> Vec<String> {
    let requests = server
        .received_requests()
        .await
        .expect("the fake APNs records requests");
    requests
        .iter()
        .map(|request| {
            let authorization = request.headers["authorization"].to_str().expect("ASCII");
            authorization
                .strip_prefix("bearer ")
                .unwrap_or_else(|| panic!("{authorization} is a bearer token"))
                .to_string()
        })
        .collect()
}

/// The `iat` of the provider token of each request that `server` got, in
/// order.
async fn issued_at(server: &MockServer) -> Vec<u64> {
    provider_tokens(server)
        .await
        .iter()
        .map(|token| claims(token)["iat"].as_u64().expect("an iat"))
        .collect()
}

/// A transport to `base_urls` with a key for each of `environments`, each
/// in a temporary `.p8` file, which the caller keeps.
fn transport(
    base_urls: ApnsBaseUrls,
    clock: Arc<TestClock>,
    environments: &[Environment],
) -> (ApnsTransport, Vec<tempfile::NamedTempFile>) {
    let mut key_files = Vec::new();
    let mut key = |scalar: [u8; 32], id: &str| {
        let key = SecretKey::from_slice(&scalar).expect("a P-256 scalar");
        let pem = key.to_pkcs8_pem(LineEnding::LF).expect("a PKCS#8 PEM");
        let key_file = tempfile::NamedTempFile::new().expect("a temporary file");
        std::fs::write(key_file.path(), pem.as_bytes()).expect("write the key");
        let key = ApnsKey {
            path: key_file.path().to_path_buf(),
            id: id.to_string(),
        };
        key_files.push(key_file);
        key
    };
    let production = environments
        .contains(&Environment::Production)
        .then(|| key(PRODUCTION_SCALAR, PRODUCTION_KEY_ID));
    let sandbox = environments
        .contains(&Environment::Sandbox)
        .then(|| key(SANDBOX_SCALAR, SANDBOX_KEY_ID));
    let settings = ApnsSettings {
        team_id: "DEF123GHIJ".to_string(),
        topic: "co.pagis.mobile".to_string(),
        production,
        sandbox,
    };
    let transport = ApnsTransport::new(&settings, base_urls, clock).expect("the APNs transport");
    (transport, key_files)
}

/// The decoded JSON of one part of a JWT.
fn jwt_part(part: &str) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).expect("base64url")).expect("JSON")
}

/// `true` when the ES256 signature of `token` verifies with the public
/// key of `scalar`.
fn signed_by(token: &str, scalar: [u8; 32]) -> bool {
    let (signed, signature) = token.rsplit_once('.').expect("a JWT");
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).expect("base64url"))
        .expect("an ES256 signature");
    let public = SecretKey::from_slice(&scalar)
        .expect("a P-256 scalar")
        .public_key();
    VerifyingKey::from(public)
        .verify(signed.as_bytes(), &signature)
        .is_ok()
}

fn registration(environment: Environment) -> Registration {
    Registration {
        platform: Platform::Ios(environment),
        token: DEVICE_TOKEN.to_string(),
    }
}

fn message() -> Message {
    Message {
        body: vec![0xab; 120],
        ttl: Duration::from_secs(86400),
        urgency: Urgency::High,
        topic: Some("item-42".to_string()),
    }
}

fn claims(token: &str) -> Value {
    jwt_part(token.split('.').nth(1).expect("a JWT"))
}

fn jwt_header(token: &str) -> Value {
    jwt_part(token.split('.').next().expect("a JWT"))
}

fn apns_error(status: u16, reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({ "reason": reason }))
}

#[tokio::test]
async fn a_push_posts_the_device_path_the_bearer_token_and_the_body_over_http2() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;

    let delivery = apns.send(Environment::Production).await;

    // The client speaks only HTTP/2 with prior knowledge, so an answer
    // shows that the request went over HTTP/2.
    assert_eq!(delivery, Delivery::Delivered);
    let requests = apns
        .production
        .received_requests()
        .await
        .expect("the fake APNs records requests");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url.path(), DEVICE_PATH);
    let authorization = request.headers["authorization"].to_str().expect("ASCII");
    let token = authorization
        .strip_prefix("bearer ")
        .unwrap_or_else(|| panic!("{authorization} is a bearer token"));
    assert_eq!(
        claims(token),
        json!({ "iss": "DEF123GHIJ", "iat": NOON }),
        "the provider token of the team at the clock's now"
    );
    assert_eq!(request.headers["apns-topic"], "co.pagis.mobile");
    assert_eq!(request.headers["apns-push-type"], "alert");
    assert_eq!(request.headers["apns-priority"], "10");
    assert_eq!(request.headers["apns-collapse-id"], "item-42");
    let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
    assert_eq!(body["p"], URL_SAFE_NO_PAD.encode([0xab; 120]));
    assert_eq!(body["aps"]["mutable-content"], 1);
    assert!(
        apns.sandbox
            .received_requests()
            .await
            .expect("records")
            .is_empty()
    );
}

#[tokio::test]
async fn a_sandbox_registration_goes_to_the_sandbox() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;

    let delivery = apns.send(Environment::Sandbox).await;

    assert_eq!(delivery, Delivery::Delivered);
    assert_eq!(
        apns.sandbox
            .received_requests()
            .await
            .expect("records")
            .len(),
        1
    );
    assert!(
        apns.production
            .received_requests()
            .await
            .expect("records")
            .is_empty()
    );
}

#[tokio::test]
async fn each_answer_of_apns_maps_to_its_delivery() {
    let apns = Apns::start().await;
    let cases = [
        (ResponseTemplate::new(200), Delivery::Delivered),
        (apns_error(410, "Unregistered"), Delivery::Gone),
        (apns_error(400, "BadDeviceToken"), Delivery::Gone),
        (apns_error(400, "DeviceTokenNotForTopic"), Delivery::Gone),
        (
            apns_error(429, "TooManyRequests"),
            Delivery::Failed("APNs answered 429 TooManyRequests".to_string()),
        ),
        (
            apns_error(500, "InternalServerError"),
            Delivery::Failed("APNs answered 500 InternalServerError".to_string()),
        ),
    ];
    for (answer, expected) in cases {
        apns.answer(answer).await;

        let delivery = apns.send(Environment::Sandbox).await;

        assert_eq!(delivery, expected);
        if let Delivery::Failed(reason) = &delivery {
            assert!(!reason.contains(DEVICE_TOKEN), "{reason}");
        }
    }
}

#[tokio::test]
async fn a_transport_error_names_no_device_token() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = closed.local_addr().expect("the bound address");
    drop(closed);
    let base_urls = ApnsBaseUrls {
        production: format!("http://{address}"),
        sandbox: format!("http://{address}"),
    };
    let (transport, _key_files) = transport(
        base_urls,
        TestClock::at(NOON),
        &[Environment::Production, Environment::Sandbox],
    );

    let delivery = transport
        .send(&registration(Environment::Sandbox), &message())
        .await;

    let Delivery::Failed(reason) = delivery else {
        panic!("{delivery:?} is a failure, as nothing listens on {address}");
    };
    assert!(reason.starts_with("APNs did not answer"), "{reason}");
    assert!(!reason.contains(DEVICE_TOKEN), "{reason}");
}

#[tokio::test]
async fn the_transport_keeps_one_provider_token_for_50_minutes() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;

    apns.send(Environment::Sandbox).await;
    apns.clock.set(NOON + 49 * MINUTE);
    apns.send(Environment::Sandbox).await;
    apns.clock.set(NOON + 50 * MINUTE);
    apns.send(Environment::Sandbox).await;
    apns.clock.set(NOON + 51 * MINUTE);
    apns.send(Environment::Sandbox).await;

    assert_eq!(
        apns.issued_at().await,
        [NOON, NOON, NOON + 50 * MINUTE, NOON + 50 * MINUTE]
    );
}

#[tokio::test]
async fn a_refused_provider_token_is_dropped_and_the_next_push_makes_a_new_one() {
    for reason in ["ExpiredProviderToken", "InvalidProviderToken"] {
        let apns = Apns::start().await;
        apns.answer(apns_error(403, reason)).await;

        let refused = apns.send(Environment::Sandbox).await;
        apns.clock.set(NOON + MINUTE);
        apns.answer(ResponseTemplate::new(200)).await;
        let delivered = apns.send(Environment::Sandbox).await;
        apns.clock.set(NOON + 2 * MINUTE);
        apns.send(Environment::Sandbox).await;

        assert_eq!(
            refused,
            Delivery::Failed(format!("APNs answered 403 {reason}"))
        );
        assert_eq!(delivered, Delivery::Delivered);
        // `answer` resets the fake, so it holds the two later requests.
        assert_eq!(
            apns.issued_at().await,
            [NOON + MINUTE, NOON + MINUTE],
            "{reason}: a new token after the refusal, then the transport keeps it"
        );
    }
}

#[tokio::test]
async fn another_403_keeps_the_provider_token() {
    let apns = Apns::start().await;
    apns.answer(apns_error(403, "Forbidden")).await;
    apns.send(Environment::Sandbox).await;
    apns.clock.set(NOON + MINUTE);
    apns.send(Environment::Sandbox).await;

    assert_eq!(apns.issued_at().await, [NOON, NOON]);
}

#[tokio::test]
async fn each_environment_signs_with_its_own_key_and_goes_to_its_own_host() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;

    let sandbox = apns.send(Environment::Sandbox).await;
    let production = apns.send(Environment::Production).await;

    assert_eq!(sandbox, Delivery::Delivered);
    assert_eq!(production, Delivery::Delivered);
    let [sandbox] = &provider_tokens(&apns.sandbox).await[..] else {
        panic!("one request to the sandbox host");
    };
    let [production] = &provider_tokens(&apns.production).await[..] else {
        panic!("one request to the production host");
    };
    assert_eq!(
        jwt_header(sandbox),
        json!({ "alg": "ES256", "kid": SANDBOX_KEY_ID })
    );
    assert!(signed_by(sandbox, SANDBOX_SCALAR));
    assert!(!signed_by(sandbox, PRODUCTION_SCALAR));
    assert_eq!(
        jwt_header(production),
        json!({ "alg": "ES256", "kid": PRODUCTION_KEY_ID })
    );
    assert!(signed_by(production, PRODUCTION_SCALAR));
    assert_eq!(claims(sandbox)["iss"], "DEF123GHIJ");
    assert_eq!(claims(production)["iss"], "DEF123GHIJ");
}

#[tokio::test]
async fn each_key_keeps_its_own_provider_token() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;

    apns.send(Environment::Production).await;
    apns.clock.set(NOON + 10 * MINUTE);
    apns.send(Environment::Sandbox).await;
    apns.clock.set(NOON + 20 * MINUTE);
    apns.send(Environment::Production).await;
    apns.send(Environment::Sandbox).await;

    assert_eq!(issued_at(&apns.production).await, [NOON, NOON]);
    assert_eq!(
        issued_at(&apns.sandbox).await,
        [NOON + 10 * MINUTE, NOON + 10 * MINUTE]
    );
}

#[tokio::test]
async fn a_refused_provider_token_drops_only_the_token_of_its_environment() {
    let apns = Apns::start().await;
    apns.answer(ResponseTemplate::new(200)).await;
    apns.send(Environment::Production).await;
    apns.sandbox.reset().await;
    Mock::given(method("POST"))
        .and(path(DEVICE_PATH))
        .respond_with(apns_error(403, "ExpiredProviderToken"))
        .mount(&apns.sandbox)
        .await;
    apns.send(Environment::Sandbox).await;

    apns.clock.set(NOON + MINUTE);
    apns.send(Environment::Production).await;
    apns.send(Environment::Sandbox).await;

    assert_eq!(issued_at(&apns.production).await, [NOON, NOON]);
    assert_eq!(issued_at(&apns.sandbox).await, [NOON, NOON + MINUTE]);
}

#[tokio::test]
async fn a_transport_serves_only_the_environments_that_have_a_key() {
    let apns = Apns::start_with(&[Environment::Production]).await;
    apns.answer(ResponseTemplate::new(200)).await;

    let sandbox = apns.send(Environment::Sandbox).await;

    assert_eq!(apns.transport.environments(), [Environment::Production]);
    let Delivery::Failed(reason) = sandbox else {
        panic!("{sandbox:?} is a failure, as the transport has no sandbox key");
    };
    assert!(reason.contains("sandbox"), "{reason}");
    assert!(
        apns.sandbox
            .received_requests()
            .await
            .expect("records")
            .is_empty()
    );
    let both = Apns::start().await;
    assert_eq!(
        both.transport.environments(),
        [Environment::Production, Environment::Sandbox]
    );
}
