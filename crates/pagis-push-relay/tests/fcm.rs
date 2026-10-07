//! The FCM transport against a fake FCM: the request that it sends and
//! the delivery of each answer.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pagis_push_relay::{
    Delivery, FcmTransport, Message, Platform, Registration, TokenSource, Transport, Urgency,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DEVICE_TOKEN: &str = "dX3k9:APA91bH-q_7rT2";
const SEND_PATH: &str = "/v1/projects/pagis-mobile/messages:send";

/// A token source that answers one fixed access token, or a failure.
struct FixedToken(Result<&'static str, &'static str>);

#[async_trait::async_trait]
impl TokenSource for FixedToken {
    async fn access_token(&self) -> Result<String, String> {
        self.0.map(str::to_string).map_err(str::to_string)
    }
}

/// A fake FCM and a transport that sends to it with a fixed token.
struct Fcm {
    server: MockServer,
    transport: FcmTransport,
}

impl Fcm {
    async fn start() -> Self {
        Self::with_tokens(FixedToken(Ok("ya29.access-token"))).await
    }

    async fn with_tokens(tokens: FixedToken) -> Self {
        let server = MockServer::start().await;
        let transport = FcmTransport::new("pagis-mobile", Arc::new(tokens), &server.uri())
            .expect("the FCM transport");
        Self { server, transport }
    }

    async fn answer(&self, answer: ResponseTemplate) {
        self.server.reset().await;
        Mock::given(method("POST"))
            .and(path(SEND_PATH))
            .respond_with(answer)
            .mount(&self.server)
            .await;
    }

    async fn send(&self, message: &Message) -> Delivery {
        self.transport.send(&registration(), message).await
    }

    /// The JSON body of each request, in order.
    async fn bodies(&self) -> Vec<Value> {
        self.server
            .received_requests()
            .await
            .expect("the fake FCM records requests")
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
            .collect()
    }
}

fn registration() -> Registration {
    Registration {
        platform: Platform::Android,
        token: DEVICE_TOKEN.to_string(),
    }
}

fn message(urgency: Urgency, topic: Option<&str>) -> Message {
    Message {
        body: vec![0xab; 120],
        ttl: Duration::from_secs(86400),
        urgency,
        topic: topic.map(str::to_string),
    }
}

/// An FCM error with `status` and the `details` of the error.
fn fcm_error(status: u16, code: &str, details: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {
            "code": status,
            "message": "the message of FCM",
            "status": code,
            "details": details,
        }
    }))
}

fn fcm_error_code(code: &str) -> Value {
    json!({
        "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
        "errorCode": code,
    })
}

fn field_violation(field: &str) -> Value {
    json!({
        "@type": "type.googleapis.com/google.rpc.BadRequest",
        "fieldViolations": [{ "field": field, "description": "Invalid value" }],
    })
}

#[tokio::test]
async fn a_push_posts_a_data_message_to_the_project_with_the_bearer_token() {
    let fcm = Fcm::start().await;
    fcm.answer(ResponseTemplate::new(200).set_body_json(
        json!({ "name": "projects/pagis-mobile/messages/0:1500415314455276%31bd1c9631bd1c96" }),
    ))
    .await;

    let delivery = fcm.send(&message(Urgency::High, Some("item-42"))).await;

    assert_eq!(delivery, Delivery::Delivered);
    let requests = fcm
        .server
        .received_requests()
        .await
        .expect("the fake FCM records requests");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url.path(), SEND_PATH);
    assert_eq!(request.headers["authorization"], "Bearer ya29.access-token");
    assert_eq!(request.headers["content-type"], "application/json");
    let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
    assert_eq!(
        body,
        json!({
            "message": {
                "token": DEVICE_TOKEN,
                "data": { "p": URL_SAFE_NO_PAD.encode([0xab; 120]) },
                "android": {
                    "priority": "HIGH",
                    "ttl": "86400s",
                    "collapse_key": "item-42",
                },
            }
        })
    );
}

#[tokio::test]
async fn each_urgency_but_high_is_normal_and_a_push_with_no_topic_has_no_collapse_key() {
    let fcm = Fcm::start().await;
    fcm.answer(ResponseTemplate::new(200)).await;
    let mut short = message(Urgency::Normal, None);
    short.ttl = Duration::from_secs(0);

    for urgency in [Urgency::VeryLow, Urgency::Low] {
        fcm.send(&message(urgency, None)).await;
    }
    fcm.send(&short).await;

    let bodies = fcm.bodies().await;
    assert_eq!(bodies.len(), 3);
    for body in &bodies[..2] {
        assert_eq!(
            body["message"]["android"],
            json!({ "priority": "NORMAL", "ttl": "86400s" })
        );
    }
    assert_eq!(
        bodies[2]["message"]["android"],
        json!({ "priority": "NORMAL", "ttl": "0s" })
    );
    assert!(
        bodies
            .iter()
            .all(|body| body["message"].get("notification").is_none()),
        "a data message has no notification"
    );
}

#[tokio::test]
async fn each_answer_of_fcm_maps_to_its_delivery() {
    let fcm = Fcm::start().await;
    let failed = |reason: &str| Delivery::Failed(reason.to_string());
    let cases = [
        ("200", ResponseTemplate::new(200), Delivery::Delivered),
        (
            "UNREGISTERED",
            fcm_error(404, "NOT_FOUND", json!([fcm_error_code("UNREGISTERED")])),
            Delivery::Gone,
        ),
        (
            "INVALID_ARGUMENT on the token",
            fcm_error(
                400,
                "INVALID_ARGUMENT",
                json!([
                    fcm_error_code("INVALID_ARGUMENT"),
                    field_violation("message.token")
                ]),
            ),
            Delivery::Gone,
        ),
        (
            "INVALID_ARGUMENT on another field",
            fcm_error(
                400,
                "INVALID_ARGUMENT",
                json!([
                    fcm_error_code("INVALID_ARGUMENT"),
                    field_violation("message.android.ttl")
                ]),
            ),
            failed("FCM answered 400 INVALID_ARGUMENT"),
        ),
        (
            "INVALID_ARGUMENT with no field",
            fcm_error(
                400,
                "INVALID_ARGUMENT",
                json!([fcm_error_code("INVALID_ARGUMENT")]),
            ),
            failed("FCM answered 400 INVALID_ARGUMENT"),
        ),
        (
            "429",
            fcm_error(
                429,
                "RESOURCE_EXHAUSTED",
                json!([fcm_error_code("QUOTA_EXCEEDED")]),
            ),
            failed("FCM answered 429 QUOTA_EXCEEDED"),
        ),
        (
            "503",
            fcm_error(503, "UNAVAILABLE", json!([fcm_error_code("UNAVAILABLE")])),
            failed("FCM answered 503 UNAVAILABLE"),
        ),
        (
            "503 with no JSON",
            ResponseTemplate::new(503).set_body_string("Service Unavailable"),
            failed("FCM answered 503 with no error code"),
        ),
    ];
    for (case, answer, expected) in cases {
        fcm.answer(answer).await;

        let delivery = fcm.send(&message(Urgency::High, None)).await;

        assert_eq!(delivery, expected, "{case}");
        if let Delivery::Failed(reason) = &delivery {
            assert!(!reason.contains(DEVICE_TOKEN), "{case}: {reason}");
        }
    }
}

#[tokio::test]
async fn a_token_source_that_fails_sends_nothing() {
    let fcm = Fcm::with_tokens(FixedToken(Err("the token endpoint answered 500"))).await;
    fcm.answer(ResponseTemplate::new(200)).await;

    let delivery = fcm.send(&message(Urgency::High, None)).await;

    assert_eq!(
        delivery,
        Delivery::Failed("no FCM access token: the token endpoint answered 500".to_string())
    );
    assert!(fcm.bodies().await.is_empty());
}
