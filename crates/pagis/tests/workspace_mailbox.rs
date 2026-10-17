//! A Person creates and reads their Workspace Mailbox through the Product API.
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct MailboxApp {
    daemon: TestDaemon,
    host: Arc<FakeMailboxHost>,
    transport: Arc<FakeMailTransport>,
    connection_id: String,
    client: reqwest::Client,
    brain: Arc<ScriptedBrain>,
}

impl MailboxApp {
    async fn start() -> Self {
        Self::start_with_source(Arc::new(pagis_mail::HostSpamVerdicts)).await
    }

    async fn start_with_source(source: Arc<dyn pagis_mail::SpamVerdictSource>) -> Self {
        let host = Arc::new(FakeMailboxHost::default());
        let transport = Arc::new(FakeMailTransport::default());
        let brain = Arc::new(ScriptedBrain::default());
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            brain: brain.clone(),
            spam_verdict_source: source,
            mail_host: host.clone(),
            mail_transport: transport.clone(),
            ..TestDaemonOptions::default()
        })
        .await;
        let connection_id = daemon
            .connect_installation(
                "migadu",
                json!({
                    "account": "owner@example.com", "api_key": "key", "domain": "example.com",
                }),
            )
            .await;
        Self {
            daemon,
            host,
            transport,
            connection_id,
            client: reqwest::Client::new(),
            brain,
        }
    }

    async fn with_mailbox() -> Self {
        let app = Self::start().await;
        assert_eq!(app.create("person").await.0, 201);
        app
    }

    async fn create(&self, local_part: &str) -> (reqwest::StatusCode, Value) {
        let response = self
            .client
            .post(format!("{}/api/v1/workspace/mailbox", self.daemon.base_url))
            .header("cookie", self.daemon.cookie())
            .json(&json!({"connection_id": self.connection_id, "local_part": local_part}))
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    }

    async fn get(&self, path: &str) -> Value {
        let response = self
            .client
            .get(format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "GET {path}");
        response.json().await.unwrap()
    }

    fn deliver_thread(&self) {
        self.transport
            .deliver("sender@example.com", "Invoice", "invoice first");
        let thread = self.transport.summaries("INBOX")[0].thread_id.clone();
        self.transport.deliver_reply(
            &thread,
            "sender@example.com",
            "Re: Invoice",
            "invoice second",
        );
    }
}

#[tokio::test]
async fn workspace_mailbox_is_created_in_settings() {
    let app = MailboxApp::start().await;
    assert!(app.get("/api/v1/workspace/mailbox").await["mailbox"].is_null());
    let (status, mailbox) = app.create("person").await;
    assert_eq!(status, 201);
    assert_eq!(mailbox["address"], "person@example.com");
    assert!(mailbox.get("password").is_none());
    assert_eq!(app.host.addresses(), ["person@example.com"]);
    assert_eq!(
        app.get("/api/v1/workspace/mailbox").await["mailbox"]["address"],
        "person@example.com"
    );
}

#[tokio::test]
async fn workspace_mailbox_refuses_a_second_create() {
    let app = MailboxApp::with_mailbox().await;
    assert_eq!(app.create("other").await.0, 409);
    assert_eq!(app.host.addresses(), ["person@example.com"]);
}

#[tokio::test]
async fn workspace_mailbox_all_mail_lists_newest_first_and_unread() {
    let app = MailboxApp::with_mailbox().await;
    app.deliver_thread();
    let list = app.get("/api/v1/workspace/mail/messages").await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert_eq!(list["items"][0]["subject"], "Re: Invoice");
    assert_eq!(list["items"][1]["subject"], "Invoice");
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["seen"] == false)
    );
}

#[tokio::test]
async fn workspace_mailbox_open_reads_the_thread_and_marks_it_seen() {
    let app = MailboxApp::with_mailbox().await;
    app.deliver_thread();
    let list = app.get("/api/v1/workspace/mail/messages").await;
    let id = list["items"][0]["id"].as_str().unwrap();
    let opened = app
        .get(&format!("/api/v1/workspace/mail/messages/{id}"))
        .await;
    assert_eq!(opened["text"], "invoice second");
    assert_eq!(opened["seen"], true);
    let mut url = reqwest::Url::parse(&app.daemon.base_url).unwrap();
    url.path_segments_mut().unwrap().extend([
        "api",
        "v1",
        "workspace",
        "mail",
        "threads",
        opened["thread_id"].as_str().unwrap(),
    ]);
    let response = app
        .client
        .get(url)
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let thread: Value = response.json().await.unwrap();
    assert_eq!(thread["items"].as_array().unwrap().len(), 2);
    assert!(
        thread["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["seen"] == true)
    );
    let read = app.get("/api/v1/workspace/mail/messages").await;
    assert!(
        read["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["seen"] == true)
    );
}

#[tokio::test]
async fn workspace_mailbox_search_finds_mail() {
    let app = MailboxApp::with_mailbox().await;
    app.deliver_thread();
    let found = app.get("/api/v1/workspace/mail/messages?q=invoice").await;
    assert_eq!(found["items"].as_array().unwrap().len(), 2);
    let absent = app.get("/api/v1/workspace/mail/messages?q=absent").await;
    assert!(absent["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn workspace_mailbox_folders_list_inbox() {
    let app = MailboxApp::with_mailbox().await;
    let folders = app.get("/api/v1/workspace/mail/folders").await;
    assert!(
        folders["items"]
            .as_array()
            .unwrap()
            .contains(&json!("INBOX"))
    );
}

#[tokio::test]
async fn workspace_mailbox_person_sends_and_organizes_mail() {
    let app = MailboxApp::with_mailbox().await;
    let response = app
        .client
        .post(format!(
            "{}/api/v1/workspace/mail/messages",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"to":["recipient@example.com"],"subject":"Hello","body":"Hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(app.transport.sent()[0].to, ["recipient@example.com"]);
    let sent = app.get("/api/v1/workspace/mail/messages?folder=Sent").await;
    assert_eq!(sent["items"][0]["subject"], "Hello");
    for (method, path, body, status) in [
        (
            reqwest::Method::POST,
            "/folders",
            json!({"name":"Invoices"}),
            204,
        ),
        (
            reqwest::Method::PATCH,
            "/folders/Invoices",
            json!({"name":"Paid"}),
            204,
        ),
        (reqwest::Method::DELETE, "/folders/Paid", json!({}), 204),
        (reqwest::Method::DELETE, "/folders/INBOX", json!({}), 422),
    ] {
        let response = app
            .client
            .request(
                method,
                format!("{}/api/v1/workspace/mail{path}", app.daemon.base_url),
            )
            .header("cookie", app.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}

impl MailboxApp {
    async fn mail_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
        status: u16,
    ) -> Value {
        let response = self
            .client
            .request(
                method,
                format!("{}/api/v1/workspace/mail{path}", self.daemon.base_url),
            )
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
        if status == 204 {
            Value::Null
        } else {
            response.json().await.unwrap()
        }
    }
}

#[tokio::test]
async fn workspace_mailbox_attachments_replies_forward_move_and_labels() {
    let app = MailboxApp::with_mailbox().await;
    let response = app
        .client
        .post(format!("{}/api/v1/files", app.daemon.base_url))
        .header("cookie", app.daemon.cookie())
        .multipart(
            reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(b"invoice".to_vec())
                    .file_name("invoice.txt")
                    .mime_str("text/plain")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let file: Value = response.json().await.unwrap();
    app.mail_request(reqwest::Method::POST, "/messages", json!({"to":["recipient@example.com"],"subject":"Invoice","body":"Please read","files":[file["id"]]}), 201).await;
    assert_eq!(app.transport.sent()[0].attachments[0].data, b"invoice");
    let received = app.transport.received("recipient@example.com");
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].text.trim(), "Please read");
    assert_eq!(received[0].attachments[0].name, "invoice.txt");
    assert_eq!(
        app.transport.sent()[0].attachments[0].content_type,
        "text/plain"
    );
    let sent = app.get("/api/v1/workspace/mail/messages?folder=Sent").await;
    let sent_id = sent["items"][0]["id"].as_str().unwrap();
    let opened = app
        .get(&format!("/api/v1/workspace/mail/messages/{sent_id}"))
        .await;
    assert_eq!(opened["attachments"][0]["name"], "invoice.txt");
    app.transport.deliver_raw(b"Message-ID: <original@example.com>\r\nReferences: <root@example.com>\r\nFrom: Sender <sender@example.com>\r\nTo: Person <PERSON@example.com>, Other <other@example.com>\r\nCc: copy@example.com\r\nSubject: Question\r\n\r\nHello\r\n");
    let id = app.transport.summaries("INBOX")[0].id.to_string();
    for action in ["reply", "reply-all"] {
        let prefill = app
            .get(&format!(
                "/api/v1/workspace/mail/messages/{id}/composer?action={action}"
            ))
            .await;
        assert_eq!(prefill["to"][0], "sender@example.com");
        if action == "reply-all" {
            assert_eq!(
                prefill["to"],
                json!(["sender@example.com", "other@example.com"])
            );
            assert_eq!(prefill["cc"], json!(["copy@example.com"]));
        }
        app.mail_request(reqwest::Method::POST, &format!("/messages/{id}/{action}"), json!({"to":prefill["to"],"cc":prefill["cc"],"subject":prefill["subject"],"body":"Answer"}), 201).await;
        let sent = app.transport.sent();
        let reply = sent.last().unwrap();
        assert_eq!(reply.in_reply_to.as_deref(), Some("<original@example.com>"));
        assert_eq!(
            reply.references,
            ["<root@example.com>", "<original@example.com>"]
        );
    }
    app.mail_request(reqwest::Method::POST, &format!("/messages/{sent_id}/forward"), json!({"to":["forward@example.com"],"subject":"Fwd: Invoice","body":"Please read\n\n> Please read"}), 201).await;
    let sent = app.transport.sent();
    assert_eq!(sent.last().unwrap().attachments[0].data, b"invoice");
    assert!(sent.last().unwrap().in_reply_to.is_none());
    app.mail_request(
        reqwest::Method::POST,
        "/folders",
        json!({"name":"Invoices"}),
        204,
    )
    .await;
    app.mail_request(
        reqwest::Method::PATCH,
        "/folders/Invoices",
        json!({"name":"Paid"}),
        204,
    )
    .await;
    app.mail_request(
        reqwest::Method::POST,
        &format!("/messages/{id}/labels"),
        json!({"name":"finance"}),
        204,
    )
    .await;
    assert_eq!(
        app.get(&format!("/api/v1/workspace/mail/messages/{id}"))
            .await["labels"],
        json!(["finance"])
    );
    app.mail_request(
        reqwest::Method::DELETE,
        &format!("/messages/{id}/labels/finance"),
        json!({}),
        204,
    )
    .await;
    assert_eq!(
        app.get(&format!("/api/v1/workspace/mail/messages/{id}"))
            .await["labels"],
        json!([])
    );
    app.mail_request(
        reqwest::Method::POST,
        &format!("/messages/{id}/move"),
        json!({"name":"Paid"}),
        204,
    )
    .await;
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages?folder=Paid").await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for name in ["INBOX", "Sent", "Drafts", "Junk", "Trash"] {
        app.mail_request(
            reqwest::Method::DELETE,
            &format!("/folders/{name}"),
            json!({}),
            422,
        )
        .await;
        app.mail_request(
            reqwest::Method::PATCH,
            &format!("/folders/{name}"),
            json!({"name":"Other"}),
            422,
        )
        .await;
    }
    app.mail_request(reqwest::Method::DELETE, "/folders/Paid", json!({}), 204)
        .await;
    let response = app
        .client
        .get(format!(
            "{}/api/v1/workspace/mail/messages/{sent_id}/attachments/0",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(response.bytes().await.unwrap(), b"invoice".as_slice());
}

#[tokio::test]
async fn workspace_mailbox_stores_verdicts_without_moving_mail() {
    let app = MailboxApp::with_mailbox().await;
    assert_eq!(
        app.get("/api/v1/workspace/mailbox").await["mailbox"]["spam_mode"],
        "score_only"
    );
    app.transport.deliver_raw(b"From: sender@example.com\r\nSubject: Spam sample\r\nX-Spam-Status: Yes, score=7.2 required=5.0 tests=GTUBE autolearn=no\r\n\r\nbody");
    let page = app.get("/api/v1/workspace/mail/messages").await;
    let item = &page["items"][0];
    assert_eq!(item["spam_verdict"]["score"], 7.2);
    assert_eq!(item["spam_verdict"]["action"], "add header");
    assert_eq!(item["spam_verdict"]["reasons"], json!(["GTUBE"]));
    assert_eq!(item["trust_tier"], "unknown");
    assert_eq!(app.transport.summaries("INBOX").len(), 1);
    let id = item["id"].as_str().unwrap();
    let message = app
        .get(&format!(
            "/api/v1/workspace/mail/messages/{}",
            id.replace(":", "%3A")
        ))
        .await;
    assert_eq!(message["spam_verdict"], item["spam_verdict"]);
}

#[tokio::test]
async fn workspace_mailbox_default_grant_moves_with_the_chief_and_keeps_manual_grants() {
    let app = MailboxApp::with_mailbox().await;
    let grants = app.get("/api/v1/grants").await;
    let default = grants["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|grant| grant["resource_kind"] == "workspace_mailbox")
        .expect("the seeded Chief holds a default Workspace Mailbox Grant");
    assert_eq!(default["agent_id"], app.daemon.agent_id.as_str());
    assert_eq!(default["is_default"], true);
    let response = app
        .client
        .post(format!("{}/api/v1/agents", app.daemon.base_url))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"name":"Sage", "job":"Mail"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let agent: Value = response.json().await.unwrap();
    let response = app
        .client
        .post(format!(
            "{}/api/v1/grants/workspace-mailbox",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"agent_id":agent["id"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let manual: Value = response.json().await.unwrap();
    assert_eq!(manual["is_default"], false);
    let response = app
        .client
        .put(format!(
            "{}/api/v1/workspace/chief-of-staff",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"agent_id":agent["id"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let grants = app.get("/api/v1/grants").await;
    let mailbox_grants: Vec<_> = grants["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|grant| grant["resource_kind"] == "workspace_mailbox")
        .collect();
    assert_eq!(mailbox_grants.len(), 2);
    assert!(
        mailbox_grants
            .iter()
            .all(|grant| grant["agent_id"] == agent["id"])
    );
    assert!(
        mailbox_grants
            .iter()
            .any(|grant| grant["id"] == manual["id"])
    );
    let response = app
        .client
        .put(format!(
            "{}/api/v1/grants/{}/rules",
            app.daemon.base_url,
            manual["id"].as_str().unwrap()
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"allow":[" Mail.Example.COM "]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let changed: Value = response.json().await.unwrap();
    assert_eq!(changed["allow"], json!(["example.com"]));
    let response = app
        .client
        .post(format!(
            "{}/api/v1/agents/{}/archive",
            app.daemon.base_url,
            agent["id"].as_str().unwrap()
        ))
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let grants = app.get("/api/v1/grants").await;
    let default = grants["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["is_default"] == true)
        .unwrap();
    assert_eq!(default["agent_id"], app.daemon.agent_id.as_str());
}

impl MailboxApp {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let base =
            if path.starts_with("/api/v1/administration/") || path.starts_with("/api/v1/admin/") {
                &self.daemon.administration_base_url
            } else {
                &self.daemon.base_url
            };
        let mut request = reqwest::Client::new()
            .request(method, format!("{base}{path}"))
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

    async fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn ask(&self, pending_id: &str, text: &str) -> Option<serde_json::Value> {
        self.ask_in(&self.daemon.dm_channel_id, pending_id, text)
            .await
    }

    async fn ask_in(
        &self,
        channel: &str,
        pending_id: &str,
        text: &str,
    ) -> Option<serde_json::Value> {
        let completed = self.completed_runs().await;
        let (status, body) = self
            .post(
                &format!("/api/v1/channels/{channel}/messages"),
                serde_json::json!({"pending_id": pending_id, "text": text}),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let page = self.get("/api/v1/requests?state=pending").await;
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
        let page = self.get("/api/v1/runs").await;
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
}

#[tokio::test]
async fn workspace_mailbox_send_waits_for_approval_and_a_standing_grant_sends_at_once() {
    let app = MailboxApp::with_mailbox().await;
    let arguments = json!({"to":["recipient@other.test"],"cc":["copy@other.test"],"subject":"Invoice","body":"Paid"});
    app.brain.push(Script::tool_call(
        &[],
        "workspace_mail_send",
        arguments.clone(),
    ));
    app.brain.push(Script::reply(&["Sent."]));
    let card = app
        .ask("send-1", "Send the invoice")
        .await
        .expect("an Approval");
    assert_eq!(card["kind"], "tool_action");
    assert_eq!(card["payload"]["arguments"]["from"], "person@example.com");
    assert_eq!(card["payload"]["arguments"]["cc"], arguments["cc"]);
    assert_eq!(card["payload"]["arguments"]["body"], "Paid");
    assert!(app.transport.sent().is_empty());
    app.decide(card["id"].as_str().unwrap(), "once").await;
    app.wait_for_completed_runs(1).await;
    assert_eq!(app.transport.sent().len(), 1);
    let grants = app.get("/api/v1/grants").await;
    let id = grants["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["resource_kind"] == "workspace_mailbox")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (status, _) = app
        .request(
            reqwest::Method::PUT,
            &format!("/api/v1/grants/{id}/rules"),
            Some(json!({"allow":["other.test"]})),
        )
        .await;
    assert_eq!(status, 200);
    app.brain
        .push(Script::tool_call(&[], "workspace_mail_send", arguments));
    app.brain.push(Script::reply(&["Sent."]));
    assert!(app.ask("send-2", "Send another invoice").await.is_none());
    assert_eq!(app.transport.sent().len(), 2);
}

#[tokio::test]
async fn workspace_mailbox_agent_reply_keeps_the_thread_and_copies_the_person() {
    let app = MailboxApp::with_mailbox().await;
    let (status, _) = app
        .post(
            &format!("/api/v1/agents/{}/mailbox", app.daemon.agent_id),
            json!({"connection_id":app.connection_id,"local_part":"pixie"}),
        )
        .await;
    assert_eq!(status, 201);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mailbox = app
                .get(&format!("/api/v1/agents/{}/mailbox", app.daemon.agent_id))
                .await;
            if mailbox["mailbox"]["state"] == "active" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    app.transport.deliver_raw(b"Message-ID: <question@other.test>\r\nReferences: <root@other.test>\r\nFrom: sender@other.test\r\nTo: person@example.com\r\nSubject: Question\r\n\r\nHello");
    let id = app.transport.summaries("INBOX")[0].id.to_string();
    app.brain.push(Script::tool_call(&[], "mail__send", json!({"mailbox":"own", "to":["sender@other.test"], "subject":"Ignored", "body":"Answer", "reply_to_workspace_message_id":id})));
    app.brain.push(Script::reply(&["Sent."]));
    let card = app
        .ask("reply-1", "Answer from your mailbox")
        .await
        .expect("the Agent Mailbox send rules apply");
    assert_eq!(
        card["payload"]["arguments"]["cc"],
        json!(["person@example.com"])
    );
    assert_eq!(card["payload"]["arguments"]["subject"], "Re: Question");
    app.decide(card["id"].as_str().unwrap(), "once").await;
    app.wait_for_completed_runs(1).await;
    let sent = app.transport.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].cc, ["person@example.com"]);
    assert_eq!(
        sent[0].in_reply_to.as_deref(),
        Some("<question@other.test>")
    );
    assert_eq!(
        sent[0].references,
        ["<root@other.test>", "<question@other.test>"]
    );
    assert_eq!(sent[0].subject, "Re: Question");
}

#[tokio::test]
async fn workspace_mailbox_read_needs_a_grant_and_revocation_stops_dispatch() {
    let app = MailboxApp::with_mailbox().await;
    app.deliver_thread();
    let (status, agent) = app
        .post("/api/v1/agents", json!({"name":"Sage", "job":"Mail"}))
        .await;
    assert_eq!(status, 201);
    let stores = app.daemon.stores();
    let workspace = stores
        .workspaces
        .for_user(&app.daemon.user_id)
        .await
        .unwrap()
        .unwrap();
    let agent_id = pagis_core::AgentId::from(agent["id"].as_str().unwrap().to_string());
    let channel = stores
        .channels
        .find_user_dm(&workspace.id, &agent_id)
        .await
        .unwrap()
        .unwrap();
    app.brain
        .push(Script::reply(&["I have no Workspace Mailbox Grant."]));
    assert!(
        app.ask_in(channel.id.as_str(), "read-1", "Read the Workspace Mailbox")
            .await
            .is_none()
    );
    assert!(
        !app.brain
            .requests()
            .iter()
            .flat_map(|r| &r.tools)
            .any(|t| t.name.starts_with("workspace_mail_"))
    );

    assert!(
        stores
            .grants
            .list_live_for_agent(&workspace.id, &agent_id)
            .await
            .unwrap()
            .is_empty()
    );
    let (status, grant) = app
        .post(
            "/api/v1/grants/workspace-mailbox",
            json!({"agent_id":agent_id}),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(grant["resource_id"], workspace.id.as_str());
    app.brain.push(Script::tool_call(
        &[],
        "workspace_mail_search",
        json!({"query":"invoice"}),
    ));
    app.brain.push(Script::reply(&["I read the invoices."]));
    assert!(
        app.ask_in(channel.id.as_str(), "read-2", "Read the invoices")
            .await
            .is_none()
    );
    assert!(
        app.brain
            .requests()
            .iter()
            .flat_map(|r| &r.tools)
            .any(|t| t.name == "workspace_mail_search")
    );
    let summary = stores
        .mail_assessments
        .get(
            &workspace.id,
            "person@example.com",
            &app.transport.summaries("INBOX")[0].message_id,
        )
        .await
        .unwrap();
    assert!(summary.is_some());

    let (status, _) = app
        .request(
            reqwest::Method::DELETE,
            &format!("/api/v1/grants/{}", grant["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert!(
        stores
            .grants
            .list_live_for_agent(&workspace.id, &agent_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn imip_invitations_replies_and_cancellations_through_the_product_api() {
    let app = MailboxApp::with_mailbox().await;
    let fixture = include_str!("../../pagis-core/tests/fixtures/imip/google-request.ics")
        .replace("reader@example.com", "person@example.com");
    let incoming = pagis_mail::build_mime(
        "organizer@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["person@example.com".into()],
            body: "Please join.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "invite.ics".into(),
                content_type: "text/calendar; method=REQUEST".into(),
                data: fixture.into_bytes(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let id = app.transport.deliver_raw(&incoming.raw);
    let events = app
        .get("/api/v1/calendar/events?from=1792454400000&to=1792627200000")
        .await;
    assert_eq!(events[0]["event"]["uid"], "meeting@example.org");
    assert_eq!(
        events[0]["event"]["data"]["invitation"]["status"],
        "tentative"
    );
    let mail = app
        .get(&format!("/api/v1/workspace/mail/messages/{id}"))
        .await;
    assert_eq!(mail["invitation"]["uid"], "meeting@example.org");
    for status in ["accepted", "tentative", "declined"] {
        let response = app
            .client
            .post(format!(
                "{}/api/v1/calendar/events/{}/response",
                app.daemon.base_url,
                events[0]["event"]["id"].as_str().unwrap()
            ))
            .header("cookie", app.daemon.cookie())
            .json(&json!({"status": status}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let event: Value = response.json().await.unwrap();
        assert_eq!(event["data"]["attendees"][0]["status"], status);
        let sent = app.transport.sent();
        let reply = sent.last().unwrap();
        assert_eq!(reply.to, ["organizer@example.org"]);
        let text = std::str::from_utf8(&reply.attachments[0].data).unwrap();
        assert_eq!(
            pagis_core::imip::SchedulingMessage::parse(text)
                .unwrap()
                .method,
            pagis_core::imip::SchedulingMethod::Reply
        );
    }
}

#[tokio::test]
async fn shared_workspace_mail_visibility_limits_reads_and_audits_searches() {
    let app = MailboxApp::start().await;
    let (status, shared) = app
        .post(
            "/api/v1/administration/workspaces",
            json!({"name":"Support", "manager_id":app.daemon.user_id}),
        )
        .await;
    assert_eq!(status, 201, "{shared}");
    let prefix = format!("/api/v1/workspaces/{}", shared["id"].as_str().unwrap());
    let (status, _) = app
        .post(
            &format!("{prefix}/workspace/mailbox"),
            json!({"connection_id":app.connection_id,"local_part":"support"}),
        )
        .await;
    assert_eq!(status, 201);
    app.deliver_thread();
    let page = app.get(&format!("{prefix}/workspace/mail/messages")).await;
    let id = page["items"][0]["id"].as_str().unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    app.get(&format!("{prefix}/workspace/mail/messages/{id}"))
        .await;
    for level in ["escalations", "agent_only", "all_mail"] {
        let (status, body) = app
            .request(
                reqwest::Method::PUT,
                &format!("{prefix}/workspace/mailbox/visibility"),
                Some(json!({"visibility":level})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["effective_visibility"], level);
        let global = app.get("/api/v1/search?q=invoice").await;
        assert_eq!(
            global["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["kind"] == "mail")
                .count(),
            if level == "all_mail" { 2 } else { 0 }
        );
        let page = app.get(&format!("{prefix}/workspace/mail/messages")).await;
        assert_eq!(
            page["items"].as_array().unwrap().len(),
            if level == "all_mail" { 2 } else { 0 }
        );
        let (status, _) = app
            .request(
                reqwest::Method::GET,
                &format!("{prefix}/workspace/mail/messages/{id}"),
                None,
            )
            .await;
        assert_eq!(status, if level == "all_mail" { 200 } else { 404 });
        let (status, _) = app
            .post(
                &format!("{prefix}/workspace/mail/messages/{id}/spam"),
                json!({"spam": false}),
            )
            .await;
        assert_eq!(status, if level == "all_mail" { 204 } else { 404 });

        let (status, _) = app
            .request(
                reqwest::Method::GET,
                &format!("{prefix}/workspace/mail/messages?q=invoice"),
                None,
            )
            .await;
        assert_eq!(status, if level == "escalations" { 422 } else { 200 });
    }
    app.request(
        reqwest::Method::PUT,
        &format!("{prefix}/workspace/mailbox/visibility"),
        Some(json!({"visibility":"escalations"})),
    )
    .await;
    let page = app
        .get(&format!(
            "{prefix}/workspace/mail/messages?q=invoice&reason=Customer%20request"
        ))
        .await;
    assert!(page["items"].as_array().unwrap().is_empty());
    let audit = app.get(&format!("{prefix}/workspace/mail/audit")).await;
    assert!(audit["items"].as_array().unwrap().iter().any(|entry| {
        entry["action"] == "mail.search"
            && entry["reason"]
                .as_str()
                .unwrap()
                .contains("Customer request")
    }));
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "mail.message.read"
                && entry["reason"].as_str().unwrap().contains(id))
    );
}

#[tokio::test]
async fn shared_workspace_mail_needs_you_opens_only_one_message_and_limits_apply_live() {
    let app = MailboxApp::start().await;
    let (_, shared) = app
        .post(
            "/api/v1/administration/workspaces",
            json!({"name":"Support","manager_id":app.daemon.user_id}),
        )
        .await;
    let workspace = shared["id"].as_str().unwrap();
    let prefix = format!("/api/v1/workspaces/{workspace}");
    let (status, _) = app
        .post(
            &format!("{prefix}/workspace/mailbox"),
            json!({"connection_id":app.connection_id,"local_part":"support"}),
        )
        .await;
    assert_eq!(status, 201);
    app.deliver_thread();
    let page = app.get(&format!("{prefix}/workspace/mail/messages")).await;
    let id = page["items"][0]["id"].as_str().unwrap();
    let hidden = page["items"][1]["id"].as_str().unwrap();
    let thread = page["items"][0]["thread_id"].as_str().unwrap();
    let (_, agent) = app
        .post(
            &format!("{prefix}/agents"),
            json!({"name":"Aria","job":"Handle mail"}),
        )
        .await;
    let agent_id = pagis_core::AgentId::from(agent["id"].as_str().unwrap().to_string());
    assert_eq!(
        app.post(
            &format!("{prefix}/grants/workspace-mailbox"),
            json!({"agent_id":agent_id})
        )
        .await
        .0,
        201
    );
    let channel = app
        .daemon
        .stores()
        .channels
        .find_user_dm(
            &pagis_core::WorkspaceId::from(workspace.to_string()),
            &agent_id,
        )
        .await
        .unwrap()
        .unwrap();
    app.request(
        reqwest::Method::PUT,
        &format!("{prefix}/workspace/mailbox/visibility"),
        Some(json!({"visibility":"agent_only"})),
    )
    .await;
    app.brain.push(Script::tool_call(
        &[],
        "workspace_mail_triage",
        json!({"message_id":id,"outcome":"needs_you","note":"Please decide."}),
    ));
    app.brain.push(Script::reply(&["This message needs you."]));
    assert_eq!(
        app.post(
            &format!("{prefix}/channels/{}/messages", channel.id),
            json!({"pending_id":"escalate-mail","text":"Escalate this message"})
        )
        .await
        .0,
        201
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let runs = app.get(&format!("{prefix}/runs")).await;
            if runs["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["state"] == "completed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        app.get(&format!("{prefix}/workspace/mail/messages")).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.request(
            reqwest::Method::PUT,
            &format!("{prefix}/workspace/mailbox/visibility"),
            Some(json!({"visibility":"escalations"}))
        )
        .await
        .0,
        200
    );
    let page = app.get(&format!("{prefix}/workspace/mail/messages")).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    app.get(&format!("{prefix}/workspace/mail/messages/{id}"))
        .await;
    let mut url = reqwest::Url::parse(&app.daemon.base_url).unwrap();
    url.path_segments_mut().unwrap().extend([
        "api",
        "v1",
        "workspaces",
        workspace,
        "workspace",
        "mail",
        "threads",
        thread,
    ]);
    let response: Value = app
        .client
        .get(url)
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["items"].as_array().unwrap().len(), 1);
    for suffix in [
        format!("messages/{hidden}"),
        format!("messages/{hidden}/attachments/0"),
        format!("messages/{hidden}/composer?action=reply"),
    ] {
        assert_eq!(
            app.request(
                reqwest::Method::GET,
                &format!("{prefix}/workspace/mail/{suffix}"),
                None
            )
            .await
            .0,
            404
        );
    }
    let limits = format!("/api/v1/administration/workspaces/{workspace}/limits");
    assert_eq!(app.request(reqwest::Method::PUT, &limits, Some(json!({"permitted_plugins":null,"permitted_connections":null,"max_mail_visibility":"agent_only"}))).await.0, 200);
    let mailbox = app.get(&format!("{prefix}/workspace/mailbox")).await;
    assert_eq!(mailbox["mailbox"]["visibility"], "escalations");
    assert_eq!(mailbox["mailbox"]["effective_visibility"], "agent_only");
    assert_eq!(
        app.request(
            reqwest::Method::GET,
            &format!("{prefix}/workspace/mail/messages/{id}"),
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        app.request(
            reqwest::Method::PUT,
            &format!("{prefix}/workspace/mailbox/visibility"),
            Some(json!({"visibility":"all_mail"}))
        )
        .await
        .0,
        409
    );
    let audit = app.get_admin_audit().await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "mail.message.read" && e["subject_workspace_id"] == workspace)
    );
}

impl MailboxApp {
    async fn get_admin_audit(&self) -> Value {
        self.request(reqwest::Method::GET, "/api/v1/admin/audit", None)
            .await
            .1
    }
}

#[tokio::test]
async fn mail_invitation_response_checks_message_visibility() {
    let app = MailboxApp::start().await;
    let (status, shared) = app
        .post(
            "/api/v1/administration/workspaces",
            json!({"name":"Support", "manager_id":app.daemon.user_id}),
        )
        .await;
    assert_eq!(status, 201);
    let prefix = format!("/api/v1/workspaces/{}", shared["id"].as_str().unwrap());
    assert_eq!(
        app.post(
            &format!("{prefix}/workspace/mailbox"),
            json!({"connection_id":app.connection_id,"local_part":"support"})
        )
        .await
        .0,
        201
    );
    let fixture = include_str!("../../pagis-core/tests/fixtures/imip/google-request.ics")
        .replace("reader@example.com", "support@example.com");
    let incoming = pagis_mail::build_mime(
        "organizer@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["support@example.com".into()],
            body: "Please join.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "invite.ics".into(),
                content_type: "text/calendar; method=REQUEST".into(),
                data: fixture.into_bytes(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let id = app.transport.deliver_raw(&incoming.raw);
    let path = format!("{prefix}/workspace/mail/messages/{id}/response");
    for visibility in ["agent_only", "escalations", "all_mail"] {
        assert_eq!(
            app.request(
                reqwest::Method::PUT,
                &format!("{prefix}/workspace/mailbox/visibility"),
                Some(json!({"visibility":visibility}))
            )
            .await
            .0,
            200
        );
        let (status, event) = app.post(&path, json!({"status":"accepted"})).await;
        if visibility == "all_mail" {
            assert_eq!(status, 200, "{event}");
            assert_eq!(event["data"]["attendees"][0]["status"], "accepted");
            assert_eq!(app.transport.sent().len(), 1);
        } else {
            assert_eq!(status, 404, "{event}");
            assert!(app.transport.sent().is_empty());
        }
    }
    let audit = app.get(&format!("{prefix}/workspace/mail/audit")).await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "mail.invitation.respond")
    );
}

#[tokio::test]
async fn workspace_mailbox_deleted_folder_keeps_mail_in_trash_until_message_delete() {
    let app = MailboxApp::with_mailbox().await;
    let id = app
        .transport
        .deliver("sender@example.com", "Records", "Keep this mail");
    app.get(&format!("/api/v1/workspace/mail/messages/{id}"))
        .await;
    app.mail_request(
        reqwest::Method::POST,
        "/folders",
        json!({"name":"Records"}),
        204,
    )
    .await;
    app.mail_request(
        reqwest::Method::POST,
        &format!("/messages/{id}/labels"),
        json!({"name":"finance"}),
        204,
    )
    .await;
    app.mail_request(
        reqwest::Method::POST,
        &format!("/messages/{id}/move"),
        json!({"name":"Records"}),
        204,
    )
    .await;
    app.mail_request(reqwest::Method::DELETE, "/folders/Records", json!({}), 204)
        .await;
    let page = app
        .get("/api/v1/workspace/mail/messages?folder=Trash")
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["labels"], json!(["finance"]));
    assert_eq!(page["items"][0]["seen"], true);
    let folders = app.get("/api/v1/workspace/mail/folders").await;
    assert!(
        !folders["items"]
            .as_array()
            .unwrap()
            .contains(&json!("Records"))
    );
    let id = page["items"][0]["id"].as_str().unwrap();
    app.mail_request(
        reqwest::Method::DELETE,
        &format!("/messages/{id}"),
        json!({}),
        204,
    )
    .await;
    assert!(
        app.get("/api/v1/workspace/mail/messages?folder=Trash")
            .await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let id = app
        .transport
        .deliver("sender@example.com", "Another", "Move to Trash");
    app.mail_request(
        reqwest::Method::DELETE,
        &format!("/messages/{id}"),
        json!({}),
        204,
    )
    .await;
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages?folder=Trash")
            .await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for folder in ["INBOX", "Trash"] {
        app.mail_request(
            reqwest::Method::DELETE,
            &format!("/folders/{folder}"),
            json!({}),
            422,
        )
        .await;
    }
}

#[tokio::test]
async fn junk_corrections_survive_host_headers_and_folder_moves() {
    let app = MailboxApp::with_mailbox().await;
    app.transport
        .deliver("sender@example.com", "Offer", "Offer text");
    let id = app.get("/api/v1/workspace/mail/messages").await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = app
        .client
        .post(format!(
            "{}/api/v1/workspace/mail/messages/{id}/spam",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"spam":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let junk = app.get("/api/v1/workspace/mail/messages?folder=Junk").await;
    assert_eq!(junk["items"].as_array().unwrap().len(), 1);
    assert_eq!(junk["items"][0]["spam_verdict"]["action"], "junk");
    let id = junk["items"][0]["id"].as_str().unwrap();
    let response = app
        .client
        .post(format!(
            "{}/api/v1/workspace/mail/messages/{id}/spam",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"spam":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let inbox = app
        .get("/api/v1/workspace/mail/messages?folder=INBOX")
        .await;
    assert_eq!(inbox["items"][0]["spam_verdict"]["action"], "no action");
}

impl MailboxApp {
    async fn put_junk_policy(&self, org: bool, policy: Value) {
        let base = if org {
            &self.daemon.administration_base_url
        } else {
            &self.daemon.base_url
        };
        let path = if org {
            "/api/v1/administration/mail/junk-policy"
        } else {
            "/api/v1/workspace/mail/junk-policy"
        };
        let response = self
            .client
            .put(format!("{base}{path}"))
            .header("cookie", self.daemon.cookie())
            .json(&policy)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
    }
}

#[tokio::test]
async fn junk_sensitivity_and_score_only_control_host_moves() {
    let app = MailboxApp::with_mailbox().await;
    app.transport.deliver_raw(b"From: sender@example.com\r\nMessage-ID: <threshold@example.com>\r\nX-Spam-Score: 5\r\n\r\nOffer");
    app.put_junk_policy(
        true,
        json!({"sensitivity":"high","lists":{"allow":[],"block":[]}}),
    )
    .await;
    let inbox = app
        .get("/api/v1/workspace/mail/messages?folder=INBOX")
        .await;
    assert_eq!(inbox["items"][0]["junk"], true);
    assert_eq!(app.transport.summaries("INBOX").len(), 1);
    app.put_junk_policy(
        false,
        json!({"spam_mode":"move_to_junk","lists":{"allow":[],"block":[]}}),
    )
    .await;
    assert!(
        app.get("/api/v1/workspace/mail/messages?folder=INBOX")
            .await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages?folder=Junk").await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn junk_org_lists_take_priority_and_a_person_mark_overrides_the_message() {
    let source = Arc::new(pagis_mail::fake::FakeSpamVerdicts::default());
    let app = MailboxApp::start_with_source(source.clone()).await;
    assert_eq!(app.create("person").await.0, 201);
    app.put_junk_policy(false, json!({"spam_mode":"move_to_junk","lists":{"allow":["sender@example.com"],"block":["example.com"]}})).await;
    app.put_junk_policy(true, json!({"sensitivity":"low","lists":{"allow":["example.com"],"block":["blocked@example.com"]}})).await;
    app.transport
        .deliver("sender@example.com", "Allowed", "Allowed");
    app.transport
        .deliver("blocked@example.com", "Blocked", "Blocked");
    let inbox = app
        .get("/api/v1/workspace/mail/messages?folder=INBOX")
        .await;
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["items"][0]["subject"], "Allowed");
    let junk = app.get("/api/v1/workspace/mail/messages?folder=Junk").await;
    let id = junk["items"][0]["id"].as_str().unwrap();
    let response = app
        .client
        .post(format!(
            "{}/api/v1/workspace/mail/messages/{id}/spam",
            app.daemon.base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({"spam":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(
        source.corrections(),
        vec![(pagis_mail::MessageId::new(id), false)]
    );
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages?folder=INBOX")
            .await["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn quarantine_blocks_every_product_read_and_release_is_audited() {
    let app = MailboxApp::with_mailbox().await;
    let id = app.transport.deliver_raw(b"From: sender@example.com\r\nMessage-ID: <malware@example.com>\r\nSubject: Unsafe\r\nX-Spamd-Result: default: False [20 / 15]; CLAM_VIRUS(20)[]\r\n\r\nUnsafe content");
    let thread = app.transport.summaries("INBOX")[0].thread_id.clone();
    for (method, suffix, body) in [
        (reqwest::Method::DELETE, format!("messages/{id}"), json!({})),
        (
            reqwest::Method::POST,
            format!("messages/{id}/move"),
            json!({"name":"INBOX"}),
        ),
    ] {
        let (status, _) = app
            .request(
                method,
                &format!("/api/v1/workspace/mail/{suffix}"),
                Some(body),
            )
            .await;
        assert_eq!(status, 404);
    }
    assert!(
        app.get("/api/v1/workspace/mail/messages").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for suffix in [
        format!("messages/{id}"),
        format!("messages/{id}/attachments/0"),
    ] {
        let response = app
            .client
            .get(format!(
                "{}/api/v1/workspace/mail/{suffix}",
                app.daemon.base_url
            ))
            .header("cookie", app.daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
    let mut url = reqwest::Url::parse(&format!(
        "{}/api/v1/workspace/mail/threads/",
        app.daemon.base_url
    ))
    .unwrap();
    url.path_segments_mut()
        .unwrap()
        .pop_if_empty()
        .push(&thread);
    let response = app
        .client
        .get(url)
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let page: Value = app
        .client
        .get(format!(
            "{}/api/v1/administration/mail/quarantine",
            app.daemon.administration_base_url
        ))
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let entry = &page["items"][0];
    let request = json!({"workspace_id": entry["workspace_id"], "mailbox": entry["mailbox"], "message_key": entry["message_key"]});
    let read = app
        .client
        .post(format!(
            "{}/api/v1/administration/mail/quarantine/read",
            app.daemon.administration_base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    assert_eq!(
        read.json::<Value>().await.unwrap()["text"],
        "Unsafe content"
    );
    let release = app
        .client
        .post(format!(
            "{}/api/v1/administration/mail/quarantine/release",
            app.daemon.administration_base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(release.status(), 204);
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages").await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let audit: Value = app
        .client
        .get(format!(
            "{}/api/v1/admin/audit",
            app.daemon.administration_base_url
        ))
        .header("cookie", app.daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["action"] == "mail.quarantine_released")
    );
}

#[tokio::test]
async fn shared_calendar_agent_creates_member_event_with_agent_author() {
    let app = MailboxApp::start().await;
    let (_, shared) = app
        .post(
            "/api/v1/administration/workspaces",
            json!({
                "name": "Support",
                "manager_id": app.daemon.user_id
            }),
        )
        .await;
    let workspace = shared["id"].as_str().unwrap();
    let prefix = format!("/api/v1/workspaces/{workspace}");
    let (_, agent) = app
        .post(
            &format!("{prefix}/agents"),
            json!({
                "name": "Aria",
                "job": "Arrange team meetings"
            }),
        )
        .await;
    let agent_id = pagis_core::AgentId::from(agent["id"].as_str().unwrap().to_owned());
    let (status, _) = app
        .post(
            &format!("{prefix}/grants/calendar"),
            json!({
                "agent_id": agent_id
            }),
        )
        .await;
    assert_eq!(status, 201);
    let email = "manager@example.com";
    app.daemon
        .stores()
        .users
        .set_email_and_password(&app.daemon.user_id, email, "hash", pagis_core::now_ms())
        .await
        .unwrap();
    let channel = app
        .daemon
        .stores()
        .channels
        .find_user_dm(
            &pagis_core::WorkspaceId::from(workspace.to_owned()),
            &agent_id,
        )
        .await
        .unwrap()
        .unwrap();
    app.brain.push(Script::tool_call(
        &[],
        "calendar_create_event",
        json!({
            "event": {
                "title": "Team review",
                "description": "Plan",
                "location": "Office",
                "start": "2026-03-08T09:00:00",
                "end": "2026-03-08T10:00:00",
                "timezone": "UTC",
                "all_day": false,
                "rrule": null,
                "exdates": [],
                "attendees": [{
                    "email": email,
                    "name": "Manager",
                    "status": "needs-action"
                }],
                "reminders": [],
                "meeting_url": null
            }
        }),
    ));
    app.brain
        .push(Script::reply(&["The Event is in the Calendar."]));
    assert_eq!(
        app.post(
            &format!("{prefix}/channels/{}/messages", channel.id),
            json!({
                "pending_id": "calendar-member",
                "text": "Arrange a team review"
            })
        )
        .await
        .0,
        201
    );
    let events = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let events = app
                .get(&format!(
                    "{prefix}/calendar/events?from=1772928000000&to=1773014400000"
                ))
                .await;
            if !events.as_array().unwrap().is_empty() {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(events[0]["event"]["created_by"]["kind"], "agent");
    assert_eq!(
        events[0]["event"]["created_by"]["agent_id"],
        agent_id.as_str()
    );
    assert_eq!(events[0]["event"]["workspace_id"], workspace);
    let private = app
        .get("/api/v1/calendar/events?from=1772928000000&to=1773014400000")
        .await;
    assert_eq!(private[0]["event"]["id"], events[0]["event"]["id"]);
    assert_eq!(private[0]["workspace_name"], "Support");
}

#[tokio::test]
async fn shared_calendar_outside_invitation_sends_from_team_workspace_mailbox() {
    let app = MailboxApp::start().await;
    let (_, shared) = app
        .post(
            "/api/v1/administration/workspaces",
            json!({
                "name": "Support",
                "manager_id": app.daemon.user_id
            }),
        )
        .await;
    let prefix = format!("/api/v1/workspaces/{}", shared["id"].as_str().unwrap());
    let data = json!({
        "title": "Team review",
        "description": "Plan",
        "location": "Office",
        "start": "2026-03-08T09:00:00",
        "end": "2026-03-08T10:00:00",
        "timezone": "UTC",
        "all_day": false,
        "rrule": null,
        "exdates": [],
        "attendees": [{
                    "email": "guest@other.test",
                    "name": "Guest",
                    "status": "needs-action"
                }],
        "reminders": [],
        "meeting_url": null
    });
    let (status, error) = app
        .post(&format!("{prefix}/calendar/events"), data.clone())
        .await;
    assert_eq!(status, 422);
    assert!(error.to_string().contains("Create a Workspace Mailbox"));
    assert_eq!(
        app.post(
            &format!("{prefix}/workspace/mailbox"),
            json!({
                "connection_id": app.connection_id,
                "local_part": "support"
            })
        )
        .await
        .0,
        201
    );
    assert_eq!(
        app.post(&format!("{prefix}/calendar/events"), data).await.0,
        201
    );
    let sent = app.transport.sent();
    let mail = sent.last().unwrap();
    let received = app.transport.received("guest@other.test");
    assert_eq!(received.last().unwrap().summary.from, "support@example.com");
    assert_eq!(mail.to, ["guest@other.test"]);
    assert!(
        String::from_utf8_lossy(&mail.attachments[0].data)
            .contains("ORGANIZER:mailto:support@example.com")
    );
}

impl MailboxApp {
    async fn triage_poll(&self) {
        pagis::mail_triage::MailTriagePoll::new(
            self.daemon.stores().clone(),
            self.daemon.state.mailbox_desk.clone(),
            self.daemon.state.trigger.clone(),
            self.daemon.state.person_activity.clone(),
            self.daemon.state.clock.clone(),
        )
        .poll_workspace(&self.daemon.workspace_id)
        .await
        .unwrap();
    }

    async fn wait_triage(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let message = self
                    .get(&format!("/api/v1/workspace/mail/messages/{id}"))
                    .await;
                if !message["triage"].is_null() {
                    break message;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn rules(&self, rules: Value) {
        assert_eq!(
            self.request(
                reqwest::Method::PUT,
                "/api/v1/workspace/mailbox/triage-rules",
                Some(rules)
            )
            .await
            .0,
            200
        );
    }
}

#[tokio::test]
async fn chief_reads_mail_first_and_records_handled_without_a_needs_you_item() {
    let app = MailboxApp::with_mailbox().await;
    let id = app
        .transport
        .deliver("sender@example.com", "Receipt", "No action is needed.");
    app.brain.push(Script::tool_call_as(
        "read-mail",
        &[],
        "workspace_mail_get_message",
        json!({"message_id":id.to_string()}),
    ));
    app.brain.push(Script::tool_call_as(
        "triage-mail",
        &[],
        "workspace_mail_triage",
        json!({"message_id":id.to_string(),"outcome":"handled","note":"Read the receipt."}),
    ));
    app.brain.push(Script::reply(&["Done."]));
    app.triage_poll().await;
    let message = app.wait_triage(id.as_str()).await;
    assert_eq!(message["triage"]["outcome"], "handled");
    assert_eq!(message["triage"]["note"], "Read the receipt.");
    assert!(
        app.get("/api/v1/needs-you").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn chief_needs_you_outcome_makes_exactly_one_queue_item() {
    let app = MailboxApp::with_mailbox().await;
    let id = app
        .transport
        .deliver("sender@example.com", "Decision", "Please decide.");
    app.brain.push(Script::tool_call(
        &[],
        "workspace_mail_triage",
        json!({"message_id":id.to_string(),"outcome":"needs_you","note":"Please decide."}),
    ));
    app.brain.push(Script::reply(&["Done."]));
    app.triage_poll().await;
    app.wait_triage(id.as_str()).await;
    app.triage_poll().await;
    let queue = app.get("/api/v1/needs-you").await;
    assert_eq!(queue["items"].as_array().unwrap().len(), 1);
    assert!(queue["items"][0]["url"].as_str().unwrap().contains("mail"));
    assert_eq!(
        app.request(
            reqwest::Method::POST,
            &format!("/api/v1/workspace/mail/messages/{id}/done"),
            None
        )
        .await
        .0,
        204
    );
    assert!(
        app.get("/api/v1/needs-you").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let message = app
        .get(&format!("/api/v1/workspace/mail/messages/{id}"))
        .await;
    assert!(message["triage"]["resolved_at"].is_i64());
}

#[tokio::test]
async fn needs_you_rule_has_no_run_and_first_match_wins() {
    let app = MailboxApp::with_mailbox().await;
    app.rules(json!([
        {"sender":"sender@example.com","action":"needs_you"},
        {"sender":"example.com","action":"agent"}
    ]))
    .await;
    let id = app
        .transport
        .deliver("SENDER@EXAMPLE.COM", "Decision", "Please decide.");
    app.triage_poll().await;
    let message = app.wait_triage(id.as_str()).await;
    assert_eq!(message["triage"]["outcome"], "needs_you");
    assert!(
        message["triage"]["note"]
            .as_str()
            .unwrap()
            .contains("sender@example.com")
    );
    assert!(
        app.get("/api/v1/runs").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.get("/api/v1/workspace/mail/messages?view=needs_you")
            .await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn active_person_gets_mail_unless_delegate_everything_or_an_agent_rule_applies() {
    for (delegate, rule) in [(false, false), (true, false), (false, true)] {
        let app = MailboxApp::with_mailbox().await;
        app.daemon
            .state
            .person_activity
            .record(&app.daemon.user_id, app.daemon.state.clock.now_ms());
        if delegate {
            assert_eq!(
                app.request(
                    reqwest::Method::PUT,
                    "/api/v1/workspace/delegate-everything",
                    Some(json!({"enabled":true}))
                )
                .await
                .0,
                200
            );
        }
        if rule {
            app.rules(json!([{"sender":"example.com","action":"agent"}]))
                .await;
        }
        let id = app
            .transport
            .deliver("sender@example.com", "Receipt", "Receipt.");
        if delegate || rule {
            app.brain.push(Script::tool_call(
                &[],
                "workspace_mail_triage",
                json!({"message_id":id.to_string(),"outcome":"handled","note":"Read the receipt."}),
            ));
            app.brain.push(Script::reply(&["Done."]));
        }
        app.triage_poll().await;
        let message = app.wait_triage(id.as_str()).await;
        assert_eq!(
            message["triage"]["outcome"],
            if delegate || rule {
                "handled"
            } else {
                "needs_you"
            }
        );
        if !delegate && !rule {
            assert!(
                app.get("/api/v1/runs").await["items"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[tokio::test]
async fn chief_run_without_an_outcome_gets_a_system_needs_you_outcome() {
    let app = MailboxApp::with_mailbox().await;
    let id = app
        .transport
        .deliver("sender@example.com", "Decision", "Please decide.");
    app.brain.push(Script::reply(&["I read the message."]));
    app.triage_poll().await;
    let message = app.wait_triage(id.as_str()).await;
    assert_eq!(message["triage"]["outcome"], "needs_you");
    assert_eq!(
        message["triage"]["note"],
        "The Chief of Staff recorded no outcome."
    );
}

#[tokio::test]
async fn junk_and_quarantine_have_no_mail_triage_in_score_only_mode() {
    let app = MailboxApp::with_mailbox().await;
    for header in [
        "X-Spamd-Result: default: False [20 / 15]; SPAM(20)[]",
        "X-Spamd-Result: default: False [20.0 / 15.0]; CLAM_VIRUS(20.0)[virus]",
    ] {
        app.transport.deliver_raw(format!("From: sender@example.com\r\nTo: person@example.com\r\nSubject: Junk\r\n{header}\r\n\r\nBad mail.").as_bytes());
    }
    app.triage_poll().await;
    assert!(
        app.daemon
            .stores()
            .workspace_mailboxes
            .open_needs_you(&app.daemon.workspace_id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        app.get("/api/v1/runs").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn failed_chief_mail_run_has_one_system_needs_you_item() {
    let app = MailboxApp::with_mailbox().await;
    let id = app
        .transport
        .deliver("sender@example.com", "Decision", "Please decide.");
    app.brain.push(Script::fail_after(&[], "The model failed."));
    app.triage_poll().await;
    let message = app.wait_triage(id.as_str()).await;
    assert_eq!(message["triage"]["outcome"], "needs_you");
    assert_eq!(
        message["triage"]["note"],
        "The Chief of Staff recorded no outcome."
    );
    assert_eq!(
        app.get("/api/v1/needs-you").await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn chief_schedules_a_meeting_by_mail() {
    use pagis_core::{Author, CalendarRange};

    let app = MailboxApp::with_mailbox().await;
    app.daemon
        .stores()
        .users
        .set_email_and_password(
            &app.daemon.user_id,
            "person@example.com",
            "hash",
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let (status, _) = app
        .post(
            "/api/v1/grants/calendar",
            json!({
                "agent_id": app.daemon.agent_id
            }),
        )
        .await;
    assert_eq!(status, 201);
    let (status, _) = app
        .post(
            &format!("/api/v1/agents/{}/mailbox", app.daemon.agent_id),
            json!({
                "connection_id": app.connection_id,
                "local_part": "pixie"
            }),
        )
        .await;
    assert_eq!(status, 201);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mailbox = app
                .get(&format!("/api/v1/agents/{}/mailbox", app.daemon.agent_id))
                .await;
            if mailbox["mailbox"]["state"] == "active" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    app.rules(json!([{
        "sender": "other.test",
        "action": "agent"
    }]))
    .await;
    let roster = app.get("/api/v1/agents").await;
    let chief = roster["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == app.daemon.agent_id.as_str())
        .unwrap();
    let chief_name = chief["name"].as_str().unwrap();
    let range = json!({
        "from": "2026-10-20T00:00:00Z",
        "to": "2026-10-21T00:00:00Z"
    });
    app.brain.push_for(
        chief_name,
        Script::tool_call_as("busy", &[], "calendar_free_busy", range.clone()),
    );
    app.brain.push_for(
        chief_name,
        Script::tool_call_as("events", &[], "calendar_list_events", range),
    );
    app.brain.push_for(chief_name, Script::tool_call_as("proposal", &[], "mail__send", json!({
        "mailbox": "own",
        "to": ["guest@other.test"],
        "cc": ["person@example.com"],
        "subject": "Meeting times",
        "body": "Can you meet on October 20 at 09:00 or 14:00 UTC? Reply all to keep person@example.com informed."
    })));
    app.brain.push_for(
        chief_name,
        Script::reply(&["I sent two meeting times to the guest."]),
    );
    let approval = app.ask("meeting-times", "Arrange a meeting with guest@other.test. Propose two times on October 20. Copy my Workspace Mailbox.").await.expect("proposal Approval");
    assert!(app.transport.sent().is_empty());
    app.decide(approval["id"].as_str().unwrap(), "once").await;
    app.wait_for_completed_runs(1).await;
    let proposal = app.transport.received("guest@other.test").pop().unwrap();
    assert_eq!(proposal.summary.from, "pixie@example.com");
    assert_eq!(app.transport.sent()[0].cc, ["person@example.com"]);
    let thread = proposal.summary.thread_id;
    app.transport.deliver_answer(
        &thread,
        "guest@other.test",
        "Re: Meeting times",
        "14:00 UTC works for me.",
    );
    let reply = app
        .transport
        .summaries("INBOX")
        .into_iter()
        .find(|mail| mail.from == "guest@other.test")
        .unwrap();
    app.brain.push_for(
        chief_name,
        Script::tool_call_as(
            "read-reply",
            &[],
            "workspace_mail_get_message",
            json!({
                "message_id": reply.id.to_string()
            }),
        ),
    );
    app.brain.push_for(
        chief_name,
        Script::tool_call_as(
            "read-thread",
            &[],
            "workspace_mail_get_thread",
            json!({
                "thread_id": reply.thread_id
            }),
        ),
    );
    app.brain.push_for(
        chief_name,
        Script::tool_call_as(
            "create-meeting",
            &[],
            "calendar_create_event",
            json!({
                "event": {
                    "title": "Meeting with guest",
                    "description": "The guest selected 14:00 UTC by mail.",
                    "location": "Office",
                    "start": "2026-10-20T14:00:00",
                    "end": "2026-10-20T14:30:00",
                    "timezone": "UTC",
                    "all_day": false,
                    "rrule": null,
                    "exdates": [],
                    "attendees": [{
                        "email": "guest@other.test",
                        "name": "Guest",
                        "status": "needs-action"
                    }],
                    "reminders": [],
                    "meeting_url": null
                }
            }),
        ),
    );
    app.brain.push_for(
        chief_name,
        Script::tool_call_as(
            "record-outcome",
            &[],
            "workspace_mail_triage",
            json!({
                "message_id": reply.id.to_string(),
                "outcome": "handled",
                "note": "The meeting is in the Calendar at 14:00 UTC. The invitation is sent."
            }),
        ),
    );
    app.brain.push_for(
        chief_name,
        Script::reply(&[
            "Your meeting with the guest is on October 20 at 14:00 UTC. The invitation is sent.",
        ]),
    );
    app.triage_poll().await;
    let approval = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let page = app.get("/api/v1/requests?state=pending").await;
            if let Some(card) = page["items"].as_array().and_then(|items| items.first()) {
                break card.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("invitation Approval");
    assert_eq!(
        approval["payload"]["arguments"]["from"],
        "person@example.com"
    );
    assert_eq!(
        approval["payload"]["arguments"]["to"],
        json!(["guest@other.test"])
    );
    app.decide(approval["id"].as_str().unwrap(), "once").await;
    let handled = app.wait_triage(reply.id.as_str()).await;
    assert_eq!(handled["triage"]["outcome"], "handled");
    assert_eq!(handled["triage"]["author"]["kind"], "agent");
    assert!(
        handled["triage"]["note"]
            .as_str()
            .unwrap()
            .contains("14:00 UTC")
    );
    let range = CalendarRange::parse("2026-10-20T00:00:00Z", "2026-10-21T00:00:00Z").unwrap();
    let events = app
        .daemon
        .stores()
        .calendar
        .list(&app.daemon.workspace_id, range.from(), range.to())
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event.created_by,
        Author::Agent(app.daemon.agent_id.clone().into())
    );
    assert_eq!(events[0].event.data.start, "2026-10-20T14:00:00");
    let received = app.transport.received("guest@other.test");
    assert_eq!(received.len(), 2);
    assert_eq!(
        reply.in_reply_to.as_deref(),
        Some(proposal.summary.message_id.as_str())
    );
    assert!(received[0].text.contains("09:00 or 14:00 UTC"));
    assert_eq!(received[1].summary.from, "person@example.com");
    let sent = app.transport.sent();
    let request = String::from_utf8_lossy(&sent[1].attachments[0].data);
    assert!(request.contains("METHOD:REQUEST"));
    assert!(request.contains("ORGANIZER:mailto:person@example.com"));
    assert!(request.contains("ATTENDEE;"));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let messages = app
                .get(&format!(
                    "/api/v1/channels/{}/messages",
                    app.daemon.dm_channel_id
                ))
                .await;
            if messages
                .to_string()
                .contains("Your meeting with the guest is on October 20 at 14:00 UTC.")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Chief of Staff reports in its DM Channel");
    let requests = app.brain.requests();
    for (call, text) in [
        ("busy", "[]"),
        ("events", "[]"),
        ("read-reply", "14:00 UTC works for me."),
        ("read-thread", "14:00 UTC works for me."),
    ] {
        assert!(
            requests
                .iter()
                .flat_map(|request| &request.messages)
                .any(|message| {
                    message.tool_call_id.as_deref() == Some(call) && message.text.contains(text)
                }),
            "{call} returns its result to the Chief of Staff"
        );
    }
    let briefing = requests
        .iter()
        .find(|request| {
            request
                .system
                .contains("from: guest@other.test | subject: Re: Meeting times")
        })
        .expect("the Mail Triage Wake-up has a Briefing");
    assert!(briefing.system.contains("trust_tier: unknown"));
}
