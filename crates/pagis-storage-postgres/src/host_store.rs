//! The machines a person's clients run on.

use async_trait::async_trait;
use pagis_core::{Host, HostId, HostStore, StoreError, UnixMillis, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, name, platform, capabilities, last_seen_at, created_at";

#[derive(Clone)]
pub struct PostgresHostStore {
    pool: PgPool,
}

impl PostgresHostStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_host(row: &sqlx::postgres::PgRow) -> Result<Host, StoreError> {
    let capabilities: String = row.get("capabilities");
    Ok(Host {
        id: HostId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        name: row.get("name"),
        platform: row.get("platform"),
        capabilities: serde_json::from_str(&capabilities)
            .map_err(|error| StoreError::Corrupt(format!("host capabilities: {error}")))?,
        last_seen_at: row.get("last_seen_at"),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl HostStore for PostgresHostStore {
    async fn register(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
        platform: &str,
        capabilities: &[String],
        at: UnixMillis,
    ) -> Result<Host, StoreError> {
        let capabilities = serde_json::to_string(capabilities)
            .map_err(|error| StoreError::Corrupt(format!("host capabilities: {error}")))?;
        // The machine's name is its identity inside the Workspace, so a
        // client that connects again lands on its own row and the Grant
        // that names it still reaches it.
        let row = sqlx::query(&format!(
            "INSERT INTO hosts ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (workspace_id, name) DO UPDATE SET \
             platform = excluded.platform, capabilities = excluded.capabilities, \
             last_seen_at = excluded.last_seen_at \
             RETURNING {COLUMNS}"
        ))
        .bind(HostId::generate().as_str())
        .bind(workspace_id.as_str())
        .bind(name)
        .bind(platform)
        .bind(&capabilities)
        .bind(at)
        .bind(at)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row_to_host(&row)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &HostId,
    ) -> Result<Option<Host>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM hosts WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_host).transpose()
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Host>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM hosts WHERE workspace_id = $1 ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_host).collect()
    }

    async fn touch(&self, id: &HostId, at: UnixMillis) -> Result<bool, StoreError> {
        let updated = sqlx::query("UPDATE hosts SET last_seen_at = $1 WHERE id = $2")
            .bind(at)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(updated.rows_affected() == 1)
    }
}
