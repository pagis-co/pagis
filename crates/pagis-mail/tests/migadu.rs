//! The Migadu host (ADR-0019) against a local HTTP server: the create
//! call, the failures the user meets, the delete and the list.

use pagis_mail::{Deletion, HostAccount, HostErrorCode, MailboxHost, MailboxPassword, MigaduHost};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn account() -> HostAccount {
    HostAccount::new("example.com", "owner@example.com", "migadu-key")
}

/// HTTP Basic with the account email address and the API key.
fn basic_auth() -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode("owner@example.com:migadu-key");
    format!("Basic {encoded}")
}

#[tokio::test]
async fn create_makes_the_mailbox_with_the_password_the_daemon_chose() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/domains/example.com/mailboxes"))
        .and(header("authorization", basic_auth()))
        .and(body_json(json!({
            "local_part": "ava",
            "password": "generated-secret",
            "password_method": "password",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "local_part": "ava",
            "domain_name": "example.com",
            "address": "ava@example.com",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let made = host
        .create(
            &account(),
            "ava",
            &MailboxPassword::new("generated-secret"),
            20,
        )
        .await
        .unwrap();

    assert_eq!(made.address, "ava@example.com");
    assert_eq!(made.local_part, "ava");
    assert_eq!(made.domain, "example.com");
}

#[tokio::test]
async fn a_taken_address_is_a_conflict() {
    let server = MockServer::start().await;
    // Migadu answers every failure with 400, so the body names the
    // reason.
    Mock::given(method("POST"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "errors": { "local_part": ["has already been taken"] },
        })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let refused = host
        .create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap_err();

    assert_eq!(refused.0, HostErrorCode::AddressTaken);
}

#[tokio::test]
async fn a_refused_key_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized" })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let refused = host
        .create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap_err();

    assert_eq!(refused.0, HostErrorCode::Unauthorized);
}

#[tokio::test]
async fn an_account_at_its_plan_limit_reports_the_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "mailbox limit for this plan is reached",
        })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let refused = host
        .create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap_err();

    assert_eq!(refused.0, HostErrorCode::CapReached);
}

#[tokio::test]
async fn delete_removes_the_mailbox_at_the_host() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/domains/example.com/mailboxes/ava"))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let deleted = host.delete(&account(), "ava@example.com").await.unwrap();

    assert_eq!(deleted, Deletion::Removed);
    assert!(host.capabilities().delete_mailbox);
}

#[tokio::test]
async fn a_mailbox_the_host_lost_is_deleted_already() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/domains/example.com/mailboxes/ava"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "not found" })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());

    assert_eq!(
        host.delete(&account(), "ava@example.com").await.unwrap(),
        Deletion::Removed
    );
}

#[tokio::test]
async fn an_address_on_another_domain_is_not_this_accounts() {
    let server = MockServer::start().await;
    let host = MigaduHost::with_base_url(server.uri());

    let refused = host.delete(&account(), "ava@other.com").await.unwrap_err();

    assert_eq!(refused.0, HostErrorCode::MailboxUnknown);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn reset_password_updates_the_mailbox() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/domains/example.com/mailboxes/ava"))
        .and(body_json(json!({ "password": "next-secret" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    host.reset_password(
        &account(),
        "ava@example.com",
        &MailboxPassword::new("next-secret"),
    )
    .await
    .unwrap();

    assert!(host.capabilities().reset_password);
}

#[tokio::test]
async fn list_reads_the_domains_mailboxes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domains/example.com/mailboxes"))
        .and(header("authorization", basic_auth()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mailboxes": [
                { "local_part": "ava", "domain_name": "example.com" },
                { "local_part": "ben", "domain_name": "example.com" },
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    let found = host.list(&account()).await.unwrap();

    let addresses: Vec<&str> = found
        .iter()
        .map(|mailbox| mailbox.address.as_str())
        .collect();
    assert_eq!(addresses, ["ava@example.com", "ben@example.com"]);
}

#[tokio::test]
async fn an_answer_this_client_cannot_read_is_unreadable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "surprise": true })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());

    assert_eq!(
        host.list(&account()).await.unwrap_err().0,
        HostErrorCode::Unreadable
    );
}

#[tokio::test]
async fn a_host_failure_is_temporary() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(503).set_body_string("gateway down"))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());

    assert_eq!(
        host.list(&account()).await.unwrap_err().0,
        HostErrorCode::TemporarilyUnavailable
    );
}

/// The key never travels in a URL, a body or a `Debug` line.
#[tokio::test]
async fn the_api_key_stays_out_of_the_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/domains/example.com/mailboxes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "local_part": "ava",
            "domain_name": "example.com",
        })))
        .mount(&server)
        .await;

    let host = MigaduHost::with_base_url(server.uri());
    host.create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(!sent.to_string().contains("migadu-key"));
    assert!(!requests[0].url.as_str().contains("migadu-key"));
    assert!(!format!("{:?}", account()).contains("migadu-key"));
}
