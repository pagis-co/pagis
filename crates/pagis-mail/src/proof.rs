//! The login proof and the backoff the mail loops share
//! (ADR-0019).
//!
//! A host propagates a new mailbox slowly, so the first login after a
//! create is not proof that the mailbox is wrong; it is proof only
//! after the window closes. The same backoff carries the reconnect of
//! a session the host dropped.

use std::time::Duration;

use crate::transport::{
    MailSession, MailTransport, MailboxCredential, TransportError, TransportErrorCode,
};

/// The first wait after a failure.
pub const RECONNECT_BACKOFF_START: Duration = Duration::from_secs(1);
/// The longest wait between two attempts. It doubles up to here.
pub const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(300);
/// How long the first login after a host create keeps trying. Migadu
/// propagates a new mailbox in about three minutes (ADR-0019).
pub const LOGIN_PROOF_WINDOW: Duration = Duration::from_secs(180);

/// An exponential wait that doubles to a cap. It is a value, so a loop
/// holds it across attempts and resets it on the first success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    start: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(start: Duration, max: Duration) -> Self {
        Self {
            start,
            max,
            next: start,
        }
    }

    /// The reconnect backoff of a mail session: one second, doubling
    /// to five minutes.
    pub fn reconnect() -> Self {
        Self::new(RECONNECT_BACKOFF_START, RECONNECT_BACKOFF_MAX)
    }

    /// How long to wait before the next attempt, and double the wait
    /// that follows.
    pub fn wait(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(self.max);
        wait
    }

    /// Back to the first wait. A loop calls it after a success.
    pub fn reset(&mut self) {
        self.next = self.start;
    }
}

/// Whether one more attempt can change the answer. A host that
/// propagates a mailbox slowly refuses the password and reports the
/// mailbox gone until the mailbox exists; anything else is the host's
/// final word.
fn is_worth_retrying(code: TransportErrorCode) -> bool {
    matches!(
        code,
        TransportErrorCode::Unauthorized
            | TransportErrorCode::Unreachable
            | TransportErrorCode::MailboxGone
    )
}

/// Log in, and keep trying for `window` while the failure can still
/// change (ADR-0019). The session it answers with proves the mailbox
/// is `active`; the error it answers with is the reason the mailbox is
/// `unavailable`.
pub async fn prove_login(
    transport: &dyn MailTransport,
    credential: &MailboxCredential,
    window: Duration,
) -> Result<Box<dyn MailSession>, TransportError> {
    let deadline = tokio::time::Instant::now() + window;
    let mut backoff = Backoff::reconnect();
    loop {
        let error = match transport.login(credential).await {
            Ok(session) => return Ok(session),
            Err(error) => error,
        };
        let wait = backoff.wait();
        if !is_worth_retrying(error.0) || tokio::time::Instant::now() + wait >= deadline {
            return Err(error);
        }
        tracing::debug!(
            address = credential.address(),
            code = error.0.as_str(),
            wait_ms = wait.as_millis(),
            "the first login of a mailbox failed; trying again"
        );
        tokio::time::sleep(wait).await;
    }
}
