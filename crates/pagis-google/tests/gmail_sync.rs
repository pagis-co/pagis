use async_trait::async_trait;
use pagis_google::{
    ConnectionBinding, GogCommand, GogRunner, GoogleProvider, ProcessFailure, ProcessOutput,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Fake(Mutex<Vec<serde_json::Value>>);

struct RecordingFake {
    responses: Mutex<Vec<serde_json::Value>>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl RecordingFake {
    fn new(responses: Vec<serde_json::Value>) -> Self {
        Self {
            responses: Mutex::new(responses),
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl GogRunner for RecordingFake {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        self.seen.lock().unwrap().push(command.args().to_vec());
        Ok(ProcessOutput {
            status: Some(0),
            stdout: self
                .responses
                .lock()
                .unwrap()
                .remove(0)
                .to_string()
                .into_bytes(),
        })
    }
}

#[tokio::test]
async fn each_discovery_call_allows_only_its_exact_gmail_method() {
    let fake = Arc::new(RecordingFake::new(vec![
        json!({"historyId":"100"}),
        json!({"messages":[{"id":"a"}]}),
        json!({"id":"a","threadId":"t","historyId":"90","internalDate":"1700000000000"}),
        json!({"messages":[{"id":"a","historyId":"90","internalDate":"1700000000000","payload":{"headers":[]}}]}),
        json!({"historyId":"101"}),
    ]));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        Arc::clone(&fake),
    );

    provider
        .sync_gmail(&serde_json::Value::Null, 0)
        .await
        .unwrap();
    provider.knowledge_thread("t").await.unwrap();
    provider
        .sync_gmail(&json!({"phase":"history","history":"100","page":null}), 0)
        .await
        .unwrap();

    let policies = fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|args| {
            let flag = args
                .iter()
                .position(|arg| arg == "--enable-commands-exact")
                .unwrap();
            args[flag + 1].clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        policies,
        [
            "api.call,api.gmail.users.getprofile",
            "api.call,api.gmail.users.messages.list",
            "api.call,api.gmail.users.messages.get",
            "api.call,api.gmail.users.threads.get",
            "api.call,api.gmail.users.history.list",
        ]
    );
}

#[tokio::test]
async fn hydration_keeps_source_dates_and_decodes_plain_text_without_storing_a_body() {
    let fake = Arc::new(Fake(Mutex::new(vec![json!({"messages":[{
        "id":"a", "threadId":"t", "historyId":"120", "internalDate":"1700000000000",
        "payload":{"mimeType":"text/plain", "headers":[{"name":"Subject","value":"PTA"}],
            "body":{"data":"TWVldGluZyBjYW5jZWxsZWQ"}}
    }]})])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let content = provider.knowledge_thread("t").await.unwrap();
    assert_eq!(content.items[0].source_at, Some(1700000000000));
    assert_eq!(content.items[0].version, "120");
    assert!(content.contents[0].text.contains("Meeting cancelled"));
    assert_eq!(content.contents[0].id, "a");
    assert_eq!(content.contents[0].version, "120");
    assert!(content.contents[0].native_episode.is_none());
}

#[async_trait]
impl GogRunner for Fake {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        assert!(command.args().iter().any(|a| a == "--readonly"));
        assert!(!command.is_write());
        Ok(ProcessOutput {
            status: Some(0),
            stdout: self.0.lock().unwrap().remove(0).to_string().into_bytes(),
        })
    }
}

#[tokio::test]
async fn history_keeps_its_start_until_the_last_page_and_records_deletion() {
    let fake = Arc::new(Fake(Mutex::new(vec![
        json!({"historyId":"200", "nextPageToken":"more", "history":[{"id":"101", "messagesDeleted":[{"message":{"id":"a","threadId":"t"}}]}]}),
        json!({"historyId":"201", "history":[{"id":"199", "messagesAdded":[{"message":{"id":"b","threadId":"t2"}}]}]}),
        json!({"id":"b", "threadId":"t2", "historyId":"199", "internalDate":"1700000000000", "labelIds":["INBOX"], "payload":{"headers":[{"name":"From","value":"school@example.com"}]}}),
    ])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let first = provider
        .sync_gmail(&json!({"phase":"history", "history":"100", "page":null}), 0)
        .await
        .unwrap();
    assert!(!first.caught_up);
    assert_eq!(first.checkpoint["history"], "100");
    assert_eq!(
        first.changes[0].operation,
        pagis_core::knowledge::SourceOperation::Deleted
    );
    let second = provider.sync_gmail(&first.checkpoint, 0).await.unwrap();
    assert!(second.caught_up);
    assert_eq!(second.checkpoint["history"], "201");
    assert_eq!(second.changes[0].id, "b");
}

#[tokio::test]
async fn full_sync_keeps_the_initial_history_cursor_through_pagination() {
    let fake = Arc::new(Fake(Mutex::new(vec![
        json!({"historyId":"100"}),
        json!({"messages":[{"id":"a", "threadId":"t"}], "nextPageToken":"page2"}),
        json!({"id":"a", "threadId":"t", "historyId":"90", "internalDate":"1700000000000"}),
        json!({"messages":[{"id":"b", "threadId":"t"}]}),
        json!({"id":"b", "threadId":"t", "historyId":"95", "internalDate":"1700000000001"}),
    ])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let first = provider
        .sync_gmail(&serde_json::Value::Null, 0)
        .await
        .unwrap();
    assert!(!first.caught_up);
    assert_eq!(first.checkpoint["history"], "100");
    assert_eq!(first.checkpoint["page"], "page2");
    assert_eq!(
        first.changes[0].version, "90",
        "scan cursor is not the message version"
    );
    assert_eq!(first.changes[0].source_at, Some(1700000000000));
    assert_eq!(first.arrivals.len(), 1);
    assert_eq!(first.arrivals[0].provider_event_id, "a");
    let second = provider.sync_gmail(&first.checkpoint, 0).await.unwrap();
    assert!(
        !second.caught_up,
        "history must be drained before declaring catch-up"
    );
    assert_eq!(second.checkpoint["phase"], "history");
    assert_eq!(second.checkpoint["history"], "100");
    assert_eq!(second.changes[0].id, "b");
}

#[tokio::test]
async fn repeated_history_entries_hydrate_one_message_once() {
    let fake = Arc::new(Fake(Mutex::new(vec![
        json!({"historyId":"45", "history":[{"id":"43","messagesAdded":[{"message":{"id":"a"}}]},{"id":"44","labelsAdded":[{"message":{"id":"a"}}]}]}),
        json!({"id":"a","historyId":"44","internalDate":"1700000000000","labelIds":["INBOX"]}),
    ])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let page = provider
        .sync_gmail(&json!({"phase":"history","history":"42","page":null}), 0)
        .await
        .unwrap();
    assert_eq!(page.arrivals.len(), 1);
    assert_eq!(page.changes.len(), 1);
}

#[tokio::test]
async fn bounded_hydration_counts_decoded_content_separately_from_transport_envelope() {
    use base64::Engine;
    let response = json!({"messages":[{
        "id":"a", "threadId":"t", "historyId":"120", "internalDate":"1700000000000",
        "payload":{"mimeType":"text/plain", "headers":[], "body":{"data":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("x".repeat(8192))}}
    }]});
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        Arc::new(Fake(Mutex::new(vec![
            response.clone(),
            response.clone(),
            response.clone(),
        ]))),
    );
    let content = provider.knowledge_thread("t").await.unwrap();
    let decoded = content.contents[0].text.len();
    assert!(
        response.to_string().len() > decoded,
        "the fixture must exercise base64 and JSON transport overhead"
    );
    let bounded = provider
        .knowledge_thread_bounded("t", decoded)
        .await
        .unwrap();
    assert_eq!(bounded.content.contents[0].text, content.contents[0].text);
    assert_eq!(bounded.source_bytes as usize, decoded);
    assert!(
        provider
            .knowledge_thread_bounded("t", decoded - 1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bounded_hydration_does_not_refund_discarded_html_markup() {
    use base64::Engine;
    let markup = format!("{}clear{}", "<span>".repeat(1000), "</span>".repeat(1000));
    let response = json!({"messages":[{
        "id":"a", "threadId":"t", "historyId":"120", "internalDate":"1700000000000",
        "payload":{"mimeType":"text/html", "headers":[], "body":{"data":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&markup)}}
    }]});
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        Arc::new(Fake(Mutex::new(vec![response.clone(), response]))),
    );
    let content = provider.knowledge_thread("t").await.unwrap();
    assert!(content.contents[0].text.len() < 512);
    assert!(
        provider.knowledge_thread_bounded("t", 512).await.is_err(),
        "source-byte accounting must include decoded HTML bytes that the text projection discards"
    );
}

#[tokio::test]
async fn spam_is_outside_the_mailbox_and_keeps_no_metadata() {
    let fake = Arc::new(Fake(Mutex::new(vec![
        json!({"historyId":"45", "history":[{"id":"44", "messagesAdded":[{"message":{"id":"a","threadId":"t"}}]}]}),
        json!({"id":"a", "threadId":"t", "historyId":"44", "internalDate":"1700000000000",
            "labelIds":["SPAM","UNREAD"], "payload":{"headers":[{"name":"From","value":"win@lottery.test"}]}}),
    ])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let page = provider
        .sync_gmail(&json!({"phase":"history", "history":"42", "page":null}), 0)
        .await
        .unwrap();
    assert_eq!(page.changes.len(), 1);
    assert_eq!(
        page.changes[0].operation,
        pagis_core::knowledge::SourceOperation::Deleted
    );
    assert_eq!(
        page.changes[0].metadata,
        serde_json::Value::Null,
        "a spam message must leave no signals for a Subject Page"
    );
    assert!(
        page.arrivals.is_empty(),
        "a spam message must not wake the agent"
    );
}

#[tokio::test]
async fn trash_is_outside_the_mailbox_too() {
    let fake = Arc::new(Fake(Mutex::new(vec![
        json!({"historyId":"45", "history":[{"id":"44", "labelsAdded":[{"message":{"id":"a","threadId":"t"}}]}]}),
        json!({"id":"a", "threadId":"t", "historyId":"44", "internalDate":"1700000000000",
            "labelIds":["TRASH"], "payload":{"headers":[{"name":"From","value":"news@shop.test"}]}}),
    ])));
    let provider = GoogleProvider::new(
        ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        fake,
    );
    let page = provider
        .sync_gmail(&json!({"phase":"history", "history":"42", "page":null}), 0)
        .await
        .unwrap();
    assert_eq!(
        page.changes[0].operation,
        pagis_core::knowledge::SourceOperation::Deleted
    );
    assert_eq!(page.changes[0].version, "44");
    assert!(page.arrivals.is_empty());
}
