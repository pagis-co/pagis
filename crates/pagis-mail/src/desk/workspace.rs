//! Workspace Mailbox provisioning and access use the Agent Mailbox host seams.
use super::*;
use crate::{ImapWorkspaceMailStore, WorkspaceMailStore};
use pagis_core::WorkspaceMailbox;

impl MailboxDesk {
    pub async fn workspace_mailbox(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Option<WorkspaceMailbox>, MailboxError> {
        Ok(self.workspace_mailboxes.get(workspace).await?)
    }

    pub async fn provision_workspace(
        &self,
        workspace: &WorkspaceId,
        request: NewMailbox,
    ) -> Result<WorkspaceMailbox, MailboxError> {
        if self.workspace_mailboxes.get(workspace).await?.is_some() {
            return Err(MailboxError::Conflict(
                "this Workspace already has a Workspace Mailbox".into(),
            ));
        }
        let checked = self.checked(&request).await?;
        if checked.settings.imap.is_none()
            || checked.settings.smtp.is_none()
            || checked.password.is_none()
        {
            return Err(MailboxError::Validation(
                "this Workspace Mailbox needs an IMAP login".into(),
            ));
        }
        let host = self.host_of(&checked.connection);
        let account = self.host_account(&checked.connection, &checked.settings)?;
        let mailbox = WorkspaceMailbox {
            workspace_id: workspace.clone(),
            connection_id: checked.connection.id,
            address: checked.address,
            created_at: now_ms(),
        };
        self.workspace_mailboxes
            .create(&mailbox)
            .await
            .map_err(store_error)?;
        if let Err(error) = host
            .create(
                &account,
                &checked.local_part,
                checked.password.as_ref(),
                checked.outgoing_cap,
            )
            .await
        {
            self.workspace_mailboxes
                .drop_reservation(workspace, &mailbox.address)
                .await?;
            return Err(error.into());
        }
        let password = checked
            .password
            .as_ref()
            .expect("the checked mailbox has a password");
        if self
            .secrets
            .set(
                &mailbox_password_secret_name(workspace, &mailbox.address),
                password.expose(),
            )
            .is_err()
        {
            if host.delete(&account, &mailbox.address).await.is_ok() {
                self.workspace_mailboxes
                    .drop_reservation(workspace, &mailbox.address)
                    .await?;
            }
            return Err(MailboxError::Validation(
                "the daemon could not store the mailbox password".into(),
            ));
        }
        self.bus
            .publish(NewEvent {
                workspace_id: workspace.clone(),
                event_type: "workspace_mailbox.provisioned".into(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({"address": mailbox.address}),
            })
            .await?;
        Ok(mailbox)
    }

    pub async fn open_workspace(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let mailbox = self
            .workspace_mailboxes
            .get(workspace)
            .await?
            .ok_or(MailboxError::NotFound)?;
        let (_, settings) = self.connected_provider(&mailbox.connection_id).await?;
        let (Some(imap), Some(smtp)) = (settings.imap, settings.smtp) else {
            return Err(MailboxError::Validation(
                "this Workspace Mailbox needs an IMAP login".into(),
            ));
        };
        let password = self
            .secrets
            .get(&mailbox_password_secret_name(workspace, &mailbox.address))
            .ok()
            .flatten()
            .ok_or_else(|| {
                MailboxError::Conflict("the Workspace Mailbox password is unavailable".into())
            })?;
        let credential = MailboxCredential::new(mailbox.address, password, imap, smtp);
        Ok(Box::new(ImapWorkspaceMailStore::new(
            self.transport.login(&credential).await?,
        )))
    }
}
