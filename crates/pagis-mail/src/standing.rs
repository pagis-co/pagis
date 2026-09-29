//! The Standing Mail Rule (ADR-0019).
//!
//! Provisioning a mailbox creates one Event Subscription, so an Agent
//! with a mailbox wakes on every message that reaches it. The rule
//! lives in the Trigger module and the mailbox lives here, so the desk
//! reaches it through this seam.

use async_trait::async_trait;
use pagis_core::AgentMailbox;

/// The name the rule carries on the mailbox panel and in the Agent's
/// rule list.
pub const STANDING_MAIL_RULE_NAME: &str = "Inbound mail";

/// What the Wake-up tells the Agent to do (ADR-0019). It names the
/// tool, because the event carries the envelope and never the words.
pub const STANDING_MAIL_INSTRUCTION: &str =
    "You received mail. Read it with `mail__get_message` and act on your job.";

/// Why a Standing Mail Rule was not created. It carries the words the
/// provisioning form shows.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StandingRuleError(pub String);

/// Creates the Standing Mail Rule of one new mailbox (ADR-0019). The
/// rule watches `mailbox: own`, wakes the Agent in its Thread with the
/// user, and carries the provenance of the user, so the Agent may
/// narrow it and never delete it.
#[async_trait]
pub trait StandingMailRule: Send + Sync {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StandingRuleError>;
}
