//! The manual host (ADR-0019): any mail server with IMAP and SMTP,
//! where the user makes the mailbox and gives Pagis its address and
//! password. There is no host API, so this host records what the user
//! typed and does nothing at the host.
//!
//! It declares `delete_mailbox` and `reset_password` absent (ADR-0019).
//! A delete forgets the record and tells the user to delete the mailbox
//! at the host. A password reset takes the password the user pasted;
//! the daemon cannot mint one.

use async_trait::async_trait;

use crate::host::{
    Deletion, HostAccount, HostCapabilities, HostError, HostErrorCode, HostedMailbox, MailboxHost,
    MailboxPassword, local_part_on,
};

#[derive(Debug, Default)]
pub struct ManualHost;

impl ManualHost {
    pub fn new() -> Self {
        Self
    }

    /// What the desk tells the user after a delete.
    pub fn delete_notice(address: &str) -> String {
        format!("Pagis forgot {address}. Delete the mailbox at your mail host.")
    }
}

#[async_trait]
impl MailboxHost for ManualHost {
    fn capabilities(&self) -> HostCapabilities {
        HostCapabilities {
            outgoing_cap: false,
            delete_mailbox: false,
            reset_password: false,
        }
    }

    /// Record the mailbox the user made. `outgoing_cap` is the
    /// daemon's alone, because there is no host to set it at.
    async fn create(
        &self,
        account: &HostAccount,
        local_part: &str,
        password: &MailboxPassword,
        _outgoing_cap: u32,
    ) -> Result<HostedMailbox, HostError> {
        if local_part.is_empty() || password.expose().is_empty() {
            return Err(HostError(HostErrorCode::Refused));
        }
        Ok(HostedMailbox::new(local_part, account.domain()))
    }

    async fn delete(&self, account: &HostAccount, address: &str) -> Result<Deletion, HostError> {
        local_part_on(account, address)?;
        Ok(Deletion::UserMustDelete {
            notice: Self::delete_notice(address),
        })
    }

    /// Take the password the user pasted. The mailbox proves it at the
    /// next login (ADR-0019).
    async fn reset_password(
        &self,
        account: &HostAccount,
        address: &str,
        password: &MailboxPassword,
    ) -> Result<(), HostError> {
        local_part_on(account, address)?;
        if password.expose().is_empty() {
            return Err(HostError(HostErrorCode::Refused));
        }
        Ok(())
    }

    /// A manual host has no directory to read. The Address Ledger is
    /// the record of what Pagis knows (ADR-0019).
    async fn list(&self, _account: &HostAccount) -> Result<Vec<HostedMailbox>, HostError> {
        Ok(Vec::new())
    }
}
