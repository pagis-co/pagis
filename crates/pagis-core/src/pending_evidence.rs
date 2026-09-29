use serde::{Deserialize, Serialize};

use crate::{
    AgentId, ChannelId, MemoryExposure, MessageId, PendingEvidenceId, Run, RunId, WorkspaceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingUrgency {
    Normal,
    Urgent,
}

impl PendingUrgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Urgent => "urgent",
        }
    }
}

impl std::str::FromStr for PendingUrgency {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "normal" => Ok(Self::Normal),
            "urgent" => Ok(Self::Urgent),
            other => Err(format!("unknown pending evidence urgency: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingEvidenceState {
    Pending,
    Leased,
    Completed,
    Failed,
    Invalidated,
}

impl PendingEvidenceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Invalidated => "invalidated",
        }
    }
}

impl std::str::FromStr for PendingEvidenceState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "leased" => Ok(Self::Leased),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "invalidated" => Ok(Self::Invalidated),
            other => Err(format!("unknown pending evidence state: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEvidence {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub subject: String,
    pub after_exclusive: Option<MessageId>,
    pub through_inclusive: MessageId,
    pub source_message_ids: Vec<MessageId>,
    pub exposures: Vec<MemoryExposure>,
    pub reason: String,
    pub urgency: PendingUrgency,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEvidenceRecord {
    pub id: PendingEvidenceId,
    pub evidence: PendingEvidence,
    pub eligible_at: i64,
    pub maximum_due_at: i64,
    pub attempt_count: u32,
    pub state: PendingEvidenceState,
    pub revision: u32,
    pub lease_run_id: Option<RunId>,
    pub error: Option<String>,
    pub memory_revision: Option<String>,
    pub completed_at: Option<i64>,
    pub failed_overlap_id: Option<PendingEvidenceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReviewClaim {
    pub evidence: PendingEvidenceRecord,
    pub run: Run,
    /// The queue revision the worker must present when it settles.
    pub lease_revision: u32,
}

impl std::ops::Deref for PendingEvidenceRecord {
    type Target = PendingEvidence;

    fn deref(&self) -> &Self::Target {
        &self.evidence
    }
}
