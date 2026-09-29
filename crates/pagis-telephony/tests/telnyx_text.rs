//! The Telnyx text half (ADR-0020) against a stand-in for the
//! carrier's REST API: the Workspace objects `prepare` makes or
//! reuses, the send, the delivery read, and the KV queue the relay
//! writes inbound texts into.

use std::time::Duration;

use pagis_core::{ConnectionId, PhoneNumber, PhoneNumberId, TextDeliveryStatus, WorkspaceId};
use pagis_telephony::{
    CarrierKey, TELNYX_KV_NAMESPACE_KEY, TELNYX_MESSAGING_PROFILE_KEY, TELNYX_RELAY_URL_KEY,
    TelnyxTextTransport, TextError, TextTransport,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WORKSPACE: &str = "01JWORKSPACE";
const PROFILE_ID: &str = "40017a7b-0000-0000-0000-000000000001";
const NAMESPACE_ID: &str = "8fb2c0d1-0000-0000-0000-000000000002";
const NUMBER_ID: &str = "1293384261075731499";
const E164: &str = "+14155550100";
const RELAY_URL: &str = "https://pagis-sms-relay-1234567890.telnyxcompute.com";

fn key() -> CarrierKey {
    CarrierKey::new("", "KEY123")
}

fn transport(server: &MockServer) -> TelnyxTextTransport {
    TelnyxTextTransport::with_base_url(format!("{}/v2", server.uri()))
}

fn number() -> PhoneNumber {
    PhoneNumber::new(
        PhoneNumberId::from("num-1".to_string()),
        WorkspaceId::from(WORKSPACE.to_string()),
        ConnectionId::from("conn-1".to_string()),
        E164.to_string(),
        NUMBER_ID.to_string(),
        None,
        1_700_000_000_000,
    )
}

/// The name every Workspace object carries, lowercased for KV.
fn object_name() -> String {
    format!("pagis-{}", WORKSPACE.to_lowercase())
}

async fn mount_attach(server: &MockServer) {
    Mock::given(method("PATCH"))
        .and(path(format!("/v2/phone_numbers/{NUMBER_ID}/messaging")))
        .and(body_json(json!({ "messaging_profile_id": PROFILE_ID })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn prepare_makes_a_profile_a_namespace_and_attaches_the_number() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/messaging_profiles"))
        .and(query_param("filter[name]", "pagis-"))
        .and(header("authorization", "Bearer KEY123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/messaging_profiles"))
        .and(body_json(json!({
            "name": object_name(),
            // A `+1` line is North American, and the catalog sells in
            // both countries of the calling code.
            "whitelisted_destinations": ["US", "CA"],
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "id": PROFILE_ID, "name": object_name() } })),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_attach(&server).await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/storage/kvs"))
        .and(body_json(json!({ "name": object_name() })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "id": NAMESPACE_ID, "name": object_name() } })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let prepared = transport(&server)
        .prepare(&key(), &number(), &json!({}))
        .await
        .unwrap();

    assert_eq!(prepared.messaging_object_id.as_deref(), Some(PROFILE_ID));
    assert_eq!(
        prepared.connection_config[TELNYX_MESSAGING_PROFILE_KEY],
        json!(PROFILE_ID)
    );
    assert_eq!(
        prepared.connection_config[TELNYX_KV_NAMESPACE_KEY],
        json!(NAMESPACE_ID)
    );
}

#[tokio::test]
async fn prepare_reuses_the_workspace_profile_and_namespace_it_finds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/messaging_profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "id": "other", "name": "pagis-01jotherworkspace" },
                { "id": PROFILE_ID, "name": object_name() },
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/messaging_profiles"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    mount_attach(&server).await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": NAMESPACE_ID, "name": object_name(), "status": "provision_ok" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let prepared = transport(&server)
        .prepare(&key(), &number(), &json!({}))
        .await
        .unwrap();

    assert_eq!(prepared.messaging_object_id.as_deref(), Some(PROFILE_ID));
    assert_eq!(
        prepared.connection_config[TELNYX_KV_NAMESPACE_KEY],
        json!(NAMESPACE_ID)
    );
}

#[tokio::test]
async fn prepare_points_the_profile_webhook_at_the_relay_url_of_the_connection() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!("/v2/messaging_profiles/{PROFILE_ID}")))
        .and(body_json(json!({
            "webhook_url": RELAY_URL,
            "webhook_api_version": "2",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
        .expect(1)
        .mount(&server)
        .await;
    mount_attach(&server).await;

    // The ids of an earlier prepare are on the Connection already, so
    // a second prepare points the existing profile at the relay.
    let prepared = transport(&server)
        .prepare(
            &key(),
            &number(),
            &json!({
                TELNYX_MESSAGING_PROFILE_KEY: PROFILE_ID,
                TELNYX_KV_NAMESPACE_KEY: NAMESPACE_ID,
                TELNYX_RELAY_URL_KEY: RELAY_URL,
            }),
        )
        .await
        .unwrap();

    assert_eq!(prepared.messaging_object_id.as_deref(), Some(PROFILE_ID));
}

#[tokio::test]
async fn a_fresh_profile_carries_the_relay_url_as_its_webhook() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/messaging_profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/messaging_profiles"))
        .and(body_json(json!({
            "name": object_name(),
            "whitelisted_destinations": ["US", "CA"],
            "webhook_url": RELAY_URL,
            "webhook_api_version": "2",
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "id": PROFILE_ID, "name": object_name() } })),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_attach(&server).await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": NAMESPACE_ID, "name": object_name() }]
        })))
        .mount(&server)
        .await;

    transport(&server)
        .prepare(
            &key(),
            &number(),
            &json!({ TELNYX_RELAY_URL_KEY: RELAY_URL }),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_number_outside_north_america_whitelists_its_own_country() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/messaging_profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/messaging_profiles"))
        .and(body_json(json!({
            "name": object_name(),
            "whitelisted_destinations": ["GB"],
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": { "id": PROFILE_ID, "name": object_name() } })),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_attach(&server).await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": NAMESPACE_ID, "name": object_name() }]
        })))
        .mount(&server)
        .await;

    let mut london = number();
    london.e164 = "+442079460123".to_string();

    transport(&server)
        .prepare(&key(), &london, &json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn send_posts_the_body_whole_and_reports_the_carrier_id_and_parts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .and(header("authorization", "Bearer KEY123"))
        .and(body_json(json!({
            "from": E164,
            "to": "+14155550111",
            "text": "the roof needs a look",
            "encoding": "auto",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": "msg-1", "parts": 2, "to": [{ "status": "queued" }] }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let sent = transport(&server)
        .send(&key(), &number(), "+14155550111", "the roof needs a look")
        .await
        .unwrap();

    assert_eq!(sent.carrier_id, "msg-1");
    assert_eq!(sent.segments, 2);
}

#[tokio::test]
async fn a_destination_the_profile_does_not_whitelist_is_not_enabled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "errors": [{ "code": "40309", "title": "destination not whitelisted" }]
        })))
        .mount(&server)
        .await;

    let error = transport(&server)
        .send(&key(), &number(), "+919876543210", "hello")
        .await
        .unwrap_err();

    assert_eq!(
        error,
        TextError::DestinationNotEnabled {
            code: "40309".to_string()
        }
    );
}

#[tokio::test]
async fn another_carrier_refusal_keeps_its_code_and_words() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "errors": [{ "code": "40305", "title": "invalid from address",
                          "detail": "sending number not associated with messaging profile" }]
        })))
        .mount(&server)
        .await;

    let error = transport(&server)
        .send(&key(), &number(), "+14155550111", "hello")
        .await
        .unwrap_err();

    assert_eq!(
        error,
        TextError::Carrier {
            code: "40305".to_string(),
            message: "sending number not associated with messaging profile".to_string(),
        }
    );
}

#[tokio::test]
async fn a_rate_limited_send_carries_the_wait_the_carrier_asked_for() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "12"))
        .mount(&server)
        .await;

    let error = transport(&server)
        .send(&key(), &number(), "+14155550111", "hello")
        .await
        .unwrap_err();

    assert_eq!(
        error,
        TextError::RateLimited {
            retry_after: Some(Duration::from_secs(12))
        }
    );
}

async fn status_of(status: &str, errors: Value) -> TextDeliveryStatus {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/messages/msg-1"))
        .and(header("authorization", "Bearer KEY123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "id": "msg-1", "to": [{ "status": status }], "errors": errors }
        })))
        .mount(&server)
        .await;
    transport(&server)
        .delivery_status(&key(), "msg-1")
        .await
        .unwrap()
}

#[tokio::test]
async fn every_telnyx_status_maps_to_one_of_the_four_states() {
    for status in ["queued", "sending"] {
        assert_eq!(
            status_of(status, json!([])).await,
            TextDeliveryStatus::Queued
        );
    }
    for status in ["sent", "delivery_unconfirmed"] {
        assert_eq!(status_of(status, json!([])).await, TextDeliveryStatus::Sent);
    }
    assert_eq!(
        status_of("delivered", json!([])).await,
        TextDeliveryStatus::Delivered
    );
    for status in ["expired", "sending_failed", "delivery_failed"] {
        assert_eq!(
            status_of(
                status,
                json!([{ "code": "40010", "title": "not 10DLC registered" }])
            )
            .await,
            TextDeliveryStatus::Failed {
                code: Some("40010".to_string()),
                reason: Some("not 10DLC registered".to_string()),
            }
        );
    }
}

/// The KV key the relay writes: the number's digits, the arrival
/// millisecond and the message id. KV keys take no colon.
fn kv_key(received_ms: i64, id: &str) -> String {
    format!("text/14155550100/{received_ms}-{id}")
}

fn received_payload(id: &str, media: Value) -> Value {
    json!({
        "data": {
            "event_type": "message.received",
            "payload": {
                "id": id,
                "from": { "phone_number": "+14155550111" },
                "to": [{ "phone_number": E164 }],
                "text": "on my way",
                "received_at": "2026-09-05T10:00:00.000Z",
                "media": media,
            }
        }
    })
}

#[tokio::test]
async fn polling_reads_the_queue_in_key_order_and_empties_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": NAMESPACE_ID, "name": object_name() }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/v2/storage/kvs/{NAMESPACE_ID}/keys")))
        .and(query_param("prefix", "text/14155550100/"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "key": kv_key(1_757_066_500_000, "msg-b"), "size_bytes": 300 },
                { "key": kv_key(1_757_066_400_000, "msg-a"), "size_bytes": 300 },
            ],
            "meta": { "has_more": false }
        })))
        .expect(1)
        .mount(&server)
        .await;
    for (received_ms, id, media) in [
        (1_757_066_400_000_i64, "msg-a", json!([])),
        (
            1_757_066_500_000,
            "msg-b",
            json!([{ "url": "https://media.telnyx.com/pic.jpg", "content_type": "image/jpeg" }]),
        ),
    ] {
        let encoded = kv_key(received_ms, id).replace('/', "%2F");
        Mock::given(method("GET"))
            .and(path(format!(
                "/v2/storage/kvs/{NAMESPACE_ID}/keys/{encoded}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(received_payload(id, media)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!(
                "/v2/storage/kvs/{NAMESPACE_ID}/keys/{encoded}"
            )))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }

    let (texts, cursor) = transport(&server)
        .poll_inbound(&key(), &number(), None)
        .await
        .unwrap();

    assert_eq!(cursor, None, "the queue is the cursor");
    let ids: Vec<&str> = texts.iter().map(|text| text.carrier_id.as_str()).collect();
    assert_eq!(ids, ["msg-a", "msg-b"], "in key order");
    assert_eq!(texts[0].from_e164, "+14155550111");
    assert_eq!(texts[0].to_e164, E164);
    assert_eq!(texts[0].body, "on my way");
    // The payload's own arrival time, not the key's.
    assert_eq!(texts[0].received_at, 1_788_602_400_000);
    assert!(texts[0].media_urls.is_empty());
    assert_eq!(texts[1].media_urls, ["https://media.telnyx.com/pic.jpg"]);
}

#[tokio::test]
async fn a_workspace_with_no_namespace_has_no_relay_and_reads_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;

    let (texts, cursor) = transport(&server)
        .poll_inbound(&key(), &number(), None)
        .await
        .unwrap();

    assert!(texts.is_empty());
    assert_eq!(cursor, None);
}

#[tokio::test]
async fn a_rate_limited_poll_carries_the_wait_the_carrier_asked_for() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/storage/kvs"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;

    let error = transport(&server)
        .poll_inbound(&key(), &number(), None)
        .await
        .unwrap_err();

    assert_eq!(
        error,
        TextError::RateLimited {
            retry_after: Some(Duration::from_secs(30))
        }
    );
}

#[tokio::test]
async fn media_is_fetched_with_the_account_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/media/pic.jpg"))
        .and(header("authorization", "Bearer KEY123"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"jpeg-bytes".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let bytes = transport(&server)
        .fetch_media(&key(), &format!("{}/media/pic.jpg", server.uri()))
        .await
        .unwrap();

    assert_eq!(bytes, b"jpeg-bytes");
}

#[tokio::test]
async fn the_telnyx_transport_carries_texts_and_their_media() {
    let server = MockServer::start().await;
    let capabilities = transport(&server).capabilities();

    assert!(capabilities.texting);
    assert!(capabilities.inbound_media);
}
