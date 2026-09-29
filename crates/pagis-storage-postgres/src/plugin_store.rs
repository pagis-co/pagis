//! The installed Plugins of a Workspace (ADR-0017).
//!
//! An install writes the record and every Binding in one transaction,
//! so a Plugin never stands with half its Bindings. An uninstall
//! deletes the record with its Bindings and revokes every live Grant
//! on it in one transaction; the audit rows are not touched.

use async_trait::async_trait;
use pagis_core::{
    Plugin, PluginBinding, PluginBindingValue, PluginId, PluginSource, PluginState, PluginStore,
    StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct PostgresPluginStore {
    pool: PgPool,
}

impl PostgresPluginStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const PLUGIN_COLUMNS: &str = "id, workspace_id, name, source, installed_commit, \
     manifest_version, state, created_at, updated_at";

fn plugin_from(row: &sqlx::postgres::PgRow) -> Result<Plugin, StoreError> {
    let source: String = row.get("source");
    let state: String = row.get("state");
    Ok(Plugin {
        id: PluginId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        name: row.get("name"),
        source: serde_json::from_str::<PluginSource>(&source)
            .map_err(|error| StoreError::Corrupt(format!("plugin source: {error}")))?,
        installed_commit: row.get("installed_commit"),
        manifest_version: row.get("manifest_version"),
        state: PluginState::parse(&state)
            .ok_or_else(|| StoreError::Corrupt(format!("plugin state {state:?}")))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn binding_from(row: &sqlx::postgres::PgRow) -> Result<PluginBinding, StoreError> {
    let value: String = row.get("value");
    Ok(PluginBinding {
        plugin_id: PluginId::from(row.get::<String, _>("plugin_id")),
        field: row.get("field"),
        value: serde_json::from_str::<PluginBindingValue>(&value)
            .map_err(|error| StoreError::Corrupt(format!("plugin binding: {error}")))?,
    })
}

fn binding_json(binding: &PluginBinding) -> Result<String, StoreError> {
    serde_json::to_string(&binding.value)
        .map_err(|error| StoreError::Corrupt(format!("plugin binding: {error}")))
}

#[async_trait]
impl PluginStore for PostgresPluginStore {
    async fn create(&self, plugin: &Plugin, bindings: &[PluginBinding]) -> Result<(), StoreError> {
        let source = serde_json::to_string(&plugin.source)
            .map_err(|error| StoreError::Corrupt(format!("plugin source: {error}")))?;
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(&format!(
            "INSERT INTO plugins ({PLUGIN_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        ))
        .bind(plugin.id.as_str())
        .bind(plugin.workspace_id.as_str())
        .bind(&plugin.name)
        .bind(&source)
        .bind(&plugin.installed_commit)
        .bind(&plugin.manifest_version)
        .bind(plugin.state.as_str())
        .bind(plugin.created_at)
        .bind(plugin.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict(format!("plugin name {:?} is taken", plugin.name))
            } else {
                db_err(error)
            }
        })?;
        for binding in bindings {
            sqlx::query(
                "INSERT INTO plugin_bindings (plugin_id, field, value) VALUES ($1, $2, $3)",
            )
            .bind(binding.plugin_id.as_str())
            .bind(&binding.field)
            .bind(binding_json(binding)?)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Option<Plugin>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE workspace_id = $1 AND id = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(plugin_from).transpose()
    }

    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<Plugin>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE workspace_id = $1 AND name = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(plugin_from).transpose()
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Plugin>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {PLUGIN_COLUMNS} FROM plugins WHERE workspace_id = $1 ORDER BY name"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(plugin_from).collect()
    }

    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        state: PluginState,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let done = sqlx::query(
            "UPDATE plugins SET state = $1, updated_at = $2 WHERE id = $3 AND workspace_id = $4",
        )
        .bind(state.as_str())
        .bind(updated_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() > 0)
    }

    async fn set_installed(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        installed_commit: &str,
        manifest_version: &str,
        state: PluginState,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let done = sqlx::query(
            "UPDATE plugins SET installed_commit = $1, manifest_version = $2, state = $3, \
             updated_at = $4 WHERE id = $5 AND workspace_id = $6",
        )
        .bind(installed_commit)
        .bind(manifest_version)
        .bind(state.as_str())
        .bind(updated_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() > 0)
    }

    async fn put_binding(
        &self,
        workspace_id: &WorkspaceId,
        binding: &PluginBinding,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO plugin_bindings (plugin_id, field, value) \
             SELECT id, $1, $2 FROM plugins WHERE id = $3 AND workspace_id = $4 \
             ON CONFLICT (plugin_id, field) DO UPDATE SET value = excluded.value",
        )
        .bind(&binding.field)
        .bind(binding_json(binding)?)
        .bind(binding.plugin_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_bindings(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Vec<PluginBinding>, StoreError> {
        // `plugin_bindings` carries no workspace of its own, so the read
        // joins the Plugin that owns the Binding.
        let rows = sqlx::query(
            "SELECT b.plugin_id, b.field, b.value FROM plugin_bindings b \
             JOIN plugins p ON p.id = b.plugin_id \
             WHERE b.plugin_id = $1 AND p.workspace_id = $2 ORDER BY b.field",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(binding_from).collect()
    }

    async fn delete_binding(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        field: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "DELETE FROM plugin_bindings WHERE field = $1 AND plugin_id IN \
             (SELECT id FROM plugins WHERE id = $2 AND workspace_id = $3)",
        )
        .bind(field)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        // The Bindings reference the Plugin, so they go first.
        sqlx::query("DELETE FROM plugin_bindings WHERE plugin_id = $1")
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        let deleted = sqlx::query("DELETE FROM plugins WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id.as_str())
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        if deleted.rows_affected() == 0 {
            transaction.rollback().await.map_err(db_err)?;
            return Ok(false);
        }
        // The Plugin is the Org's, and each person grants it in their own
        // Workspace, so the Grants of every Workspace go with it.
        sqlx::query(
            "UPDATE grants SET revoked_at = $1 \
             WHERE resource_kind = $2 AND resource_id = $3 AND revoked_at IS NULL",
        )
        .bind(revoked_at)
        .bind(pagis_core::Grant::PLUGIN_KIND)
        .bind(id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(true)
    }
}
