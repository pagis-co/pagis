//! A Web Push to the endpoint of a registration: the checks of the relay,
//! the forward to the transport of the platform, and the answer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use pagis_push::{Options, Outcome, Policy, Subscription, Topic, WebPush};
use pagis_push_relay::{Delivery, Environment, Platform, Urgency};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::{APNS_TOKEN, Relay, TrustedProxy, VAPID_SECRET};

/// 2026-10-06 23:00:00 UTC, one hour before a UTC midnight.
const LATE_EVENING: u64 = 1_791_327_600;
/// 2026-10-07 00:00:00 UTC.
const NEXT_MIDNIGHT: u64 = 1_791_331_200;

/// The secret key of the client that a Web Push of `pagis-push` is
/// encrypted for, and its auth secret.
const CLIENT_SECRET: [u8; 32] = [9; 32];
const CLIENT_AUTH: [u8; 16] = [5; 16];

fn signing_key(secret: &[u8; 32]) -> SigningKey {
    SigningKey::from_slice(secret).expect("a P-256 scalar")
}

fn base64url(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The VAPID token and key that a push service reads.
struct Vapid {
    header: Value,
    aud: String,
    exp: u64,
    /// The key that signs the token.
    signer: SigningKey,
    /// The key that `k` names.
    key: SigningKey,
}

impl Vapid {
    /// A valid token for the relay: signed with the registered key, for
    /// the relay's origin, good for 12 hours after the relay's now.
    fn valid(relay: &Relay) -> Self {
        Self {
            header: json!({ "typ": "JWT", "alg": "ES256" }),
            aud: relay.origin.clone(),
            exp: unix_seconds(pagis_push_relay::Clock::now(relay.clock.as_ref())) + 12 * 3600,
            signer: signing_key(&VAPID_SECRET),
            key: signing_key(&VAPID_SECRET),
        }
    }

    /// The `Authorization` header: `vapid t=<jwt>, k=<key>`.
    fn authorization(&self) -> String {
        let claims =
            json!({ "aud": self.aud, "exp": self.exp, "sub": "https://pagis.example.test" });
        let input = format!(
            "{}.{}",
            base64url(self.header.to_string()),
            base64url(claims.to_string())
        );
        let signature: Signature = self.signer.sign(input.as_bytes());
        let key = self.key.verifying_key().to_encoded_point(false);
        format!(
            "vapid t={input}.{}, k={}",
            base64url(signature.to_bytes()),
            base64url(key.as_bytes())
        )
    }
}

/// One push request with the headers of a valid Web Push, which a test
/// changes one at a time.
struct Push {
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Push {
    fn new(relay: &Relay) -> Self {
        Self {
            headers: vec![
                ("content-encoding", "aes128gcm".to_string()),
                ("authorization", Vapid::valid(relay).authorization()),
                ("ttl", "86400".to_string()),
                ("urgency", "high".to_string()),
                ("topic", "item-42".to_string()),
            ],
            body: vec![0xab; 120],
        }
    }

    fn with(self, name: &'static str, value: impl Into<String>) -> Self {
        let mut push = self.without(name);
        push.headers.push((name, value.into()));
        push
    }

    fn without(mut self, name: &'static str) -> Self {
        self.headers.retain(|(header, _)| *header != name);
        self
    }

    fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    async fn send(self, relay: &Relay, endpoint: &str) -> reqwest::Response {
        let mut request = relay.client.post(endpoint).body(self.body);
        for (name, value) in self.headers {
            request = request.header(name, value);
        }
        request.send().await.expect("the relay answers")
    }
}

/// Register an iOS installation and answer its id and its endpoint.
async fn registered(relay: &Relay) -> (String, String) {
    let body = relay.registered().await;
    let id = body["id"].as_str().expect("an id").to_string();
    let endpoint = body["endpoint"].as_str().expect("an endpoint").to_string();
    (id, endpoint)
}

async fn count_of(relay: &Relay, id: &str) -> (i64, Option<String>, Option<i64>) {
    sqlx::query_as("SELECT pushes_today, day, last_push_at FROM registrations WHERE id = ?")
        .bind(id)
        .fetch_one(&relay.pool)
        .await
        .expect("read the registration")
}

async fn set_count(relay: &Relay, id: &str, pushes: i64, day: &str) {
    sqlx::query("UPDATE registrations SET pushes_today = ?, day = ? WHERE id = ?")
        .bind(pushes)
        .bind(day)
        .bind(id)
        .execute(&relay.pool)
        .await
        .expect("set the count");
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("a {name} header"))
        .to_str()
        .expect("ASCII")
}

/// The sender of a server whose VAPID Key is the registered key.
fn sender() -> WebPush {
    let key = SecretKey::from_slice(&VAPID_SECRET).expect("the VAPID secret key");
    WebPush::new(&key, "https://pagis.example.test", Policy::AllowLoopback)
        .expect("the Web Push sender")
}

fn subscription(endpoint: &str) -> Subscription {
    let client = SecretKey::from_slice(&CLIENT_SECRET).expect("the client secret key");
    Subscription {
        endpoint: endpoint.to_string(),
        p256dh: base64url(client.public_key().to_encoded_point(false).as_bytes()),
        auth: base64url(CLIENT_AUTH),
    }
}

fn notification_options() -> Options {
    Options {
        ttl: Duration::from_secs(86400),
        urgency: pagis_push::Urgency::High,
        topic: Some(Topic::new("item-42").expect("a topic")),
    }
}

#[tokio::test]
async fn a_web_push_of_pagis_push_reaches_the_transport_unchanged() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;
    let plaintext = br#"{"web_push":8030,"notification":{"title":"Approve?"}}"#;

    let outcome = sender()
        .send(&subscription(&endpoint), plaintext, notification_options())
        .await;

    assert_eq!(outcome, Outcome::Delivered);
    let sent = relay.transport.sent();
    assert_eq!(sent.len(), 1);
    let (registration, message) = &sent[0];
    assert_eq!(registration.platform, Platform::Ios(Environment::Sandbox));
    assert_eq!(registration.token, APNS_TOKEN);
    assert_eq!(message.ttl, Duration::from_secs(86400));
    assert_eq!(message.urgency, Urgency::High);
    assert_eq!(message.topic.as_deref(), Some("item-42"));
    let client = SecretKey::from_slice(&CLIENT_SECRET).expect("the client secret key");
    let decrypted = web_push_native::decrypt(
        message.body.clone(),
        &client,
        &web_push_native::Auth::clone_from_slice(&CLIENT_AUTH),
    )
    .expect("the client decrypts the body that the transport got");
    assert_eq!(decrypted, plaintext);
}

#[tokio::test]
async fn a_delivered_push_answers_created_with_a_location_and_the_ttl() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (id, endpoint) = registered(&relay).await;
    let body: Vec<u8> = (0..=255u8).cycle().take(1000).collect();

    let response = Push::new(&relay)
        .with("ttl", "600")
        .body(body.clone())
        .send(&relay, &endpoint)
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let location = header(&response, "location").to_string();
    let message_id = location
        .strip_prefix(&format!("{}/v1/messages/", relay.origin))
        .unwrap_or_else(|| panic!("{location} is a message on the public origin"));
    assert!(!message_id.is_empty());
    assert_eq!(header(&response, "ttl"), "600");
    let sent = relay.transport.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].1.body, body,
        "the transport gets the body unchanged"
    );
    assert_eq!(sent[0].1.ttl, Duration::from_secs(600));
    let (pushes, _, last_push_at) = count_of(&relay, &id).await;
    assert_eq!(pushes, 1);
    assert!(
        last_push_at.is_some(),
        "the relay keeps the time of the push"
    );
    let message = relay
        .client
        .get(&location)
        .send()
        .await
        .expect("the relay answers");
    assert_eq!(
        message.status(),
        StatusCode::NOT_FOUND,
        "the relay stores no message"
    );
}

#[tokio::test]
async fn a_header_with_only_alg_and_an_exp_24_hours_ahead_is_valid() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;
    relay.clock.set(at(LATE_EVENING));
    let vapid = Vapid {
        header: json!({ "alg": "ES256" }),
        exp: LATE_EVENING + 24 * 3600,
        ..Vapid::valid(&relay)
    };

    let response = Push::new(&relay)
        .with("authorization", vapid.authorization())
        .without("urgency")
        .without("topic")
        .send(&relay, &endpoint)
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let sent = relay.transport.sent();
    assert_eq!(sent[0].1.urgency, Urgency::Normal, "the default urgency");
    assert_eq!(sent[0].1.topic, None);
}

#[tokio::test]
async fn an_unknown_id_answers_not_found() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let unknown = URL_SAFE_NO_PAD.encode([7u8; 16]);

    for id in [unknown.as_str(), "not-an-id"] {
        let response = Push::new(&relay)
            .send(&relay, &format!("{}/v1/push/{id}", relay.origin))
            .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{id}");
    }
    assert!(relay.transport.sent().is_empty());
}

#[tokio::test]
async fn a_body_that_is_not_aes128gcm_answers_unsupported_media_type() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;

    for push in [
        Push::new(&relay).without("content-encoding"),
        Push::new(&relay).with("content-encoding", "aesgcm"),
        Push::new(&relay).with("content-encoding", "gzip"),
    ] {
        let response = push.send(&relay, &endpoint).await;

        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    assert!(relay.transport.sent().is_empty());
}

#[tokio::test]
async fn a_missing_or_malformed_authorization_answers_unauthorized() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;
    let token = Vapid::valid(&relay).authorization();
    let jwt_only = token.split(", k=").next().expect("the t part").to_string();

    for push in [
        Push::new(&relay).without("authorization"),
        Push::new(&relay).with("authorization", "Bearer abc"),
        Push::new(&relay).with("authorization", jwt_only),
        Push::new(&relay).with("authorization", "vapid"),
        Push::new(&relay).with("authorization", format!("WebPush {}", &token[6..])),
    ] {
        let response = push.send(&relay, &endpoint).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(header(&response, "www-authenticate"), "vapid");
    }
    assert!(relay.transport.sent().is_empty());
}

#[tokio::test]
async fn a_vapid_token_that_is_not_valid_answers_forbidden() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;
    let other = signing_key(&[2; 32]);
    let now = unix_seconds(SystemTime::now());

    let cases = [
        (
            "a wrong k",
            Vapid {
                signer: other.clone(),
                key: other.clone(),
                ..Vapid::valid(&relay)
            },
        ),
        (
            "a bad signature",
            Vapid {
                signer: other.clone(),
                ..Vapid::valid(&relay)
            },
        ),
        (
            "a wrong aud",
            Vapid {
                aud: "https://push.example.test".to_string(),
                ..Vapid::valid(&relay)
            },
        ),
        (
            "an exp in the past",
            Vapid {
                exp: now - 60,
                ..Vapid::valid(&relay)
            },
        ),
        (
            "an exp over 24 hours ahead",
            Vapid {
                exp: now + 24 * 3600 + 60,
                ..Vapid::valid(&relay)
            },
        ),
        (
            "an alg other than ES256",
            Vapid {
                header: json!({ "typ": "JWT", "alg": "HS256" }),
                ..Vapid::valid(&relay)
            },
        ),
        (
            "the alg none",
            Vapid {
                header: json!({ "alg": "none" }),
                ..Vapid::valid(&relay)
            },
        ),
    ];
    for (case, vapid) in cases {
        let response = Push::new(&relay)
            .with("authorization", vapid.authorization())
            .send(&relay, &endpoint)
            .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{case}");
    }
    assert!(relay.transport.sent().is_empty());
}

#[tokio::test]
async fn a_body_over_2800_bytes_answers_payload_too_large() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;

    let over = Push::new(&relay)
        .body(vec![1; 2801])
        .send(&relay, &endpoint)
        .await;
    let at_the_limit = Push::new(&relay)
        .body(vec![1; 2800])
        .send(&relay, &endpoint)
        .await;

    assert_eq!(over.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(at_the_limit.status(), StatusCode::CREATED);
    assert_eq!(relay.transport.sent().len(), 1);
}

#[tokio::test]
async fn a_bad_ttl_urgency_or_topic_answers_bad_request() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (_, endpoint) = registered(&relay).await;

    let cases = [
        ("no TTL", Push::new(&relay).without("ttl")),
        (
            "a TTL that is not a number",
            Push::new(&relay).with("ttl", "a day"),
        ),
        ("a negative TTL", Push::new(&relay).with("ttl", "-1")),
        ("a bad Urgency", Push::new(&relay).with("urgency", "urgent")),
        (
            "a long Topic",
            Push::new(&relay).with("topic", "t".repeat(33)),
        ),
        (
            "a Topic outside base64url",
            Push::new(&relay).with("topic", "item+42"),
        ),
    ];
    for (case, push) in cases {
        let response = push.send(&relay, &endpoint).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{case}");
    }
    assert!(relay.transport.sent().is_empty());
}

#[tokio::test]
async fn the_1001st_push_of_a_utc_day_answers_too_many_requests_until_midnight() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (id, endpoint) = registered(&relay).await;
    relay.clock.set(at(LATE_EVENING));
    set_count(&relay, &id, 999, "2026-10-06").await;

    let thousandth = Push::new(&relay).send(&relay, &endpoint).await;
    let refused = Push::new(&relay).send(&relay, &endpoint).await;

    assert_eq!(thousandth.status(), StatusCode::CREATED);
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(header(&refused, "retry-after"), "3600");
    assert_eq!(relay.transport.sent().len(), 1);
    assert_eq!(count_of(&relay, &id).await.0, 1000);
}

#[tokio::test]
async fn a_new_utc_day_resets_the_count() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (id, endpoint) = registered(&relay).await;
    relay.clock.set(at(LATE_EVENING));
    set_count(&relay, &id, 1000, "2026-10-06").await;
    let refused = Push::new(&relay).send(&relay, &endpoint).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);

    relay.clock.set(at(NEXT_MIDNIGHT));
    let response = Push::new(&relay).send(&relay, &endpoint).await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let (pushes, day, _) = count_of(&relay, &id).await;
    assert_eq!(pushes, 1);
    assert_eq!(day.as_deref(), Some("2026-10-07"));
}

#[tokio::test]
async fn a_gone_registration_is_removed_and_pagis_push_reports_it_gone() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (id, endpoint) = registered(&relay).await;
    relay.transport.answer(Delivery::Gone);

    let outcome = sender()
        .send(&subscription(&endpoint), b"{}", notification_options())
        .await;

    assert_eq!(outcome, Outcome::Gone);
    assert_eq!(relay.token_of(&id).await, None);
    let again = Push::new(&relay).send(&relay, &endpoint).await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_failed_forward_answers_bad_gateway_and_does_not_count() {
    let relay = Relay::start(TrustedProxy::none()).await;
    let (id, endpoint) = registered(&relay).await;
    relay
        .transport
        .answer(Delivery::Failed("APNs answered 500".to_string()));

    let response = Push::new(&relay).send(&relay, &endpoint).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let (pushes, _, last_push_at) = count_of(&relay, &id).await;
    assert_eq!(pushes, 0);
    assert_eq!(last_push_at, None);
    relay.transport.answer(Delivery::Delivered);
    let delivered = Push::new(&relay).send(&relay, &endpoint).await;
    assert_eq!(delivered.status(), StatusCode::CREATED);
    assert_eq!(count_of(&relay, &id).await.0, 1);
}
