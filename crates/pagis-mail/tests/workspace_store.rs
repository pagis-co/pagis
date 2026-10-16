//! The same read operations run against each mail transport.
use pagis_mail::fake::FakeMailTransport;
use pagis_mail::{
    Endpoint, FlagChange, ImapWorkspaceMailStore, MailTransport, MailboxCredential, PageRequest,
    WorkspaceMailStore,
};

/// The fixture holds two messages in one thread in INBOX.
pub async fn conformance(store: &mut dyn WorkspaceMailStore) {
    assert!(
        store
            .folders()
            .await
            .unwrap()
            .contains(&"INBOX".to_string())
    );
    let first = store
        .messages("INBOX", "", &PageRequest::new(None, 1))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = store
        .messages("INBOX", "", &PageRequest::new(first.next, 1))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].summary.id, second.items[0].summary.id);
    assert!(second.next.is_none());
    for (query, count) in [
        (first.items[0].summary.from.as_str(), 2),
        ("Re: Invoice", 1),
    ] {
        assert_eq!(
            store
                .messages("INBOX", query, &PageRequest::new(None, 20))
                .await
                .unwrap()
                .items
                .len(),
            count
        );
    }
    let id = first.items[0].summary.id.clone();
    assert!(!first.items[0].flags.seen);
    assert!(first.items[0].labels.is_empty());
    let message = store.message(&id).await.unwrap();
    assert!(message.message.text.contains("invoice"));
    let thread = store
        .thread(&message.message.summary.thread_id)
        .await
        .unwrap();
    assert_eq!(thread.len(), 2);
    assert!(thread[0].message.summary.date <= thread[1].message.summary.date);
    assert_eq!(
        store
            .messages("INBOX", "invoice", &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(
        store
            .messages("INBOX", "absent", &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    store
        .set_flags(
            &id,
            FlagChange {
                seen: Some(true),
                flagged: Some(true),
            },
        )
        .await
        .unwrap();
    let read = store.message(&id).await.unwrap();
    assert!(read.flags.seen);
    assert!(read.flags.flagged);
    assert!(
        store
            .messages("INBOX", "", &PageRequest::new(Some("invalid".into()), 20))
            .await
            .is_err()
    );
    assert!(
        store
            .message(&pagis_mail::MessageId::new("invalid"))
            .await
            .is_err()
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn workspace_mail_store_fake_transport_conforms() {
    let transport = FakeMailTransport::default();
    transport.deliver("sender@example.test", "Invoice", "invoice first");
    let thread = transport.summaries("INBOX")[0].thread_id.clone();
    transport.deliver_reply(
        &thread,
        "sender@example.test",
        "Re: Invoice",
        "invoice second",
    );
    let credential = MailboxCredential::new(
        "person@example.test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut store = ImapWorkspaceMailStore::new(transport.login(&credential).await.unwrap());
    conformance(&mut store).await;
}

#[tokio::test]
async fn workspace_mail_store_all_mail_includes_archive_and_scopes_cursors() {
    let transport = FakeMailTransport::default();
    let first = transport.deliver("sender@example.test", "Invoice", "first");
    let credential = MailboxCredential::new(
        "person@example.test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut session = transport.login(&credential).await.unwrap();
    session.move_to_archive(&first).await.unwrap();
    transport.deliver("sender@example.test", "Other", "second");
    let mut store = ImapWorkspaceMailStore::new(session);
    let first = store
        .messages("", "", &PageRequest::new(None, 1))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let next = first.next.unwrap();
    assert!(
        store
            .messages("INBOX", "", &PageRequest::new(Some(next.clone()), 1))
            .await
            .is_err()
    );
    assert!(
        store
            .messages("", "other", &PageRequest::new(Some(next.clone()), 1))
            .await
            .is_err()
    );
    let second = store
        .messages("", "", &PageRequest::new(Some(next), 1))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].summary.id, second.items[0].summary.id);
    assert!(second.next.is_none());
}

#[tokio::test]
async fn text_search_matches_headers_and_body_in_the_fake_transport() {
    let transport = FakeMailTransport::default();
    transport.deliver(
        "unique-sender@example.test",
        "Unique subject",
        "unique body",
    );
    let credential = MailboxCredential::new(
        "person@example.test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut session = transport.login(&credential).await.unwrap();
    session.select("INBOX").await.unwrap();
    for text in ["unique-sender", "Unique subject", "unique body"] {
        let found = session
            .search(&pagis_mail::MailQuery {
                text: Some(text.into()),
                limit: 20,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(found.len(), 1, "TEXT matches {text}");
    }
}
