//! Where the mail crate meets the Trigger module (ADR-0019).
//!
//! The mail crate stops at two seams: it asks for the Standing Mail
//! Rule when it provisions a mailbox, and it hands each collected pass
//! to `ingest`. Both are filled here, because the Trigger module reads
//! the declarations the broker holds and is therefore built after the
//! mailbox desk.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{AgentMailbox, ChannelStore, CreatorKind, IngestBatch, now_ms};
use pagis_mail::{
    IngestError, MAIL_MESSAGE_RECEIVED, MailIngest, OWN_MAILBOX, STANDING_MAIL_INSTRUCTION,
    STANDING_MAIL_RULE_NAME, StandingMailRule, StandingRuleError,
};
use pagis_trigger::{NewSubscription, Trigger};

/// One Standing Mail Rule slot, filled once the Trigger module exists.
/// The desk provisions no mailbox before the daemon has finished
/// starting, so an empty slot is a fault and reads as one.
#[derive(Default)]
pub struct DeferredStandingRule {
    inner: std::sync::OnceLock<Arc<dyn StandingMailRule>>,
}

impl DeferredStandingRule {
    /// Fill the slot. A second call keeps the first rule maker.
    pub fn set(&self, rule: Arc<dyn StandingMailRule>) {
        let _ = self.inner.set(rule);
    }
}

#[async_trait]
impl StandingMailRule for DeferredStandingRule {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StandingRuleError> {
        match self.inner.get() {
            Some(rule) => rule.create(mailbox).await,
            None => Err(StandingRuleError(
                "the trigger module is not ready on this daemon".to_string(),
            )),
        }
    }
}

/// The Standing Mail Rule of ADR-0019: one Event Subscription on the
/// Agent's own mailbox, in its Thread with the user, written with the
/// provenance of the user.
pub struct StandingMailRules {
    trigger: Arc<Trigger>,
    channels: Arc<dyn ChannelStore>,
}

impl StandingMailRules {
    pub fn new(trigger: Arc<Trigger>, channels: Arc<dyn ChannelStore>) -> Self {
        Self { trigger, channels }
    }
}

#[async_trait]
impl StandingMailRule for StandingMailRules {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StandingRuleError> {
        let channel = self
            .channels
            .find_user_dm(&mailbox.workspace_id, &mailbox.agent_id)
            .await
            .map_err(|error| StandingRuleError(error.to_string()))?
            .ok_or_else(|| {
                StandingRuleError("that Agent has no thread with the user".to_string())
            })?;
        self.trigger
            .create_subscription(NewSubscription {
                workspace_id: mailbox.workspace_id.clone(),
                agent_id: mailbox.agent_id.clone(),
                connection_id: mailbox.connection_id.clone(),
                event_kind: MAIL_MESSAGE_RECEIVED.to_string(),
                name: STANDING_MAIL_RULE_NAME.to_string(),
                instruction: STANDING_MAIL_INSTRUCTION.to_string(),
                channel_id: channel.id,
                root_message_id: None,
                filter: serde_json::json!({"mailbox": OWN_MAILBOX}),
                // The user asked for the mailbox, so the rule is the
                // user's. The Agent may narrow it and never delete it.
                creator: CreatorKind::User,
                now: now_ms(),
            })
            .await
            .map_err(|error| StandingRuleError(error.to_string()))?;
        Ok(())
    }
}

/// The collector's `ingest` seam over the Trigger module.
pub struct TriggerIngest(pub Arc<Trigger>);

#[async_trait]
impl MailIngest for TriggerIngest {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), IngestError> {
        self.0
            .ingest(batch)
            .await
            .map(|_| ())
            .map_err(|error| IngestError(error.to_string()))
    }
}
