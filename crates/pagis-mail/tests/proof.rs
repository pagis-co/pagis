//! The first login after a host create, and the backoff it shares with
//! the reconnect loop (ADR-0019). The clock is paused, so the
//! three-minute window costs no time.

use std::sync::Arc;
use std::time::Duration;

use pagis_mail::fake::FakeMailTransport;
use pagis_mail::{
    Backoff, Endpoint, LOGIN_PROOF_WINDOW, MailboxCredential, RECONNECT_BACKOFF_MAX,
    RECONNECT_BACKOFF_START, TransportErrorCode, prove_login,
};

fn credential() -> MailboxCredential {
    MailboxCredential::new(
        "ava@pagis.test",
        "pw",
        Endpoint::new("mail.pagis.test", 993),
        Endpoint::new("mail.pagis.test", 465),
    )
}

#[test]
fn the_reconnect_backoff_doubles_from_one_second_to_five_minutes() {
    let mut backoff = Backoff::reconnect();
    assert_eq!(backoff.wait(), RECONNECT_BACKOFF_START);
    assert_eq!(backoff.wait(), Duration::from_secs(2));
    assert_eq!(backoff.wait(), Duration::from_secs(4));
    for _ in 0..20 {
        backoff.wait();
    }
    assert_eq!(backoff.wait(), RECONNECT_BACKOFF_MAX);
}

#[test]
fn a_success_puts_the_backoff_back_to_the_first_wait() {
    let mut backoff = Backoff::reconnect();
    backoff.wait();
    backoff.wait();
    backoff.reset();
    assert_eq!(backoff.wait(), RECONNECT_BACKOFF_START);
}

#[tokio::test(start_paused = true)]
async fn a_mailbox_the_host_has_not_propagated_yet_proves_itself_inside_the_window() {
    let transport = Arc::new(FakeMailTransport::default());
    transport.refuse_login_with(Some(TransportErrorCode::Unauthorized));
    let propagating = Arc::clone(&transport);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        propagating.refuse_login_with(None);
    });

    prove_login(transport.as_ref(), &credential(), LOGIN_PROOF_WINDOW)
        .await
        .expect("the mailbox proves itself");
    assert_eq!(transport.logins(), ["ava@pagis.test"]);
}

#[tokio::test(start_paused = true)]
async fn a_login_the_host_keeps_refusing_gives_up_at_the_window() {
    let transport = FakeMailTransport::default();
    transport.refuse_login_with(Some(TransportErrorCode::Unauthorized));

    let started = tokio::time::Instant::now();
    let error = prove_login(&transport, &credential(), LOGIN_PROOF_WINDOW)
        .await
        .err()
        .expect("the host never accepts the password");

    assert_eq!(error.0, TransportErrorCode::Unauthorized);
    assert!(
        started.elapsed() <= LOGIN_PROOF_WINDOW,
        "the proof waited {:?}, longer than the window",
        started.elapsed()
    );
    assert!(transport.logins().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_refusal_one_more_attempt_cannot_change_returns_at_once() {
    let transport = FakeMailTransport::default();
    transport.refuse_login_with(Some(TransportErrorCode::Rejected));

    let started = tokio::time::Instant::now();
    let error = prove_login(&transport, &credential(), LOGIN_PROOF_WINDOW)
        .await
        .err()
        .expect("the host refuses the mailbox");

    assert_eq!(error.0, TransportErrorCode::Rejected);
    assert_eq!(started.elapsed(), Duration::ZERO);
}
