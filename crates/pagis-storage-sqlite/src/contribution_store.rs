//! The Contributions of a Workspace (ADR-0016).
//!
//! A record opens once and closes once. The patch is text in the row,
//! not an Artifact: the author reads it as a message, and the record
//! is what the desk shows later.

use async_trait::async_trait;
use pagis_core::{
    Contribution, ContributionId, ContributionStatus, ContributionStore, RunId, SoftwarePackageId,
    StoreError, UnixMillis, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteContributionStore {
    pool: SqlitePool,
}

impl SqliteContributionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, package_id, base_version, latest_at_open, \
     fork_package_id, fork_version, patch, summary, status, outcome_reason, created_at, \
     closed_at, run_id";

fn contribution_from(row: &sqlx::sqlite::SqliteRow) -> Result<Contribution, StoreError> {
    let status: String = row.get("status");
    Ok(Contribution {
        id: ContributionId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        package_id: SoftwarePackageId::from(row.get::<String, _>("package_id")),
        base_version: row.get("base_version"),
        latest_at_open: row.get("latest_at_open"),
        fork_package_id: SoftwarePackageId::from(row.get::<String, _>("fork_package_id")),
        fork_version: row.get("fork_version"),
        patch: row.get("patch"),
        summary: row.get("summary"),
        status: ContributionStatus::parse(&status)
            .ok_or_else(|| StoreError::Corrupt(format!("contribution status {status:?}")))?,
        outcome_reason: row.get("outcome_reason"),
        created_at: row.get("created_at"),
        closed_at: row.get("closed_at"),
        run_id: RunId::from(row.get::<String, _>("run_id")),
    })
}

#[async_trait]
impl ContributionStore for SqliteContributionStore {
    async fn create(&self, contribution: &Contribution) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO contributions ({COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(contribution.id.as_str())
        .bind(contribution.workspace_id.as_str())
        .bind(contribution.package_id.as_str())
        .bind(&contribution.base_version)
        .bind(&contribution.latest_at_open)
        .bind(contribution.fork_package_id.as_str())
        .bind(&contribution.fork_version)
        .bind(&contribution.patch)
        .bind(&contribution.summary)
        .bind(contribution.status.as_str())
        .bind(contribution.outcome_reason.as_deref())
        .bind(contribution.created_at)
        .bind(contribution.closed_at)
        .bind(contribution.run_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
    ) -> Result<Option<Contribution>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM contributions WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(contribution_from).transpose()
    }

    async fn list_by_fork(
        &self,
        workspace_id: &WorkspaceId,
        fork_package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM contributions \
             WHERE fork_package_id = ? AND workspace_id = ? ORDER BY created_at, id"
        ))
        .bind(fork_package_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(contribution_from).collect()
    }

    async fn list_by_package(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM contributions \
             WHERE package_id = ? AND workspace_id = ? ORDER BY created_at, id"
        ))
        .bind(package_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(contribution_from).collect()
    }

    async fn close(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
        status: ContributionStatus,
        reason: &str,
        closed_at: UnixMillis,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE contributions SET status = ?, outcome_reason = ?, closed_at = ? \
             WHERE id = ? AND workspace_id = ?",
        )
        .bind(status.as_str())
        .bind(reason)
        .bind(closed_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
