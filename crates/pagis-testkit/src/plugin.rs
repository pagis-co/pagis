//! An in-memory Plugin store. It holds the rules the SQLite
//! store holds — one name per Workspace, one Binding per field, and an
//! uninstall that revokes every live Grant on the Plugin in every
//! Workspace — so a test that drives an install needs no database.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    Grant, Plugin, PluginBinding, PluginId, PluginState, PluginStore, PluginToolStore, PluginTools,
    StoreError, WorkspaceId,
};

use crate::MemoryGrantStore;

pub struct MemoryPluginStore {
    /// Uninstall revokes the Grants on the Plugin in every Workspace,
    /// so the fake needs the Grant store the test writes.
    grants: Arc<MemoryGrantStore>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    plugins: Vec<Plugin>,
    bindings: Vec<PluginBinding>,
}

impl MemoryPluginStore {
    pub fn new(grants: Arc<MemoryGrantStore>) -> Self {
        Self {
            grants,
            state: Mutex::new(State::default()),
        }
    }
}

#[async_trait]
impl PluginStore for MemoryPluginStore {
    async fn create(&self, plugin: &Plugin, bindings: &[PluginBinding]) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("plugin store lock");
        if state
            .plugins
            .iter()
            .any(|held| held.workspace_id == plugin.workspace_id && held.name == plugin.name)
        {
            return Err(StoreError::Conflict(format!(
                "plugin name {:?} is taken",
                plugin.name
            )));
        }
        state.plugins.push(plugin.clone());
        state.bindings.extend(bindings.iter().cloned());
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Option<Plugin>, StoreError> {
        let state = self.state.lock().expect("plugin store lock");
        Ok(state
            .plugins
            .iter()
            .find(|held| &held.workspace_id == workspace_id && &held.id == id)
            .cloned())
    }

    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<Plugin>, StoreError> {
        let state = self.state.lock().expect("plugin store lock");
        Ok(state
            .plugins
            .iter()
            .find(|held| &held.workspace_id == workspace_id && held.name == name)
            .cloned())
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Plugin>, StoreError> {
        let state = self.state.lock().expect("plugin store lock");
        let mut plugins: Vec<Plugin> = state
            .plugins
            .iter()
            .filter(|held| &held.workspace_id == workspace_id)
            .cloned()
            .collect();
        plugins.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(plugins)
    }

    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        plugin_state: PluginState,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let mut state = self.state.lock().expect("plugin store lock");
        match state
            .plugins
            .iter_mut()
            .find(|held| &held.id == id && &held.workspace_id == workspace_id)
        {
            Some(plugin) => {
                plugin.state = plugin_state;
                plugin.updated_at = updated_at;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_installed(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        installed_commit: &str,
        manifest_version: &str,
        plugin_state: PluginState,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let mut state = self.state.lock().expect("plugin store lock");
        match state
            .plugins
            .iter_mut()
            .find(|held| &held.id == id && &held.workspace_id == workspace_id)
        {
            Some(plugin) => {
                plugin.installed_commit = installed_commit.to_string();
                plugin.manifest_version = manifest_version.to_string();
                plugin.state = plugin_state;
                plugin.updated_at = updated_at;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn put_binding(
        &self,
        workspace_id: &WorkspaceId,
        binding: &PluginBinding,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("plugin store lock");
        if !owns(&state.plugins, workspace_id, &binding.plugin_id) {
            return Ok(());
        }
        match state
            .bindings
            .iter_mut()
            .find(|held| held.plugin_id == binding.plugin_id && held.field == binding.field)
        {
            Some(held) => *held = binding.clone(),
            None => state.bindings.push(binding.clone()),
        }
        Ok(())
    }

    async fn list_bindings(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Vec<PluginBinding>, StoreError> {
        let state = self.state.lock().expect("plugin store lock");
        // A Binding belongs to its Plugin, so the Plugin decides whether
        // this Workspace reads it.
        if !owns(&state.plugins, workspace_id, id) {
            return Ok(Vec::new());
        }
        let mut bindings: Vec<PluginBinding> = state
            .bindings
            .iter()
            .filter(|held| &held.plugin_id == id)
            .cloned()
            .collect();
        bindings.sort_by(|left, right| left.field.cmp(&right.field));
        Ok(bindings)
    }

    async fn delete_binding(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        field: &str,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("plugin store lock");
        if !owns(&state.plugins, workspace_id, id) {
            return Ok(());
        }
        state
            .bindings
            .retain(|held| !(&held.plugin_id == id && held.field == field));
        Ok(())
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        {
            let mut state = self.state.lock().expect("plugin store lock");
            let before = state.plugins.len();
            state
                .plugins
                .retain(|held| !(&held.workspace_id == workspace_id && &held.id == id));
            if state.plugins.len() == before {
                return Ok(false);
            }
            state.bindings.retain(|held| &held.plugin_id != id);
        }
        // The Plugin is the Org's, and each person grants it in their own
        // Workspace, so the Grants of every Workspace go with it.
        self.grants
            .revoke_resource(Grant::PLUGIN_KIND, id.as_str(), revoked_at);
        Ok(true)
    }
}

/// Whether one Workspace holds the Plugin an id names.
fn owns(plugins: &[Plugin], workspace_id: &WorkspaceId, id: &PluginId) -> bool {
    plugins
        .iter()
        .any(|held| &held.id == id && &held.workspace_id == workspace_id)
}

/// An in-memory store of the frozen tool catalogs (ADR-0017).
/// One catalog belongs to one Capability Manifest version, as the
/// SQLite store holds it. This double holds no Plugin table, so it
/// cannot resolve the owner of a catalog and the Workspace it is given
/// goes unread; the SQLite store is where that scoping is proved.
#[derive(Default)]
pub struct MemoryPluginToolStore {
    catalogs: Mutex<Vec<PluginTools>>,
}

#[async_trait]
impl PluginToolStore for MemoryPluginToolStore {
    async fn put(&self, tools: &PluginTools) -> Result<(), StoreError> {
        let mut catalogs = self.catalogs.lock().expect("plugin tool store lock");
        catalogs
            .retain(|held| !(held.plugin_id == tools.plugin_id && held.version == tools.version));
        catalogs.push(tools.clone());
        Ok(())
    }

    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
    ) -> Result<Option<PluginTools>, StoreError> {
        Ok(self
            .catalogs
            .lock()
            .expect("plugin tool store lock")
            .iter()
            .find(|held| &held.plugin_id == plugin_id && held.version == version)
            .cloned())
    }

    async fn set_tools_changed(
        &self,
        _workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
        changed: bool,
    ) -> Result<(), StoreError> {
        if let Some(held) = self
            .catalogs
            .lock()
            .expect("plugin tool store lock")
            .iter_mut()
            .find(|held| &held.plugin_id == plugin_id && held.version == version)
        {
            held.tools_changed = changed;
        }
        Ok(())
    }

    async fn delete(
        &self,
        _workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
    ) -> Result<(), StoreError> {
        self.catalogs
            .lock()
            .expect("plugin tool store lock")
            .retain(|held| &held.plugin_id != plugin_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pagis_core::{
        AgentId, Grant, GrantStore, Plugin, PluginId, PluginSource, PluginState, PluginStore,
        WorkspaceId,
    };

    use super::MemoryPluginStore;
    use crate::MemoryGrantStore;

    fn plugin_grant(workspace_id: &WorkspaceId, plugin_id: &PluginId) -> Grant {
        Grant {
            resource_id: Some(plugin_id.to_string()),
            ..crate::fixture::grant(workspace_id, &AgentId::generate(), Grant::PLUGIN_KIND, &[])
        }
    }

    /// The Plugin is the Org's, and each person grants it in their own
    /// Workspace, so an uninstall revokes the Grants of every Workspace,
    /// as both backends do.
    #[tokio::test]
    async fn an_uninstall_revokes_the_grants_of_every_workspace() {
        let grants = Arc::new(MemoryGrantStore::default());
        let store = MemoryPluginStore::new(Arc::clone(&grants));
        let owner = WorkspaceId::generate();
        let other = WorkspaceId::generate();
        let plugin = Plugin {
            id: PluginId::generate(),
            workspace_id: owner.clone(),
            name: "weather".to_string(),
            source: PluginSource::Git {
                url: "https://example.com/weather.git".to_string(),
                reference: None,
            },
            installed_commit: "0".repeat(40),
            manifest_version: "v1".to_string(),
            state: PluginState::Enabled,
            created_at: 1,
            updated_at: 1,
        };
        store.create(&plugin, &[]).await.unwrap();
        let owners_grant = plugin_grant(&owner, &plugin.id);
        let others_grant = plugin_grant(&other, &plugin.id);
        grants.create(&owners_grant).await.unwrap();
        grants.create(&others_grant).await.unwrap();

        assert!(
            store
                .delete_and_revoke(&owner, &plugin.id, 9)
                .await
                .unwrap()
        );

        for (workspace_id, grant) in [(&owner, &owners_grant), (&other, &others_grant)] {
            let revoked_at = grants
                .get(workspace_id, &grant.id)
                .await
                .unwrap()
                .unwrap()
                .revoked_at;
            assert_eq!(revoked_at, Some(9), "the grant in {workspace_id}");
        }
    }
}
