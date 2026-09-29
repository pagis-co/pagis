//! The failed-attempt count of each Workspace's Keypad Code (ADR-0021).
//!
//! One row holds the count of one Workspace, and a Workspace with no
//! row has no failure. A failure reads the row and writes the next
//! count in one write transaction, so two failures on two lines of the
//! Workspace both count. The rule of the next count is
//! [`KeypadFailures::after_failure`], which the Postgres store applies
//! too.

use async_trait::async_trait;
use pagis_core::{KeypadFailureStore, KeypadFailures, StoreError, UnixMillis, WorkspaceId};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteKeypadFailureStore {
    pool: SqlitePool,
}

impl SqliteKeypadFailureStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn failures_from(row: &sqlx::sqlite::SqliteRow) -> Result<KeypadFailures, StoreError> {
    let failed_attempts: i64 = row.get("failed_attempts");
    Ok(KeypadFailures {
        failed_attempts: u32::try_from(failed_attempts).map_err(|_| {
            StoreError::Corrupt(format!(
                "{failed_attempts} is not a count of failed attempts"
            ))
        })?,
        suspended_until: row.get("suspended_until"),
    })
}

const SELECT: &str =
    "SELECT failed_attempts, suspended_until FROM keypad_failures WHERE workspace_id = ?";

#[async_trait]
impl KeypadFailureStore for SqliteKeypadFailureStore {
    async fn get(&self, workspace_id: &WorkspaceId) -> Result<KeypadFailures, StoreError> {
        let row = sqlx::query(SELECT)
            .bind(workspace_id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.as_ref()
            .map(failures_from)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<KeypadFailures, StoreError> {
        // `BEGIN IMMEDIATE` holds the write lock from the read to the
        // commit, so no other failure reads the count in between.
        let mut tx = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let current = sqlx::query(SELECT)
            .bind(workspace_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?
            .as_ref()
            .map(failures_from)
            .transpose()?
            .unwrap_or_default();
        let next = current.after_failure(now);
        sqlx::query(
            "INSERT INTO keypad_failures \
             (workspace_id, failed_attempts, suspended_until, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT (workspace_id) DO UPDATE SET \
             failed_attempts = excluded.failed_attempts, \
             suspended_until = excluded.suspended_until, \
             updated_at = excluded.updated_at",
        )
        .bind(workspace_id.as_str())
        .bind(i64::from(next.failed_attempts))
        .bind(next.suspended_until)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(next)
    }

    async fn clear(&self, workspace_id: &WorkspaceId) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM keypad_failures WHERE workspace_id = ?")
            .bind(workspace_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
