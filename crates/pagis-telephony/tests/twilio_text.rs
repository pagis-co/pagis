//! The Twilio text seam (ADR-0020) against a local HTTP server: the
//! Messaging Service made once and reused, the sender pool, the send,
//! the thirteen delivery states in four, the inbound poll with its
//! paging, its direction filter and its cursor, the refusals, and the
//! media fetch.

use std::time::Duration;

use pagis_core::{
    AgentId, ConnectionId, PhoneNumber, PhoneNumberId, TextDeliveryStatus, WorkspaceId, now_ms,
};
use pagis_telephony::{
    CarrierKey, TWILIO_MESSAGING_SERVICE_KEY, TextError, TextTransport, TwilioTextTransport,
};
use serde_json::json;
use wiremock::matchers::{
    body_string_contains, header, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT_SID: &str = "AC123";
const SERVICE_SID: &str = "MG9";
const E164: &str = "+14155550100";

fn key() -> CarrierKey {
    CarrierKey::new(ACCOUNT_SID, "token")
}

/// HTTP Basic with the Account SID and the Auth Token.
fn basic_auth() -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode("AC123:token");
    format!("Basic {encoded}")
}

/// A number the Workspace holds, with the messaging object the send
/// reads back.
fn number(messaging_object_id: Option<&str>) -> PhoneNumber {
    let mut number = PhoneNumber::new(
        PhoneNumberId::generate(),
        WorkspaceId::generate(),
        ConnectionId::generate(),
        E164.to_string(),
        "PN9".to_string(),
        Some(AgentId::generate()),
        now_ms(),
    );
    number.messaging_object_id = messaging_object_id.map(str::to_string);
    number
}

/// One inbound message as Twilio lists it.
fn inbound(sid: &str, date_sent: &str, body: &str) -> serde_json::Value {
    json!({
        "sid": sid,
        "from": "+14155550111",
        "to": E164,
        "body": body,
        "direction": "inbound",
        "date_sent": date_sent,
        "num_segments": "1",
        "num_media": "0",
        "status": "received",
    })
}

#[tokio::test]
async fn prepare_makes_one_messaging_service_and_puts_the_number_in_its_pool() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Services"))
        .and(header("authorization", basic_auth()))
        .and(body_string_contains("FriendlyName=Pagis"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sid": SERVICE_SID,
            "friendly_name": "Pagis",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/Services/MG9/PhoneNumbers"))
        .and(header("authorization", basic_auth()))
        .and(body_string_contains("PhoneNumberSid=PN9"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sid": "PN9",
            "service_sid": SERVICE_SID,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let prepared = transport
        .prepare(&key(), &number(None), &json!({}))
        .await
        .unwrap();

    // The number sends through the service, so the service is the
    // number's messaging object.
    assert_eq!(prepared.messaging_object_id.as_deref(), Some(SERVICE_SID));
    // The Workspace keeps the sid on its carrier Connection, so the
    // next number of the same Workspace reuses the service.
    assert_eq!(
        prepared.connection_config[TWILIO_MESSAGING_SERVICE_KEY],
        json!(SERVICE_SID)
    );
}

#[tokio::test]
async fn prepare_reuses_the_messaging_service_the_connection_already_names() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Services"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sid": "MG-second" })))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/Services/MG9/PhoneNumbers"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "sid": "PN9" })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let prepared = transport
        .prepare(
            &key(),
            &number(None),
            &json!({ TWILIO_MESSAGING_SERVICE_KEY: SERVICE_SID }),
        )
        .await
        .unwrap();

    assert_eq!(prepared.messaging_object_id.as_deref(), Some(SERVICE_SID));
    // Nothing new goes on the Connection, because nothing was made.
    assert!(prepared.connection_config.is_empty());
}

#[tokio::test]
async fn a_number_the_pool_already_holds_is_prepared() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Services/MG9/PhoneNumbers"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "code": 21_712,
            "message": "Phone Number is associated with another Messaging Service",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let prepared = transport
        .prepare(
            &key(),
            &number(None),
            &json!({ TWILIO_MESSAGING_SERVICE_KEY: SERVICE_SID }),
        )
        .await
        .unwrap();

    assert_eq!(prepared.messaging_object_id.as_deref(), Some(SERVICE_SID));
}

#[tokio::test]
async fn send_carries_the_messaging_service_and_reads_the_segment_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .and(header("authorization", basic_auth()))
        .and(body_string_contains("From=%2B14155550100"))
        .and(body_string_contains("To=%2B14155550111"))
        .and(body_string_contains("Body=hello"))
        .and(body_string_contains("MessagingServiceSid=MG9"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sid": "SM1",
            "num_segments": "2",
            "status": "queued",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let sent = transport
        .send(&key(), &number(Some(SERVICE_SID)), "+14155550111", "hello")
        .await
        .unwrap();

    assert_eq!(sent.carrier_id, "SM1");
    // The carrier segments the body and reports the count; the daemon
    // never computes one.
    assert_eq!(sent.segments, 2);
}

#[tokio::test]
async fn a_number_with_no_messaging_object_sends_from_the_number_alone() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sid": "SM1",
            "num_segments": "1",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let sent = transport
        .send(&key(), &number(None), "+14155550111", "hello")
        .await
        .unwrap();

    assert_eq!(sent.segments, 1);
    let request = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8(request.body.clone()).unwrap();
    assert!(!body.contains("MessagingServiceSid"), "body was {body}");
}

#[tokio::test]
async fn a_region_the_account_may_not_text_is_destination_not_enabled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": 21_408,
            "message": "Permission to send an SMS has not been enabled for the region",
        })))
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let refused = transport
        .send(&key(), &number(Some(SERVICE_SID)), "+919000000000", "hi")
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        TextError::DestinationNotEnabled {
            code: "21408".to_string()
        }
    );
}

#[tokio::test]
async fn a_refused_request_backs_off_for_the_time_the_carrier_names() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "30")
                .set_body_json(json!({
                    "code": 20_429,
                    "message": "Too Many Requests",
                })),
        )
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let refused = transport
        .send(&key(), &number(Some(SERVICE_SID)), "+14155550111", "hi")
        .await
        .unwrap_err();

    assert_eq!(
        refused,
        TextError::RateLimited {
            retry_after: Some(Duration::from_secs(30))
        }
    );
}

#[tokio::test]
async fn a_rate_limited_poll_backs_off_with_no_time_when_the_carrier_names_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "code": 20_429,
            "message": "Too Many Requests",
        })))
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let refused = transport
        .poll_inbound(&key(), &number(Some(SERVICE_SID)), None)
        .await
        .unwrap_err();

    assert_eq!(refused, TextError::RateLimited { retry_after: None });
}

#[tokio::test]
async fn a_refusal_the_seam_does_not_name_keeps_the_carriers_own_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "code": 20_003,
            "message": "Authenticate",
        })))
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let refused = transport
        .send(&key(), &number(Some(SERVICE_SID)), "+14155550111", "hi")
        .await
        .unwrap_err();

    assert_eq!(refused.as_str(), "carrier");
    assert_eq!(refused.carrier_code(), Some("20003"));
}

#[tokio::test]
async fn the_thirteen_delivery_states_answer_in_four() {
    let server = MockServer::start().await;
    let states = [
        ("queued", TextDeliveryStatus::Queued),
        ("accepted", TextDeliveryStatus::Queued),
        ("scheduled", TextDeliveryStatus::Queued),
        ("sending", TextDeliveryStatus::Queued),
        ("receiving", TextDeliveryStatus::Queued),
        ("sent", TextDeliveryStatus::Sent),
        ("partially_delivered", TextDeliveryStatus::Sent),
        ("delivered", TextDeliveryStatus::Delivered),
        ("received", TextDeliveryStatus::Delivered),
        ("read", TextDeliveryStatus::Delivered),
    ];
    for (state, _) in &states {
        Mock::given(method("GET"))
            .and(path(format!(
                "/2010-04-01/Accounts/AC123/Messages/SM-{state}.json"
            )))
            .and(header("authorization", basic_auth()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sid": format!("SM-{state}"),
                "status": state,
            })))
            .mount(&server)
            .await;
    }
    // The three states that mean the text did not arrive carry the
    // carrier's code and words.
    for state in ["failed", "undelivered", "canceled"] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/2010-04-01/Accounts/AC123/Messages/SM-{state}.json"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sid": format!("SM-{state}"),
                "status": state,
                "error_code": 30_003,
                "error_message": "Unreachable destination handset",
            })))
            .mount(&server)
            .await;
    }

    let transport = TwilioTextTransport::with_base_url(server.uri());
    for (state, expected) in &states {
        let read = transport
            .delivery_status(&key(), &format!("SM-{state}"))
            .await
            .unwrap();
        assert_eq!(&read, expected, "{state}");
    }
    for state in ["failed", "undelivered", "canceled"] {
        let read = transport
            .delivery_status(&key(), &format!("SM-{state}"))
            .await
            .unwrap();
        assert_eq!(
            read,
            TextDeliveryStatus::Failed {
                code: Some("30003".to_string()),
                reason: Some("Unreachable destination handset".to_string()),
            },
            "{state}"
        );
    }
}

#[tokio::test]
async fn the_first_poll_reads_every_inbound_text_and_gives_a_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .and(header("authorization", basic_auth()))
        .and(query_param("To", E164))
        .and(query_param("PageSize", "50"))
        // No cursor yet, so the poll names no moment.
        .and(query_param_is_missing("DateSent>="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            // Twilio answers newest first.
            "messages": [
                inbound("SM2", "Sat, 05 Sep 2026 12:00:30 +0000", "second"),
                {
                    "sid": "SM-out",
                    "from": E164,
                    "to": "+14155550111",
                    "body": "mine",
                    // Twilio has no direction filter, so an outbound
                    // text of the daemon's own is dropped here.
                    "direction": "outbound-api",
                    "date_sent": "Sat, 05 Sep 2026 12:00:20 +0000",
                    "num_segments": "1",
                    "num_media": "0",
                },
                inbound("SM1", "Sat, 05 Sep 2026 12:00:10 +0000", "first"),
            ],
            "next_page_uri": null,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let (texts, cursor) = transport
        .poll_inbound(&key(), &number(Some(SERVICE_SID)), None)
        .await
        .unwrap();

    // Oldest first, as the texts arrived.
    let read: Vec<&str> = texts.iter().map(|text| text.carrier_id.as_str()).collect();
    assert_eq!(read, ["SM1", "SM2"]);
    assert_eq!(texts[0].body, "first");
    assert_eq!(texts[0].from_e164, "+14155550111");
    assert_eq!(texts[0].to_e164, E164);
    assert!(texts[0].media_urls.is_empty());
    assert_eq!(cursor.as_deref(), Some("2026-09-05T12:00:30Z|SM2"));
}

#[tokio::test]
async fn the_poll_follows_the_carriers_own_next_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .and(query_param_is_missing("Page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [inbound("SM2", "Sat, 05 Sep 2026 12:00:30 +0000", "second")],
            "next_page_uri": "/2010-04-01/Accounts/AC123/Messages.json?PageSize=50&Page=1&PageToken=PA2",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .and(query_param("Page", "1"))
        .and(query_param("PageToken", "PA2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            // The same text can shift onto the next page as new ones
            // arrive, so a repeat is dropped.
            "messages": [
                inbound("SM2", "Sat, 05 Sep 2026 12:00:30 +0000", "second"),
                inbound("SM1", "Sat, 05 Sep 2026 12:00:10 +0000", "first"),
            ],
            "next_page_uri": null,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let (texts, _) = transport
        .poll_inbound(&key(), &number(Some(SERVICE_SID)), None)
        .await
        .unwrap();

    let read: Vec<&str> = texts.iter().map(|text| text.carrier_id.as_str()).collect();
    assert_eq!(read, ["SM1", "SM2"]);
}

#[tokio::test]
async fn a_cursor_names_the_moment_and_skips_the_texts_read_at_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .and(query_param("DateSent>=", "2026-09-05T12:00:00Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [
                inbound("SM3", "Sat, 05 Sep 2026 12:00:00 +0000", "same second, new"),
                inbound("SM1", "Sat, 05 Sep 2026 12:00:00 +0000", "already read"),
            ],
            "next_page_uri": null,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let (texts, cursor) = transport
        .poll_inbound(
            &key(),
            &number(Some(SERVICE_SID)),
            Some("2026-09-05T12:00:00Z|SM1"),
        )
        .await
        .unwrap();

    let read: Vec<&str> = texts.iter().map(|text| text.carrier_id.as_str()).collect();
    assert_eq!(read, ["SM3"]);
    // Both ids of that second travel on, so neither is read again.
    assert_eq!(cursor.as_deref(), Some("2026-09-05T12:00:00Z|SM1,SM3"));
}

#[tokio::test]
async fn a_poll_that_finds_nothing_keeps_the_cursor_it_came_with() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [],
            "next_page_uri": null,
        })))
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let (texts, cursor) = transport
        .poll_inbound(
            &key(),
            &number(Some(SERVICE_SID)),
            Some("2026-09-05T12:00:00Z|SM1"),
        )
        .await
        .unwrap();

    assert!(texts.is_empty());
    assert_eq!(cursor.as_deref(), Some("2026-09-05T12:00:00Z|SM1"));
}

#[tokio::test]
async fn an_inbound_text_with_media_names_the_urls_the_media_needs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [{
                "sid": "MM1",
                "from": "+14155550111",
                "to": E164,
                "body": "look",
                "direction": "inbound",
                "date_sent": "Sat, 05 Sep 2026 12:00:10 +0000",
                "num_segments": "1",
                "num_media": "2",
            }],
            "next_page_uri": null,
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages/MM1/Media.json"))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "media_list": [{ "sid": "ME1" }, { "sid": "ME2" }],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages/MM1/Media/ME1"))
        // Twilio enforces Basic auth on every media URL.
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1_u8, 2, 3]))
        .expect(1)
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let (texts, _) = transport
        .poll_inbound(&key(), &number(Some(SERVICE_SID)), None)
        .await
        .unwrap();

    assert_eq!(
        texts[0].media_urls,
        [
            format!(
                "{}/2010-04-01/Accounts/AC123/Messages/MM1/Media/ME1",
                server.uri()
            ),
            format!(
                "{}/2010-04-01/Accounts/AC123/Messages/MM1/Media/ME2",
                server.uri()
            ),
        ]
    );
    let bytes = transport
        .fetch_media(&key(), &texts[0].media_urls[0])
        .await
        .unwrap();
    assert_eq!(bytes, vec![1_u8, 2, 3]);
}

#[tokio::test]
async fn media_the_carrier_does_not_serve_is_a_carrier_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/Messages/MM1/Media/ME1"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "code": 20_404,
            "message": "Not found",
        })))
        .mount(&server)
        .await;

    let transport = TwilioTextTransport::with_base_url(server.uri());
    let refused = transport
        .fetch_media(
            &key(),
            &format!(
                "{}/2010-04-01/Accounts/AC123/Messages/MM1/Media/ME1",
                server.uri()
            ),
        )
        .await
        .unwrap_err();

    assert_eq!(refused.carrier_code(), Some("20404"));
}

#[tokio::test]
async fn twilio_carries_texts_and_inbound_media() {
    let transport = TwilioTextTransport::with_base_url("http://127.0.0.1:1");
    assert!(transport.capabilities().texting);
    assert!(transport.capabilities().inbound_media);
}
