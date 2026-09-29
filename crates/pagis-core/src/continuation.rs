//! Durable working context for one Agent conversation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentId, ChannelId, MemoryExposure, MessageId, StoreError, UnixMillis, WorkspaceId};

/// The Agent-owned conversation that one checkpoint continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationKey {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
}

/// Facts about work that the next model request must preserve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationState {
    pub active_goal: Vec<String>,
    pub constraints: Vec<String>,
    pub corrections: Vec<String>,
    pub accepted_decisions: Vec<String>,
    pub proposals: Vec<String>,
    pub completed_work: Vec<String>,
    pub failed_attempts: Vec<String>,
    pub open_questions: Vec<String>,
    pub next_steps: Vec<String>,
    pub references: Vec<String>,
}

/// One committed replacement for the older source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationCheckpoint {
    pub key: ContinuationKey,
    pub revision: u32,
    pub after_exclusive: Option<MessageId>,
    pub through_inclusive: MessageId,
    pub state: ContinuationState,
    pub source_message_ids: Vec<MessageId>,
    pub exposures: Vec<MemoryExposure>,
    pub created_at: UnixMillis,
}

#[async_trait]
pub trait ContinuationStore: Send + Sync {
    async fn get(
        &self,
        key: &ContinuationKey,
    ) -> Result<Option<ContinuationCheckpoint>, StoreError>;

    /// Replace the checkpoint only when its revision still equals
    /// `expected_revision`. `None` creates the first checkpoint.
    async fn replace(
        &self,
        checkpoint: &ContinuationCheckpoint,
        expected_revision: Option<u32>,
    ) -> Result<bool, StoreError>;
}
