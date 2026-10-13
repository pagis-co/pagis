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

/// The attached streams of one server process.
pub struct ServerIo {
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub stdout: Pin<Box<dyn AsyncRead + Send + Unpin>>,
    /// The server's stderr, chunk by chunk. The stdio client writes it to
    /// the Plugin's log; nothing of it reaches the model (ADR-0005).
    /// The channel is bounded, so a server that writes faster than the
    /// log takes it waits, and the stdio client reads it to its end.
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
