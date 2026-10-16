//! The append-only Org Audit Log and transactional hold changes.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{AuditEntry, AuditEntryId, AuditLog, LegalHold, StoreError, WorkspaceId};
use sqlx::{Row, SqliteConnection, SqlitePool};

#[derive(Clone)]
pub struct SqliteAuditLog {
    pool: SqlitePool,
}
impl SqliteAuditLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn encode(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
async fn append(tx: &mut SqliteConnection, entry: &AuditEntry) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO audit_entries (id, org_workspace_id, actor, action, subject_workspace_id, reason, at) VALUES (?1,?2,?3,?4,?5,?6,?7)")
        .bind(entry.id.as_str()).bind(entry.org_workspace_id.as_str()).bind(encode(&entry.actor)?)
        .bind(&entry.action).bind(entry.subject_workspace_id.as_str()).bind(&entry.reason).bind(entry.at)
        .execute(tx).await.map_err(db_err)?;
    Ok(())
}
pub(crate) async fn change_hold(
    pool: &SqlitePool,
    entry: &AuditEntry,
    start: bool,
) -> Result<(), StoreError> {
    let _hold = LegalHold::lock(&entry.subject_workspace_id).await;
    let action = if start {
        "legal_hold.started"
    } else {
        "legal_hold.ended"
    };
    if entry.action != action
        || (start && entry.reason.as_ref().is_none_or(|r| r.trim().is_empty()))
    {
        return Err(StoreError::Conflict(
            "A Legal Hold needs a reason and the correct action.".into(),
        ));
    }
    let hold = if start {
        Some(encode(&LegalHold {
            started_at: entry.at,
            started_by: entry.actor.clone(),
        })?)
    } else {
        None
    };
    let mut tx = crate::begin_write(pool).await.map_err(db_err)?;
    let condition = if start { "IS NULL" } else { "IS NOT NULL" };
    let changed = sqlx::query(&format!("UPDATE workspaces SET legal_hold=?1 WHERE id=?2 AND legal_hold {condition} AND id NOT IN (SELECT workspace_id FROM orgs)"))
        .bind(hold).bind(entry.subject_workspace_id.as_str()).execute(&mut *tx).await.map_err(db_err)?;
    if changed.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "The Legal Hold state changed or the Workspace is missing.".into(),
        ));
    }
    append(&mut tx, entry).await?;
    tx.commit().await.map_err(db_err)
}

/// Check before a content delete. A write transaction holds this check through its delete.
pub(crate) async fn refuse_delete(
    tx: &mut SqliteConnection,
    workspace: &WorkspaceId,
) -> Result<(), StoreError> {
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1 AND legal_hold IS NOT NULL)",
    )
    .bind(workspace.as_str())
    .fetch_one(tx)
    .await
    .map_err(db_err)?;
    LegalHold::refuse_delete(held)
}
#[async_trait]
impl AuditLog for SqliteAuditLog {
    async fn append(&self, entry: &AuditEntry) -> Result<(), StoreError> {
        append(&mut *self.pool.acquire().await.map_err(db_err)?, entry).await
    }
    async fn list(
        &self,
        org_workspace_id: &WorkspaceId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        let rows = sqlx::query("SELECT * FROM audit_entries WHERE org_workspace_id=?1 ORDER BY at DESC, id DESC LIMIT ?2 OFFSET ?3")
            .bind(org_workspace_id.as_str()).bind(i64::from(limit)).bind(i64::from(offset)).fetch_all(&self.pool).await.map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(AuditEntry {
                    id: AuditEntryId::from(row.get::<String, _>("id")),
                    org_workspace_id: WorkspaceId::from(row.get::<String, _>("org_workspace_id")),
                    actor: serde_json::from_str(row.get::<&str, _>("actor"))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    action: row.get("action"),
                    subject_workspace_id: WorkspaceId::from(
                        row.get::<String, _>("subject_workspace_id"),
                    ),
                    reason: row.get("reason"),
                    at: row.get("at"),
                })
            })
            .collect()
    }
}
