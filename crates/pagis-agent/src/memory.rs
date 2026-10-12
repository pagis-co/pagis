//! The in-run memory staging overlay. Writes and deletes
//! stage here during the run; reads see staged content first. The run
//! settles after the reply succeeds. Reflection can inspect the staged
//! foreground state, but it records its later changes separately. A failed
//! or canceled run drops both sets. Staging in the loop keeps the store
//! stateless.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pagis_core::{
    MEMORY_INDEX_FILE, MemoryAccess, MemoryChangeset, MemoryError, MemoryIndexes, MemoryScope,
    MemoryStore, ScopedPath, WorkspaceId,
};

/// One run's staged memory state over the committed store.
#[derive(Clone, Default)]
pub(crate) struct MemoryOverlay {
    writes: BTreeMap<ScopedPath, String>,
    deletes: BTreeSet<ScopedPath>,
    base_revision: Option<Option<String>>,
    exposures: Vec<pagis_core::MemoryExposure>,
    /// Stamps of data read into the context, separate from held permissions.
    read_exposures: Vec<pagis_core::MemoryExposure>,
    /// Once Google data enters the context, Compaction cannot clear it.
    holds_google_data: bool,
}

impl MemoryOverlay {
    pub fn observe_read(&mut self, exposures: &[pagis_core::MemoryExposure]) {
        for exposure in exposures {
            if !self.read_exposures.contains(exposure) {
                self.read_exposures.push(exposure.clone());
            }
        }
    }

    pub fn observe_tool_result(&mut self, result: &pagis_broker::ToolResult) {
        self.holds_google_data |= result.metadata["holds_google_data"] == true;
    }

    /// Resolve the source of each read before a request can be captured.
    /// An unavailable source prevents capture.
    pub async fn holds_google_data(
        &mut self,
        workspace_id: &WorkspaceId,
        grants: &dyn pagis_core::GrantStore,
        connections: &dyn pagis_core::ConnectionStore,
    ) -> Result<bool, pagis_core::StoreError> {
        if self.holds_google_data {
            return Ok(true);
        }
        for exposure in &self.read_exposures {
            let grant = grants
                .get(workspace_id, &exposure.grant_id)
                .await?
                .ok_or_else(|| pagis_core::StoreError::Corrupt("read Grant".into()))?;
            if grant.resource_kind != pagis_core::Grant::CONNECTION_KIND {
                continue;
            }
            let id = grant
                .resource_id
                .ok_or_else(|| pagis_core::StoreError::Corrupt("read Connection".into()))?;
            let connection = connections
                .get(workspace_id, &id.into())
                .await?
                .ok_or_else(|| pagis_core::StoreError::Corrupt("read Connection".into()))?;
            if connection.provider == "google" {
                self.holds_google_data = true;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes.is_empty()
    }

    /// Retain every permission available to the run. Revocation cannot
    /// make staged content or an existing transcript unrestricted.
    pub fn observe(&mut self, access: &MemoryAccess) -> Result<(), MemoryError> {
        if !access.permits(&self.exposures) {
            return Err(MemoryError::Unavailable);
        }
        for exposure in access.exposures() {
            if !self.exposures.contains(exposure) {
                self.exposures.push(exposure.clone());
            }
        }
        Ok(())
    }

    /// Stage one whole-file write.
    pub fn write(&mut self, path: ScopedPath, content: String) {
        self.deletes.remove(&path);
        self.writes.insert(path, content);
    }

    /// Stage one delete.
    pub fn delete(&mut self, path: ScopedPath) {
        self.writes.remove(&path);
        self.deletes.insert(path);
    }

    /// Read through the overlay: staged content first, then the store.
    pub async fn read(
        &mut self,
        store: &Arc<dyn MemoryStore>,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        path: &ScopedPath,
    ) -> Result<String, MemoryError> {
        self.observe(access)?;
        if self.deletes.contains(path) {
            return Err(MemoryError::NotFound(path.display()));
        }
        if let Some(content) = self.writes.get(path) {
            return Ok(content.clone());
        }
        let file = store.read(workspace_id, access, path).await?;
        self.observe_read(&file.exposures);
        Ok(file.content)
    }

    /// Both indexes with staged writes applied, for the turn context.
    pub async fn indexes(
        &mut self,
        store: &Arc<dyn MemoryStore>,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
    ) -> Result<MemoryIndexes, MemoryError> {
        self.observe(access)?;
        let mut indexes = store.load_indexes(workspace_id, access).await?;
        self.base_revision.get_or_insert(indexes.revision.clone());
        self.observe_read(&indexes.exposures);
        let index_path = |scope| ScopedPath {
            scope,
            rel: MEMORY_INDEX_FILE.to_string(),
        };
        if let Some(staged) = self.writes.get(&index_path(MemoryScope::Shared)) {
            indexes.shared = staged.clone();
        }
        if let Some(staged) = self.writes.get(&index_path(MemoryScope::Private)) {
            indexes.private = staged.clone();
        }
        Ok(indexes)
    }

    /// The changeset one commit applies.
    pub fn changeset(&self) -> MemoryChangeset {
        MemoryChangeset {
            writes: self.writes.clone(),
            deletes: self.deletes.clone(),
        }
    }

    /// The state staged before Reflection starts. The run keeps this snapshot
    /// until it knows that cancellation can no longer discard the write.
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Changes that Reflection made after `earlier`.
    pub fn changes_after(&self, earlier: &Self) -> MemoryChangeset {
        let mut changes = MemoryChangeset::default();
        for (path, content) in &self.writes {
            if earlier.writes.get(path) != Some(content) || earlier.deletes.contains(path) {
                changes.writes.insert(path.clone(), content.clone());
            }
        }
        for path in &self.deletes {
            if !earlier.deletes.contains(path) || earlier.writes.contains_key(path) {
                changes.deletes.insert(path.clone());
            }
        }
        changes
    }

    pub fn base_revision(&self) -> Option<&str> {
        self.base_revision
            .as_ref()
            .and_then(|revision| revision.as_deref())
    }

    /// Move an empty overlay to a memory revision committed by another phase
    /// of the same Run. Staged foreground work keeps its original base.
    pub fn move_empty_base_to(&mut self, revision: Option<String>) -> bool {
        if !self.is_empty() {
            return false;
        }
        self.base_revision = Some(revision);
        true
    }

    pub fn access(&self, agent_id: pagis_core::AgentId) -> MemoryAccess {
        MemoryAccess::agent(agent_id, self.exposures.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> ScopedPath {
        ScopedPath::parse(p).unwrap()
    }

    #[test]
    fn a_write_after_a_delete_undoes_the_delete() {
        let mut overlay = MemoryOverlay::default();
        overlay.delete(path("shared/x.md"));
        overlay.write(path("shared/x.md"), "back".to_string());

        let changeset = overlay.changeset();
        assert!(changeset.deletes.is_empty());
        assert_eq!(changeset.writes[&path("shared/x.md")], "back");
    }

    #[test]
    fn a_delete_after_a_write_undoes_the_write() {
        let mut overlay = MemoryOverlay::default();
        overlay.write(path("shared/x.md"), "gone".to_string());
        overlay.delete(path("shared/x.md"));

        let changeset = overlay.changeset();
        assert!(changeset.writes.is_empty());
        assert!(changeset.deletes.contains(&path("shared/x.md")));
    }

    #[test]
    fn reflection_changes_exclude_unchanged_foreground_writes() {
        let mut foreground = MemoryOverlay::default();
        foreground.write(path("shared/preference.md"), "tea\n".to_string());
        let mut combined = foreground.snapshot();
        combined.write(path("shared/preference.md"), "coffee\n".to_string());
        combined.write(path("private/decision.md"), "verified\n".to_string());

        let changes = combined.changes_after(&foreground);

        assert_eq!(changes.writes.len(), 2);
        assert_eq!(changes.writes[&path("shared/preference.md")], "coffee\n");
        assert_eq!(changes.writes[&path("private/decision.md")], "verified\n");
    }

    #[test]
    fn reflection_changes_are_empty_when_it_only_reads_foreground_memory() {
        let mut foreground = MemoryOverlay::default();
        foreground.write(path("shared/preference.md"), "tea\n".to_string());

        assert!(foreground.changes_after(&foreground).is_empty());
    }
}
