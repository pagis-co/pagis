//! Mail Assignments belong to the Shared Workspace that holds the message.
use crate::{AgentId, ChannelId, MailAssignmentId, StoreError, UnixMillis, UserId, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailAssignmentRole {
    Decide,
    Approve,
    Fyi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailAssignmentState {
    Assigned,
    Handled,
    Escalated,
    TimedOut,
}
impl MailAssignmentState {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Assigned | Self::Escalated)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Handled => "handled",
            Self::Escalated => "escalated",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MailAssignment {
    #[schema(value_type = String)]
    pub id: MailAssignmentId,
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    pub message_key: String,
    pub message_id: String,
    #[schema(value_type = String)]
    pub assignee: UserId,
    #[schema(value_type = String)]
    pub private_workspace_id: WorkspaceId,
    pub role: MailAssignmentRole,
    pub state: MailAssignmentState,
    #[schema(value_type = String)]
    pub assigning_agent_id: AgentId,
    #[schema(value_type = Option<String>)]
    pub assigning_run_id: Option<crate::RunId>,
    #[schema(value_type = String)]
    pub channel_id: ChannelId,
    pub summary: String,
    pub note: String,
    pub due_at: UnixMillis,
    pub created_at: UnixMillis,
    pub changed_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MailAssignmentSettings {
    pub assignment_limit: Option<u32>,
    pub assignment_timeout_minutes: u32,
}
impl Default for MailAssignmentSettings {
    fn default() -> Self {
        Self {
            assignment_limit: None,
            assignment_timeout_minutes: 1440,
        }
    }
}
impl MailAssignmentSettings {
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.assignment_limit == Some(0) || self.assignment_timeout_minutes == 0 {
            return Err(StoreError::Conflict(
                "Use a positive assignment limit and timeout.".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
pub trait MailAssignmentStore: Send + Sync {
    async fn find(&self, id: &MailAssignmentId) -> Result<Option<MailAssignment>, StoreError>;
    async fn by_channel(&self, channel: &ChannelId) -> Result<Option<MailAssignment>, StoreError>;
    async fn settings(&self, workspace: &WorkspaceId)
    -> Result<MailAssignmentSettings, StoreError>;
    async fn set_settings(
        &self,
        workspace: &WorkspaceId,
        settings: &MailAssignmentSettings,
    ) -> Result<(), StoreError>;
    /// Create the assignment rows, Channels and participants in one write.
    /// Check the open assignment limit in that same write.
    async fn create_batch(
        &self,
        workspace: &WorkspaceId,
        assignments: &[MailAssignment],
    ) -> Result<(), StoreError>;
    async fn list(
        &self,
        workspace: &WorkspaceId,
        message: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError>;
    async fn for_person(
        &self,
        person: &UserId,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError>;
    /// Assigned rows only, oldest due time first. The id breaks ties.
    async fn overdue(&self, at: UnixMillis, limit: u32) -> Result<Vec<MailAssignment>, StoreError>;
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
    ) -> Result<Option<MailAssignment>, StoreError>;
    async fn update(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
        expected: MailAssignmentState,
        state: MailAssignmentState,
        note: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
}
