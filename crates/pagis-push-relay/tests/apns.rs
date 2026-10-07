//! The APNs transport against a fake APNs: the request that it sends, the
//! provider token that it keeps, and the delivery of each answer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use pagis_push_relay::{
    ApnsBaseUrls, ApnsSettings, ApnsTransport, Clock, Delivery, Environment, Message, Platform,
    Registration, Transport, Urgency,
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

/// A fake APNs for each environment, and a transport that sends to them
/// with a key in a temporary `.p8` file.
struct Apns {
    production: MockServer,
    sandbox: MockServer,
    clock: Arc<TestClock>,
    transport: ApnsTransport,
    _key_file: tempfile::NamedTempFile,
}

impl Apns {
    async fn start() -> Self {
        let production = MockServer::start().await;
        let sandbox = MockServer::start().await;
        let base_urls = ApnsBaseUrls {
            production: production.uri(),
            sandbox: sandbox.uri(),
        };
        let clock = TestClock::at(NOON);
        let (transport, key_file) = transport(base_urls, clock.clone());
        Self {
            production,
            sandbox,
            clock,
            transport,
            _key_file: key_file,
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

    /// The `iat` of the provider token of each request, in order.
    async fn issued_at(&self) -> Vec<u64> {
        let requests = self
            .sandbox
            .received_requests()
            .await
            .expect("the fake APNs records requests");
        requests
            .iter()
            .map(|request| {
                let authorization = request.headers["authorization"].to_str().expect("ASCII");
                let token = authorization.strip_prefix("bearer ").expect("a bearer");
                claims(token)["iat"].as_u64().expect("an iat")
            })
            .collect()
    }
}

/// A transport to `base_urls` with a key in a temporary `.p8` file, which
/// the caller keeps.
fn transport(
    base_urls: ApnsBaseUrls,
    clock: Arc<TestClock>,
) -> (ApnsTransport, tempfile::NamedTempFile) {
    let key = SecretKey::from_slice(&[3; 32]).expect("a P-256 scalar");
    let pem = key.to_pkcs8_pem(LineEnding::LF).expect("a PKCS#8 PEM");
    let key_file = tempfile::NamedTempFile::new().expect("a temporary file");
    std::fs::write(key_file.path(), pem.as_bytes()).expect("write the key");
    let settings = ApnsSettings {
        key_path: key_file.path().to_path_buf(),
        key_id: "ABC123DEFG".to_string(),
        team_id: "DEF123GHIJ".to_string(),
        topic: "app.pagis.mobile".to_string(),
    };
    let transport = ApnsTransport::new(&settings, base_urls, clock).expect("the APNs transport");
    (transport, key_file)
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
    let claims = token.split('.').nth(1).expect("a JWT");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).expect("base64url")).expect("JSON")
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
    assert_eq!(request.headers["apns-topic"], "app.pagis.mobile");
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
    let (transport, _key_file) = transport(base_urls, TestClock::at(NOON));

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
