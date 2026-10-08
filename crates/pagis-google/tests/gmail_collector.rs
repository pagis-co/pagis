//! Gmail collector contract tests against a fake `gog` adapter.
//!
//! Nothing here starts a process. The fake answers the search command
//! the collector builds, so the tests pin the contract: the baseline,
//! the overlap window, the recent-id deduplication, pagination, and the
//! `mail.message_received` metadata of ADR-0019.

use std::sync::Mutex;

use async_trait::async_trait;
use pagis_google::{
    BROWSE_QUERY, ConnectionBinding, GmailCollector, GogRunner, GoogleProvider, ProcessFailure,
    ProcessOutput,
};

/// The alias of the Connection the collector reads. It is the same
/// name the Agent passes to a `mail__*` tool (ADR-0019).
const MAILBOX: &str = "work";

const NOW: i64 = 1_800_000_000_000;

/// Answers each search in order and records the arguments it saw.
struct FakeGog {
    pages: Mutex<Vec<serde_json::Value>>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl FakeGog {
    fn new(pages: Vec<serde_json::Value>) -> Self {
        Self {
            pages: Mutex::new(pages),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn queries(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|args| {
                // The query is the one positional value, after `--`.
                let index = args.iter().position(|arg| arg == "--")?;
                args.get(index + 1).cloned()
            })
            .collect()
    }
}

#[async_trait]
impl GogRunner for FakeGog {
    async fn run(
        &self,
        command: &pagis_google::GogCommand,
    ) -> Result<ProcessOutput, ProcessFailure> {
        self.seen.lock().unwrap().push(command.args().to_vec());
        let mut pages = self.pages.lock().unwrap();
        let page = if pages.is_empty() {
            serde_json::json!({"messages": []})
        } else {
            pages.remove(0)
        };
        Ok(ProcessOutput {
            status: Some(0),
            stdout: page.to_string().into_bytes(),
        })
    }
}

fn message(id: &str, from: &str, subject: &str, at: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "threadId": format!("t-{id}"),
        "from": from,
        "to": ["user@example.com"],
        "subject": subject,
        "internalDate": at,
        "hasAttachments": true,
    })
}

fn collector(fake: std::sync::Arc<FakeGog>) -> GmailCollector<std::sync::Arc<FakeGog>> {
    let binding = ConnectionBinding::new("user@example.com", "work", "/tmp/pagis-test-gog-home")
        .expect("binding");
    GmailCollector::new(GoogleProvider::new(binding, fake), MAILBOX)
}

#[tokio::test]
async fn the_first_collection_is_a_baseline_and_emits_no_event() {
    let fake = std::sync::Arc::new(FakeGog::new(vec![serde_json::json!({
        "messages": [message("m1", "a@example.com", "Old", NOW - 60_000)]
    })]));
    let collected = collector(std::sync::Arc::clone(&fake))
        .collect(None, NOW)
        .await
        .expect("collect");

    assert!(collected.baseline);
    assert!(collected.events.is_empty());
    assert!(
        !collected.cursor.is_empty(),
        "the baseline still records where it stopped"
    );
    assert_eq!(
        fake.queries(),
        vec![BROWSE_QUERY.to_string()],
        "the first search has no window"
    );
}

#[tokio::test]
async fn the_next_collection_reads_an_overlapping_window_and_emits_new_mail() {
    let baseline = std::sync::Arc::new(FakeGog::new(vec![serde_json::json!({
        "messages": [message("m1", "a@example.com", "First", NOW)]
    })]));
    let first = collector(baseline)
        .collect(None, NOW)
        .await
        .expect("baseline");

    let fake = std::sync::Arc::new(FakeGog::new(vec![serde_json::json!({
        "messages": [
            message("m1", "a@example.com", "First", NOW),
            message("m2", "b@example.com", "Second", NOW + 30_000),
        ]
    })]));
    let second = collector(std::sync::Arc::clone(&fake))
        .collect(Some(&first.cursor), NOW + 40_000)
        .await
        .expect("collect");

    assert!(!second.baseline);
    let ids: Vec<&str> = second
        .events
        .iter()
        .map(|event| event.provider_event_id.as_str())
        .collect();
    assert_eq!(ids, ["m2"], "the message already seen is not new again");
    assert!(
        fake.queries()[0].contains("after:"),
        "the window starts behind the high-water mark"
    );
    let metadata = &second.events[0].metadata;
    assert_eq!(metadata["mailbox"], MAILBOX);
    assert_eq!(metadata["from"], "b@example.com");
    assert_eq!(metadata["from_domain"], "example.com");
    assert_eq!(metadata["to"], serde_json::json!(["user@example.com"]));
    assert_eq!(metadata["subject"], "Second");
    assert_eq!(metadata["thread_id"], "t-m2");
    assert_eq!(metadata["has_attachments"], true);
    assert_eq!(metadata["received_at"], NOW + 30_000);
    assert!(
        metadata.get("body").is_none() && metadata.get("snippet").is_none(),
        "no body or snippet is normalized"
    );
    // The metadata a message carries is what the declaration allows.
    let declared = pagis_broker::message_received(pagis_broker::MAIL_PROVIDER);
    jsonschema::validator_for(&declared.metadata_schema)
        .expect("the metadata schema compiles")
        .validate(metadata)
        .expect("the collector stays inside the declaration");
}

#[tokio::test]
async fn pagination_continues_until_the_pages_run_out() {
    let baseline = std::sync::Arc::new(FakeGog::new(vec![serde_json::json!({
        "messages": [message("m0", "a@example.com", "Zero", NOW)]
    })]));
    let first = collector(baseline)
        .collect(None, NOW)
        .await
        .expect("baseline");

    let fake = std::sync::Arc::new(FakeGog::new(vec![
        serde_json::json!({
            "messages": [message("m1", "a@example.com", "One", NOW + 10_000)],
            "next_page": "page-2"
        }),
        serde_json::json!({
            "messages": [message("m2", "a@example.com", "Two", NOW + 20_000)]
        }),
    ]));
    let collected = collector(std::sync::Arc::clone(&fake))
        .collect(Some(&first.cursor), NOW + 30_000)
        .await
        .expect("collect");

    assert_eq!(collected.events.len(), 2);
    assert_eq!(fake.queries().len(), 2, "the second page is read");
}

#[tokio::test]
async fn a_provider_failure_is_a_stable_code_and_no_cursor_move() {
    struct Failing;

    #[async_trait]
    impl GogRunner for Failing {
        async fn run(
            &self,
            _command: &pagis_google::GogCommand,
        ) -> Result<ProcessOutput, ProcessFailure> {
            Ok(ProcessOutput {
                status: Some(4),
                stdout: b"secret upstream text".to_vec(),
            })
        }
    }

    let binding = ConnectionBinding::new("user@example.com", "work", "/tmp/pagis-test-gog-home")
        .expect("binding");
    let collector = GmailCollector::new(
        GoogleProvider::new(binding, std::sync::Arc::new(Failing)),
        MAILBOX,
    );
    let error = collector.collect(None, NOW).await.unwrap_err();

    assert_eq!(error.code, pagis_google::ProviderErrorCode::ReauthRequired);
    assert!(!format!("{error:?}").contains("secret upstream text"));
}

#[test]
fn the_mail_manifest_declares_the_one_event_kind_and_google_declares_none() {
    assert!(pagis_google::capability_manifest().event_kinds.is_empty());

    let declared = pagis_broker::message_received(pagis_broker::MAIL_PROVIDER);
    assert_eq!(declared.name, pagis_broker::MAIL_MESSAGE_RECEIVED);
    assert_eq!(declared.required_capability, "gmail_read");
    assert_eq!(declared.provider, "google");
    assert_eq!(
        declared.filter_schema["additionalProperties"], false,
        "a filter cannot widen past the declaration"
    );
    let filter_fields: std::collections::BTreeSet<&str> = declared.filter_schema["properties"]
        .as_object()
        .expect("filter fields")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        filter_fields,
        std::collections::BTreeSet::from([
            "mailbox",
            "min_trust",
            "replies_only",
            "senders",
            "subject_contains"
        ]),
        "the five typed filter fields ADR-0019 fixes"
    );
    assert!(
        declared.metadata_schema["properties"]
            .as_object()
            .expect("metadata fields")
            .keys()
            .all(|name| !matches!(name.as_str(), "labels" | "is_inbox"))
    );
}
