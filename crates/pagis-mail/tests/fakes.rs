//! The contracts of the two fakes (ADR-0019). The daemon's tests
//! drive mailboxes and mail through these, so what they promise is
//! tested here.

use std::sync::Arc;
use std::time::Duration;

use pagis_mail::fake::{ARCHIVE, FakeMailTransport, FakeMailboxHost, HostCall};
use pagis_mail::{
    Deletion, Endpoint, FlagChange, HostAccount, HostCapabilities, HostErrorCode, INBOX,
    IdleOutcome, MailQuery, MailTransport, MailboxCredential, MailboxHost, MailboxPassword,
    MessageId, TransportCapabilities, TransportErrorCode,
};

fn account() -> HostAccount {
    HostAccount::new("example.com", "owner@example.com", "key")
}

fn credential() -> MailboxCredential {
    MailboxCredential::new(
        "ava@example.com",
        "mailbox-secret",
        Endpoint::new("imap.example.com", 993),
        Endpoint::new("smtp.example.com", 465),
    )
}

#[tokio::test]
async fn the_fake_host_holds_a_domain_in_memory() {
    let host = FakeMailboxHost::default();

    let made = host
        .create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();

    assert_eq!(made.address, "ava@example.com");
    assert_eq!(host.addresses(), ["ava@example.com"]);
    assert_eq!(
        host.password_of("ava@example.com").as_deref(),
        Some("secret")
    );
    assert_eq!(
        host.calls(),
        [HostCall::Create {
            local_part: "ava".to_string(),
            outgoing_cap: 20,
        }]
    );
}

#[tokio::test]
async fn the_fake_host_refuses_an_address_it_holds() {
    let host = FakeMailboxHost::default();
    host.create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();

    let refused = host
        .create(&account(), "ava", &MailboxPassword::new("other"), 20)
        .await
        .unwrap_err();

    assert_eq!(refused.0, HostErrorCode::AddressTaken);
}

#[tokio::test]
async fn the_fake_host_fails_with_the_scripted_code() {
    let host = FakeMailboxHost::default();
    host.fail_with(Some(HostErrorCode::Unauthorized));

    assert_eq!(
        host.list(&account()).await.unwrap_err().0,
        HostErrorCode::Unauthorized
    );

    host.fail_with(None);
    assert!(host.list(&account()).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_fake_host_deletes_resets_and_lists() {
    let host = FakeMailboxHost::default();
    host.create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();
    host.create(&account(), "ben", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();

    host.reset_password(&account(), "ava@example.com", &MailboxPassword::new("next"))
        .await
        .unwrap();
    assert_eq!(host.password_of("ava@example.com").as_deref(), Some("next"));

    assert_eq!(
        host.delete(&account(), "ben@example.com").await.unwrap(),
        Deletion::Removed
    );
    let found = host.list(&account()).await.unwrap();
    assert_eq!(
        found
            .iter()
            .map(|mailbox| mailbox.address.as_str())
            .collect::<Vec<_>>(),
        ["ava@example.com"]
    );
    assert_eq!(
        host.reset_password(&account(), "ben@example.com", &MailboxPassword::new("x"))
            .await
            .unwrap_err()
            .0,
        HostErrorCode::MailboxUnknown
    );
}

/// A host without `delete_mailbox` answers the delete with the notice
/// the manual host gives, so a test runs against both shapes.
#[tokio::test]
async fn a_fake_host_without_delete_tells_the_user_to_delete() {
    let host = FakeMailboxHost::with_capabilities(HostCapabilities {
        outgoing_cap: false,
        delete_mailbox: false,
        reset_password: false,
    });
    host.create(&account(), "ava", &MailboxPassword::new("secret"), 20)
        .await
        .unwrap();

    let deleted = host.delete(&account(), "ava@example.com").await.unwrap();

    assert!(matches!(deleted, Deletion::UserMustDelete { .. }));
    assert_eq!(host.addresses(), ["ava@example.com"]);
}

#[tokio::test]
async fn a_delivered_message_is_fetched_after_the_cursor() {
    let transport = FakeMailTransport::default();

    let mut session = transport.login(&credential()).await.unwrap();
    let state = session.select(INBOX).await.unwrap();
    let cursor = state.cursor_here();
    assert_eq!(transport.logins(), ["ava@example.com"]);

    let id = transport.deliver("ben@example.com", "Invoice", "The invoice is ready.");
    let (found, next) = session.fetch_since(&cursor).await.unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);
    assert_eq!(found[0].subject, "Invoice");
    assert_eq!(next.last_uid, id.uid);

    let message = session.fetch(&id).await.unwrap();
    assert_eq!(message.text, "The invoice is ready.");

    // The same cursor answers nothing a second time.
    let (again, _) = session.fetch_since(&next).await.unwrap();
    assert!(again.is_empty());
}

#[tokio::test]
async fn mail_that_arrived_before_the_cursor_wakes_nobody() {
    let transport = FakeMailTransport::default();
    transport.deliver(
        "ben@example.com",
        "Older",
        "Sent before the mailbox was assigned.",
    );

    let mut session = transport.login(&credential()).await.unwrap();
    let cursor = session.select(INBOX).await.unwrap().cursor_here();
    let (found, _) = session.fetch_since(&cursor).await.unwrap();

    assert!(found.is_empty());
}

#[tokio::test]
async fn a_pending_idle_returns_when_a_message_arrives() {
    let transport = Arc::new(FakeMailTransport::default());
    let mut session = transport.login(&credential()).await.unwrap();
    session.select(INBOX).await.unwrap();

    let idling = tokio::spawn(async move {
        let outcome = session.idle(Duration::from_secs(30)).await.unwrap();
        (outcome, session)
    });
    // Let the task reach the wait before the message arrives.
    tokio::task::yield_now().await;
    transport.deliver("ben@example.com", "Now", "This wakes the collector.");

    let (outcome, _session) = idling.await.unwrap();
    assert_eq!(outcome, IdleOutcome::Changed);
}

#[tokio::test]
async fn an_idle_turn_ends_at_the_deadline() {
    let transport = FakeMailTransport::default();
    let mut session = transport.login(&credential()).await.unwrap();
    session.select(INBOX).await.unwrap();

    let outcome = session.idle(Duration::from_millis(10)).await.unwrap();

    assert_eq!(outcome, IdleOutcome::Deadline);
    assert!(transport.capabilities().idle);
}

#[tokio::test]
async fn a_transport_without_idle_declares_it_absent() {
    let transport = FakeMailTransport::with_capabilities(TransportCapabilities { idle: false });

    assert!(!transport.capabilities().idle);
}

#[tokio::test]
async fn a_sent_message_is_captured() {
    let transport = FakeMailTransport::default();
    let mut session = transport.login(&credential()).await.unwrap();

    let mut message = pagis_mail::OutgoingMessage {
        to: vec!["ben@example.com".to_string()],
        subject: "Re: Invoice".to_string(),
        body: "It is paid.".to_string(),
        ..Default::default()
    };
    message.in_reply_to = Some("<1@fake.invalid>".to_string());
    let sent = session.send(&message).await.unwrap();

    assert!(!sent.message_id.is_empty());
    assert_eq!(transport.sent(), [message]);
}

#[tokio::test]
async fn flags_and_the_archive_move_are_recorded() {
    let transport = FakeMailTransport::default();
    let id = transport.deliver("ben@example.com", "Invoice", "The invoice is ready.");
    let mut session = transport.login(&credential()).await.unwrap();

    session
        .set_flags(
            &id,
            FlagChange {
                seen: Some(true),
                flagged: Some(true),
            },
        )
        .await
        .unwrap();
    let flags = transport.flags_of(&id).unwrap();
    assert!(flags.seen && flags.flagged);

    session.move_to_archive(&id).await.unwrap();
    assert!(transport.summaries(INBOX).is_empty());
    assert_eq!(transport.summaries(ARCHIVE).len(), 1);
    assert_eq!(
        session.fetch(&id).await.unwrap_err().0,
        TransportErrorCode::StaleId
    );
}

#[tokio::test]
async fn search_reads_the_selected_folder() {
    let transport = FakeMailTransport::default();
    transport.deliver("ben@example.com", "Invoice", "The invoice is ready.");
    transport.deliver("cat@example.com", "Lunch", "Are you free today?");
    let mut session = transport.login(&credential()).await.unwrap();
    session.select(INBOX).await.unwrap();

    let found = session
        .search(&MailQuery {
            from: Some("ben@".to_string()),
            limit: 25,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subject, "Invoice");
}

#[tokio::test]
async fn a_renumbered_folder_makes_the_cursor_stale() {
    let transport = FakeMailTransport::default();
    let mut session = transport.login(&credential()).await.unwrap();
    let cursor = session.select(INBOX).await.unwrap().cursor_here();

    transport.renumber(INBOX);

    assert_eq!(
        session.fetch_since(&cursor).await.unwrap_err().0,
        TransportErrorCode::StaleId
    );
}

#[tokio::test]
async fn a_refused_login_carries_the_scripted_code() {
    let transport = FakeMailTransport::default();
    transport.refuse_login_with(Some(TransportErrorCode::Unauthorized));

    let refused = match transport.login(&credential()).await {
        Ok(_) => panic!("a refused login must not open a session"),
        Err(refused) => refused,
    };
    assert_eq!(refused.0, TransportErrorCode::Unauthorized);

    transport.refuse_login_with(None);
    let mut session = transport.login(&credential()).await.unwrap();
    transport.fail_with(Some(TransportErrorCode::MailboxGone));
    assert_eq!(
        session.select(INBOX).await.unwrap_err().0,
        TransportErrorCode::MailboxGone
    );
}

#[test]
fn a_message_id_reads_as_the_folder_and_the_uid() {
    let id: MessageId = "INBOX:1234".parse().unwrap();

    assert_eq!(id, MessageId::new(INBOX, 1234));
    assert_eq!(id.to_string(), "INBOX:1234");
    assert!("INBOX".parse::<MessageId>().is_err());
    assert!(":12".parse::<MessageId>().is_err());
}
