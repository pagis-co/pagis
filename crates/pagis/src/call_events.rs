//! Where the telephony crate meets the Trigger module (ADR-0020).
//!
//! The telephony crate stops at two seams: it asks for the Standing
//! Call Rule when a Call settles, and it hands each settled
//! call to `ingest`. Both are filled here, because the Trigger module
//! reads the declarations the broker holds and is therefore built
//! after the number desk. It is the mail pair next door, for calls.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{AgentId, ChannelStore, EventSubscriptionState, IngestBatch, PhoneNumber};
use pagis_telephony::{
    CALL_ENDED, CallEvents, CallIngestError, STANDING_CALL_INSTRUCTION, STANDING_CALL_RULE_NAME,
    StandingCallRule, StandingRuleError,
};
use pagis_trigger::{NewSubscription, Trigger};

/// The most rules one Agent is read through while the Standing Call
/// Rule is looked for. The Person can archive an existing rule.
const RULE_PAGE: u32 = 200;

/// One Standing Call Rule slot, filled once the Trigger module exists.
/// A Call starts after the daemon fills the slot. An empty slot is a fault.
#[derive(Default)]
pub struct DeferredStandingCallRule {
    inner: std::sync::OnceLock<Arc<dyn StandingCallRule>>,
}

impl DeferredStandingCallRule {
    /// Fill the slot. A second call keeps the first rule maker.
    pub fn set(&self, rule: Arc<dyn StandingCallRule>) {
        let _ = self.inner.set(rule);
    }

    fn rule(&self) -> Result<&Arc<dyn StandingCallRule>, StandingRuleError> {
        self.inner.get().ok_or_else(|| {
            StandingRuleError("the trigger module is not ready on this daemon".to_string())
        })
    }
}

#[async_trait]
impl StandingCallRule for DeferredStandingCallRule {
    async fn create(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
    ) -> Result<(), StandingRuleError> {
        self.rule()?.create(number, agent_id).await
    }
}

/// The Standing Call Rule of ADR-0020: one Event Subscription on the
/// Workspace Number, in the holder's Thread with the Person (ADR-0022),
/// written with the provenance of the user.
pub struct StandingCallRules {
    trigger: Arc<Trigger>,
    channels: Arc<dyn ChannelStore>,
}

impl StandingCallRules {
    pub fn new(trigger: Arc<Trigger>, channels: Arc<dyn ChannelStore>) -> Self {
        Self { trigger, channels }
    }
}

#[async_trait]
impl StandingCallRule for StandingCallRules {
    async fn create(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
    ) -> Result<(), StandingRuleError> {
        let rules = self
            .trigger
            .list_subscriptions(&number.workspace_id, Some(agent_id), None, RULE_PAGE)
            .await
            .map_err(|error| StandingRuleError(error.to_string()))?;
        if rules.iter().any(|rule| {
            rule.event_kind == CALL_ENDED
                && rule.source.connection_id() == Some(&number.connection_id)
                && rule.state != EventSubscriptionState::Archived
        }) {
            return Ok(());
        }
        let channel = self
            .channels
            .find_user_dm(&number.workspace_id, agent_id)
            .await
            .map_err(|error| StandingRuleError(error.to_string()))?
            .ok_or_else(|| {
                StandingRuleError("that Agent has no thread with your Person".to_string())
            })?;
        self.trigger
            .create_subscription(NewSubscription {
                created_by: pagis_core::Author::System,
                workspace_id: number.workspace_id.clone(),
                agent_id: agent_id.clone(),
                source: pagis_core::EventSource::connection(number.connection_id.clone()),
                event_kind: CALL_ENDED.to_string(),
                name: STANDING_CALL_RULE_NAME.to_string(),
                instruction: STANDING_CALL_INSTRUCTION.to_string(),
                channel_id: channel.id,
                root_message_id: None,
                // Every call the line answers, whoever called. The
                // tier gates what the words are worth and never
                // whether the Agent wakes (ADR-0021); a `min_trust`
                // the user or the Agent writes is what narrows it.
                filter: serde_json::json!({}),
                // The daemon supplies this rule for the Person.
                // The Agent may narrow it but cannot delete it.
                now: number.created_at.saturating_sub(1),
            })
            .await
            .map_err(|error| StandingRuleError(error.to_string()))?;
        Ok(())
    }
}

/// The settled call's `ingest` seam over the Trigger module.
pub struct TriggerCallIngest(pub Arc<Trigger>);

#[async_trait]
impl CallEvents for TriggerCallIngest {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), CallIngestError> {
        self.0
            .ingest(batch)
            .await
            .map(|_| ())
            .map_err(|error| CallIngestError(error.to_string()))
    }
}

/// One settled-call slot, filled once the Trigger module exists. A
/// call that ends before the daemon has finished starting cannot reach
/// a rule, so it says so rather than dropping the call in silence.
#[derive(Default)]
pub struct DeferredCallEvents {
    inner: std::sync::OnceLock<Arc<dyn CallEvents>>,
}

impl DeferredCallEvents {
    /// Fill the slot. A second call keeps the first sink.
    pub fn set(&self, events: Arc<dyn CallEvents>) {
        let _ = self.inner.set(events);
    }
}

#[async_trait]
impl CallEvents for DeferredCallEvents {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), CallIngestError> {
        match self.inner.get() {
            Some(events) => events.ingest(batch).await,
            None => Err(CallIngestError(
                "the trigger module is not ready on this daemon".to_string(),
            )),
        }
    }
}
