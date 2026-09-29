//! An in-memory Contribution store (ADR-0016), so a test that
//! drives a fork and a contribute-back needs no database.

use std::sync::Mutex;

use async_trait::async_trait;
use pagis_core::{
    Contribution, ContributionId, ContributionStatus, ContributionStore, SoftwarePackageId,
    StoreError, UnixMillis, WorkspaceId,
};

#[derive(Default)]
pub struct MemoryContributionStore {
    contributions: Mutex<Vec<Contribution>>,
}

#[async_trait]
impl ContributionStore for MemoryContributionStore {
    async fn create(&self, contribution: &Contribution) -> Result<(), StoreError> {
        self.contributions
            .lock()
            .expect("contribution store lock")
            .push(contribution.clone());
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
    ) -> Result<Option<Contribution>, StoreError> {
        Ok(self
            .contributions
            .lock()
            .expect("contribution store lock")
            .iter()
            .find(|held| &held.id == id && &held.workspace_id == workspace_id)
            .cloned())
    }

    async fn list_by_fork(
        &self,
        workspace_id: &WorkspaceId,
        fork_package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        Ok(self
            .contributions
            .lock()
            .expect("contribution store lock")
            .iter()
            .filter(|held| {
                &held.fork_package_id == fork_package_id && &held.workspace_id == workspace_id
            })
            .cloned()
            .collect())
    }

    async fn list_by_package(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        Ok(self
            .contributions
            .lock()
            .expect("contribution store lock")
            .iter()
            .filter(|held| &held.package_id == package_id && &held.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    async fn close(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
        status: ContributionStatus,
        reason: &str,
        closed_at: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut contributions = self.contributions.lock().expect("contribution store lock");
        if let Some(held) = contributions
            .iter_mut()
            .find(|held| &held.id == id && &held.workspace_id == workspace_id)
        {
            held.status = status;
            held.outcome_reason = Some(reason.to_string());
            held.closed_at = Some(closed_at);
        }
        Ok(())
    }
}
