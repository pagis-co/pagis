//! A Person creates and reads their Workspace Mailbox through the Product API.
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use serde_json::{Value, json};
use std::sync::Arc;

struct MailboxApp {
    daemon: TestDaemon,
    host: Arc<FakeMailboxHost>,
    transport: Arc<FakeMailTransport>,
    connection_id: String,
    client: reqwest::Client,
}

impl MailboxApp {
    async fn start() -> Self {
        let host = Arc::new(FakeMailboxHost::default());
        let transport = Arc::new(FakeMailTransport::default());
        let daemon = TestDaemon::start_with(TestDaemonOptions {
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
