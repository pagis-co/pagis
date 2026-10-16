//! Shared Workspace Limits and the configuration history of each Agent.
use crate::{AgentId, AuditEntry, Author, StoreError, UnixMillis, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MailVisibility {
    AgentOnly,
    Escalations,
    #[default]
    AllMail,
}

impl MailVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentOnly => "agent_only",
            Self::Escalations => "escalations",
            Self::AllMail => "all_mail",
        }
    }
    /// The current Administrator limit applies to every human read.
    pub fn permits(self, escalated: bool) -> bool {
        self == Self::AllMail || (self == Self::Escalations && escalated)
    }
}
impl std::str::FromStr for MailVisibility {
    type Err = StoreError;
    fn from_str(value: &str) -> Result<Self, StoreError> {
        match value {
            "agent_only" => Ok(Self::AgentOnly),
            "escalations" => Ok(Self::Escalations),
            "all_mail" => Ok(Self::AllMail),
            _ => Err(StoreError::Corrupt("unknown Mail Visibility".into())),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SharedWorkspaceLimits {
    /// None permits all Plugins. An empty list permits none.
    pub permitted_plugins: Option<Vec<String>>,
    /// Connection ids, as named by Connection Grants.
    pub permitted_connections: Option<Vec<String>>,
    #[serde(default)]
    pub max_mail_visibility: MailVisibility,
}
impl SharedWorkspaceLimits {
    pub fn validate(&self) -> Result<(), StoreError> {
        if [&self.permitted_plugins, &self.permitted_connections]
            .iter()
            .any(|list| {
                list.as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id.trim().is_empty()))
            })
        {
            return Err(StoreError::Conflict(
                "A permitted resource must have an id.".into(),
            ));
        }
        Ok(())
    }
    pub fn check_mail_visibility(&self, selected: MailVisibility) -> Result<(), StoreError> {
        if selected > self.max_mail_visibility {
            Err(StoreError::Conflict(
                "Mail Visibility exceeds the Shared Workspace Limits.".into(),
            ))
        } else {
            Ok(())
        }
    }
    pub fn check_resource(&self, kind: &str, id: &str) -> Result<(), StoreError> {
        let list = match kind {
            crate::Grant::PLUGIN_KIND => &self.permitted_plugins,
            crate::Grant::CONNECTION_KIND => &self.permitted_connections,
            _ => return Ok(()),
        };
        if list
            .as_ref()
            .is_some_and(|ids| !ids.iter().any(|allowed| allowed == id))
        {
            Err(StoreError::Conflict(format!(
                "The {kind} {id} is outside the Shared Workspace Limits."
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentVersion {
    #[schema(value_type = String)]
    pub agent_id: AgentId,
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    pub version: i64,
    pub author: Author,
    pub configuration: serde_json::Value,
    pub at: UnixMillis,
}

#[async_trait]
pub trait WorkspaceGovernanceStore: Send + Sync {
    async fn limits(&self, workspace: &WorkspaceId) -> Result<SharedWorkspaceLimits, StoreError>;
    /// Write the limits and the Audit Entry together.
    async fn set_limits(
        &self,
        limits: &SharedWorkspaceLimits,
        entry: &AuditEntry,
    ) -> Result<(), StoreError>;
    /// Assign the next number and append the Agent Version and Audit Entry together.
    async fn record_version(
        &self,
        agent: &AgentId,
        configuration: &serde_json::Value,
        entry: &AuditEntry,
    ) -> Result<AgentVersion, StoreError>;
    /// Newest version first, scoped to the owning Workspace.
    async fn versions(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
    ) -> Result<Vec<AgentVersion>, StoreError>;
}
