use super::*;
use crate::{AgentVersion, SharedWorkspaceLimits, WorkspaceGovernanceStore};

#[async_trait::async_trait]
impl WorkspaceGovernanceStore for FakeWorkspaceGovernanceStore {
    async fn limits(&self, workspace: &WorkspaceId) -> Result<SharedWorkspaceLimits, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .limits
            .get(workspace)
            .cloned()
            .unwrap_or_default())
    }
    async fn set_limits(
        &self,
        limits: &SharedWorkspaceLimits,
        entry: &AuditEntry,
    ) -> Result<(), StoreError> {
        limits.validate()?;
        let mut data = self.0.lock().unwrap();
        if !data
            .workspaces
            .get(&entry.subject_workspace_id)
            .is_some_and(|w| w.kind == WorkspaceKind::Shared)
            || entry.action != "workspace.limits_changed"
            || !matches!(entry.actor, Author::Person(_))
        {
            return Err(conflict(
                "Shared Workspace Limits need a Shared Workspace and a Person author.",
            ));
        }
        super::audit::append(&mut data, entry)?;
        data.limits
            .insert(entry.subject_workspace_id.clone(), limits.clone());
        Ok(())
    }
    async fn record_version(
        &self,
        agent: &AgentId,
        configuration: &serde_json::Value,
        entry: &AuditEntry,
    ) -> Result<AgentVersion, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .agents
            .get(&entry.subject_workspace_id, agent)
            .is_none()
            || entry.action != "agent.changed"
            || !matches!(entry.actor, Author::Person(_))
        {
            return Err(conflict(
                "An Agent Version needs an Agent and a Person author.",
            ));
        }
        let version = AgentVersion {
            agent_id: agent.clone(),
            workspace_id: entry.subject_workspace_id.clone(),
            version: data
                .versions
                .iter()
                .filter(|v| &v.agent_id == agent && v.workspace_id == entry.subject_workspace_id)
                .map(|v| v.version)
                .max()
                .unwrap_or(0)
                + 1,
            author: entry.actor.clone(),
            configuration: configuration.clone(),
            at: entry.at,
        };
        super::audit::append(&mut data, entry)?;
        data.versions.push(version.clone());
        Ok(version)
    }
    async fn versions(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
    ) -> Result<Vec<AgentVersion>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .versions
            .iter()
            .rev()
            .filter(|v| &v.workspace_id == workspace && &v.agent_id == agent)
            .cloned()
            .collect())
    }
}
