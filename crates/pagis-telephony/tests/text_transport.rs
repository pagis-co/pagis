//! The text seam (ADR-0020): the fake carrier, the carrier that
//! carries no text, and the map that finds one per carrier.

use std::sync::Arc;

use pagis_core::{
    AgentId, ConnectionId, PhoneNumber, PhoneNumberId, TextDeliveryStatus, WorkspaceId,
};
use pagis_telephony::fake::FakeTextTransport;
use pagis_telephony::{
    CarrierKey, InboundText, NoTextTransport, PLIVO_PROVIDER, Prepared, TELNYX_PROVIDER,
    TWILIO_PROVIDER, TextCapabilities, TextError, TextTransport, TextTransports,
};
use serde_json::json;

fn key() -> CarrierKey {
    CarrierKey::new("", "carrier-secret")
}

fn number(e164: &str) -> PhoneNumber {
    PhoneNumber::new(
        PhoneNumberId::generate(),
        WorkspaceId::generate(),
        ConnectionId::generate(),
        e164.to_string(),
        format!("carrier-{e164}"),
        Some(AgentId::generate()),
        1_700_000_000_000,
    )
}

fn inbound(to_e164: &str, body: &str, carrier_id: &str) -> InboundText {
    InboundText {
        carrier_id: carrier_id.to_string(),
        from_e164: "+14155550999".to_string(),
        to_e164: to_e164.to_string(),
        body: body.to_string(),
        received_at: 1_700_000_100_000,
        media_urls: Vec::new(),
    }
}

#[tokio::test]
async fn the_fake_carries_a_send_and_reports_the_segments_the_carrier_billed() {
    let transport = FakeTextTransport::default();
    let from = number("+14155550123");

    let long = "a".repeat(200);
    let short = transport
        .send(&key(), &from, "+14155550999", "one segment")
        .await
        .unwrap();
    let split = transport
        .send(&key(), &from, "+14155550999", &long)
        .await
        .unwrap();

    assert_eq!(short.segments, 1);
    assert_eq!(split.segments, 2);
    assert_ne!(short.carrier_id, split.carrier_id);

    let sends = transport.sends();
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].from_e164, "+14155550123");
    assert_eq!(sends[0].to_e164, "+14155550999");
    assert_eq!(sends[0].body, "one segment");
    assert_eq!(sends[0].carrier_id, short.carrier_id);
    assert_eq!(sends[1].body, long);
}

#[tokio::test]
async fn a_send_carries_the_messaging_object_the_number_holds() {
    let transport = FakeTextTransport::default();
    let mut from = number("+14155550123");
    from.messaging_object_id = Some("MG-workspace".to_string());

    transport
        .send(&key(), &from, "+14155550999", "hello")
        .await
        .unwrap();

    assert_eq!(
        transport.sends()[0].messaging_object_id.as_deref(),
        Some("MG-workspace")
    );
}

#[tokio::test]
async fn a_send_starts_queued_and_the_test_moves_it_to_delivered() {
    let transport = FakeTextTransport::default();
    let sent = transport
        .send(&key(), &number("+14155550123"), "+14155550999", "hello")
        .await
        .unwrap();

    assert_eq!(
        transport.delivery_status(&key(), &sent.carrier_id).await,
        Ok(TextDeliveryStatus::Queued)
    );

    transport.set_delivery_status(
        &sent.carrier_id,
        TextDeliveryStatus::Failed {
            code: Some("30007".to_string()),
            reason: Some("carrier filtered".to_string()),
        },
    );

    assert_eq!(
        transport
            .delivery_status(&key(), &sent.carrier_id)
            .await
            .unwrap()
            .code(),
        Some("30007")
    );
}

#[tokio::test]
async fn a_delivery_read_of_a_text_the_carrier_never_sent_names_the_carrier_code() {
    let transport = FakeTextTransport::default();

    let error = transport
        .delivery_status(&key(), "no-such-text")
        .await
        .unwrap_err();

    assert_eq!(error.as_str(), "carrier");
    assert_eq!(error.carrier_code(), Some("not_found"));
}

#[tokio::test]
async fn the_cursor_hands_each_inbound_text_over_once() {
    let transport = FakeTextTransport::default();
    let held = number("+14155550123");
    transport.push_inbound(inbound("+14155550123", "first", "in-1"));
    transport.push_inbound(inbound("+14155550123", "second", "in-2"));
    // A text for another number is not this number's.
    transport.push_inbound(inbound("+14155550124", "elsewhere", "in-3"));

    let (first, cursor) = transport.poll_inbound(&key(), &held, None).await.unwrap();
    assert_eq!(
        first
            .iter()
            .map(|text| text.body.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );

    let (again, cursor) = transport
        .poll_inbound(&key(), &held, cursor.as_deref())
        .await
        .unwrap();
    assert!(again.is_empty());

    transport.push_inbound(inbound("+14155550123", "third", "in-4"));
    let (fresh, _) = transport
        .poll_inbound(&key(), &held, cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].body, "third");
    assert_eq!(fresh[0].carrier_id, "in-4");
}

#[tokio::test]
async fn a_cursor_the_carrier_never_gave_is_refused() {
    let transport = FakeTextTransport::default();

    let error = transport
        .poll_inbound(&key(), &number("+14155550123"), Some("not-a-cursor"))
        .await
        .unwrap_err();

    assert_eq!(error.as_str(), "unreachable");
}

#[tokio::test]
async fn prepare_records_the_number_and_answers_the_ids_to_persist() {
    let transport = FakeTextTransport::default();
    let held = number("+14155550123");
    let config = json!({ "sip_username": "pagis-1" });

    let prepared = transport.prepare(&key(), &held, &config).await.unwrap();

    assert_eq!(
        prepared.messaging_object_id.as_deref(),
        Some("messaging-object-+14155550123")
    );
    let prepares = transport.prepares();
    assert_eq!(prepares.len(), 1);
    assert_eq!(prepares[0].e164, "+14155550123");
    assert_eq!(prepares[0].connection_config, config);

    let mut connection_config = serde_json::Map::new();
    connection_config.insert("messaging_profile".to_string(), json!("mp-1"));
    transport.set_prepared(Prepared {
        messaging_object_id: None,
        connection_config: connection_config.clone(),
    });

    let prepared = transport.prepare(&key(), &held, &config).await.unwrap();
    assert_eq!(prepared.messaging_object_id, None);
    assert_eq!(prepared.connection_config, connection_config);
}

#[tokio::test]
async fn the_fake_serves_the_media_of_an_inbound_text() {
    let transport = FakeTextTransport::default();
    transport.push_media("https://carrier.example/media/1", b"PNGBYTES".to_vec());

    assert_eq!(
        transport
            .fetch_media(&key(), "https://carrier.example/media/1")
            .await
            .unwrap(),
        b"PNGBYTES".to_vec()
    );

    transport.set_capabilities(TextCapabilities {
        texting: true,
        inbound_media: false,
    });
    let error = transport
        .fetch_media(&key(), "https://carrier.example/media/1")
        .await
        .unwrap_err();
    assert_eq!(error.carrier_code(), Some("media_absent"));
}

#[tokio::test]
async fn the_fake_declares_the_capabilities_it_is_given() {
    let transport = FakeTextTransport::default();
    assert_eq!(
        transport.capabilities(),
        TextCapabilities {
            texting: true,
            inbound_media: true,
        }
    );

    transport.set_capabilities(TextCapabilities::ABSENT);

    assert_eq!(transport.capabilities(), TextCapabilities::ABSENT);
    assert_eq!(
        transport
            .send(&key(), &number("+14155550123"), "+14155550999", "hello")
            .await,
        Err(TextError::TextingAbsent)
    );
}

#[tokio::test]
async fn a_scripted_failure_stands_in_for_the_carriers_own() {
    let transport = FakeTextTransport::default();
    transport.fail_with(Some(TextError::DestinationNotEnabled {
        code: "21408".to_string(),
    }));

    let error = transport
        .send(&key(), &number("+14155550123"), "+441632960999", "hello")
        .await
        .unwrap_err();

    assert_eq!(error.as_str(), "destination_not_enabled");
    assert_eq!(error.carrier_code(), Some("21408"));

    transport.fail_with(None);
    assert!(
        transport
            .send(&key(), &number("+14155550123"), "+14155550999", "hello")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn every_request_signs_with_the_carrier_key() {
    let transport = FakeTextTransport::default();

    transport
        .send(&key(), &number("+14155550123"), "+14155550999", "hello")
        .await
        .unwrap();

    assert_eq!(
        transport.last_key().unwrap().expose_secret(),
        "carrier-secret"
    );
}

#[tokio::test]
async fn a_carrier_without_texting_answers_texting_absent_to_every_call() {
    let transport = NoTextTransport;
    let held = number("+14155550123");

    assert_eq!(transport.capabilities(), TextCapabilities::ABSENT);
    assert_eq!(
        transport.prepare(&key(), &held, &json!({})).await.err(),
        Some(TextError::TextingAbsent)
    );
    assert_eq!(
        transport
            .send(&key(), &held, "+14155550999", "hello")
            .await
            .err(),
        Some(TextError::TextingAbsent)
    );
    assert_eq!(
        transport.delivery_status(&key(), "any").await.err(),
        Some(TextError::TextingAbsent)
    );
    assert_eq!(
        transport.poll_inbound(&key(), &held, None).await.err(),
        Some(TextError::TextingAbsent)
    );
    assert_eq!(
        transport
            .fetch_media(&key(), "https://carrier.example/media/1")
            .await
            .err(),
        Some(TextError::TextingAbsent)
    );
    assert_eq!(TextError::TextingAbsent.as_str(), "texting_absent");
}

#[test]
fn the_map_finds_one_transport_per_carrier() {
    let telnyx = Arc::new(FakeTextTransport::default());
    let transports = TextTransports::new()
        .with(TELNYX_PROVIDER, Arc::clone(&telnyx) as _)
        .with(PLIVO_PROVIDER, Arc::new(NoTextTransport));

    assert!(transports.get(TELNYX_PROVIDER).is_some());
    assert!(transports.texts(TELNYX_PROVIDER));
    assert!(transports.get(PLIVO_PROVIDER).is_some());
    assert!(!transports.texts(PLIVO_PROVIDER));
    assert_eq!(
        transports.capabilities(PLIVO_PROVIDER),
        TextCapabilities::ABSENT
    );
}

#[test]
fn a_carrier_the_map_does_not_list_carries_no_text() {
    let transports =
        TextTransports::single(TELNYX_PROVIDER, Arc::new(FakeTextTransport::default()) as _);

    assert!(transports.get(TWILIO_PROVIDER).is_none());
    assert!(!transports.texts(TWILIO_PROVIDER));
    assert_eq!(
        transports.capabilities(TWILIO_PROVIDER),
        TextCapabilities::ABSENT
    );
}
