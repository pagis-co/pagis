//! Shared Workspace access, separate from Org roles.
use crate::{Author, StoreError, UserId, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Private,
    Shared,
}
impl WorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Shared => "shared",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    Reader,
    Responder,
    Manager,
}
impl WorkspaceRole {
    /// Every Shared Workspace member has the Team tier.
    pub fn trust_tier(self) -> crate::TrustTier {
        crate::TrustTier::Team
    }

    /// Only a Manager can instruct the Shared Workspace's Agents.
    pub fn can_instruct(self) -> bool {
        self == Self::Manager
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Responder => "responder",
            Self::Manager => "manager",
        }
    }
}
impl std::str::FromStr for WorkspaceRole {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "reader" => Ok(Self::Reader),
            "responder" => Ok(Self::Responder),
            "manager" => Ok(Self::Manager),
            _ => Err(format!("invalid Workspace role: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub workspace_id: WorkspaceId,
    pub person_id: UserId,
    pub role: WorkspaceRole,
    pub author: Author,
}

#[async_trait]
pub trait WorkspaceMemberStore: Send + Sync {
    /// Create the Shared Workspace and its first Manager in one transaction.
    async fn create_workspace(
        &self,
        workspace: &crate::Workspace,
        manager: &WorkspaceMember,
    ) -> Result<(), StoreError>;
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<WorkspaceMember>, StoreError>;
    async fn get(
        &self,
        workspace: &WorkspaceId,
        person: &UserId,
    ) -> Result<Option<WorkspaceMember>, StoreError>;
    async fn add(&self, member: &WorkspaceMember) -> Result<(), StoreError>;
    /// A last Manager cannot be demoted or removed. Each change is atomic.
    async fn change(&self, member: &WorkspaceMember) -> Result<bool, StoreError>;
    async fn remove(&self, workspace: &WorkspaceId, person: &UserId) -> Result<bool, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_authority_follows_the_member_role() {
        for (role, instructions) in [
            (WorkspaceRole::Manager, true),
            (WorkspaceRole::Responder, false),
            (WorkspaceRole::Reader, false),
        ] {
            assert_eq!(role.trust_tier(), crate::TrustTier::Team);
            assert_eq!(role.can_instruct(), instructions);
        }
        assert_eq!(
            "team".parse::<crate::TrustTier>().unwrap(),
            crate::TrustTier::Team
        );
    }
}
