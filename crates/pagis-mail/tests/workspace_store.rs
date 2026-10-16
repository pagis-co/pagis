//! The same read operations run against each mail transport.
use pagis_mail::fake::FakeMailTransport;
use pagis_mail::{
    Endpoint, FlagChange, ImapWorkspaceMailStore, MailTransport, MailboxCredential, PageRequest,
    WorkspaceMailReader, WorkspaceMailStore, WorkspaceMailWriter,
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
    let mut store = ImapWorkspaceMailStore::new(
        transport.login(&credential).await.unwrap(),
        crate::spam::context(),
    );
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
    let mut store = ImapWorkspaceMailStore::new(session, crate::spam::context());
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

#[tokio::test]
async fn workspace_mail_store_writes_and_organizes() {
    let transport = FakeMailTransport::default();
    let credential = MailboxCredential::new(
        "person@example.test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut store = ImapWorkspaceMailStore::new(
        transport.login(&credential).await.unwrap(),
        crate::spam::context(),
    );
    write_conformance(&mut store, credential.address(), "recipient@example.test").await;
    assert_eq!(transport.sent()[0].attachments[0].data, b"invoice");
    assert_eq!(
        transport.received("recipient@example.test")[0].attachments[0].name,
        "invoice.txt"
    );
}

/// Each store sends, reads attachments and changes folders and labels.
pub async fn write_conformance(store: &mut dyn WorkspaceMailStore, own: &str, recipient: &str) {
    use pagis_mail::{MessageId, OutgoingAttachment, OutgoingMessage};
    let attachment = OutgoingAttachment {
        name: "invoice.txt".into(),
        content_type: "text/plain".into(),
        data: b"invoice".to_vec(),
    };
    store
        .send(&OutgoingMessage {
            to: vec![recipient.into()],
            subject: "Compose attachment".into(),
            body: "Please read".into(),
            attachments: vec![attachment.clone()],
            ..Default::default()
        })
        .await
        .unwrap();
    let sent = store
        .messages("Sent", "Compose attachment", &PageRequest::new(None, 20))
        .await
        .unwrap();
    assert_eq!(sent.items.len(), 1);
    let id = sent.items[0].summary.id.clone();
    assert_eq!(store.attachment(&id, 0).await.unwrap(), attachment);
    let original = store.message(&id).await.unwrap().message;
    let reply = pagis_mail::reply_message(&original, own, true);
    assert_eq!(reply.to, [recipient]);
    assert_eq!(reply.in_reply_to, Some(original.summary.message_id.clone()));
    assert_eq!(reply.references.last(), Some(&original.summary.message_id));
    store.send(&reply).await.unwrap();
    assert_eq!(
        store
            .thread(&original.summary.thread_id)
            .await
            .unwrap()
            .len(),
        2
    );
    store.create_folder("Invoices").await.unwrap();
    store.rename_folder("Invoices", "Paid").await.unwrap();
    store.set_label(&id, "finance", true).await.unwrap();
    assert_eq!(store.message(&id).await.unwrap().labels, ["finance"]);
    store.set_label(&id, "finance", false).await.unwrap();
    assert!(store.message(&id).await.unwrap().labels.is_empty());
    assert!(store.set_label(&id, "\\Seen", true).await.is_err());
    store.move_message(&id, "Paid").await.unwrap();
    assert_eq!(
        store
            .messages("Paid", "", &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(store.message(&id).await.is_err());
    for system in ["INBOX", "Sent", "Drafts", "Junk", "Trash"] {
        assert!(store.rename_folder(system, "Other").await.is_err());
        assert!(store.delete_folder(system).await.is_err());
    }
    store.delete_folder("Paid").await.unwrap();
    assert!(!store.folders().await.unwrap().contains(&"Paid".into()));
    assert!(store.attachment(&MessageId::new("bad"), 0).await.is_err());
    store.close().await.unwrap();
}

#[tokio::test]
async fn workspace_mail_reply_all_uses_sender_to_cc_and_excludes_own_address() {
    let transport = FakeMailTransport::default();
    let id = transport.deliver_raw(b"Message-ID: <last@test>\r\nReferences: <root@test> <middle@test>\r\nFrom: Sender <sender@test>\r\nTo: Person <PERSON@test>, Other <other@test>\r\nCc: Copy <copy@test>, sender@test\r\nSubject: Re: Hello\r\n\r\nHello\r\n");
    let credential = MailboxCredential::new(
        "person@test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut store = ImapWorkspaceMailStore::new(
        transport.login(&credential).await.unwrap(),
        crate::spam::context(),
    );
    let original = store.message(&id).await.unwrap().message;
    let reply = pagis_mail::reply_message(&original, "person@test", true);
    assert_eq!(reply.to, ["sender@test", "other@test"]);
    assert_eq!(reply.cc, ["copy@test"]);
    assert_eq!(
        reply.references,
        ["<root@test>", "<middle@test>", "<last@test>"]
    );
    store.send(&reply).await.unwrap();
    let thread = store.thread("<root@test>").await.unwrap();
    assert_eq!(thread.len(), 2);
    let forwarded = pagis_mail::forward_message(&original);
    assert!(forwarded.in_reply_to.is_none());
    assert!(forwarded.body.contains("> Hello"));
}

#[tokio::test]
async fn workspace_mail_store_protects_host_system_folders_and_rejects_bad_names() {
    let transport = FakeMailTransport::default();
    let credential = MailboxCredential::new(
        "person@test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut store = ImapWorkspaceMailStore::new(
        transport.login(&credential).await.unwrap(),
        crate::spam::context(),
    );
    assert!(store.rename_folder("Archive", "Other").await.is_err());
    assert!(store.delete_folder("Archive").await.is_err());
    for name in ["", "  ", "folder\r\nDELETE INBOX"] {
        assert!(store.create_folder(name).await.is_err());
    }
}

#[tokio::test]
async fn a_thread_shows_mail_sent_to_the_own_address_once() {
    let transport = FakeMailTransport::default();
    let credential = MailboxCredential::new(
        "person@example.test",
        "pw",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let mut store = ImapWorkspaceMailStore::new(
        transport.login(&credential).await.unwrap(),
        crate::spam::context(),
    );
    let sent = store
        .send(&pagis_mail::OutgoingMessage {
            to: vec![credential.address().to_string()],
            subject: "Note".into(),
            body: "note to self".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    // The host delivers the same message to INBOX and keeps the copy in Sent.
    let inbox_copy = transport.deliver_raw(
        format!(
            "Message-ID: {}\r\nFrom: person@example.test\r\nTo: person@example.test\r\n\
             Subject: Note\r\nDate: Thu, 1 Jan 2026 00:00:00 +0000\r\n\r\nnote to self\r\n",
            sent.message_id
        )
        .as_bytes(),
    );
    let thread_id = store
        .message(&inbox_copy)
        .await
        .unwrap()
        .message
        .summary
        .thread_id;
    assert_eq!(store.thread(&thread_id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn junk_folder_sets_a_stored_verdict_without_a_header() {
    let transport = FakeMailTransport::default();
    let credential = MailboxCredential::new(
        "person@example.com",
        "password",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    transport.deliver("sender@test", "Junk sample", "body");
    let mut session = transport.login(&credential).await.unwrap();
    session.create_folder("Junk").await.unwrap();
    session
        .move_message(&pagis_mail::MessageId::new("INBOX:1"), "Junk")
        .await
        .unwrap();
    let context = crate::spam::context();
    let records = context.intake.records.clone();
    let workspace = context.workspace.clone();
    let address = context.address.clone();
    let mut store = ImapWorkspaceMailStore::new(session, context);
    let page = store
        .messages("Junk", "", &PageRequest::new(None, 20))
        .await
        .unwrap();
    let summary = &page.items[0].summary;
    assert_eq!(
        summary.assessment.as_ref().unwrap().verdict.action,
        pagis_core::SpamAction::Junk
    );
    assert_eq!(
        records
            .get(&workspace, &address, summary.assessment_key())
            .await
            .unwrap(),
        summary.assessment
    );
    assert!(
        store
            .messages("INBOX", "", &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        store
            .messages("Junk", "", &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
