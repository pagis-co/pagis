use crate::db_err;
use pagis_core::{
    StoreError, WorkspaceId,
    knowledge::{KnowledgeInvalidation, SourceKey},
};
use sqlx::{Row, SqlitePool};

pub(crate) async fn pending(
    pool: &SqlitePool,
    workspace: &WorkspaceId,
    limit: u32,
) -> Result<Vec<KnowledgeInvalidation>, StoreError> {
    let rows = sqlx::query("SELECT id,connection_id,resource FROM knowledge_invalidations WHERE workspace_id=? AND published=0 ORDER BY id LIMIT ?")
        .bind(workspace.as_str()).bind(limit.min(100)).fetch_all(pool).await.map_err(db_err)?;
    Ok(rows
        .iter()
        .map(|row| KnowledgeInvalidation {
            id: row.get("id"),
            source: SourceKey {
                workspace_id: workspace.clone(),
                connection_id: row.get::<String, _>("connection_id").into(),
                resource: row.get("resource"),
            },
        })
        .collect())
}

pub(crate) async fn acknowledge(
    pool: &SqlitePool,
    workspace: &WorkspaceId,
    id: i64,
) -> Result<(), StoreError> {
    sqlx::query("UPDATE knowledge_invalidations SET published=1 WHERE workspace_id=? AND id=?")
        .bind(workspace.as_str())
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(())
}
