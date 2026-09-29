//! Full-daemon Agent Mailbox tests (ADR-0019): the
//! mailbox section of the Agent creation form, the mailbox card, the
//! password reset, the delete behind a typed address, and the archive
//! that puts a mailbox to sleep. A fake mail host and transport stand
//! in for Migadu, so no network is reached.

use std::sync::Arc;
use std::time::Duration;

use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_testkit::{TestDaemon, TestDaemonOptions};

struct Harness {
    daemon: TestDaemon,
    host: Arc<FakeMailboxHost>,
    transport: Arc<FakeMailTransport>,
}

async fn boot() -> Harness {
    let host = Arc::new(FakeMailboxHost::default());
    let transport = Arc::new(FakeMailTransport::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        mail_host: Arc::clone(&host) as _,
        mail_transport: Arc::clone(&transport) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        host,
        transport,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

impl Harness {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let mut request = client()
            .request(method, format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (
            status,
            response.json().await.unwrap_or(serde_json::json!({})),
        )
    }

    async fn get(&self, path: &str) -> (reqwest::StatusCode, serde_json::Value) {
        self.request(reqwest::Method::GET, path, None).await
    }

    async fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn delete(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        self.request(reqwest::Method::DELETE, path, Some(body))
            .await
    }

    /// A Migadu Mailbox Provider, as an Administrator sets one up.
    async fn connect_provider(&self) -> String {
        self.daemon
            .connect_installation(
                "migadu",
                serde_json::json!({
                    "account": "owner@example.com",
                    "api_key": "migadu-key",
                    "domain": "example.com",
                }),
            )
            .await
    }

    async fn create_agent(&self, name: &str, mailbox: Option<serde_json::Value>) -> String {
        let mut body = serde_json::json!({ "name": name, "job": "assistant" });
        if let Some(mailbox) = mailbox {
            body["mailbox"] = mailbox;
        }
        let (status, agent) = self.post("/api/v1/agents", body).await;
        assert_eq!(status, 201, "{agent}");
        agent["id"].as_str().unwrap().to_string()
    }

    /// The mailbox card once the background login proof has settled.
    async fn settled_mailbox(&self, agent_id: &str) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (_, page) = self
                    .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
                    .await;
                if page["mailbox"]["state"] != "provisioning" {
                    return page["mailbox"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the login proof settles")
    }
}

#[tokio::test]
async fn the_creation_form_gives_the_agent_its_own_mailbox() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;

    let (status, agent) = harness
        .post(
            "/api/v1/agents",
            serde_json::json!({
                "name": "Ada Lovelace",
                "job": "assistant",
                "mailbox": {
                    "connection_id": connection_id,
                    "local_part": "ada.lovelace",
                    "outgoing_cap": 5,
                },
            }),
        )
        .await;

    assert_eq!(status, 201, "{agent}");
    // The Agent's address is a read-through view of the record.
    assert_eq!(agent["email_address"], "ada.lovelace@example.com");
    assert_eq!(harness.host.addresses(), vec!["ada.lovelace@example.com"]);

    let agent_id = agent["id"].as_str().unwrap();
    let mailbox = harness.settled_mailbox(agent_id).await;
    assert_eq!(mailbox["state"], "active");
    assert_eq!(mailbox["outgoing_cap"], 5);
    assert_eq!(mailbox["sends_today"], 0);
    assert_eq!(mailbox["connection_id"], connection_id);
    // The login proof signed in as the Agent. The mail collector starts
    // a watcher as soon as the mailbox is active and signs in again, so
    // the count is the collector's business; the address is the claim.
    let logins = harness.transport.logins();
    assert!(!logins.is_empty(), "the login proof signed in");
    assert!(
        logins
            .iter()
            .all(|login| login == "ada.lovelace@example.com"),
        "only the Agent's own mailbox is signed into, saw {logins:?}"
    );
}

/// A host failure lands in the form and makes no Agent (ADR-0019).
#[tokio::test]
async fn a_name_the_ledger_holds_makes_no_agent() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;

    let (status, body) = harness
        .post(
            "/api/v1/agents",
            serde_json::json!({
                "name": "Ada Two",
                "job": "assistant",
                "mailbox": { "connection_id": connection_id, "local_part": "ada" },
            }),
        )
        .await;

    assert_eq!(status, 409, "{body}");
    let (_, roster) = harness.get("/api/v1/agents").await;
    let named: Vec<&str> = roster["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert!(!named.contains(&"Ada Two"), "{roster}");
}

#[tokio::test]
async fn the_form_refuses_a_name_the_mail_host_keeps() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;

    let (status, body) = harness
        .post(
            "/api/v1/agents",
            serde_json::json!({
                "name": "Post Master",
                "job": "assistant",
                "mailbox": { "connection_id": connection_id, "local_part": "postmaster" },
            }),
        )
        .await;

    assert_eq!(status, 422, "{body}");
    // The host was never asked.
    assert_eq!(harness.host.addresses(), Vec::<String>::new());
}

#[tokio::test]
async fn an_agent_with_no_mailbox_is_offered_a_name_on_each_provider() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness.create_agent("Ada Lovelace", None).await;

    let (status, page) = harness
        .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
        .await;

    assert_eq!(status, 200, "{page}");
    assert_eq!(page["mailbox"], serde_json::Value::Null);
    let offers = page["offers"].as_array().unwrap();
    assert_eq!(offers.len(), 1, "{page}");
    assert_eq!(offers[0]["connection_id"], connection_id);
    assert_eq!(offers[0]["domain"], "example.com");
    assert_eq!(offers[0]["suggested_local_part"], "ada.lovelace");
    assert_eq!(offers[0]["default_outgoing_cap"], 20);
    assert_eq!(offers[0]["mints_password"], true);
    assert_eq!(offers[0]["deletes_mailbox"], true);
}

/// The same flow provisions a mailbox for an Agent that already exists
/// (ADR-0019).
#[tokio::test]
async fn an_existing_agent_gets_a_mailbox_later() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness.create_agent("Ada Lovelace", None).await;

    let (status, mailbox) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": connection_id, "local_part": "ada" }),
        )
        .await;

    assert_eq!(status, 201, "{mailbox}");
    assert_eq!(mailbox["address"], "ada@example.com");
    // The answer arrives once the host has made the mailbox; the login
    // is proven afterwards.
    assert_eq!(mailbox["state"], "provisioning");
    assert_eq!(harness.settled_mailbox(&agent_id).await["state"], "active");

    let (_, roster) = harness.get("/api/v1/agents").await;
    let ada = roster["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == agent_id.as_str())
        .unwrap();
    assert_eq!(ada["email_address"], "ada@example.com");
}

#[tokio::test]
async fn one_agent_holds_one_mailbox() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;

    let (status, body) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": connection_id, "local_part": "ada2" }),
        )
        .await;

    assert_eq!(status, 409, "{body}");
}

#[tokio::test]
async fn a_password_reset_proves_the_login_again() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;
    harness.settled_mailbox(&agent_id).await;
    // The collector opens sessions of its own once the mailbox is
    // active, so the proof shows as one more login, not as a count.
    let logins_before = harness.transport.logins().len();

    let (status, mailbox) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox/reset-password"),
            serde_json::json!({}),
        )
        .await;

    assert_eq!(status, 200, "{mailbox}");
    assert_eq!(mailbox["state"], "provisioning");
    assert_eq!(harness.settled_mailbox(&agent_id).await["state"], "active");
    // The mailbox logged in again with the new password.
    assert!(harness.transport.logins().len() > logins_before);
}

/// The user types the address, because the host's mail goes with the
/// mailbox and Pagis keeps no copy (ADR-0019).
#[tokio::test]
async fn a_delete_needs_the_address_typed_back() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;
    harness.settled_mailbox(&agent_id).await;

    let (status, body) = harness
        .delete(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "confirm_address": "grace@example.com" }),
        )
        .await;
    assert_eq!(status, 422, "{body}");

    let (status, body) = harness
        .delete(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "confirm_address": "ada@example.com" }),
        )
        .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mailbox"]["state"], "deleted");
    // The host mailbox is gone with it.
    assert_eq!(harness.host.addresses(), Vec::<String>::new());
    let (_, page) = harness
        .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
        .await;
    assert_eq!(page["mailbox"], serde_json::Value::Null);
}

/// The Address Ledger keeps the tombstone, so the address never comes
/// back (ADR-0019).
#[tokio::test]
async fn a_deleted_address_is_never_reused() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;
    harness
        .delete(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "confirm_address": "ada@example.com" }),
        )
        .await;

    let (status, body) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": connection_id, "local_part": "ada" }),
        )
        .await;
    assert_eq!(status, 409, "{body}");

    // The form suggests the next free name instead.
    let (_, page) = harness
        .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
        .await;
    assert_eq!(page["offers"][0]["suggested_local_part"], "ada2");
    let (status, fresh) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": connection_id, "local_part": "ada2" }),
        )
        .await;
    assert_eq!(status, 201, "{fresh}");
    assert_eq!(fresh["address"], "ada2@example.com");
}

/// Archiving an Agent puts its mailbox to sleep; unlike a phone number
/// the mailbox stays with the Agent (ADR-0019).
#[tokio::test]
async fn archiving_the_agent_makes_its_mailbox_dormant() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;
    harness.settled_mailbox(&agent_id).await;

    let (status, agent) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/archive"),
            serde_json::json!({}),
        )
        .await;

    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["status"], "archived");
    assert_eq!(agent["email_address"], "ada@example.com");
    let (_, page) = harness
        .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
        .await;
    assert_eq!(page["mailbox"]["state"], "dormant");
    assert_eq!(page["mailbox"]["address"], "ada@example.com");
    // The host still holds the mail.
    assert_eq!(harness.host.addresses(), vec!["ada@example.com"]);
}

#[tokio::test]
async fn a_workspace_with_no_provider_offers_nothing() {
    let harness = boot().await;
    let agent_id = harness.create_agent("Ada", None).await;

    let (status, page) = harness
        .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
        .await;

    assert_eq!(status, 200, "{page}");
    assert_eq!(page["offers"].as_array().unwrap().len(), 0);

    let (status, body) = harness
        .post(
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": "missing", "local_part": "ada" }),
        )
        .await;
    assert_eq!(status, 409, "{body}");
}

/// The creation form has no Agent yet, so the offers come from the
/// name in the form (ADR-0019).
#[tokio::test]
async fn the_creation_form_reads_the_offers_by_the_typed_name() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;

    let (status, page) = harness
        .get("/api/v1/settings/mailbox-offers?agent_name=Ada%20Lovelace")
        .await;

    assert_eq!(status, 200, "{page}");
    let offers = page["offers"].as_array().unwrap();
    assert_eq!(offers.len(), 1, "{page}");
    assert_eq!(offers[0]["connection_id"], connection_id);
    assert_eq!(offers[0]["domain"], "example.com");
    assert_eq!(offers[0]["suggested_local_part"], "ada.lovelace");
    assert_eq!(offers[0]["default_outgoing_cap"], 20);
}

#[tokio::test]
async fn the_form_reads_a_typed_name_against_the_address_ledger() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;

    let free = format!(
        "/api/v1/settings/mailbox-offers/name?connection_id={connection_id}&local_part=grace"
    );
    let (status, name) = harness.get(&free).await;
    assert_eq!(status, 200, "{name}");
    assert_eq!(name["address"], "grace@example.com");
    assert_eq!(name["available"], true);
    assert_eq!(name["reason"], serde_json::Value::Null);

    let taken = format!(
        "/api/v1/settings/mailbox-offers/name?connection_id={connection_id}&local_part=ada"
    );
    let (status, name) = harness.get(&taken).await;
    assert_eq!(status, 200, "{name}");
    assert_eq!(name["available"], false);
    assert!(
        name["reason"]
            .as_str()
            .unwrap()
            .contains("ada@example.com is in the address ledger"),
        "{name}"
    );

    let reserved = format!(
        "/api/v1/settings/mailbox-offers/name?connection_id={connection_id}&local_part=postmaster"
    );
    let (status, name) = harness.get(&reserved).await;
    assert_eq!(status, 200, "{name}");
    assert_eq!(name["available"], false);
    assert!(!name["reason"].as_str().unwrap().is_empty(), "{name}");
}

/// The Connections page reads the mailboxes of the Workspace, so a
/// provider row says which Agents hold a mailbox on it (ADR-0019).
#[tokio::test]
async fn the_connections_page_reads_the_mailboxes_of_the_workspace() {
    let harness = boot().await;
    let connection_id = harness.connect_provider().await;
    let agent_id = harness
        .create_agent(
            "Ada",
            Some(serde_json::json!({
                "connection_id": connection_id,
                "local_part": "ada",
            })),
        )
        .await;

    let (status, page) = harness.get("/api/v1/settings/mailboxes").await;

    assert_eq!(status, 200, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{page}");
    assert_eq!(items[0]["address"], "ada@example.com");
    assert_eq!(items[0]["agent_id"], agent_id);
    assert_eq!(items[0]["connection_id"], connection_id);
}
