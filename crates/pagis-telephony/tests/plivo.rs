//! The Plivo number half (ADR-0020) against a local HTTP server: the
//! request shape, the happy answers, a refused key and a number the
//! carrier will not sell.

use pagis_telephony::{
    CarrierKey, CatalogErrorCode, NumberCatalog, NumberSearch, PlivoNumberCatalog,
};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUTH_ID: &str = "MA123";

fn key() -> CarrierKey {
    CarrierKey::new(AUTH_ID, "token")
}

/// HTTP Basic with the Auth ID and the Auth Token.
fn basic_auth() -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode("MA123:token");
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
async fn search_asks_for_voice_local_numbers_and_reads_the_rental_rate() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/Account/MA123/PhoneNumber/"))
        .and(header("authorization", basic_auth()))
        .and(query_param("country_iso", "US"))
        .and(query_param("type", "local"))
        .and(query_param("services", "voice"))
        .and(query_param("limit", "5"))
        .and(query_param("pattern", "415"))
        .and(query_param("city", "San Francisco"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "objects": [{
                "number": "14155550100",
                "city": "SAN FRANCISCO",
                "region": "California",
                "monthly_rental_rate": "0.50",
            }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let found = catalog.search(&key(), &search()).await.unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].e164, "+14155550100");
    assert_eq!(
        found[0].region.as_deref(),
        Some("SAN FRANCISCO, California")
    );
    assert_eq!(found[0].monthly_cost.as_deref(), Some("0.50"));
    assert_eq!(found[0].currency.as_deref(), Some("USD"));
}

#[tokio::test]
async fn a_refused_key_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/Account/MA123/PhoneNumber/"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "authentication failed",
        })))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let refused = catalog.search(&key(), &search()).await.unwrap_err();

    assert_eq!(refused.0, CatalogErrorCode::Unauthorized);
    // No upstream text reaches the caller.
    assert!(!refused.to_string().contains("authentication failed"));
}

#[tokio::test]
async fn buy_posts_the_number_and_the_number_is_its_own_handle() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Account/MA123/PhoneNumber/14155550100/"))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "numbers": [{ "number": "14155550100", "status": "Success" }],
            "status": "fulfilled",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let bought = catalog
        .buy(&key(), "+14155550100", "intent-1")
        .await
        .unwrap();

    assert_eq!(bought.e164, "+14155550100");
    assert_eq!(bought.provider_number_id, "14155550100");
}

#[tokio::test]
async fn a_number_pending_activation_is_still_bought() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Account/MA123/PhoneNumber/14155550100/"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({
            "numbers": [{ "number": "14155550100", "status": "pending" }],
        })))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let bought = catalog
        .buy(&key(), "+14155550100", "intent-1")
        .await
        .unwrap();

    assert_eq!(bought.provider_number_id, "14155550100");
}

#[tokio::test]
async fn a_number_the_carrier_will_not_sell_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Account/MA123/PhoneNumber/14155550100/"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "number not available",
        })))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let refused = catalog
        .buy(&key(), "+14155550100", "intent-1")
        .await
        .unwrap_err();

    assert_eq!(refused.0, CatalogErrorCode::NumberUnavailable);
}

#[tokio::test]
async fn a_number_the_carrier_does_not_know_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/Account/MA123/PhoneNumber/14155550100/"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let refused = catalog
        .buy(&key(), "+14155550100", "intent-1")
        .await
        .unwrap_err();

    assert_eq!(refused.0, CatalogErrorCode::NumberUnavailable);
}

#[tokio::test]
async fn find_purchased_matches_the_held_number_exactly() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/Account/MA123/Number/"))
        .and(header("authorization", basic_auth()))
        .and(query_param("number_startswith", "14155550100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "objects": [
                { "number": "141555501000" },
                { "number": "14155550100" },
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    let held = catalog
        .find_purchased(&key(), "+14155550100")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(held.e164, "+14155550100");
    assert_eq!(held.provider_number_id, "14155550100");
}

#[tokio::test]
async fn find_purchased_answers_none_for_a_number_the_account_does_not_hold() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/Account/MA123/Number/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "objects": [] })))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
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
        .and(path("/v1/Account/MA123/Number/14155550100/"))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    catalog.release(&key(), "14155550100").await.unwrap();
}

#[tokio::test]
async fn a_number_already_released_is_released() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v1/Account/MA123/Number/14155550100/"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "not found" })))
        .mount(&server)
        .await;

    let catalog = PlivoNumberCatalog::with_base_url(server.uri());
    catalog.release(&key(), "14155550100").await.unwrap();
}
