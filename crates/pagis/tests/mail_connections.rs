//! Full-daemon Mailbox Provider tests (ADR-0019): the
//! administration path that records a mail domain, says what the provider
//! can do, and refuses to remove a Connection an Agent Mailbox still
//! points at. A fake mail host stands in for Migadu, so no network is
//! reached.

use std::sync::Arc;

use pagis_mail::fake::FakeMailboxHost;
use pagis_testkit::{TestDaemon, TestDaemonOptions};

struct Harness {
    daemon: TestDaemon,
    host: Arc<FakeMailboxHost>,
}

async fn boot() -> Harness {
    let host = Arc::new(FakeMailboxHost::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        mail_host: Arc::clone(&host) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness { daemon, host }
}

/// One Agent with its own mailbox on this Connection.
async fn create_agent_with_mailbox(daemon: &TestDaemon, name: &str, connection_id: &str) {
    let response = client()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "name": name,
            "job": "tester",
            "mailbox": { "connection_id": connection_id, "local_part": name },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Set up one mail domain the way an Administrator does, and answer
/// the status and the Connection as the product lists it, or the error
/// body.
async fn create_connection(
    daemon: &TestDaemon,
    provider: &str,
    fields: serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, setup) = daemon.set_up_provider(provider, "connection", fields).await;
    if status != 200 {
        return (status, setup);
    }
    let connection_id = setup["parts"][0]["connection_id"].clone();
    let page: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/connections", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let connection = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connection| connection["id"] == connection_id)
        .expect("the product lists the mail domain")
        .clone();
    (status, connection)
}

/// Remove one mail domain the way an Administrator does.
async fn delete_connection(daemon: &TestDaemon, provider: &str) -> (u16, serde_json::Value) {
    let response = client()
        .delete(format!(
            "{}/api/v1/administration/providers/{provider}/connection",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (
        status,
        response.json().await.unwrap_or(serde_json::json!({})),
    )
}

fn migadu_fields() -> serde_json::Value {
    serde_json::json!({
        "account": "owner@example.com",
        "api_key": "migadu-key",
        "domain": "example.com",
    })
}

fn manual_fields() -> serde_json::Value {
    serde_json::json!({
        "domain": "example.org",
        "imap_host": "imap.fastmail.com",
        "imap_port": "993",
        "smtp_host": "smtp.fastmail.com",
        "smtp_port": "465",
    })
}

#[tokio::test]
async fn connecting_migadu_records_the_domain_and_what_the_provider_does() {
    let harness = boot().await;

    let (status, body) = create_connection(&harness.daemon, "migadu", migadu_fields()).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "connected");
    assert_eq!(body["account"], "owner@example.com");
    assert_eq!(body["mail"]["domain"], "example.com");
    assert_eq!(body["mail"]["imap_host"], "imap.migadu.com");
    assert_eq!(body["mail"]["imap_port"], 993);
    assert_eq!(body["mail"]["smtp_host"], "smtp.migadu.com");
    assert_eq!(body["mail"]["smtp_port"], 465);
    assert_eq!(body["mail"]["idle"], true);
    assert_eq!(body["mail"]["delete_mailbox"], true);
    assert_eq!(body["mail"]["reset_password"], true);
    // The key proved itself against the host before the row existed.
    assert_eq!(harness.host.calls(), vec![pagis_mail::fake::HostCall::List]);
}

#[tokio::test]
async fn a_manual_provider_says_which_capabilities_are_absent() {
    let harness = boot().await;

    let (status, body) = create_connection(&harness.daemon, "manual", manual_fields()).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "connected");
    assert_eq!(body["mail"]["domain"], "example.org");
    assert_eq!(body["mail"]["imap_host"], "imap.fastmail.com");
    // The user deletes the mailbox and pastes a new password there.
    assert_eq!(body["mail"]["delete_mailbox"], false);
    assert_eq!(body["mail"]["reset_password"], false);
    assert_eq!(body["mail"]["outgoing_cap"], false);
    assert_eq!(body["mail"]["idle"], true);
}

#[tokio::test]
async fn a_request_without_a_domain_is_refused() {
    let harness = boot().await;

    let (status, _) = create_connection(
        &harness.daemon,
        "migadu",
        serde_json::json!({
            "account": "owner@example.com",
            "api_key": "migadu-key",
        }),
    )
    .await;

    assert_eq!(status, 422);
}

#[tokio::test]
async fn a_connection_a_mailbox_points_at_is_not_removed() {
    let harness = boot().await;
    let (status, connection) = create_connection(&harness.daemon, "migadu", migadu_fields()).await;
    assert_eq!(status, 200, "{connection}");
    assert_eq!(connection["installation"], true);
    let connection_id = connection["id"].as_str().unwrap();

    create_agent_with_mailbox(&harness.daemon, "ada", connection_id).await;
    create_agent_with_mailbox(&harness.daemon, "grace", connection_id).await;

    let (status, body) = delete_connection(&harness.daemon, "migadu").await;

    assert_eq!(status, 409);
    // The count tells the user how many mailboxes to delete first.
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains('2'),
        "{body}"
    );
}
