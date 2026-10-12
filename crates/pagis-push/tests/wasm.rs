//! The Web Push request and answer need no network on native or wasm targets.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, Method, StatusCode};
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use pagis_core::now_ms;
use pagis_push::{MAX_PLAINTEXT, Options, Outcome, Policy, Subscription, Topic, Urgency, WebPush};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test;
use web_push_native::Auth;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256PublicKey};
use web_push_native::jwt_simple::claims::NoCustomClaims;

const ENDPOINT: &str = "https://push.example.com:8443/push/one";
const PUBLIC_ORIGIN: &str = "https://pagis.example.com";
const PROJECT: &str = "https://github.com/pagis-co/pagis";
const AUTH: [u8; 16] = [3; 16];

fn client_key() -> SecretKey {
    SecretKey::from_slice(&[1; 32]).unwrap()
}

fn vapid_key() -> SecretKey {
    SecretKey::from_slice(&[2; 32]).unwrap()
}

fn subscription() -> Subscription {
    Subscription {
        endpoint: ENDPOINT.to_string(),
        p256dh: URL_SAFE_NO_PAD
            .encode(client_key().public_key().to_encoded_point(false).as_bytes()),
        auth: URL_SAFE_NO_PAD.encode(AUTH),
    }
}

fn sender() -> WebPush {
    WebPush::new(&vapid_key(), PUBLIC_ORIGIN, Policy::Public).unwrap()
}

fn options() -> Options {
    Options {
        ttl: Duration::from_secs(86_400),
        urgency: Urgency::High,
        topic: None,
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_client_decrypts_the_request_body() {
    let sender = sender();
    for plaintext in [b"hello".as_slice(), &[], &[b'a'; MAX_PLAINTEXT]] {
        let request = sender
            .request(&subscription(), plaintext, options())
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri().to_string(), ENDPOINT);
        assert_eq!(request.body().len(), plaintext.len() + 103);
        assert_ne!(request.body(), plaintext);
        assert_eq!(
            web_push_native::decrypt(
                request.into_body(),
                &client_key(),
                &Auth::clone_from_slice(&AUTH),
            )
            .unwrap(),
            plaintext,
        );
    }
    let first = sender
        .request(&subscription(), b"hello", options())
        .unwrap();
    let second = sender
        .request(&subscription(), b"hello", options())
        .unwrap();
    assert_ne!(
        first.body(),
        second.body(),
        "each request uses new random values"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_vapid_token_verifies_with_the_public_key() {
    for (origin, contact) in [
        (PUBLIC_ORIGIN, PUBLIC_ORIGIN),
        ("http://localhost", PROJECT),
    ] {
        let sender = WebPush::new(&vapid_key(), origin, Policy::Public).unwrap();
        let request = sender
            .request(&subscription(), b"hello", options())
            .unwrap();
        let (token, key) = request.headers()["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix("vapid t=")
            .unwrap()
            .split_once(", k=")
            .unwrap();
        let key = URL_SAFE_NO_PAD.decode(key).unwrap();
        assert_eq!(
            key,
            vapid_key().public_key().to_encoded_point(false).as_bytes()
        );
        let claims = ES256PublicKey::from_bytes(&key)
            .unwrap()
            .verify_token::<NoCustomClaims>(token, None)
            .unwrap();
        assert_eq!(
            claims.audiences.unwrap().into_string().unwrap(),
            "https://push.example.com:8443"
        );
        assert_eq!(claims.subject.as_deref(), Some(contact));
        assert!(
            claims
                .expires_at
                .unwrap()
                .as_secs()
                .abs_diff(now_ms() as u64 / 1000 + 43_200)
                <= 1
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_request_carries_the_options_and_encoding_headers() {
    let sender = sender();
    for (urgency, header) in [
        (Urgency::VeryLow, "very-low"),
        (Urgency::Low, "low"),
        (Urgency::Normal, "normal"),
        (Urgency::High, "high"),
    ] {
        let mut options = options();
        options.urgency = urgency;
        options.topic = Some(Topic::new("request_one-2").unwrap());
        let request = sender.request(&subscription(), b"hello", options).unwrap();
        assert_eq!(request.headers()["ttl"], "86400");
        assert_eq!(request.headers()["urgency"], header);
        assert_eq!(request.headers()["topic"], "request_one-2");
        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
        assert_eq!(
            request.headers()["content-type"],
            "application/octet-stream"
        );
    }
    let request = sender
        .request(&subscription(), b"hello", options())
        .unwrap();
    assert!(!request.headers().contains_key("topic"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_request_refuses_large_plaintext_and_invalid_urls_and_keys() {
    let sender = sender();
    assert_eq!(
        sender
            .request(&subscription(), &[0; MAX_PLAINTEXT + 1], options())
            .unwrap_err(),
        Outcome::TooLarge
    );
    for endpoint in [
        "not a URL",
        "http://push.example.com/one",
        "https://127.0.0.1/one",
        "https://[::1]/one",
    ] {
        let mut subscription = subscription();
        subscription.endpoint = endpoint.to_string();
        assert!(matches!(
            sender.request(&subscription, b"hello", options()),
            Err(Outcome::Failed { status: None, .. })
        ));
    }
    for (p256dh, auth) in [
        ("!".to_string(), subscription().auth),
        (URL_SAFE_NO_PAD.encode([0; 65]), subscription().auth),
        (subscription().p256dh, "!".to_string()),
        (subscription().p256dh, URL_SAFE_NO_PAD.encode([0; 15])),
    ] {
        let subscription = Subscription {
            p256dh,
            auth,
            ..subscription()
        };
        assert!(matches!(
            sender.request(&subscription, b"hello", options()),
            Err(Outcome::Failed { status: None, .. })
        ));
    }
    // The URL check does no DNS lookup. The host checks the resolved address.
    let subscription = Subscription {
        endpoint: "https://localhost/one".to_string(),
        ..subscription()
    };
    assert!(sender.request(&subscription, b"hello", options()).is_ok());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn each_answer_maps_to_an_outcome_without_io() {
    let headers = HeaderMap::new();
    for (status, outcome) in [
        (200, Outcome::Delivered),
        (201, Outcome::Delivered),
        (299, Outcome::Delivered),
        (404, Outcome::Gone),
        (410, Outcome::Gone),
        (413, Outcome::TooLarge),
        (429, Outcome::RateLimited { retry_after: None }),
    ] {
        assert_eq!(
            Outcome::of(StatusCode::from_u16(status).unwrap(), &headers, b"", 0),
            outcome
        );
    }
    for status in [307, 400, 401, 403, 500] {
        let status = StatusCode::from_u16(status).unwrap();
        assert_eq!(
            Outcome::of(status, &headers, b" bad VAPID token \n", 0),
            Outcome::Failed {
                status: Some(status),
                error: format!("the push service answered {status}: bad VAPID token"),
            }
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn retry_after_uses_the_time_of_the_caller() {
    // The HTTP-date is 120 seconds after this fixed time.
    let now = 1_445_412_360_000;
    for (value, delay) in [
        ("120", Some(Duration::from_secs(120))),
        (" 120 ", Some(Duration::from_secs(120))),
        (
            "Wed, 21 Oct 2015 07:28:00 GMT",
            Some(Duration::from_secs(120)),
        ),
        ("Wed, 21 Oct 2015 07:25:00 GMT", Some(Duration::ZERO)),
        ("0", Some(Duration::ZERO)),
        ("invalid", None),
        ("-1", None),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", value.parse().unwrap());
        assert_eq!(
            Outcome::of(StatusCode::TOO_MANY_REQUESTS, &headers, b"", now),
            Outcome::RateLimited { retry_after: delay }
        );
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        "retry-after",
        "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
    );
    assert_eq!(
        Outcome::of(StatusCode::TOO_MANY_REQUESTS, &headers, b"", now + 500),
        Outcome::RateLimited {
            retry_after: Some(Duration::from_millis(119_500))
        }
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_failed_answer_keeps_only_the_start_of_the_body() {
    let mut body = vec![b'a'; 512];
    body.extend_from_slice(b"secret after the limit");
    let Outcome::Failed { status, error } =
        Outcome::of(StatusCode::BAD_REQUEST, &HeaderMap::new(), &body, 0)
    else {
        panic!("a failed answer");
    };
    assert_eq!(status, Some(StatusCode::BAD_REQUEST));
    assert!(error.ends_with(&"a".repeat(512)));
    assert!(!error.contains("secret"));
    let Outcome::Failed { error, .. } =
        Outcome::of(StatusCode::BAD_REQUEST, &HeaderMap::new(), &[255], 0)
    else {
        panic!("a failed answer");
    };
    assert!(error.ends_with('\u{fffd}'));
}
