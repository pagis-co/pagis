//! The Trust List (ADR-0021, ADR-0019).
//!
//! One table carries the Workspace-wide list and every Agent's list,
//! and one row names a number, an email address or a bare domain. The
//! read the call path and the mail path need is [`candidate`]: the
//! highest tier the rows that match one subject propose, over the
//! Workspace-wide list and the Agent's own list.
//!
//! [`candidate`]: pagis_core::TrustListStore::candidate

use async_trait::async_trait;
use pagis_core::{
    AgentId, StoreError, TrustEntry, TrustEntryId, TrustListStore, TrustSubject, TrustTier,
    WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresTrustListStore {
    pool: PgPool,
}

impl PostgresTrustListStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, subject, value, tier, label, created_at";

fn listed_tier(value: &str) -> Result<TrustTier, StoreError> {
    TrustTier::listed(value)
        .ok_or_else(|| StoreError::Corrupt(format!("{value:?} is not a listed tier")))
}

fn row_from(row: &sqlx::postgres::PgRow) -> Result<TrustEntry, StoreError> {
    let subject: String = row.get("subject");
    Ok(TrustEntry {
        id: TrustEntryId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: row.get::<Option<String>, _>("agent_id").map(AgentId::from),
        subject: TrustSubject::parse(&subject)
            .ok_or_else(|| StoreError::Corrupt(format!("{subject:?} is not a trust subject")))?,
        value: row.get("value"),
        tier: listed_tier(&row.get::<String, _>("tier"))?,
        label: row.get("label"),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl TrustListStore for PostgresTrustListStore {
    async fn upsert(&self, row: &TrustEntry) -> Result<(), StoreError> {
        // The two unique indexes are partial, so one statement cannot
        // name both conflicts. The delete clears whichever row holds
        // the subject in this list, and the insert writes the new one.
        let mut tx = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM trust_entries WHERE workspace_id = $1 AND subject = $2 AND value = $3 \
             AND agent_id IS NOT DISTINCT FROM $4",
        )
        .bind(row.workspace_id.as_str())
        .bind(row.subject.as_str())
        .bind(&row.value)
        .bind(row.agent_id.as_ref().map(|id| id.as_str()))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(&format!(
            "INSERT INTO trust_entries ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        ))
        .bind(row.id.as_str())
        .bind(row.workspace_id.as_str())
        .bind(row.agent_id.as_ref().map(|id| id.as_str()))
        .bind(row.subject.as_str())
        .bind(&row.value)
        .bind(row.tier.as_str())
        .bind(&row.label)
        .bind(row.created_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<TrustEntry>, StoreError> {
        // The first key puts the Workspace-wide rows first: a boolean
        // sorts false before true, as the 0 and the 1 of SQLite do.
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM trust_entries WHERE workspace_id = $1 \
             ORDER BY agent_id IS NOT NULL, agent_id, subject, value"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_from).collect()
    }

    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &TrustEntryId,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM trust_entries WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id.as_str())
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }

    async fn candidate(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        subject: TrustSubject,
        value: &str,
    ) -> Result<Option<TrustTier>, StoreError> {
        let rows = sqlx::query(
            "SELECT tier FROM trust_entries \
             WHERE workspace_id = $1 AND subject = $2 AND value = $3 \
             AND (agent_id IS NULL OR agent_id = $4)",
        )
        .bind(workspace_id.as_str())
        .bind(subject.as_str())
        .bind(value)
        .bind(agent_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut best: Option<TrustTier> = None;
        for row in &rows {
            let tier = listed_tier(&row.get::<String, _>("tier"))?;
            best = Some(best.map_or(tier, |best: TrustTier| best.max(tier)));
        }
        Ok(best)
    }
}
