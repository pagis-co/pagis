//! The shared fixture `fixtures/web-push.json`: the keys of a Push
//! Subscription, one body that this crate encrypted for it, and its
//! plaintext, the Declarative Web Push JSON of ADR-0030. The XCTest tests
//! of the iOS Notification Service Extension and the tests of the Android
//! messaging service decrypt the same body, so the clients and the sender
//! agree on one encryption.
//!
//! This command makes the file again, with new keys:
//!
//! ```bash
//! PAGIS_PUSH_WRITE_FIXTURE=1 cargo nextest run -p pagis-push --run-ignored only
//! ```
//!
//! With no `PAGIS_PUSH_WRITE_FIXTURE`, the ignored test makes a new body
//! and writes nothing, so a gate that runs the ignored tests leaves the
//! committed file as it is.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use pagis_push::{Options, Outcome, Policy, Subscription, Urgency, WebPush};
use serde_json::{Value, json};
use web_push_native::Auth;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The plaintext of the fixture: the Notification of a tool action
/// Approval, which the Mobile App can answer.
const PLAINTEXT: &str = concat!(
    r#"{"web_push":8030,"notification":{"title":"Robin","#,
    r#""body":"Robin needs your approval\nhost_shell","#,
    r#""navigate":"https://pagis.example.com/c/ch-1","#,
    r#""data":{"v":1,"item":"request:r-1","kind":"approval","#,
    r#""request":{"id":"r-1","actions":["approve_once","deny"]}}},"#,
    r#""app_badge":3,"mutable":true}"#,
);

/// The path of the fixture, read at run time, so a binary that another
/// worktree built finds the file of this checkout.
fn fixture_path() -> PathBuf {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../../fixtures/web-push.json")
}

fn text<'a>(fixture: &'a Value, pointer: &str) -> &'a str {
    fixture
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the fixture has no text at {pointer}"))
}

fn base64url(fixture: &Value, pointer: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(text(fixture, pointer))
        .unwrap_or_else(|_| panic!("{pointer} of the fixture is not unpadded base64url"))
}

/// The committed body decrypts, with the committed keys, to the committed
/// plaintext. A change to the body or to the plaintext fails this test.
#[test]
fn the_body_of_the_fixture_decrypts_to_its_plaintext() {
    let path = fixture_path();
    let file =
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let fixture: Value = serde_json::from_slice(&file).expect("the fixture is JSON");

    let secret = SecretKey::from_slice(&base64url(&fixture, "/subscription/private_key"))
        .expect("the private key is a P-256 scalar");
    assert_eq!(
        base64url(&fixture, "/subscription/p256dh"),
        secret.public_key().to_encoded_point(false).as_bytes(),
        "p256dh is the public key of the private key"
    );
    let auth = base64url(&fixture, "/subscription/auth");
    assert_eq!(auth.len(), 16, "the auth secret is 16 bytes");

    let plaintext = web_push_native::decrypt(
        base64url(&fixture, "/body"),
        &secret,
        &Auth::clone_from_slice(&auth),
    )
    .expect("the body decrypts with the keys of the fixture");

    assert_eq!(
        String::from_utf8(plaintext).expect("the plaintext is UTF-8"),
        text(&fixture, "/plaintext")
    );
}

/// Make the fixture again: new keys, and the body that `WebPush::send`
/// posts to a push service for them. It writes the file only when
/// `PAGIS_PUSH_WRITE_FIXTURE` is `1`.
#[tokio::test]
#[ignore = "makes fixtures/web-push.json; PAGIS_PUSH_WRITE_FIXTURE=1 writes it"]
async fn make_the_fixture() {
    let secret = SecretKey::from_slice(&rand::random::<[u8; 32]>()).expect("a P-256 secret key");
    let auth: [u8; 16] = rand::random();
    let vapid_key = SecretKey::from_slice(&rand::random::<[u8; 32]>()).expect("a P-256 secret key");
    let push_service = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&push_service)
        .await;
    let p256dh = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    let subscription = Subscription {
        endpoint: format!("{}/push/fixture", push_service.uri()),
        p256dh: p256dh.clone(),
        auth: URL_SAFE_NO_PAD.encode(auth),
    };

    let outcome = WebPush::new(
        &vapid_key,
        "https://pagis.example.com",
        Policy::AllowLoopback,
    )
    .expect("a Web Push sender")
    .send(
        &subscription,
        PLAINTEXT.as_bytes(),
        Options {
            ttl: Duration::from_secs(86_400),
            urgency: Urgency::High,
            topic: None,
        },
    )
    .await;

    assert_eq!(outcome, Outcome::Delivered);
    let received = push_service
        .received_requests()
        .await
        .expect("the push service records requests");
    assert_eq!(received.len(), 1);
    let plaintext = web_push_native::decrypt(
        received[0].body.to_vec(),
        &secret,
        &Auth::clone_from_slice(&auth),
    )
    .expect("the client decrypts the new body");
    assert_eq!(plaintext, PLAINTEXT.as_bytes());
    if std::env::var_os("PAGIS_PUSH_WRITE_FIXTURE").is_none_or(|value| value != "1") {
        return;
    }
    let fixture = json!({
        "subscription": {
            "private_key": URL_SAFE_NO_PAD.encode(secret.to_bytes()),
            "p256dh": p256dh,
            "auth": subscription.auth,
        },
        "body": URL_SAFE_NO_PAD.encode(&received[0].body),
        "plaintext": PLAINTEXT,
    });
    let mut file = serde_json::to_string_pretty(&fixture).expect("a JSON value serializes");
    file.push('\n');
    std::fs::write(fixture_path(), file).expect("write the fixture");
}
