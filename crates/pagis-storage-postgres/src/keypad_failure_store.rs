//! The failed-attempt count of each Workspace's Keypad Code (ADR-0021).
//!
//! One row holds the count of one Workspace, and a Workspace with no
//! row has no failure. A failure locks the row of its Workspace, reads
//! it and writes the next count in one transaction, so two failures on
//! two lines of the Workspace both count. The rule of the next count is
//! [`KeypadFailures::after_failure`], which the SQLite store applies
//! too.

use async_trait::async_trait;
use pagis_core::{KeypadFailureStore, KeypadFailures, StoreError, UnixMillis, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresKeypadFailureStore {
    pool: PgPool,
}

impl PostgresKeypadFailureStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn failures_from(row: &sqlx::postgres::PgRow) -> Result<KeypadFailures, StoreError> {
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

#[async_trait]
impl KeypadFailureStore for PostgresKeypadFailureStore {
    async fn get(&self, workspace_id: &WorkspaceId) -> Result<KeypadFailures, StoreError> {
        let row = sqlx::query(
            "SELECT failed_attempts, suspended_until FROM keypad_failures \
             WHERE workspace_id = $1",
        )
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
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        // The upsert writes an empty count when the Workspace has none,
        // and locks the row either way, so a second failure waits here
        // until this one commits and then reads its count.
        let current = sqlx::query(
            "INSERT INTO keypad_failures \
             (workspace_id, failed_attempts, suspended_until, updated_at) \
             VALUES ($1, 0, NULL, $2) \
             ON CONFLICT (workspace_id) DO UPDATE SET updated_at = keypad_failures.updated_at \
             RETURNING failed_attempts, suspended_until",
        )
        .bind(workspace_id.as_str())
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        let next = failures_from(&current)?.after_failure(now);
        sqlx::query(
            "UPDATE keypad_failures \
             SET failed_attempts = $2, suspended_until = $3, updated_at = $4 \
             WHERE workspace_id = $1",
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
        sqlx::query("DELETE FROM keypad_failures WHERE workspace_id = $1")
            .bind(workspace_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
