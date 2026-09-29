//! Bounded access to original conversation evidence.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    AgentId, ChannelId, MemoryExposure, MessageId, RunId, StoreError, UnixMillis, WorkspaceId,
};

/// The conversation of the authenticated Run. Tool arguments never
/// construct this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationScope {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArtifactRef {
    pub reference: String,
    pub available: bool,
    pub filename: Option<String>,
    pub mime: Option<String>,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationEvidenceHit {
    pub kind: String,
    pub reference: String,
    pub message_id: MessageId,
    pub speaker: String,
    pub created_at: UnixMillis,
    pub snippet: String,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationEvidenceMessage {
    pub reference: String,
    pub message_id: MessageId,
    pub speaker: String,
    pub created_at: UnixMillis,
    pub text: String,
    pub artifacts: Vec<EvidenceArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedToolEvidence {
    pub scope: ConversationScope,
    pub source_message_id: MessageId,
    pub run_id: RunId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub complete: bool,
    pub exposure: MemoryExposure,
    pub created_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationToolEvidence {
    pub kind: String,
    pub reference: String,
    pub source_message_id: MessageId,
    pub tool_name: String,
    pub created_at: UnixMillis,
    pub content: String,
    pub complete: bool,
}

#[async_trait]
pub trait ConversationEvidenceStore: Send + Sync {
    /// Search permitted message text in immutable message-id order.
    async fn search(
        &self,
        scope: &ConversationScope,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceHit>, StoreError>;

    /// Read `(after_exclusive, through_inclusive]`, bounded by `limit`.
    async fn read_range(
        &self,
        scope: &ConversationScope,
        after_exclusive: Option<&MessageId>,
        through_inclusive: &MessageId,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceMessage>, StoreError>;

    /// Read one stable message reference inside the same permitted scope.
    async fn read_message(
        &self,
        scope: &ConversationScope,
        message_id: &MessageId,
    ) -> Result<Option<ConversationEvidenceMessage>, StoreError>;

    async fn retain_tool(&self, evidence: &RetainedToolEvidence) -> Result<String, StoreError>;

    async fn read_tool(
        &self,
        scope: &ConversationScope,
        reference: &str,
    ) -> Result<Option<ConversationToolEvidence>, StoreError>;
}
