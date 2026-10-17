//! Workspace Mailbox provisioning and access use the supplied host.
use super::*;
use crate::WorkspaceMailStore;
use pagis_core::WorkspaceMailbox;

/// A checked Workspace Mailbox request. The gate holds its address free
/// from other provisioning in this Desk until the request is used or dropped.
pub struct PreparedWorkspaceMailbox {
    connection: Connection,
    request: NewMailbox,
    address: String,
    outgoing_cap: u32,
    _provisioning: tokio::sync::OwnedMutexGuard<()>,
}

impl MailboxDesk {
    /// Read scheduling messages through the supplied store, oldest first.
    pub async fn poll_workspace_calendar(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<(), MailboxError> {
        if self.workspace_mailboxes.get(workspace).await?.is_none() {
            return Ok(());
        }
        let mut store = self.open_workspace(workspace).await?;
        let result = async {
            let junk = store.junk_folder().await?;
            for folder in ["INBOX", junk.as_str()] {
                let cursor = match self
                    .workspace_mailboxes
                    .calendar_cursor(workspace, folder)
                    .await?
                {
                    Some(cursor) => cursor,
                    None => crate::Cursor {
                        last_uid: 0,
                        ..store.folder_state(folder).await?.cursor_here()
                    },
                };
                let (messages, next) = match store.messages_since(&cursor).await {
                    Ok(batch) => batch,
                    Err(crate::TransportError(crate::TransportErrorCode::StaleId)) => {
                        let head = store.folder_state(folder).await?.cursor_here();
                        self.workspace_mailboxes
                            .set_calendar_cursor(workspace, &head)
                            .await?;
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                for id in messages {
                    match store.message(&id).await {
                        Ok(_) => {}
                        // A message can move or enter quarantine after the batch read.
                        Err(crate::TransportError(crate::TransportErrorCode::StaleId)) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                self.workspace_mailboxes
                    .set_calendar_cursor(workspace, &next)
                    .await?;
            }
            Ok::<_, MailboxError>(())
        }
        .await;
        let close = store.close().await;
        result?;
        close?;
        Ok(())
    }

    pub async fn record_triage(
        &self,
        workspace: &WorkspaceId,
        message: &str,
        triage: &pagis_core::MailTriage,
    ) -> Result<(), MailboxError> {
        self.workspace_mailboxes
            .record_triage(workspace, message, triage)
            .await?;
        self.bus
            .publish(NewEvent {
                workspace_id: workspace.clone(),
                event_type: "workspace_mail.triaged".into(),
                agent_id: match &triage.author {
                    pagis_core::Author::Agent(id) => Some(id.clone()),
                    _ => None,
                },
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({"message_id": message, "outcome":triage.outcome}),
            })
            .await?;
        Ok(())
    }

    pub async fn resolve_triage(
        &self,
        workspace: &WorkspaceId,
        key: &str,
    ) -> Result<(), MailboxError> {
        let at = now_ms();
        if self
            .workspace_mailboxes
            .resolve_triage(workspace, key, at)
            .await?
        {
            self.bus
                .publish(NewEvent {
                    workspace_id: workspace.clone(),
                    event_type: "workspace_mail.triaged".into(),
                    agent_id: None,
                    run_id: None,
                    channel_id: None,
                    payload: serde_json::json!({"message_id": key, "resolved_at": at}),
                })
                .await?;
        }
        Ok(())
    }

    pub async fn read_quarantine(
        &self,
        entry: &pagis_core::JunkMessage,
    ) -> Result<crate::WorkspaceMailMessage, MailboxError> {
        if self
            .workspace_mailboxes
            .get(&entry.workspace_id)
            .await?
            .is_some_and(|m| m.address == entry.mailbox)
        {
            let mut store = self.open_workspace(&entry.workspace_id).await?;
            let result = store
                .quarantined_message(&crate::MessageId::new(entry.host_id.clone()))
                .await;
            let _ = store.close().await;
            return Ok(result?);
        }
        let mailbox = self
            .mailboxes
            .list(&entry.workspace_id)
            .await?
            .into_iter()
            .find(|m| m.address == entry.mailbox)
            .ok_or(MailboxError::NotFound)?;
        let mut session = self.open_session(&mailbox).await?;
        let result = async {
            let id = crate::MessageId::new(entry.host_id.clone());
            session
                .select(&id.as_str().parse::<crate::ImapMessageId>()?.folder)
                .await?;
            let mut message = session.fetch(&id).await?;
            self.read_context(&mailbox)
                .await
                .map_err(|_| crate::TransportError(crate::TransportErrorCode::Unreachable))?
                .assess(&mut message.summary, false)
                .await?;
            let flags = session.metadata(&id).await?;
            Ok::<_, crate::TransportError>(crate::WorkspaceMailMessage {
                labels: flags.labels.clone(),
                flags,
                message,
            })
        }
        .await;
        let _ = session.logout().await;
        Ok(result?)
    }

    pub fn junk(&self) -> &dyn pagis_core::JunkStore {
        self.intake.junk.as_ref()
    }

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

    /// Check the mailbox before the caller creates a Person or Workspace.
    pub async fn prepare_workspace(
        &self,
        request: NewMailbox,
    ) -> Result<PreparedWorkspaceMailbox, MailboxError> {
        let gate = self.provisioning.clone().lock_owned().await;
        let (connection, settings) = self.connected_provider(&request.connection_id).await?;
        let local = validate_local_part(&request.local_part).map_err(validation)?;
        let address = mailbox_address(&local, &settings.domain);
        let outgoing_cap = self.outgoing_cap(request.outgoing_cap)?;
        let request = NewMailbox {
            local_part: local.clone(),
            ..request
        };
        let held = self.workspace_mail.check(&connection, &request).await?;
        if self.mailboxes.address_taken(&address).await?
            || held
                .iter()
                .any(|mailbox| mailbox.address.eq_ignore_ascii_case(&address))
        {
            return Err(MailboxError::Conflict(
                self.conflict_message(&local, &settings.domain, &held)
                    .await?,
            ));
        }
        Ok(PreparedWorkspaceMailbox {
            connection,
            request,
            address,
            outgoing_cap,
            _provisioning: gate,
        })
    }

    pub fn supports_automatic_workspace_mail(&self, connection: &Connection) -> bool {
        self.workspace_mail
            .supports_automatic_provisioning(connection)
    }

    pub async fn provision_workspace(
        &self,
        workspace: &WorkspaceId,
        prepared: PreparedWorkspaceMailbox,
        author: pagis_core::Author,
    ) -> Result<WorkspaceMailbox, MailboxError> {
        if self.workspace_mailboxes.get(workspace).await?.is_some() {
            return Err(MailboxError::Conflict(
                "this Workspace already has a Workspace Mailbox".into(),
            ));
        }
        let PreparedWorkspaceMailbox {
            connection,
            request,
            address,
            outgoing_cap,
            _provisioning,
        } = prepared;
        let mailbox = WorkspaceMailbox {
            author,
            workspace_id: workspace.clone(),
            connection_id: connection.id.clone(),
            address,
            visibility: pagis_core::MailVisibility::AllMail,
            spam_mode: pagis_core::MailScoringMode::ScoreOnly,
            outgoing_cap,
            created_at: now_ms(),
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
        let head = store.folder_state("INBOX").await?.cursor_here();
        self.workspace_mailboxes
            .set_triage_cursor(workspace, &head)
            .await?;
        let _ = store.close().await;
        self.bus
            .publish(NewEvent {
                workspace_id: workspace.clone(),
                event_type: "workspace_mailbox.provisioned".into(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({"address": mailbox.address, "author": mailbox.author}),
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
        let inner = self
            .workspace_mail
            .open(&connection, &mailbox, context.clone())
            .await?;
        Ok(Box::new(crate::intake::CalendarMailStore {
            inner,
            context,
        }))
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
