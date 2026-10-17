//! Workspace Mailbox intake and deterministic Mail Triage outcomes.
use std::sync::Arc;

use futures::StreamExt;
use pagis_core::*;
use pagis_mail::{MailboxDesk, MailboxError, WorkspaceMailStore};
use pagis_needs_you::PersonActivity;
use pagis_trigger::{NewSubscription, Trigger};
use tokio_util::sync::CancellationToken;

pub struct MailTriagePoll {
    stores: Stores,
    desk: Arc<MailboxDesk>,
    trigger: Arc<Trigger>,
    activity: Arc<PersonActivity>,
    clock: Arc<dyn Clock>,
    gates: std::sync::Mutex<
        std::collections::HashMap<WorkspaceId, Arc<tokio::sync::Mutex<Option<String>>>>,
    >,
}

impl MailTriagePoll {
    pub fn new(
        stores: Stores,
        desk: Arc<MailboxDesk>,
        trigger: Arc<Trigger>,
        activity: Arc<PersonActivity>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            stores,
            desk,
            trigger,
            activity,
            clock,
            gates: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub async fn poll_workspace(&self, workspace: &WorkspaceId) -> Result<(), MailboxError> {
        let gate = self
            .gates
            .lock()
            .expect("Mail Triage poll locks")
            .entry(workspace.clone())
            .or_default()
            .clone();
        let mut repair_after = gate.lock().await;
        let Some(mailbox) = self.stores.workspace_mailboxes.get(workspace).await? else {
            return Ok(());
        };
        let Some(workspace) = self.stores.workspaces.get(workspace).await? else {
            return Ok(());
        };
        self.reconcile_pending(&workspace.id, &mut repair_after)
            .await?;
        let mut store = self.desk.open_workspace(&workspace.id).await?;
        let result = self.poll(&workspace, &mailbox, store.as_mut()).await;
        let close = store.close().await;
        result?;
        close?;
        Ok(())
    }

    async fn poll(
        &self,
        workspace: &Workspace,
        mailbox: &WorkspaceMailbox,
        store: &mut dyn WorkspaceMailStore,
    ) -> Result<(), MailboxError> {
        let cursor = match self
            .stores
            .workspace_mailboxes
            .triage_cursor(&workspace.id)
            .await?
        {
            Some(cursor) => cursor,
            None => {
                let head = store.folder_state("INBOX").await?.cursor_here();
                self.stores
                    .workspace_mailboxes
                    .set_triage_cursor(&workspace.id, &head)
                    .await?;
                return Ok(());
            }
        };
        let (ids, next) = match store.messages_since(&cursor).await {
            Ok(batch) => batch,
            Err(pagis_mail::TransportError(pagis_mail::TransportErrorCode::StaleId)) => {
                let head = store.folder_state("INBOX").await?.cursor_here();
                self.stores
                    .workspace_mailboxes
                    .set_triage_cursor(&workspace.id, &head)
                    .await?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let rules = self
            .stores
            .workspace_mailboxes
            .triage_rules(&workspace.id)
            .await?;
        for id in ids {
            let message = match store.message(&id).await {
                Ok(message) => message,
                // A move or quarantine can hide a message after the batch read.
                Err(pagis_mail::TransportError(pagis_mail::TransportErrorCode::StaleId)) => {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let summary = &message.message.summary;
            let assessment = summary
                .assessment
                .as_ref()
                .ok_or_else(|| MailboxError::Conflict("Mail intake has no Spam Verdict.".into()))?;
            if assessment.junk
                || self
                    .stores
                    .workspace_mailboxes
                    .triage(&workspace.id, summary.assessment_key())
                    .await?
                    .is_some()
            {
                continue;
            }
            let sender = pagis_mail::bare_address(&summary.from).unwrap_or_default();
            let rule = rules.iter().find(|rule| rule.matches(&sender));
            let reason =
                if let Some(rule) = rule.filter(|rule| rule.action == MailTriageAction::NeedsYou) {
                    Some(format!("Mail Triage Rule {}: always show me.", rule.sender))
                } else if workspace.kind == WorkspaceKind::Private
                    && !workspace.delegate_everything
                    && rule.is_none()
                    && workspace.user_id.as_ref().is_some_and(|person| {
                        self.activity
                            .hold_until(person, self.clock.now_ms())
                            .is_some()
                    })
                {
                    Some("The Person was active.".into())
                } else {
                    None
                };
            if let Some(note) = reason {
                self.system_outcome(&workspace.id, summary.assessment_key(), id.as_str(), &note)
                    .await?;
                continue;
            }
            let chief = if let Some(id) = &workspace.chief_of_staff_agent_id {
                self.stores
                    .agents
                    .get(&workspace.id, id)
                    .await?
                    .filter(|a| a.status == AgentStatus::Active)
            } else {
                None
            };
            let Some(chief) = chief else {
                self.system_outcome(
                    &workspace.id,
                    summary.assessment_key(),
                    id.as_str(),
                    "There is no active Chief of Staff.",
                )
                .await?;
                continue;
            };
            if self
                .stores
                .grants
                .live_for_resource(
                    &workspace.id,
                    &chief.id,
                    Grant::WORKSPACE_MAILBOX_KIND,
                    workspace.id.as_str(),
                )
                .await?
                .is_none()
            {
                self.system_outcome(
                    &workspace.id,
                    summary.assessment_key(),
                    id.as_str(),
                    "The Chief of Staff has no Workspace Mailbox Grant.",
                )
                .await?;
                continue;
            }
            self.wake(workspace, mailbox, &chief.id, summary).await?;
        }
        self.stores
            .workspace_mailboxes
            .set_triage_cursor(&workspace.id, &next)
            .await?;
        Ok(())
    }

    async fn wake(
        &self,
        workspace: &Workspace,
        mailbox: &WorkspaceMailbox,
        chief: &AgentId,
        summary: &pagis_mail::MessageSummary,
    ) -> Result<(), MailboxError> {
        let subscription = self
            .stores
            .subscriptions
            .system_for_source(
                &workspace.id,
                chief,
                &EventSource::connection(mailbox.connection_id.clone()),
                pagis_mail::MAIL_MESSAGE_RECEIVED,
                "Workspace Mailbox triage",
            )
            .await?;
        let subscription = if let Some(subscription) = subscription {
            subscription
        } else {
            let channel = self
                .stores
                .channels
                .find_user_dm(&workspace.id, chief)
                .await?
                .ok_or_else(|| {
                    MailboxError::Conflict("The Chief of Staff has no direct Channel.".into())
                })?;
            self.trigger
                .create_subscription(NewSubscription {
                    created_by: Author::System,
                    workspace_id: workspace.id.clone(),
                    agent_id: chief.clone(),
                    source: EventSource::connection(mailbox.connection_id.clone()),
                    event_kind: pagis_mail::MAIL_MESSAGE_RECEIVED.into(),
                    name: "Workspace Mailbox triage".into(),
                    instruction: concat!(
                        "Read each Workspace Mailbox message with workspace_mail_get_message. ",
                        "The sender's Trust Tier states its authority. ",
                        "Handle it within the Person's rules and your Grants. ",
                        "Record exactly one outcome for each message with workspace_mail_triage ",
                        "and a short note. Use needs_you only when the Person must act. ",
                        "An assigned outcome names an active Agent of this Workspace ",
                        "with a Workspace Mailbox Grant. Use existing mail tools ",
                        "to file or reply before you record the outcome.",
                    )
                    .into(),
                    channel_id: channel.id,
                    root_message_id: None,
                    filter: serde_json::json!({"mailbox": "workspace"}),
                    now: self.clock.now_ms().saturating_sub(1),
                })
                .await
                .map_err(trigger_error)?
        };
        let assessment = summary.assessment.as_ref().expect("intake precedes triage");
        let event = NormalizedEvent {
            provider_event_id: pagis_mail::provider_event_id(&mailbox.address, summary),
            metadata: serde_json::json!({
                "mailbox": "workspace",
                "message_id": summary.id.as_str(),
                "triage_key": summary.assessment_key(),
                "thread_id": summary.thread_id,
                "from": summary.from,
                "to": summary.to,
                "subject": summary.subject,
                "received_at": summary.date,
                "trust_tier": assessment.trust_tier.as_str(),
                "spam_verdict":assessment.verdict,
            }),
            // Intake time includes mail whose Date header is absent or old.
            occurred_at: self.clock.now_ms(),
            landing: None,
        };
        let ingested = self
            .trigger
            .ingest(IngestBatch {
                workspace_id: workspace.id.clone(),
                source: EventSource::connection(mailbox.connection_id.clone()),
                agent_id: Some(chief.clone()),
                event_kind: pagis_mail::MAIL_MESSAGE_RECEIVED.into(),
                cursor: None,
                events: vec![event],
                received_at: self.clock.now_ms(),
                baseline: false,
            })
            .await
            .map_err(trigger_error)?;
        for wakeup in ingested
            .created
            .iter()
            .chain(&ingested.combined)
            .filter(|wakeup| wakeup.rule.subscription_id() == Some(&subscription.id))
        {
            self.stores
                .workspace_mailboxes
                .put_pending_triage(
                    &workspace.id,
                    &PendingMailTriage {
                        key: summary.assessment_key().into(),
                        message_id: summary.id.to_string(),
                        wakeup_id: wakeup.id.clone(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn system_outcome(
        &self,
        workspace: &WorkspaceId,
        key: &str,
        id: &str,
        note: &str,
    ) -> Result<(), MailboxError> {
        self.desk
            .record_triage(
                workspace,
                key,
                &MailTriage {
                    outcome: MailTriageOutcome::NeedsYou,
                    note: note.into(),
                    author: Author::System,
                    assignee: None,
                    recorded_at: self.clock.now_ms(),
                    resolved_at: None,
                    message_id: id.into(),
                },
            )
            .await
    }

    pub async fn finish_run(&self, run: &Run) -> Result<(), MailboxError> {
        if !run.state.is_terminal() || run.trigger_kind != TriggerKind::Event {
            return Ok(());
        }
        let Some(reference) = &run.trigger_ref else {
            return Ok(());
        };
        let pending = self
            .stores
            .workspace_mailboxes
            .pending_triage(
                &run.workspace_id,
                Some(&WakeupId::from(reference.clone())),
                None,
                50,
            )
            .await?;
        for item in pending {
            self.finish_pending(&run.workspace_id, &item, Some(run))
                .await?;
        }
        Ok(())
    }

    async fn finish_pending(
        &self,
        workspace: &WorkspaceId,
        pending: &PendingMailTriage,
        run: Option<&Run>,
    ) -> Result<(), MailboxError> {
        let has_outcome = self
            .stores
            .workspace_mailboxes
            .triage(workspace, &pending.key)
            .await?
            .is_some();
        if !has_outcome && !run.is_some_and(|run| run.state.is_terminal()) {
            return Ok(());
        }
        if !has_outcome {
            self.system_outcome(
                workspace,
                &pending.key,
                &pending.message_id,
                "The Chief of Staff recorded no outcome.",
            )
            .await?;
        }
        self.stores
            .workspace_mailboxes
            .remove_pending_triage(workspace, &pending.key)
            .await?;
        Ok(())
    }

    async fn reconcile_pending(
        &self,
        workspace: &WorkspaceId,
        after: &mut Option<String>,
    ) -> Result<(), MailboxError> {
        let pending = self
            .stores
            .workspace_mailboxes
            .pending_triage(workspace, None, after.as_deref(), 50)
            .await?;
        *after = if pending.len() == 50 {
            pending.last().map(|item| item.key.clone())
        } else {
            None
        };
        for item in pending {
            let run = match self
                .stores
                .triggers
                .get_wakeup(workspace, &item.wakeup_id)
                .await?
            {
                Some(wakeup) => match wakeup.run_id {
                    Some(id) => self.stores.runs.get(workspace, &id).await?,
                    None => None,
                },
                None => None,
            };
            self.finish_pending(workspace, &item, run.as_ref()).await?;
        }
        Ok(())
    }

    /// Listen before the daemon accepts work. The poll also repairs a missed event.
    pub async fn spawn(self: &Arc<Self>, bus: Arc<dyn EventBus>, cancel: CancellationToken) {
        let mut events = bus.subscribe(EventScope::Installation, None).await;
        let triage = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    Some(event) = events.next() => {
                        if event.event_type != "run.state_changed" {
                            continue;
                        }
                        let Some(id) = event.run_id else {
                            continue;
                        };
                        let result = async {
                            if let Some(run) = triage.stores.runs.get(&event.workspace_id, &id).await? {
                                triage.finish_run(&run).await?;
                            }
                            Ok::<_, MailboxError>(())
                        }.await;
                        if let Err(error) = result {
                            tracing::warn!(%error, "Mail Triage completion failed");
                        }
                    }
                }
            }
        });
    }
}

fn trigger_error(error: pagis_trigger::TriggerError) -> MailboxError {
    MailboxError::Conflict(error.to_string())
}
