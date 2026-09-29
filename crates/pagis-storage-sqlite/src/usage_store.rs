//! What a Person spends: the Usage Records of every model call.

use async_trait::async_trait;
use pagis_core::{
    RunId, RunUsage, StoreError, UsagePeriod, UsageRecord, UsageStore, UsageTotal, WorkspaceId,
    WorkspaceUsage,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

/// The sum columns, in the order every read below selects them.
const TOTALS: &str = "COALESCE(SUM(input_tokens), 0) AS input_tokens, \
     COALESCE(SUM(output_tokens), 0) AS output_tokens, \
     COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens, \
     COALESCE(SUM(cache_write_tokens), 0) AS cache_write_tokens, \
     COALESCE(SUM(cost_usd), 0.0) AS cost_usd, \
     COUNT(*) AS calls, \
     COUNT(*) - COUNT(cost_usd) AS unpriced_calls";

fn row_to_total(row: &sqlx::sqlite::SqliteRow) -> UsageTotal {
    UsageTotal {
        input_tokens: row.get("input_tokens"),
        output_tokens: row.get("output_tokens"),
        cache_read_tokens: row.get("cache_read_tokens"),
        cache_write_tokens: row.get("cache_write_tokens"),
        cost_usd: row.get("cost_usd"),
        calls: row.get("calls"),
        unpriced_calls: row.get("unpriced_calls"),
    }
}

#[derive(Clone)]
pub struct SqliteUsageStore {
    pool: SqlitePool,
}

impl SqliteUsageStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UsageStore for SqliteUsageStore {
    async fn record(&self, usage: &UsageRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO usage (id, workspace_id, run_id, provider, model, input_tokens, \
             output_tokens, cache_read_tokens, cache_write_tokens, cost_usd, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(usage.id.as_str())
        .bind(usage.workspace_id.as_str())
        .bind(usage.run_id.as_str())
        .bind(usage.provider.as_deref())
        .bind(usage.model.as_deref())
        .bind(usage.input_tokens)
        .bind(usage.output_tokens)
        .bind(usage.cache_read_tokens)
        .bind(usage.cache_write_tokens)
        .bind(usage.cost_usd)
        .bind(usage.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn total_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
    ) -> Result<UsageTotal, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {TOTALS} FROM usage \
             WHERE workspace_id = ? AND created_at >= ? AND created_at < ?"
        ))
        .bind(workspace_id.as_str())
        .bind(period.from)
        .bind(period.to)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row_to_total(&row))
    }

    async fn totals_by_workspace(
        &self,
        period: UsagePeriod,
    ) -> Result<Vec<WorkspaceUsage>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT workspace_id, {TOTALS} FROM usage \
             WHERE created_at >= ? AND created_at < ? \
             GROUP BY workspace_id ORDER BY cost_usd DESC, workspace_id"
        ))
        .bind(period.from)
        .bind(period.to)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| WorkspaceUsage {
                workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
                total: row_to_total(row),
            })
            .collect())
    }

    async fn runs_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
        limit: u32,
    ) -> Result<Vec<RunUsage>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT run_id, MAX(created_at) AS last_at, {TOTALS} FROM usage \
             WHERE workspace_id = ? AND created_at >= ? AND created_at < ? \
             GROUP BY run_id ORDER BY last_at DESC, run_id LIMIT ?"
        ))
        .bind(workspace_id.as_str())
        .bind(period.from)
        .bind(period.to)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| RunUsage {
                run_id: RunId::from(row.get::<String, _>("run_id")),
                total: row_to_total(row),
                last_at: row.get("last_at"),
            })
            .collect())
    }
}
