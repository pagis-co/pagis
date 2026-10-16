//! Workspace Mailbox provisioning and access use the supplied store.
use super::*;
use crate::WorkspaceMailStore;
use pagis_core::WorkspaceMailbox;

impl MailboxDesk {
    pub fn workspace_mailbox_records(&self) -> &dyn pagis_core::WorkspaceMailboxRecordStore {
        self.workspace_mailboxes.as_ref()
    }

    pub async fn reconcile_workspace_grants(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<(), MailboxError> {
        for event in pagis_core::reconcile_workspace_mailbox_grants(
            workspace,
            self.intake.workspaces.as_ref(),
            self.agents.as_ref(),
            self.workspace_mailboxes.as_ref(),
            self.grants.as_ref(),
        )
        .await?
        {
            self.bus.publish(event).await?;
        }
        Ok(())
    }

    /// An Agent reads the Workspace Mailbox only under a live Grant.
    pub async fn require_workspace_grant(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
    ) -> Result<(), MailboxError> {
        let active = self
            .agents
            .get(workspace, agent)
            .await?
            .is_some_and(|a| a.status == pagis_core::AgentStatus::Active);
        if !active
            || self
                .grants
                .live_for_resource(
                    workspace,
                    agent,
                    pagis_core::Grant::WORKSPACE_MAILBOX_KIND,
                    workspace.as_str(),
                )
                .await?
                .is_none()
        {
            return Err(MailboxError::Conflict(
                "no live Workspace Mailbox Grant".into(),
            ));
        }
        Ok(())
    }

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
        let (connection, settings) = self.connected_provider(&request.connection_id).await?;
        let local_part = validate_local_part(&request.local_part).map_err(validation)?;
        let address = mailbox_address(&local_part, &settings.domain);
        if self.mailboxes.address_taken(&address).await? {
            return Err(MailboxError::Conflict(format!(
                "{address} is in the address ledger. Choose another name."
            )));
        }
        let mailbox = WorkspaceMailbox {
            workspace_id: workspace.clone(),
            connection_id: connection.id.clone(),
            address,
            spam_mode: pagis_core::MailScoringMode::ScoreOnly,
            outgoing_cap: self.outgoing_cap(request.outgoing_cap)?,
            created_at: now_ms(),
        };
        let request = NewMailbox {
            local_part,
            ..request
        };
        let context = self.workspace_read_context(&mailbox, &connection, None);
        let mut store = self
            .workspace_mail
            .provision(
                &connection,
                &mailbox,
                &request,
                context,
                self.workspace_mailboxes.as_ref(),
            )
            .await?;
        let _ = store.close().await;
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
        self.reconcile_workspace_grants(workspace).await?;
        Ok(mailbox)
    }

    pub async fn send_workspace(
        &self,
        workspace: &WorkspaceId,
        store: &mut dyn WorkspaceMailStore,
        message: &crate::OutgoingMessage,
    ) -> Result<crate::SentMessage, MailboxError> {
        let day = now_ms() / 86_400_000;
        if !self
            .workspace_mailboxes
            .reserve_send(workspace, day)
            .await?
        {
            return Err(MailboxError::Conflict("outgoing_cap_reached".into()));
        }
        let result = store.send(message).await;
        if result.as_ref().is_err_and(|error| {
            matches!(
                error.0,
                crate::TransportErrorCode::Rejected
                    | crate::TransportErrorCode::Unauthorized
                    | crate::TransportErrorCode::InvalidRequest
            )
        }) {
            self.workspace_mailboxes
                .release_send(workspace, day)
                .await?;
        }
        Ok(result?)
    }

    pub async fn open_workspace(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        self.open_workspace_for(workspace, None).await
    }

    pub async fn open_workspace_for_agent(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        self.require_workspace_grant(workspace, agent).await?;
        self.open_workspace_for(workspace, Some(agent.clone()))
            .await
    }

    async fn open_workspace_for(
        &self,
        workspace: &WorkspaceId,
        agent: Option<AgentId>,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let mailbox = self
            .workspace_mailboxes
            .get(workspace)
            .await?
            .ok_or(MailboxError::NotFound)?;
        let (connection, _) = self.connected_provider(&mailbox.connection_id).await?;
        let context = self.workspace_read_context(&mailbox, &connection, agent);
        self.workspace_mail
            .open(&connection, &mailbox, context)
            .await
    }

    fn workspace_read_context(
        &self,
        mailbox: &WorkspaceMailbox,
        connection: &Connection,
        agent: Option<AgentId>,
    ) -> crate::MailReadContext {
        crate::MailReadContext {
            intake: self.intake.clone(),
            workspace: mailbox.workspace_id.clone(),
            address: mailbox.address.clone(),
            agent,
            authserv_ids: crate::authserv_ids(&connection.provider)
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}
