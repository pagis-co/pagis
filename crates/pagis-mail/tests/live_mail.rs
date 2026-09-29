//! The transport against a real mailbox at a real host. It is
//! never in the gate: it needs an account, it sends mail, and it costs
//! money. It runs only when the environment names a mailbox:
//!
//! ```text
//! PAGIS_LIVE_MAIL=1 \
//! PAGIS_LIVE_MAIL_ADDRESS=agent@example.com \
//! PAGIS_LIVE_MAIL_PASSWORD=... \
//! PAGIS_LIVE_MAIL_IMAP_HOST=imap.example.com \
//! PAGIS_LIVE_MAIL_SMTP_HOST=smtp.example.com \
//! cargo test -p pagis-mail --test main -- --ignored live_mail::
//! ```
//!
//! The IMAP port is 993 and the SMTP port 465 unless the environment
//! names others; both are the implicit-TLS ports (ADR-0019).

use std::time::Duration;

use pagis_mail::{
    IMAPS_PORT, INBOX, IdleOutcome, MailQuery, MailTransport, MailboxCredential, OutgoingMessage,
    SMTPS_PORT, StandardTransport,
};

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn port(name: &str, fallback: u16) -> u16 {
    variable(name)
        .map(|value| value.parse().expect("the port is a number"))
        .unwrap_or(fallback)
}

/// The mailbox the environment names, or nothing.
fn live_credential() -> Option<MailboxCredential> {
    variable("PAGIS_LIVE_MAIL")?;
    Some(MailboxCredential::new(
        variable("PAGIS_LIVE_MAIL_ADDRESS").expect("PAGIS_LIVE_MAIL_ADDRESS"),
        variable("PAGIS_LIVE_MAIL_PASSWORD").expect("PAGIS_LIVE_MAIL_PASSWORD"),
        pagis_mail::Endpoint::new(
            variable("PAGIS_LIVE_MAIL_IMAP_HOST").expect("PAGIS_LIVE_MAIL_IMAP_HOST"),
            port("PAGIS_LIVE_MAIL_IMAP_PORT", IMAPS_PORT),
        ),
        pagis_mail::Endpoint::new(
            variable("PAGIS_LIVE_MAIL_SMTP_HOST").expect("PAGIS_LIVE_MAIL_SMTP_HOST"),
            port("PAGIS_LIVE_MAIL_SMTP_PORT", SMTPS_PORT),
        ),
    ))
}

#[tokio::test]
#[ignore = "needs a live mailbox; set PAGIS_LIVE_MAIL and run with --ignored"]
async fn a_live_mailbox_logs_in_sends_to_itself_and_reads_the_message_back() {
    let Some(credential) = live_credential() else {
        eprintln!("PAGIS_LIVE_MAIL is not set; the live mailbox test did nothing");
        return;
    };
    let address = credential.address().to_string();

    let mut session = StandardTransport::new()
        .login(&credential)
        .await
        .expect("the live mailbox logs in");
    let state = session.select(INBOX).await.expect("the inbox opens");
    let cursor = state.cursor_here();

    let subject = format!("Pagis live test {}", pagis_core::now_ms());
    let sent = session
        .send(&OutgoingMessage {
            to: vec![address.clone()],
            subject: subject.clone(),
            body: "This message proves the live transport.".to_string(),
            ..OutgoingMessage::default()
        })
        .await
        .expect("the host accepts the message");

    // A real host delivers to itself in seconds, not instantly.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let summary = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the message never arrived"
        );
        if session
            .idle(Duration::from_secs(30))
            .await
            .expect("the turn ends")
            == IdleOutcome::Deadline
        {
            continue;
        }
        let (summaries, _) = session.fetch_since(&cursor).await.expect("the fetch runs");
        if let Some(found) = summaries
            .into_iter()
            .find(|summary| summary.subject == subject)
        {
            break found;
        }
    };

    assert_eq!(summary.from, address);
    assert_eq!(summary.thread_id, sent.message_id);

    let message = session.fetch(&summary.id).await.expect("the body reads");
    assert!(
        message.text.contains("proves the live transport"),
        "{}",
        message.text
    );

    let found = session
        .search(&MailQuery {
            subject: Some(subject),
            limit: 5,
            ..MailQuery::default()
        })
        .await
        .expect("the search runs");
    assert!(!found.is_empty(), "the search finds the message it sent");
}
