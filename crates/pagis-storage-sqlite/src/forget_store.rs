//! Durable keyed suppression. Only an explicit owner forget operation creates it.
//!
//! A suppression row holds the keyed identity of a source item and not
//! its id. The key derives from the Tenant Data Key of the Workspace,
//! and the caller passes its source, so the database holds no key
//! (ADR-0008).
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{knowledge::SourceKey, *};
use sqlx::{SqliteConnection, SqlitePool};

#[derive(Clone)]
pub struct SqliteForgetStore {
    pool: SqlitePool,
}
impl SqliteForgetStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

pub(crate) fn encode(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|_| StoreError::Corrupt("invalid forget record".into()))
}

pub(crate) async fn suppressed(
    tx: &mut SqliteConnection,
    keys: &dyn ForgetKeys,
    key: &SourceKey,
    id: &str,
) -> Result<bool, StoreError> {
    // A Workspace that never forgot anything of this Connection needs
    // no key, so the check asks for one only after this query.
    let forgotten: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM forget_suppressions WHERE workspace_id=? AND connection_id=?)",
    )
    .bind(key.workspace_id.as_str())
    .bind(key.connection_id.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    if !forgotten {
        return Ok(false);
    }
    let identity = keys.suppression_key(&key.workspace_id)?.identity(key, id);
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM forget_suppressions WHERE workspace_id=? AND connection_id=? AND (scope='account' OR (resource=? AND identity=?)))")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .bind(identity).fetch_one(tx).await.map_err(db_err)
}

pub(crate) async fn permits_provenance(
    tx: &mut SqliteConnection,
    workspace: &WorkspaceId,
    exposures: &[MemoryExposure],
) -> Result<bool, StoreError> {
    // One indexed fast path when this workspace has never forgotten anything.
    let forgotten: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM forget_suppressions WHERE workspace_id=?)")
            .bind(workspace.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
    if !forgotten {
        return Ok(true);
    }
    if !exposures.is_empty() {
        let allowed: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM json_each(?) x JOIN forget_suppressions f ON f.workspace_id=? WHERE NOT EXISTS(SELECT 1 FROM grants g WHERE g.id=json_extract(x.value,'$.grant_id') AND g.workspace_id=f.workspace_id) OR EXISTS(SELECT 1 FROM grants g WHERE g.id=json_extract(x.value,'$.grant_id') AND g.workspace_id=f.workspace_id AND g.resource_kind='connection' AND g.resource_id=f.connection_id))")
            .bind(encode(&exposures)?).bind(workspace.as_str()).fetch_one(&mut *tx).await.map_err(db_err)?;
        if !allowed {
            return Ok(false);
        }
    }
    Ok(true)
}

#[async_trait]
impl ForgetStore for SqliteForgetStore {
    async fn preview(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
    ) -> Result<ForgetPreview, StoreError> {
        crate::forget_plan::load(
            &mut *self.pool.acquire().await.map_err(db_err)?,
            workspace,
            target,
        )
        .await?
        .preview()
    }
    async fn preview_affects(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.acquire().await.map_err(db_err)?;
        crate::forget_plan::load(&mut tx, workspace, target)
            .await?
            .affects(&mut tx, exposures)
            .await
    }
    async fn begin(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        expected_revision: &str,
        now: i64,
        keys: &dyn ForgetKeys,
    ) -> Result<ForgetOperation, StoreError> {
        crate::forget_purge::begin(&self.pool, workspace, target, expected_revision, now, keys)
            .await
    }
    async fn purge_structured(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        crate::forget_purge::purge(&self.pool, workspace, id).await
    }
    async fn is_suppressed(
        &self,
        source: &SourceKey,
        source_id: &str,
        keys: &dyn ForgetKeys,
    ) -> Result<bool, StoreError> {
        suppressed(
            &mut *self.pool.acquire().await.map_err(db_err)?,
            keys,
            source,
            source_id,
        )
        .await
    }
    async fn permits_provenance(
        &self,
        workspace: &WorkspaceId,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        permits_provenance(
            &mut *self.pool.acquire().await.map_err(db_err)?,
            workspace,
            exposures,
        )
        .await
    }
    async fn run_allowed(&self, workspace: &WorkspaceId, run: &RunId) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM wakeups w JOIN schedules s ON s.id=w.schedule_id WHERE w.run_id=? AND w.workspace_id=? AND s.forgotten=1)")
            .bind(run.as_str()).bind(workspace.as_str()).fetch_one(&self.pool).await.map_err(db_err)
    }
    async fn raw_connection_allowed(
        &self,
        workspace: &WorkspaceId,
        connection: &ConnectionId,
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM forget_suppressions WHERE workspace_id=? AND connection_id=?)")
            .bind(workspace.as_str()).bind(connection.as_str()).fetch_one(&self.pool).await.map_err(db_err)
    }
    async fn operation(
        &self,
        workspace: &WorkspaceId,
        id: &str,
    ) -> Result<Option<ForgetOperation>, StoreError> {
        crate::forget_journal::get(&self.pool, workspace, id).await
    }
    async fn operations(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        crate::forget_journal::list(&self.pool, workspace, false).await
    }
    async fn unfinished(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        crate::forget_journal::list(&self.pool, workspace, true).await
    }
    async fn memory_purged(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        crate::forget_memory::memory_purged(&self.pool, workspace, id).await
    }
    async fn complete(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        crate::forget_memory::complete(&self.pool, workspace, id).await
    }
    async fn record_failure(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        error: &str,
    ) -> Result<(), StoreError> {
        crate::forget_journal::fail(&self.pool, workspace, id, error).await
    }
    async fn retry(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        crate::forget_journal::retry(&self.pool, workspace, id).await
    }
    async fn reopt_in(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        crate::forget_journal::reopt(&self.pool, workspace, id, now).await
    }
}
