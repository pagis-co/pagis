//! An in-memory Connection store. It holds the rules the SQLite store
//! holds — one alias per Workspace, and a delete that revokes every
//! live Grant on the Connection — so a test that binds a Connection
//! needs no database.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    Connection, ConnectionId, ConnectionStore, Grant, GrantStore, SealedSecret, StoreError,
    WorkspaceId,
};

pub struct MemoryConnectionStore {
    /// Deleting a Connection revokes the Grants on it, so the fake
    /// needs the same Grant store the daemon writes.
    grants: Arc<dyn GrantStore>,
    connections: Mutex<Vec<Connection>>,
    /// The sealed refresh token of every Google Connection, keyed
    /// the way the column is. It is off [`Connection`] here for
    /// the same reason it is off the row: one caller writes it.
    refresh_tokens: Mutex<HashMap<(WorkspaceId, ConnectionId), SealedSecret>>,
}

impl MemoryConnectionStore {
    pub fn new(grants: Arc<dyn GrantStore>) -> Self {
        Self {
            grants,
            connections: Mutex::new(Vec::new()),
            refresh_tokens: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ConnectionStore for MemoryConnectionStore {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut connections = self.connections.lock().expect("connection store lock");
        if connections.iter().any(|held| {
            held.workspace_id == connection.workspace_id && held.alias == connection.alias
        }) {
            return Err(StoreError::Conflict(format!(
                "the alias {} is taken",
                connection.alias
            )));
        }
        connections.push(connection.clone());
        Ok(())
    }

    async fn set_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().expect("connection store lock");
        match connections
            .iter_mut()
            .find(|held| &held.workspace_id == workspace_id && &held.id == id)
        {
            Some(connection) => {
                connection.status = status.to_string();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_authorization(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().expect("connection store lock");
        match connections
            .iter_mut()
            .find(|held| &held.workspace_id == workspace_id && &held.id == id)
        {
            Some(connection) => {
                connection.status = Connection::CONNECTED.to_string();
                connection.authorized_capabilities = capabilities.to_vec();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_config(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().expect("connection store lock");
        match connections
            .iter_mut()
            .find(|held| &held.workspace_id == workspace_id && &held.id == id)
        {
            Some(connection) => {
                connection.config = config.clone();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        let connections = self.connections.lock().expect("connection store lock");
        Ok(connections
            .iter()
            .filter(|held| &held.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        let connections = self.connections.lock().expect("connection store lock");
        Ok(connections
            .iter()
            .find(|held| &held.workspace_id == workspace_id && &held.id == id)
            .cloned())
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        {
            let mut connections = self.connections.lock().expect("connection store lock");
            let before = connections.len();
            connections.retain(|held| !(&held.workspace_id == workspace_id && &held.id == id));
            if connections.len() == before {
                return Ok(false);
            }
        }
        // The token goes with the row: a person who disconnects leaves
        // the daemon holding nothing of theirs.
        self.refresh_tokens
            .lock()
            .expect("refresh token lock")
            .remove(&(workspace_id.clone(), id.clone()));
        for grant in self.grants.list_live(workspace_id).await? {
            if grant.resource_kind == Grant::CONNECTION_KIND
                && grant.resource_id.as_deref() == Some(id.as_str())
            {
                self.grants
                    .revoke(workspace_id, &grant.id, revoked_at)
                    .await?;
            }
        }
        Ok(true)
    }

    async fn set_refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        token: Option<&SealedSecret>,
    ) -> Result<bool, StoreError> {
        {
            let connections = self.connections.lock().expect("connection store lock");
            if !connections
                .iter()
                .any(|held| &held.workspace_id == workspace_id && &held.id == id)
            {
                return Ok(false);
            }
        }
        let mut tokens = self.refresh_tokens.lock().expect("refresh token lock");
        let key = (workspace_id.clone(), id.clone());
        match token {
            Some(token) => tokens.insert(key, token.clone()),
            None => tokens.remove(&key),
        };
        Ok(true)
    }

    async fn refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<SealedSecret>, StoreError> {
        Ok(self
            .refresh_tokens
            .lock()
            .expect("refresh token lock")
            .get(&(workspace_id.clone(), id.clone()))
            .cloned())
    }
}
