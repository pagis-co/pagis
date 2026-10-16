//! The supplied storage for a Workspace Mailbox.
use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{Connection, SecretStore, WorkspaceMailbox, WorkspaceMailboxRecordStore};

use crate::{
    ImapWorkspaceMailStore, MailReadContext, MailTransport, MailboxCredential, MailboxError,
    MailboxHost, NewMailbox, WorkspaceMailStore, host_api_key_secret_name,
    mailbox_password_secret_name, mailbox_provider,
};

/// An Embedder supplies the storage and provisioning of Workspace mail.
#[async_trait]
pub trait WorkspaceMailHost: Send + Sync {
    /// Reserve the record before creating the mailbox. Release the reservation
    /// if creation fails and the address is safe to use again.
    async fn provision(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        request: &NewMailbox,
        context: MailReadContext,
        records: &dyn WorkspaceMailboxRecordStore,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError>;

    async fn open(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        context: MailReadContext,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError>;
}

pub struct ImapWorkspaceMailHost {
    host: Arc<dyn MailboxHost>,
    transport: Arc<dyn MailTransport>,
    secrets: Arc<dyn SecretStore>,
}

impl ImapWorkspaceMailHost {
    pub fn new(
        host: Arc<dyn MailboxHost>,
        transport: Arc<dyn MailTransport>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            host,
            transport,
            secrets,
        }
    }
}

#[async_trait]
impl WorkspaceMailHost for ImapWorkspaceMailHost {
    async fn provision(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        request: &NewMailbox,
        context: MailReadContext,
        records: &dyn WorkspaceMailboxRecordStore,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let settings = mailbox_provider(connection).ok_or(MailboxError::NoProvider)?;
        let password = crate::desk::checked_password(&settings, request)?;
        let (Some(imap), Some(smtp), Some(password)) = (settings.imap, settings.smtp, password)
        else {
            return Err(MailboxError::Validation(
                "this Workspace Mailbox needs an IMAP login".into(),
            ));
        };
        let host: &dyn MailboxHost = if connection.provider == crate::MANUAL_PROVIDER {
            &crate::manual::ManualHost
        } else {
            self.host.as_ref()
        };
        let api_key = if host.capabilities().api_key {
            Some(
                self.secrets
                    .get(&host_api_key_secret_name(&connection.alias))
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        MailboxError::ProviderUnavailable(Connection::UNAVAILABLE.into())
                    })?,
            )
        } else {
            None
        };
        let account = crate::HostAccount::new(
            &settings.domain,
            settings.account.unwrap_or_default(),
            api_key,
        );
        records
            .create(mailbox)
            .await
            .map_err(crate::desk::store_error)?;
        if let Err(error) = host
            .create(
                &account,
                &request.local_part,
                Some(&password),
                mailbox.outgoing_cap,
            )
            .await
        {
            records
                .drop_reservation(&mailbox.workspace_id, &mailbox.address)
                .await?;
            return Err(error.into());
        }
        if self
            .secrets
            .set(
                &mailbox_password_secret_name(&mailbox.workspace_id, &mailbox.address),
                password.expose(),
            )
            .is_err()
        {
            if host.delete(&account, &mailbox.address).await.is_ok() {
                records
                    .drop_reservation(&mailbox.workspace_id, &mailbox.address)
                    .await?;
            }
            return Err(MailboxError::Validation(
                "the daemon could not store the mailbox password".into(),
            ));
        }
        Ok(Box::new(ImapWorkspaceMailStore::connect(
            self.transport.clone(),
            MailboxCredential::new(
                mailbox.address.clone(),
                password.expose().to_string(),
                imap,
                smtp,
            ),
            context,
        )))
    }

    async fn open(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        context: MailReadContext,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let settings = mailbox_provider(connection).ok_or(MailboxError::NoProvider)?;
        let (Some(imap), Some(smtp)) = (settings.imap, settings.smtp) else {
            return Err(MailboxError::Validation(
                "this Workspace Mailbox needs an IMAP login".into(),
            ));
        };
        let password = self
            .secrets
            .get(&mailbox_password_secret_name(
                &mailbox.workspace_id,
                &mailbox.address,
            ))
            .ok()
            .flatten()
            .ok_or_else(|| {
                MailboxError::Conflict("the Workspace Mailbox password is unavailable".into())
            })?;
        let credential = MailboxCredential::new(mailbox.address.clone(), password, imap, smtp);
        Ok(Box::new(ImapWorkspaceMailStore::new(
            self.transport.login(&credential).await?,
            context,
        )))
    }
}
