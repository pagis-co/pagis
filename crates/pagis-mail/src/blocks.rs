//! The daemon-minted `mail` block (ADR-0019, ADR-0004).
//!
//! Two moments put a mail in a Thread: an inbound mail that woke the
//! Agent, and a mail the Agent sent. Both write one system message
//! carrying one `mail` block, so the user reads what reached the Agent
//! and what left it, in the conversation it belongs to.
//!
//! The block carries the envelope alone: the mailbox, the message id,
//! the other party, the subject and the tier of the sender. The words
//! stay at the mail host, and the inspector reads them live.
//!
//! A mint that fails is logged and nothing more: a Thread line is not
//! worth losing the Wake-up that carries the mail, or a message that
//! has already left.

use std::sync::Arc;

use pagis_core::{
    AgentMailbox, AuthorKind, Block, EventBus, MailDirection, Message, MessageId, MessageStatus,
    MessageStore, NewEvent, Run, TriggerKind, TriggerStore, TrustTier, WakeupId, blocks_text,
    now_ms,
};

use crate::desk::MailboxDesk;
use crate::events::{MAIL_MESSAGE_RECEIVED, OWN_MAILBOX};

/// Everything the minter writes and reads.
pub struct MailBlockDeps {
    pub messages: Arc<dyn MessageStore>,
    pub grants: Arc<dyn pagis_core::GrantStore>,
    pub bus: Arc<dyn EventBus>,
    /// The Wake-up's own sources: the Incoming Events that woke the
    /// Agent, which carry the envelope of each message.
    pub triggers: Arc<dyn TriggerStore>,
    /// The Agent Mailbox record, for the address that labels the line.
    pub desk: Arc<MailboxDesk>,
}

/// The one path that mints a `mail` block.
pub struct MailBlocks {
    deps: MailBlockDeps,
}

impl MailBlocks {
    pub fn new(deps: MailBlockDeps) -> Self {
        Self { deps }
    }

    /// The lines of one proactive Run: one block for each inbound mail
    /// that woke the Agent. A Run woken by anything else writes none.
    pub async fn on_wake(&self, run: &Run) {
        if run.trigger_kind != TriggerKind::Event {
            return;
        }
        let Some(wakeup_id) = run.trigger_ref.clone().map(WakeupId::from) else {
            return;
        };
        let context = match self
            .deps
            .triggers
            .event_context(&run.workspace_id, &wakeup_id)
            .await
        {
            Ok(Some(context)) => context,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "reading the events of a mail wake-up failed");
                return;
            }
        };
        if context.event_kind != MAIL_MESSAGE_RECEIVED {
            return;
        }
        let own = match self
            .deps
            .desk
            .for_agent(&run.workspace_id, &run.agent_id)
            .await
        {
            Ok(mailbox) => mailbox.map(|mailbox| mailbox.address),
            Err(error) => {
                tracing::error!(%error, "reading the mailbox of a woken agent failed");
                None
            }
        };
        for event in &context.events {
            let metadata = &event.metadata;
            let block = Block::mail(
                MailDirection::Inbound,
                label(text(metadata, "mailbox"), own.as_deref()),
                text(metadata, "message_id"),
                text(metadata, "from"),
                text(metadata, "subject"),
                Some(
                    metadata["trust_tier"]
                        .as_str()
                        .and_then(|tier| tier.parse::<TrustTier>().ok())
                        .unwrap_or(TrustTier::Unknown),
                ),
            );
            let exposures = if text(metadata, "mailbox") == OWN_MAILBOX {
                Some(Vec::new())
            } else {
                match self
                    .deps
                    .grants
                    .list_live_for_agent(&run.workspace_id, &run.agent_id)
                    .await
                {
                    Ok(grants)
                        if grants.iter().any(|grant| {
                            grant.resource_kind == pagis_core::Grant::CONNECTION_KIND
                                && grant.resource_id.as_deref()
                                    == Some(event.connection_id.as_str())
                                && grant
                                    .capabilities()
                                    .iter()
                                    .any(|capability| capability == "gmail_read")
                        }) =>
                    {
                        Some(
                            grants
                                .into_iter()
                                .filter(|grant| {
                                    grant.resource_kind == pagis_core::Grant::CONNECTION_KIND
                                })
                                .map(|grant| pagis_core::MemoryExposure {
                                    grant_id: grant.id,
                                    revision: grant.revision,
                                })
                                .collect(),
                        )
                    }
                    _ => None,
                }
            };
            self.post(run, block, exposures.as_deref()).await;
        }
    }

    /// The line of one mail the Agent sent. A Run with no Thread — a
    /// Schedule, an Event Subscription with no landing — writes none.
    pub async fn on_send(
        &self,
        run: &Run,
        mailbox: &AgentMailbox,
        message_id: &str,
        to: &[String],
        subject: &str,
    ) {
        let block = Block::mail(
            MailDirection::Outbound,
            mailbox.address.clone(),
            message_id,
            to.join(", "),
            subject,
            None,
        );
        self.post(run, block, None).await;
    }

    /// Write one system message in the Run's Thread and tell the desk
    /// it is there.
    async fn post(
        &self,
        run: &Run,
        block: Block,
        exposures: Option<&[pagis_core::MemoryExposure]>,
    ) {
        let Some(channel_id) = run.channel_id.clone() else {
            return;
        };
        let now = now_ms();
        let blocks = vec![block];
        let message = Message {
            id: MessageId::generate(),
            workspace_id: run.workspace_id.clone(),
            channel_id: channel_id.clone(),
            parent_message_id: run.root_message_id.clone(),
            author_kind: AuthorKind::System,
            author_agent_id: None,
            run_id: Some(run.id.clone()),
            status: MessageStatus::Complete,
            text_content: blocks_text(&blocks),
            blocks,
            pending_id: None,
            created_at: now,
            completed_at: Some(now),
        };
        // An unstamped block reads as unverifiable, so the stamp goes in
        // with the row.
        let minted = match exposures {
            Some(exposures) => self.deps.messages.insert_stamped(&message, exposures).await,
            None => self.deps.messages.insert(&message).await,
        };
        if let Err(error) = minted {
            tracing::error!(%error, "the mail block was not minted");
            return;
        }
        let published = self
            .deps
            .bus
            .publish(NewEvent {
                workspace_id: message.workspace_id.clone(),
                event_type: "message.completed".to_string(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: Some(channel_id),
                payload: serde_json::json!({
                    "message_id": message.id.as_str(),
                    "parent_message_id": message.parent_message_id.as_ref().map(MessageId::as_str),
                    "author_kind": AuthorKind::System,
                }),
            })
            .await;
        if let Err(error) = published {
            tracing::error!(%error, "the mail block did not reach the Thread");
        }
    }
}

/// What labels the line: the Agent's own address for its own mailbox,
/// and the alias of the account for one of the user's (ADR-0019).
fn label(mailbox: String, own_address: Option<&str>) -> String {
    match mailbox == OWN_MAILBOX {
        true => own_address.unwrap_or(OWN_MAILBOX).to_string(),
        false => mailbox,
    }
}

/// One metadata field as text. Absent reads as empty: the envelope is
/// written by whoever sent the message, so nothing here is certain.
fn text(metadata: &serde_json::Value, field: &str) -> String {
    metadata[field].as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_mailbox_reads_as_the_agents_address() {
        assert_eq!(
            label(OWN_MAILBOX.to_string(), Some("ada@example.com")),
            "ada@example.com"
        );
    }

    #[test]
    fn an_account_of_the_user_reads_as_its_alias() {
        assert_eq!(label("gmail".to_string(), Some("ada@example.com")), "gmail");
    }
}
