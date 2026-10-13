//! The mail seam of a daemon that has no IMAP and SMTP client.
//!
//! This is the truth such a daemon runs on: a login never succeeds,
//! so a mailbox with IMAP and SMTP endpoints becomes `unavailable` with the
//! reason stated, and nothing pretends to read or send mail.

use async_trait::async_trait;

use crate::transport::{
    MailSession, MailTransport, MailboxCredential, TransportError, TransportErrorCode,
};

/// A transport that reaches no mail host.
#[derive(Debug, Default)]
pub struct NoMailTransport;

#[async_trait]
impl MailTransport for NoMailTransport {
    async fn login(
        &self,
        _credential: &MailboxCredential,
    ) -> Result<Box<dyn MailSession>, TransportError> {
        Err(TransportError(TransportErrorCode::Unreachable))
    }
}
