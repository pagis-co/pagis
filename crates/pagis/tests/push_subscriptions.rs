//! Full-daemon tests of the VAPID Key and the Push Subscriptions
//! (ADR-0030).
//!
//! The installation has one VAPID Key, which a client reads to
//! subscribe. A client posts its subscription, and the subscription
//! belongs to the Session that posted it and ends with that Session.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pagis_testkit::{TestDaemon, TestDaemonOptions};

/// The receiver public key of RFC 8291, Appendix A: a valid uncompressed
/// P-256 point, as a browser sends it.
const P256DH: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
/// A 16-byte auth secret. Any 16 bytes are valid.
const AUTH: &[u8] = &[7; 16];
const ENDPOINT: &str = "https://push.example.com/send/abc";

/// The body of `PushSubscription.toJSON()`, with the auth secret as
/// base64url.
fn subscription(endpoint: &str, p256dh: &str, auth: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "endpoint": endpoint,
        "expirationTime": null,
        "keys": {"p256dh": p256dh, "auth": URL_SAFE_NO_PAD.encode(auth)},
    })
}

async fn subscribe(
    daemon: &TestDaemon,
    cookie: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/push-subscriptions", daemon.base_url))
        .header("cookie", cookie)
        .json(body)
        .send()
        .await
        .expect("the daemon answers the subscription");
    let status = response.status().as_u16();
    (
        status,
        response.json().await.unwrap_or(serde_json::Value::Null),
    )
}

async fn push_subscriptions(daemon: &TestDaemon, cookie: &str) -> Vec<serde_json::Value> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/push-subscriptions", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .expect("the daemon answers the list");
    assert_eq!(response.status(), 200);
    response.json::<serde_json::Value>().await.unwrap()["items"]
        .as_array()
        .expect("the items")
        .clone()
}

async fn vapid_public_key(daemon: &TestDaemon) -> String {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/push/key", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the daemon answers the key");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    body["vapid_public_key"]
        .as_str()
        .unwrap_or_else(|| panic!("the key answer holds no key: {body}"))
        .to_string()
}

/// The VAPID Key belongs to the installation and lives in
/// `secrets.enc`, so a restart over the same secrets answers the same
/// key. The key is the uncompressed point that `PushManager.subscribe`
/// takes as `applicationServerKey`.
#[tokio::test]
async fn the_vapid_key_is_one_valid_point_that_outlives_a_restart() {
    let secrets: Arc<dyn pagis_core::SecretStore> =
        Arc::new(pagis_core::MemorySecretStore::default());
    let options = || TestDaemonOptions {
        secrets: Arc::clone(&secrets),
        ..TestDaemonOptions::default()
    };
    let daemon = TestDaemon::start_with(options()).await;

    let first = vapid_public_key(&daemon).await;
    assert_eq!(vapid_public_key(&daemon).await, first);

    let point = URL_SAFE_NO_PAD
        .decode(&first)
        .expect("the key is base64url with no padding");
    assert_eq!(point.len(), 65);
    assert_eq!(point[0], 0x04, "the point is uncompressed");
    p256::PublicKey::from_sec1_bytes(&point).expect("the point is on P-256");

    let daemon = daemon.restart(options()).await;
    assert_eq!(vapid_public_key(&daemon).await, first);
}

/// Each refused field answers 422 with a message of its own, and the
/// daemon keeps nothing of a refused body.
#[tokio::test]
async fn each_invalid_subscription_answers_422_with_its_own_message() {
    let daemon = TestDaemon::start().await;
    let long = format!("https://push.example.com/{}", "a".repeat(2049 - 25));
    assert_eq!(long.chars().count(), 2049);
    let not_on_the_curve = URL_SAFE_NO_PAD.encode([&[0x04][..], &[0u8; 64][..]].concat());
    let cases = [
        (
            "an http endpoint",
            subscription("http://push.example.com/x", P256DH, AUTH),
        ),
        (
            "an IP literal",
            subscription("https://10.0.0.1/x", P256DH, AUTH),
        ),
        (
            "a 2049-character endpoint",
            subscription(&long, P256DH, AUTH),
        ),
        (
            "a point off the curve",
            subscription(ENDPOINT, &not_on_the_curve, AUTH),
        ),
        (
            "a 15-byte auth secret",
            subscription(ENDPOINT, P256DH, &[7; 15]),
        ),
    ];

    let mut messages = Vec::new();
    for (case, body) in &cases {
        let (status, answer) = subscribe(&daemon, daemon.cookie(), body).await;
        assert_eq!(status, 422, "{case} answered {status}: {answer}");
        assert_eq!(answer["error"]["code"], "validation", "{case}: {answer}");
        messages.push(
            answer["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{case} answered no message: {answer}"))
                .to_string(),
        );
    }
    let mut distinct = messages.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        cases.len(),
        "each case has its own message: {messages:?}"
    );
    assert!(
        push_subscriptions(&daemon, daemon.cookie())
            .await
            .is_empty()
    );

    // An IPv6 literal is an IP literal too.
    let (status, _) = subscribe(
        &daemon,
        daemon.cookie(),
        &subscription("https://[2001:db8::1]/x", P256DH, AUTH),
    )
    .await;
    assert_eq!(status, 422);
}

/// A subscription belongs to the Session that posts it. The list names
/// the client of that Session and marks the one that asks.
#[tokio::test]
async fn a_subscription_lists_with_the_client_of_its_session() {
    let daemon = TestDaemon::start().await;
    let phone = daemon.cookie_for(&daemon.user_id).await;

    let (status, posted) = subscribe(&daemon, &phone, &subscription(ENDPOINT, P256DH, AUTH)).await;
    assert_eq!(status, 200, "{posted}");

    let from_phone = push_subscriptions(&daemon, &phone).await;
    assert_eq!(from_phone.len(), 1);
    assert_eq!(from_phone[0]["id"], posted["id"]);
    assert_eq!(from_phone[0]["client_kind"], "browser");
    assert_eq!(from_phone[0]["current"], true);
    assert_eq!(from_phone[0]["last_sent_at"], serde_json::Value::Null);
    assert!(from_phone[0]["created_at"].is_i64());
    assert!(
        from_phone[0].get("endpoint").is_none(),
        "the list names the client, not the push service"
    );

    let from_desk = push_subscriptions(&daemon, daemon.cookie()).await;
    assert_eq!(from_desk.len(), 1);
    assert_eq!(from_desk[0]["current"], false);

    // A known endpoint moves to the Session that sends it again.
    let (status, moved) = subscribe(
        &daemon,
        daemon.cookie(),
        &subscription(ENDPOINT, P256DH, AUTH),
    )
    .await;
    assert_eq!(status, 200, "{moved}");
    let from_desk = push_subscriptions(&daemon, daemon.cookie()).await;
    assert_eq!(from_desk.len(), 1);
    assert_eq!(from_desk[0]["current"], true);
}

/// A sign-out ends the Session, and the Push Subscription of that
/// Session ends with it. The Push Subscription of another Session stays.
#[tokio::test]
async fn a_sign_out_removes_the_push_subscription_of_that_session() {
    let daemon = TestDaemon::start().await;
    let phone = daemon.cookie_for(&daemon.user_id).await;
    let (status, _) = subscribe(&daemon, &phone, &subscription(ENDPOINT, P256DH, AUTH)).await;
    assert_eq!(status, 200);
    let (status, desk) = subscribe(
        &daemon,
        daemon.cookie(),
        &subscription("https://push.example.com/desk", P256DH, AUTH),
    )
    .await;
    assert_eq!(status, 200);

    daemon.sign_out(&phone).await;

    let left = push_subscriptions(&daemon, daemon.cookie()).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["id"], desk["id"]);
    assert!(
        daemon
            .stores()
            .push_subscriptions
            .list(&daemon.workspace_id)
            .await
            .unwrap()
            .iter()
            .all(|row| row.endpoint != ENDPOINT),
        "the row of the ended Session is gone"
    );
}

/// The Person removes one of their own Push Subscriptions, and a second
/// removal finds nothing.
#[tokio::test]
async fn a_person_removes_their_own_push_subscription() {
    let daemon = TestDaemon::start().await;
    let (_, posted) = subscribe(
        &daemon,
        daemon.cookie(),
        &subscription(ENDPOINT, P256DH, AUTH),
    )
    .await;
    let url = format!(
        "{}/api/v1/push-subscriptions/{}",
        daemon.base_url,
        posted["id"].as_str().expect("the id")
    );

    for expected in [204, 404] {
        let response = reqwest::Client::new()
            .delete(&url)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    assert!(
        push_subscriptions(&daemon, daemon.cookie())
            .await
            .is_empty()
    );
}
