//! Full-daemon mail tool tests (ADR-0019): an Agent that holds a
//! mailbox gets the `mail__*` tools, a send waits for a card that names
//! the address, `Always allow` writes the recipient domain onto the
//! mailbox record, and the next send to that domain leaves with no
//! card. A fake mail host and transport stand in for Migadu, so no
//! network is reached.

use std::sync::Arc;
use std::time::Duration;

use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};

struct Harness {
    daemon: TestDaemon,
    transport: Arc<FakeMailTransport>,
    brain: Arc<ScriptedBrain>,
}

async fn boot() -> Harness {
    let host = Arc::new(FakeMailboxHost::default());
    let transport = Arc::new(FakeMailTransport::default());
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        mail_host: host as _,
        mail_transport: Arc::clone(&transport) as _,
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        transport,
        brain,
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

    /// Give the daemon's own Agent a mailbox, proven and active.
    async fn hold_mailbox(&self) -> serde_json::Value {
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
        let (status, body) = self
            .post(
                &format!("/api/v1/agents/{}/mailbox", self.daemon.agent_id),
                serde_json::json!({
                    "connection_id": connection_id,
                    "local_part": "ada",
                    "outgoing_cap": 5,
                }),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        self.settled_mailbox().await
    }

    async fn settled_mailbox(&self) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (_, page) = self
                    .get(&format!("/api/v1/agents/{}/mailbox", self.daemon.agent_id))
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

    /// Ask the Agent to work, and answer with the card it parks on, or
    /// with `None` when the Run finished without one.
    async fn ask(&self, pending_id: &str, text: &str) -> Option<serde_json::Value> {
        let completed = self.completed_runs().await;
        let (status, body) = self
            .post(
                &format!("/api/v1/channels/{}/messages", self.daemon.dm_channel_id),
                serde_json::json!({"pending_id": pending_id, "text": text}),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (_, page) = self.get("/api/v1/requests?state=pending").await;
                if let Some(request) = page["items"].as_array().and_then(|items| items.first()) {
                    return Some(request.clone());
                }
                if self.completed_runs().await > completed {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the run parks or the message leaves")
    }

    async fn decide(&self, request_id: &str, scope: &str) {
        let (status, body) = self
            .post(
                &format!("/api/v1/requests/{request_id}/decision"),
                serde_json::json!({"decision": "approved", "scope": scope}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }

    /// How many Runs that a message started have completed.
    async fn completed_runs(&self) -> usize {
        let (_, page) = self.get("/api/v1/runs").await;
        page["items"].as_array().map_or(0, |runs| {
            runs.iter()
                .filter(|run| run["trigger_kind"] == "message" && run["state"] == "completed")
                .count()
        })
    }

    /// Wait until `count` Runs that a message started have completed.
    async fn wait_for_completed_runs(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.completed_runs().await < count {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{count} Runs complete"));
    }

    fn script_send(&self, to: &str, subject: &str) {
        self.brain.push(Script::tool_call(
            &[],
            "mail__send",
            serde_json::json!({
                "mailbox": "own",
                "to": [to],
                "subject": subject,
                "body": "the invoice is paid"
            }),
        ));
        self.brain.push(Script::reply(&["It is sent."]));
    }
}

#[tokio::test]
async fn a_send_waits_for_a_card_and_always_allow_writes_the_domain_on_the_mailbox() {
    let harness = boot().await;
    let mailbox = harness.hold_mailbox().await;
    assert_eq!(mailbox["state"], "active");
    assert_eq!(mailbox["allow_rules"], serde_json::json!([]));
    harness.script_send("bob@other.test", "Invoice 42");

    let request = harness
        .ask("p-1", "answer Bob about invoice 42")
        .await
        .expect("the send waits for the user");

    assert_eq!(request["kind"], "tool_action");
    assert_eq!(request["payload"]["tool_name"], "mail__send");
    assert_eq!(
        request["payload"]["action_title"],
        "Send an email from ada@example.com"
    );
    assert_eq!(request["payload"]["body"], "Invoice 42");
    assert_eq!(
        request["payload"]["proposed_rules"],
        serde_json::json!(["other.test"])
    );
    assert!(harness.transport.sent().is_empty());

    harness
        .decide(request["id"].as_str().unwrap(), "always")
        .await;
    // The Run counts the send after the host took the message, and
    // completes after that. The next message then starts a Run of its
    // own: a message for a thread whose Run still runs folds into it.
    harness.wait_for_completed_runs(1).await;

    let sent = harness.transport.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Invoice 42");
    assert_eq!(sent[0].to, vec!["bob@other.test".to_string()]);

    let mailbox = harness.settled_mailbox().await;
    assert_eq!(mailbox["allow_rules"], serde_json::json!(["other.test"]));
    assert_eq!(mailbox["sends_today"], 1);

    // The rule covers the next send to that domain, so no card appears.
    harness.script_send("carol@other.test", "Invoice 43");
    let card = harness.ask("p-2", "tell Carol as well").await;

    assert!(card.is_none(), "the rule sends it with no card: {card:?}");
    let sent = harness.transport.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].subject, "Invoice 43");
}

#[tokio::test]
async fn an_agent_with_no_mailbox_is_offered_no_mail_tool() {
    let harness = boot().await;
    harness.brain.push(Script::reply(&["I hold no mailbox."]));

    harness
        .post(
            &format!("/api/v1/channels/{}/messages", harness.daemon.dm_channel_id),
            serde_json::json!({"pending_id": "p-1", "text": "read my mail"}),
        )
        .await;

    let names = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(turn) = harness.brain.requests().first() {
                return turn
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the model is asked once");

    assert!(
        !names.iter().any(|name| name.starts_with("mail__")),
        "{names:?}"
    );
}
