//! An in-memory Grant store. It holds the rules the SQLite store
//! holds — one live Grant per Agent and resource, and a revocation
//! that happens once — so a test that drives Grants needs no database.

use std::sync::Mutex;

use async_trait::async_trait;
use pagis_core::{AgentId, Grant, GrantId, GrantStore, StoreError, WorkspaceId};

#[derive(Default)]
pub struct MemoryGrantStore {
    grants: Mutex<Vec<Grant>>,
}

impl MemoryGrantStore {
    /// Revoke every live Grant on one resource, in every Workspace. It
    /// is the one statement a backend runs when the resource goes, so
    /// a fake store that deletes a resource revokes through it.
    pub fn revoke_resource(&self, resource_kind: &str, resource_id: &str, revoked_at: i64) {
        for grant in self
            .grants
            .lock()
            .expect("grant store lock")
            .iter_mut()
            .filter(|grant| {
                grant.revoked_at.is_none()
                    && grant.resource_kind == resource_kind
                    && grant.resource_id.as_deref() == Some(resource_id)
            })
        {
            grant.revoked_at = Some(revoked_at);
        }
    }

    fn live(&self) -> Vec<Grant> {
        self.grants
            .lock()
            .expect("grant store lock")
            .iter()
            .filter(|grant| grant.revoked_at.is_none())
            .cloned()
            .collect()
    }
}

#[async_trait]
impl GrantStore for MemoryGrantStore {
    async fn create(&self, grant: &Grant) -> Result<(), StoreError> {
        let mut grants = self.grants.lock().expect("grant store lock");
        if grants.iter().any(|held| {
            held.revoked_at.is_none()
                && held.agent_id == grant.agent_id
                && held.resource_kind == grant.resource_kind
                && held.resource_id == grant.resource_id
        }) {
            return Err(StoreError::Conflict(format!(
                "the agent already holds a live {} grant",
                grant.resource_kind
            )));
        }
        grants.push(grant.clone());
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
    ) -> Result<Option<Grant>, StoreError> {
        Ok(self
            .grants
            .lock()
            .expect("grant store lock")
            .iter()
            .find(|grant| &grant.id == id && &grant.workspace_id == workspace_id)
            .cloned())
    }

    async fn list_live(&self, workspace_id: &WorkspaceId) -> Result<Vec<Grant>, StoreError> {
        Ok(self
            .live()
            .into_iter()
            .filter(|grant| &grant.workspace_id == workspace_id)
            .collect())
    }

    async fn live_grant(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
    ) -> Result<Option<Grant>, StoreError> {
        Ok(self.live().into_iter().find(|grant| {
            &grant.workspace_id == workspace_id
                && &grant.agent_id == agent_id
                && grant.resource_kind == resource_kind
        }))
    }

    async fn list_live_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Vec<Grant>, StoreError> {
        Ok(self
            .live()
            .into_iter()
            .filter(|grant| &grant.workspace_id == workspace_id && &grant.agent_id == agent_id)
            .collect())
    }

    async fn live_for_resource(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<Option<Grant>, StoreError> {
        Ok(self.live().into_iter().find(|grant| {
            &grant.workspace_id == workspace_id
                && &grant.agent_id == agent_id
                && grant.resource_kind == resource_kind
                && grant.resource_id.as_deref() == Some(resource_id)
        }))
    }

    async fn set_scope(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        scope: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut grants = self.grants.lock().expect("grant store lock");
        match grants.iter_mut().find(|grant| {
            &grant.id == id && &grant.workspace_id == workspace_id && grant.revoked_at.is_none()
        }) {
            Some(grant) => {
                grant.scope = scope.clone();
                grant.revision += 1;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        let mut grants = self.grants.lock().expect("grant store lock");
        match grants.iter_mut().find(|grant| {
            &grant.id == id && &grant.workspace_id == workspace_id && grant.revoked_at.is_none()
        }) {
            Some(grant) => {
                grant.revoked_at = Some(revoked_at);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
