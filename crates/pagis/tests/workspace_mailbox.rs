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
        let host = Arc::new(FakeMailboxHost::default());
        let transport = Arc::new(FakeMailTransport::default());
        let brain = Arc::new(ScriptedBrain::default());
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            brain: brain.clone(),
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
