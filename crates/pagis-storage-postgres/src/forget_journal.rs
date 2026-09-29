//! Durable purge phases. Failures preserve the last completed phase and suppression.
use crate::{begin_write, db_err};
use pagis_core::*;
use sqlx::{PgPool, Row};

fn operation(row: sqlx::postgres::PgRow) -> Result<ForgetOperation, StoreError> {
    let phase = match row.get::<&str, _>("phase") {
        "blocked" => ForgetPhase::Blocked,
        "structured_purged" => ForgetPhase::StructuredPurged,
        "memory_purged" => ForgetPhase::MemoryPurged,
        "complete" => ForgetPhase::Complete,
        _ => return Err(StoreError::Corrupt("unknown forget phase".into())),
    };
    Ok(ForgetOperation {
        id: row.get("id"),
        connection_id: row.get::<String, _>("connection_id").into(),
        phase,
        created_at: row.get("created_at"),
        error: row.get("error"),
        reopted_at: row.get("reopted_at"),
    })
}
pub(crate) async fn get(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
) -> Result<Option<ForgetOperation>, StoreError> {
    sqlx::query("SELECT * FROM forget_operations WHERE workspace_id=$1 AND id=$2")
        .bind(workspace.as_str())
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(db_err)?
        .map(operation)
        .transpose()
}
pub(crate) async fn list(
    pool: &PgPool,
    workspace: &WorkspaceId,
    unfinished: bool,
) -> Result<Vec<ForgetOperation>, StoreError> {
    sqlx::query("SELECT * FROM forget_operations WHERE workspace_id=$1 AND ($2=false OR (phase!='complete' AND error IS NULL)) ORDER BY created_at,id")
        .bind(workspace.as_str()).bind(unfinished).fetch_all(pool).await.map_err(db_err)?.into_iter().map(operation).collect()
}
pub(crate) async fn advance(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
    from: &str,
    to: &str,
) -> Result<(), StoreError> {
    let changed = sqlx::query("UPDATE forget_operations SET phase=$1 WHERE workspace_id=$2 AND id=$3 AND phase=$4 AND error IS NULL AND structured_cleaned=true")
        .bind(to).bind(workspace.as_str()).bind(id).bind(from).execute(pool).await.map_err(db_err)?.rows_affected();
    if changed == 1 {
        return Ok(());
    }
    let phase: Option<String> = sqlx::query_scalar(
        "SELECT phase FROM forget_operations WHERE workspace_id=$1 AND id=$2 AND error IS NULL",
    )
    .bind(workspace.as_str())
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    if phase.as_deref() == Some(to) || phase.as_deref() == Some("complete") {
        return Ok(());
    }
    Err(StoreError::Conflict(
        "previous forget phase has not completed".into(),
    ))
}
pub(crate) async fn fail(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
    error: &str,
) -> Result<(), StoreError> {
    let changed = sqlx::query(
        "UPDATE forget_operations SET error=$1 WHERE workspace_id=$2 AND id=$3 AND phase!='complete'",
    )
    .bind(error)
    .bind(workspace.as_str())
    .bind(id)
    .execute(pool)
    .await
    .map_err(db_err)?
    .rows_affected();
    if changed == 0 {
        return Err(StoreError::Conflict(
            "forget operation is unavailable or complete".into(),
        ));
    }
    Ok(())
}
pub(crate) async fn retry(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
) -> Result<(), StoreError> {
    let changed = sqlx::query("UPDATE forget_operations SET error=NULL WHERE workspace_id=$1 AND id=$2 AND phase!='complete'")
        .bind(workspace.as_str()).bind(id).execute(pool).await.map_err(db_err)?.rows_affected();
    if changed == 0 {
        return Err(StoreError::Conflict(
            "forget operation is unavailable or complete".into(),
        ));
    }
    Ok(())
}
pub(crate) async fn reopt(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let mut tx = begin_write(pool).await.map_err(db_err)?;
    let state = sqlx::query(
        "SELECT phase,reopted_at FROM forget_operations WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace.as_str())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?
    .ok_or_else(|| StoreError::Conflict("forget operation is unavailable".into()))?;
    if state.get::<&str, _>("phase") != "complete" {
        return Err(StoreError::Conflict(
            "deletion must complete before allowing learning again".into(),
        ));
    }
    if state.get::<Option<i64>, _>("reopted_at").is_some() {
        return Ok(());
    }
    // The purge left no row of a forgotten item, so there is nothing to
    // restore. When no suppression row blocks an item any more, its next
    // retrieval acquires it as a new item.
    sqlx::query("DELETE FROM forget_suppressions WHERE workspace_id=$1 AND operation_id=$2")
        .bind(workspace.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    sqlx::query("UPDATE forget_operations SET reopted_at=$1 WHERE workspace_id=$2 AND id=$3")
        .bind(now)
        .bind(workspace.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)
}
