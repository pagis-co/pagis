//! The frozen tool catalogs of the Plugins (ADR-0017).
//!
//! The catalog is what the broker builds a Capability Manifest from at
//! every boot, so the daemon offers a Plugin's tools without starting
//! one server.

use async_trait::async_trait;
use pagis_core::{PluginId, PluginTool, PluginToolStore, PluginTools, StoreError, WorkspaceId};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqlitePluginToolStore {
    pool: SqlitePool,
}

impl SqlitePluginToolStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn tools_from(row: &sqlx::sqlite::SqliteRow) -> Result<PluginTools, StoreError> {
    let tools: String = row.get("tools");
    Ok(PluginTools {
        plugin_id: PluginId::from(row.get::<String, _>("plugin_id")),
        version: row.get("version"),
        installed_commit: row.get("installed_commit"),
        tools: serde_json::from_str::<Vec<PluginTool>>(&tools)
            .map_err(|error| StoreError::Corrupt(format!("plugin tools: {error}")))?,
        tools_changed: row.get::<i64, _>("tools_changed") != 0,
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl PluginToolStore for SqlitePluginToolStore {
    async fn put(&self, tools: &PluginTools) -> Result<(), StoreError> {
        let catalog = serde_json::to_string(&tools.tools)
            .map_err(|error| StoreError::Corrupt(format!("plugin tools: {error}")))?;
        sqlx::query(
            "INSERT INTO plugin_tools \
             (plugin_id, version, installed_commit, tools, tools_changed, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (plugin_id, version) DO UPDATE SET \
             installed_commit = excluded.installed_commit, tools = excluded.tools, \
             tools_changed = excluded.tools_changed, created_at = excluded.created_at",
        )
        .bind(tools.plugin_id.as_str())
        .bind(&tools.version)
        .bind(&tools.installed_commit)
        .bind(&catalog)
        .bind(i64::from(tools.tools_changed))
        .bind(tools.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
    ) -> Result<Option<PluginTools>, StoreError> {
        // `plugin_tools` carries no workspace of its own, so every read
        // and write joins the Plugin that owns the catalog.
        let row = sqlx::query(
            "SELECT t.plugin_id, t.version, t.installed_commit, t.tools, t.tools_changed, \
             t.created_at FROM plugin_tools t JOIN plugins p ON p.id = t.plugin_id \
             WHERE t.plugin_id = ? AND t.version = ? AND p.workspace_id = ?",
        )
        .bind(plugin_id.as_str())
        .bind(version)
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(tools_from).transpose()
    }

    async fn set_tools_changed(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
        changed: bool,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE plugin_tools SET tools_changed = ? WHERE version = ? AND plugin_id IN \
             (SELECT id FROM plugins WHERE id = ? AND workspace_id = ?)",
        )
        .bind(i64::from(changed))
        .bind(version)
        .bind(plugin_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "DELETE FROM plugin_tools WHERE plugin_id IN \
             (SELECT id FROM plugins WHERE id = ? AND workspace_id = ?)",
        )
        .bind(plugin_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
