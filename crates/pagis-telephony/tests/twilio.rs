//! The Twilio number half (ADR-0020) against a local HTTP server: the
//! request shape of each call, the happy answers, a refused key and a
//! number the carrier will not sell.

use pagis_telephony::{
    CarrierKey, CatalogErrorCode, NumberCatalog, NumberSearch, TwilioNumberCatalog,
};
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT_SID: &str = "AC123";

fn key() -> CarrierKey {
    CarrierKey::new(ACCOUNT_SID, "token")
}

/// HTTP Basic with the Account SID and the Auth Token.
fn basic_auth() -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode("AC123:token");
    format!("Basic {encoded}")
}

fn search() -> NumberSearch {
    NumberSearch {
        country: "US".to_string(),
        area_code: Some("415".to_string()),
        locality: Some("San Francisco".to_string()),
        limit: 5,
    }
}

#[tokio::test]
async fn search_asks_for_voice_capable_local_numbers_of_the_area() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/2010-04-01/Accounts/AC123/AvailablePhoneNumbers/US/Local.json",
        ))
        .and(header("authorization", basic_auth()))
        .and(query_param("VoiceEnabled", "true"))
        .and(query_param("PageSize", "5"))
        .and(query_param("AreaCode", "415"))
        .and(query_param("InLocality", "San Francisco"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "available_phone_numbers": [{
                "phone_number": "+14155550100",
                "locality": "San Francisco",
                "region": "CA",
            }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let offered = catalog.search(&key(), &search()).await.unwrap();

    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].e164, "+14155550100");
    assert_eq!(offered[0].region.as_deref(), Some("San Francisco, CA"));
    // Twilio states no price per number, so the desk shows none.
    assert_eq!(offered[0].monthly_cost, None);
    assert_eq!(offered[0].currency, None);
}

#[tokio::test]
async fn buy_sends_the_intent_id_and_reads_the_carrier_handle() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/IncomingPhoneNumbers.json"))
        .and(header("authorization", basic_auth()))
        .and(body_string_contains("PhoneNumber=%2B14155550100"))
        .and(body_string_contains("FriendlyName=intent-7"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sid": "PN9",
            "phone_number": "+14155550100",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let bought = catalog
        .buy(&key(), "+14155550100", "intent-7")
        .await
        .unwrap();

    assert_eq!(bought.e164, "+14155550100");
    assert_eq!(bought.provider_number_id, "PN9");
}

#[tokio::test]
async fn a_number_the_carrier_will_not_sell_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/2010-04-01/Accounts/AC123/IncomingPhoneNumbers.json"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": 21_422,
            "message": "Phone number is not available",
        })))
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let refused = catalog
        .buy(&key(), "+14155550100", "intent-7")
        .await
        .unwrap_err();

    assert_eq!(refused.0, CatalogErrorCode::NumberUnavailable);
}

#[tokio::test]
async fn a_refused_key_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/2010-04-01/Accounts/AC123/AvailablePhoneNumbers/US/Local.json",
        ))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "code": 20_003,
            "message": "Authenticate",
        })))
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let refused = catalog.search(&key(), &search()).await.unwrap_err();

    assert_eq!(refused.0, CatalogErrorCode::Unauthorized);
}

#[tokio::test]
async fn find_purchased_matches_the_number_the_account_holds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/IncomingPhoneNumbers.json"))
        .and(header("authorization", basic_auth()))
        .and(query_param("PhoneNumber", "+14155550100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "incoming_phone_numbers": [{
                "sid": "PN9",
                "phone_number": "+14155550100",
            }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let held = catalog
        .find_purchased(&key(), "+14155550100")
        .await
        .unwrap()
        .expect("the account holds the number");

    assert_eq!(held.provider_number_id, "PN9");
}

#[tokio::test]
async fn find_purchased_answers_none_when_the_account_holds_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/2010-04-01/Accounts/AC123/IncomingPhoneNumbers.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "incoming_phone_numbers": [],
        })))
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    let held = catalog
        .find_purchased(&key(), "+14155550100")
        .await
        .unwrap();

    assert!(held.is_none());
}

#[tokio::test]
async fn release_gives_the_number_back() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/2010-04-01/Accounts/AC123/IncomingPhoneNumbers/PN9.json",
        ))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    catalog.release(&key(), "PN9").await.unwrap();
}

#[tokio::test]
async fn a_number_the_carrier_does_not_hold_is_released_already() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/2010-04-01/Accounts/AC123/IncomingPhoneNumbers/PN9.json",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "code": 20_404,
            "message": "Not found",
        })))
        .mount(&server)
        .await;

    let catalog = TwilioNumberCatalog::with_base_url(server.uri());
    catalog.release(&key(), "PN9").await.unwrap();
}
