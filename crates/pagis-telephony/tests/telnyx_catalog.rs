//! The Telnyx REST client against a stand-in for the carrier's API:
//! the credential SIP Connection is prepared for the media Pagis
//! offers and for the dialed number Pagis routes by (ADR-0020). Pagis
//! offers SDES-SRTP only, so the connection must have encrypted media
//! on, or the carrier answers cleartext and no audio flows. The line
//! finds the Agent of an inbound call by its dialed number in E.164,
//! so the connection must send the dialed number in E.164.

use std::time::Duration;

use pagis_telephony::{CarrierKey, CatalogErrorCode, NumberCatalog, TelnyxNumberCatalog};
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USERNAME: &str = "pagis9f9301ae";
const CONNECTION_ID: &str = "3039768267410376183";

fn catalog(server: &MockServer) -> TelnyxNumberCatalog {
    TelnyxNumberCatalog::with_base_url(format!("{}/v2", server.uri()), Duration::from_millis(1))
}

fn connection(
    encrypted_media: Option<&str>,
    dnis_number_format: Option<&str>,
) -> serde_json::Value {
    json!({
        "data": [{
            "id": CONNECTION_ID,
            "record_type": "credential_connection",
            "user_name": USERNAME,
            "encrypted_media": encrypted_media,
            "inbound": {
                "ani_number_format": "+E.164",
                "dnis_number_format": dnis_number_format,
                "channel_limit": 10,
            },
        }]
    })
}

/// The one change Pagis asks for: SRTP media, and the dialed number
/// in E.164.
fn prepared() -> serde_json::Value {
    json!({
        "encrypted_media": "SRTP",
        "inbound": { "dnis_number_format": "+e164" },
    })
}

async fn expect_the_change(server: &MockServer) {
    Mock::given(method("PATCH"))
        .and(path(format!("/v2/credential_connections/{CONNECTION_ID}")))
        .and(body_json(prepared()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_connection_without_srtp_is_switched_to_srtp_and_e164_dialed_numbers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/credential_connections"))
        .and(query_param("filter[user_name]", USERNAME))
        .respond_with(ResponseTemplate::new(200).set_body_json(connection(None, None)))
        .expect(1)
        .mount(&server)
        .await;
    expect_the_change(&server).await;

    let changed = catalog(&server)
        .prepare_sip_connection(&CarrierKey::new("", "key"), USERNAME)
        .await
        .unwrap();

    assert!(changed, "the connection was changed");
}

#[tokio::test]
async fn a_connection_that_sends_the_sip_username_is_switched_to_e164_dialed_numbers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/credential_connections"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(connection(Some("SRTP"), Some("sip_username"))),
        )
        .mount(&server)
        .await;
    expect_the_change(&server).await;

    let changed = catalog(&server)
        .prepare_sip_connection(&CarrierKey::new("", "key"), USERNAME)
        .await
        .unwrap();

    assert!(changed, "the connection was changed");
}

#[tokio::test]
async fn a_connection_with_srtp_and_e164_dialed_numbers_is_left_alone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/credential_connections"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(connection(Some("SRTP"), Some("+e164"))),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let changed = catalog(&server)
        .prepare_sip_connection(&CarrierKey::new("", "key"), USERNAME)
        .await
        .unwrap();

    assert!(!changed);
}

#[tokio::test]
async fn a_username_the_account_does_not_hold_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/credential_connections"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;

    let error = catalog(&server)
        .prepare_sip_connection(&CarrierKey::new("", "key"), "nobody")
        .await
        .unwrap_err();

    assert_eq!(error.0, CatalogErrorCode::NumberUnavailable);
}

#[tokio::test]
async fn a_refused_key_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/credential_connections"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "errors": [] })))
        .mount(&server)
        .await;

    let error = catalog(&server)
        .prepare_sip_connection(&CarrierKey::new("", "bad"), USERNAME)
        .await
        .unwrap_err();

    assert_eq!(error.0, CatalogErrorCode::Unauthorized);
}
