//! The stdio servers and logs supplied by the daemon or an Embedder.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{PluginId, WorkspaceId};

use crate::{Session, Spawn, ToolListChanged};

/// Connect a stdio session and read its Plugin's log.
#[async_trait]
pub trait StdioServers: Send + Sync {
    async fn connect(
        &self,
        workspace: &WorkspaceId,
        plugin: &PluginId,
        server: &str,
        spawn: &Spawn,
        changed: Arc<dyn ToolListChanged>,
    ) -> Result<Arc<dyn Session>, String>;
    async fn tail(&self, workspace: &WorkspaceId, plugin: &PluginId, limit: usize) -> String;
    async fn remove(&self, plugin: &PluginId);
}

/// An Embedder without stdio servers refuses their freeze and start.
pub struct NoStdioServers;

#[async_trait]
impl StdioServers for NoStdioServers {
    async fn connect(
        &self,
        _: &WorkspaceId,
        _: &PluginId,
        _: &str,
        _: &Spawn,
        _: Arc<dyn ToolListChanged>,
    ) -> Result<Arc<dyn Session>, String> {
        Err("stdio Plugin servers are not available".into())
    }
    async fn tail(&self, _: &WorkspaceId, _: &PluginId, _: usize) -> String {
        String::new()
    }
    async fn remove(&self, _: &PluginId) {}
}

/// The root of the read-only Plugin checkouts inside the Plugin
/// Computer. One directory for each installed Plugin of the tenant,
/// which is what `${PLUGIN_ROOT}` expands to.
pub const CONTAINER_PLUGIN_ROOT: &str = "/opt/pagis-plugins";
/// The root of the writable Plugin directories inside the Plugin
/// Computer, which is what `${PLUGIN_DATA}` expands to. It sits in the
/// container's own volume, under the home of the unprivileged uid that
/// runs the servers, so it survives a stop and no host directory has to
/// be writable for a plugin.
pub const CONTAINER_PLUGIN_DATA: &str = "/data/agent/plugins";

/// Where one Plugin's two directories are inside the Plugin Computer.
pub fn container_paths(plugin_id: &str) -> pagis_plugin::PluginPaths {
    pagis_plugin::PluginPaths {
        // No repository is reachable from inside the container: the
        // daemon owns the git store.
        repository: std::path::PathBuf::from(CONTAINER_PLUGIN_ROOT).join(plugin_id),
        root: std::path::PathBuf::from(CONTAINER_PLUGIN_ROOT).join(plugin_id),
        data: std::path::PathBuf::from(CONTAINER_PLUGIN_DATA).join(plugin_id),
    }
}
