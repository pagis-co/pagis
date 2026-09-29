//! Where a Plugin's MCP server process runs (ADR-0017).
//!
//! Inside the tenant's Plugin Computer, and never on the daemon host. A
//! host process has the reach of the daemon: every tenant's database,
//! every memory repository, the sealed secrets and the Docker socket. A
//! process inside the tenant's container reaches the tenant's own disk
//! and network and nothing of the daemon's.
//!
//! The seam is here and the container is behind it, so this crate knows
//! nothing about Docker: the daemon supplies the implementation.

use std::pin::Pin;

use async_trait::async_trait;
use pagis_core::WorkspaceId;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::substitute::Spawn;

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

/// The attached streams of one server process.
pub struct ServerIo {
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub stdout: Pin<Box<dyn AsyncRead + Send + Unpin>>,
    /// The server's stderr, chunk by chunk. The supervisor writes it to
    /// the Plugin's log; nothing of it reaches the model (ADR-0005).
    /// The channel is bounded, so a server that writes faster than the
    /// log takes it waits, and the supervisor reads it to its end.
    pub stderr: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

/// One tenant's server processes, inside that tenant's Plugin Computer.
#[async_trait]
pub trait ServerProcesses: Send + Sync {
    /// Start one declared server in the tenant's Plugin Computer and
    /// give back its streams. The tenant's tokens travel in this
    /// process's environment and nowhere else.
    async fn start(&self, workspace_id: &WorkspaceId, spawn: &Spawn) -> Result<ServerIo, String>;
}
