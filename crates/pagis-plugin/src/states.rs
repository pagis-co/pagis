//! Installed Plugin Package states and their in-memory comparison.

use async_trait::async_trait;
use pagis_core::{StoreError, WorkspaceId};

use crate::PackageFiles;

/// The installed states of each Plugin. State ids are opaque to callers.
#[async_trait]
pub trait PluginStates: Send + Sync {
    /// Write the files as one state and answer its id.
    async fn write(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
        files: &PackageFiles,
        message: &str,
    ) -> Result<String, StoreError>;
    /// Read the files of the named state, independent of any checkout.
    async fn read(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
        state: &str,
    ) -> Result<PackageFiles, StoreError>;
    /// Remove every state and stored file of one Plugin.
    async fn remove(&self, workspace_id: &WorkspaceId, plugin: &str) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
}

/// The files an update changes, in path order.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffSummary {
    pub files: Vec<ChangedFile>,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn between(old: &PackageFiles, new: &PackageFiles) -> Self {
        let paths: std::collections::BTreeSet<_> =
            old.iter().chain(new.iter()).map(|(path, _)| path).collect();
        let files = paths
            .into_iter()
            .filter_map(|path| {
                let status = match (old.get(path), new.get(path)) {
                    (None, Some(_)) => ChangeStatus::Added,
                    (Some(_), None) => ChangeStatus::Deleted,
                    (Some(before), Some(after)) if before != after => ChangeStatus::Modified,
                    _ => return None,
                };
                Some(ChangedFile {
                    path: path.to_string_lossy().into_owned(),
                    status,
                })
            })
            .collect();
        Self { files }
    }
}
