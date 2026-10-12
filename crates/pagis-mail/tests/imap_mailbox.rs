//! The IMAP rules run through the Mailbox operations.

use pagis_mail::fake::FakeMailTransport;
use pagis_mail::{
    Cursor, Endpoint, FlagChange, ImapMailbox, ImapMessageId, MailQuery, MailTransport, Mailbox,
    MailboxCredential, MessageChange, MessageId, PageRequest, TransportErrorCode,
};

async fn open(transport: &FakeMailTransport, cursor: Option<Cursor>) -> ImapMailbox {
    let credential = MailboxCredential::new(
        "ada@example.test",
        "secret",
        Endpoint::new("imap.example.test", 993),
        Endpoint::new("smtp.example.test", 465),
    );
    ImapMailbox::new(transport.login(&credential).await.unwrap(), cursor)
}

#[test]
fn an_imap_id_reads_as_the_folder_and_the_uid() {
    let id: ImapMessageId = "INBOX:1234".parse().unwrap();
    assert_eq!(id.folder, "INBOX");
    assert_eq!(id.uid, 1234);
    assert_eq!(id.to_string(), "INBOX:1234");
    for text in ["INBOX", ":12", "INBOX:0", "INBOX:bad"] {
        assert!(text.parse::<ImapMessageId>().is_err());
    }
    assert_eq!("a:b:12".parse::<ImapMessageId>().unwrap().folder, "a:b");
}

#[tokio::test]
async fn each_read_selects_its_folder_and_the_archive_keeps_the_flags() {
    let transport = FakeMailTransport::default();
    let id = transport.deliver("bob@example.test", "Invoice", "text");
    assert_eq!(id.to_string(), "INBOX:1");
    let mut mailbox = open(&transport, None).await;
    let page = mailbox
        .search(&MailQuery::default(), &PageRequest::new(None, 25))
        .await
        .unwrap();
    assert_eq!(page.items[0].id, id);
    let thread = page.items[0].thread_id.clone();
    assert_eq!(mailbox.message(&id).await.unwrap().text, "text");
    assert_eq!(
        mailbox
            .thread(&thread, &PageRequest::new(None, 20))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    mailbox
        .modify(
            &id,
            MessageChange {
                flags: FlagChange {
                    seen: Some(true),
                    flagged: Some(true),
                },
                archive: true,
            },
        )
        .await
        .unwrap();
    let archived = transport.summaries("Archive");
    assert_eq!(archived[0].id.to_string(), "Archive:1");
    assert!(transport.flags_of(&archived[0].id).unwrap().seen);
    assert!(transport.flags_of(&archived[0].id).unwrap().flagged);
    assert_eq!(mailbox.message(&archived[0].id).await.unwrap().text, "text");
    assert_eq!(
        transport.selected_folders(),
        ["INBOX", "INBOX", "INBOX", "INBOX", "Archive"]
    );
}

#[tokio::test]
async fn a_renumbered_folder_refuses_every_operation_before_it_reads_or_changes_mail() {
    let transport = FakeMailTransport::default();
    let id = transport.deliver("bob@example.test", "Invoice", "text");
    let cursor = Cursor {
        folder: "INBOX".into(),
        uid_validity: 1,
        last_uid: 0,
    };
    let mut mailbox = open(&transport, Some(cursor)).await;
    transport.renumber("INBOX");
    assert_eq!(
        mailbox.message(&id).await.unwrap_err().0,
        TransportErrorCode::StaleId
    );
    assert_eq!(
        mailbox
            .search(&MailQuery::default(), &PageRequest::new(None, 25))
            .await
            .unwrap_err()
            .0,
        TransportErrorCode::StaleId
    );
    assert_eq!(
        mailbox
            .thread("<1@fake.invalid>", &PageRequest::new(None, 20))
            .await
            .unwrap_err()
            .0,
        TransportErrorCode::StaleId
    );
    assert_eq!(
        mailbox
            .modify(
                &id,
                MessageChange {
                    archive: true,
                    ..Default::default()
                }
            )
            .await
            .unwrap_err()
            .0,
        TransportErrorCode::StaleId
    );
    assert_eq!(transport.summaries("INBOX").len(), 1);
    assert!(transport.summaries("Archive").is_empty());
}

#[tokio::test]
async fn an_id_outside_the_imap_shape_is_stale() {
    let transport = FakeMailTransport::default();
    let mut mailbox = open(&transport, None).await;
    assert_eq!(
        mailbox
            .message(&MessageId::new("not-an-imap-id"))
            .await
            .unwrap_err()
            .0,
        TransportErrorCode::StaleId
    );
}

#[tokio::test]
async fn search_and_thread_pages_keep_the_order_and_do_not_repeat_messages() {
    let transport = FakeMailTransport::default();
    transport.deliver("bob@example.test", "Invoice", "first");
    let thread = transport.summaries("INBOX")[0].thread_id.clone();
    for index in 0..4 {
        transport.deliver_reply(
            &thread,
            "bob@example.test",
            "Re: Invoice",
            format!("reply {index}"),
        );
    }
    let mut mailbox = open(&transport, None).await;
    let first = mailbox
        .search(&MailQuery::default(), &PageRequest::new(None, 3))
        .await
        .unwrap();
    let second = mailbox
        .search(&MailQuery::default(), &PageRequest::new(first.next, 3))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 3);
    assert_eq!(second.items.len(), 2);
    assert!(second.next.is_none());
    assert_eq!(first.items[0].id.to_string(), "INBOX:5");
    assert_eq!(second.items[0].id.to_string(), "INBOX:2");
    let newest = mailbox
        .thread(&thread, &PageRequest::new(None, 3))
        .await
        .unwrap();
    let oldest = mailbox
        .thread(&thread, &PageRequest::new(newest.next, 3))
        .await
        .unwrap();
    assert_eq!(newest.items[0].text, "reply 1");
    assert_eq!(oldest.items[0].text, "first");
    assert_eq!(oldest.items.len(), 2);
    assert!(oldest.next.is_none());
}
