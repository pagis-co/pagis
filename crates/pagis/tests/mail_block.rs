//! The `mail` block and the read behind its inspector
//! (ADR-0019): an inbound mail that wakes the Agent leaves one line in
//! the Thread, and the inspector reads the headers and the body of that
//! message live from the host.
//!
//! The mail host and transport are fakes, so no socket opens.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::{AgentId, ChannelStore, WorkspaceId};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    transport: Arc<FakeMailTransport>,
}

async fn boot() -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let transport = Arc::new(FakeMailTransport::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        mail_host: Arc::new(FakeMailboxHost::default()),
        mail_transport: Arc::clone(&transport) as _,
        collector_interval: Duration::from_millis(100),
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        brain,
        transport,
    }
}

impl Harness {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let mut request = reqwest::Client::new()
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

    /// One Agent with a proven mailbox on a Migadu Connection, and the
    /// address it holds.
    async fn agent_with_a_mailbox(&self, name: &str, local_part: &str) -> (String, String) {
        let connection_id = self
            .daemon
            .connect_installation(
                "migadu",
                serde_json::json!({
                    "account": "owner@example.com",
                    "api_key": "migadu-key",
                    "domain": "example.com",
                }),
            )
            .await;
        let (status, agent) = self
            .post(
                "/api/v1/agents",
                serde_json::json!({
                    "name": name,
                    "job": "assistant",
                    "mailbox": {
                        "connection_id": connection_id,
                        "local_part": local_part,
                    },
                }),
            )
            .await;
        assert_eq!(status, 201, "{agent}");
        let agent_id = agent["id"].as_str().unwrap().to_string();
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (_, page) = self
                    .get(&format!("/api/v1/agents/{agent_id}/mailbox"))
                    .await;
                if page["mailbox"]["state"] == "active" {
                    return page["mailbox"]["address"].as_str().unwrap().to_string();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the login proof settles");
        (agent_id, address)
    }

    /// The Agent's Thread with the user.
    async fn user_dm(&self, agent_id: &str) -> String {
        pagis_storage_sqlite::SqliteChannelStore::new(self.daemon.pool().clone())
            .find_user_dm(
                &self.workspace_id().await,
                &AgentId::from(agent_id.to_string()),
            )
            .await
            .unwrap()
            .expect("the Agent has a thread with the user")
            .id
            .to_string()
    }

    async fn workspace_id(&self) -> WorkspaceId {
        use pagis_core::AgentStore;
        pagis_storage_sqlite::SqliteAgentStore::new(self.daemon.pool().clone())
            .get(
                &self.daemon.workspace_id,
                &AgentId::from(self.daemon.agent_id.clone()),
            )
            .await
            .unwrap()
            .unwrap()
            .workspace_id
    }

    /// The first `mail` block in one Channel, or give up waiting.
    async fn wait_for_mail_block(&self, channel_id: &str) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let (_, page) = self
                .get(&format!("/api/v1/channels/{channel_id}/messages"))
                .await;
            let found = page["items"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .flat_map(|item| item["blocks"].as_array().cloned().unwrap_or_default())
                .find(|block| block["type"] == "mail");
            if let Some(block) = found {
                return block;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "a mail block reached the thread; saw {}",
                serde_json::to_string(&page["items"]).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test]
async fn a_mail_that_wakes_the_agent_leaves_a_block_in_the_thread() {
    let harness = boot().await;
    harness
        .brain
        .push_for("Ada Lovelace", Script::reply(&["I read the mail."]));
    let (agent_id, address) = harness.agent_with_a_mailbox("Ada Lovelace", "ada").await;

    let id = harness
        .transport
        .deliver("Clinic <care@clinic.test>", "Your appointment", "Tuesday");

    let channel_id = harness.user_dm(&agent_id).await;
    let block = harness.wait_for_mail_block(&channel_id).await;

    assert_eq!(block["direction"], "inbound");
    assert_eq!(
        block["mailbox"], address,
        "the line reads as the Agent's own address"
    );
    assert_eq!(block["message_id"], id.to_string());
    assert_eq!(block["counterpart"], "Clinic <care@clinic.test>");
    assert_eq!(block["subject"], "Your appointment");
    assert_eq!(block["trust_tier"], "unknown");
    // The words stay at the host: the block carries the envelope only.
    assert!(block.get("text").is_none());
    assert!(block.get("snippet").is_none());
}

#[tokio::test]
async fn the_inspector_reads_the_headers_and_the_body_from_the_host() {
    let harness = boot().await;
    let (_, address) = harness.agent_with_a_mailbox("Ada Lovelace", "ada").await;
    let id = harness
        .transport
        .deliver("Clinic <care@clinic.test>", "Your appointment", "Tuesday");

    let (status, message) = harness.get(&format!("/api/v1/mail/{address}/{id}")).await;

    assert_eq!(status, 200, "{message}");
    assert_eq!(message["mailbox"], address);
    assert_eq!(message["message_id"], id.to_string());
    assert_eq!(message["from"], "Clinic <care@clinic.test>");
    assert_eq!(message["subject"], "Your appointment");
    assert_eq!(message["text"], "Tuesday");
    assert!(
        message["headers"]
            .as_array()
            .expect("the headers")
            .iter()
            .any(|header| header["name"] == "Subject"),
        "{message}"
    );
}

#[tokio::test]
async fn a_message_the_host_no_longer_holds_says_so() {
    let harness = boot().await;
    let (_, address) = harness.agent_with_a_mailbox("Ada Lovelace", "ada").await;

    let (status, error) = harness
        .get(&format!("/api/v1/mail/{address}/INBOX:404"))
        .await;

    assert_eq!(status, 404, "{error}");
    assert_eq!(
        error["error"]["message"],
        "this message is no longer at the host"
    );
}

#[tokio::test]
async fn a_mailbox_no_agent_holds_is_not_found() {
    let harness = boot().await;
    harness.agent_with_a_mailbox("Ada Lovelace", "ada").await;

    let (status, error) = harness
        .get("/api/v1/mail/somebody@example.com/INBOX:1")
        .await;

    assert_eq!(status, 404, "{error}");
    assert_eq!(error["error"]["message"], "mailbox not found");
}
