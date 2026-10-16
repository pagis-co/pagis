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
