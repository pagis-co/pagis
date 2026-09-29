//! The machines a person's sprites act on.
//!
//! A Host is a client the person runs on a machine of their own. It
//! registers over its authenticated connection and it is present while
//! that connection is open. A host action runs there and never in the
//! daemon, on a local installation or a server: the daemon is never a
//! Host.
//!
//! A Host belongs to one Workspace, which is one Person's private scope,
//! so a sprite of one person never reaches a machine of another. Every
//! read here names the Workspace, as every other record read does.
//!
//! Presence is not in this module: it is memory of the process that
//! holds the connections, and the record keeps only the last time the
//! machine was seen.

use async_trait::async_trait;

use crate::id::{HostId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// The capability a Host declares when it can run a shell command. A
/// phone-shaped client registers without it and is never offered for a
/// shell command.
pub const SHELL_CAPABILITY: &str = "shell";

/// One machine of one Person, as the client on it registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: HostId,
    /// The Workspace of the Person who owns the machine.
    pub workspace_id: WorkspaceId,
    /// What the person calls the machine. The client sends the machine's
    /// own name, and it is what every card and every list shows.
    pub name: String,
    /// The operating system family the client reported, for example
    /// `macos`, `linux`, `windows`, `ios` or `android`.
    pub platform: String,
    /// What the client can do, for example [`SHELL_CAPABILITY`].
    pub capabilities: Vec<String>,
    /// When the daemon last held a connection from this machine.
    pub last_seen_at: UnixMillis,
    pub created_at: UnixMillis,
}

impl Host {
    /// Whether the client declared one capability.
    pub fn can(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|held| held == capability)
    }
}

#[async_trait]
pub trait HostStore: Send + Sync {
    /// Register the machine and answer its record. The name is the
    /// machine's identity inside the Workspace: the same machine that
    /// connects again is the same Host, so the Grant that names it
    /// survives a restart of the client. A second registration replaces
    /// the platform and the capabilities, because the client is the
    /// authority for both, and moves the last-seen time.
    async fn register(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
        platform: &str,
        capabilities: &[String],
        at: UnixMillis,
    ) -> Result<Host, StoreError>;
    /// One Host of one Workspace. A Host of another Workspace reads as
    /// absent, so a sprite cannot reach a machine of another person.
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &HostId,
    ) -> Result<Option<Host>, StoreError>;
    /// The Hosts of one Workspace, oldest first.
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Host>, StoreError>;
    /// Remember that the machine was seen. The daemon writes it when a
    /// connection closes, so the record answers "last seen" for a
    /// machine that is no longer present. `false` when the Host is gone.
    async fn touch(&self, id: &HostId, at: UnixMillis) -> Result<bool, StoreError>;
}
