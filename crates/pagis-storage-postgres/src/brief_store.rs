use async_trait::async_trait;
use pagis_core::{AgentId, BriefCursor, BriefStore, ChannelId, MessageId, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};
use std::collections::BTreeSet;

use crate::db_err;

#[derive(Clone)]
pub struct PostgresBriefStore {
    pool: PgPool,
}

impl PostgresBriefStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BriefStore for PostgresBriefStore {
    async fn load(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
    ) -> Result<BriefCursor, StoreError> {
        let row = sqlx::query(
            "SELECT memory_revision, shown_paths FROM memory_brief_cursors \
             WHERE workspace_id = $1 AND agent_id = $2 AND channel_id = $3 AND root_message_id = $4",
        )
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .bind(channel_id.as_str())
        .bind(root_message_id.map_or("", MessageId::as_str))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(BriefCursor::default());
        };
        let shown: String = row.get("shown_paths");
        Ok(BriefCursor {
            initialized: true,
            memory_revision: row.get("memory_revision"),
            shown_paths: serde_json::from_str(&shown)
                .map_err(|error| StoreError::Corrupt(error.to_string()))?,
        })
    }

    async fn save(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
        cursor: &BriefCursor,
    ) -> Result<(), StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let current: Option<String> = sqlx::query_scalar(
            "SELECT shown_paths FROM memory_brief_cursors WHERE workspace_id = $1 \
             AND agent_id = $2 AND channel_id = $3 AND root_message_id = $4",
        )
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .bind(channel_id.as_str())
        .bind(root_message_id.map_or("", MessageId::as_str))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut shown_paths: BTreeSet<String> = current
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StoreError::Corrupt(error.to_string()))?
            .unwrap_or_default();
        shown_paths.extend(cursor.shown_paths.iter().cloned());
        let shown = serde_json::to_string(&shown_paths)
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        sqlx::query(
            "INSERT INTO memory_brief_cursors \
             (workspace_id, agent_id, channel_id, root_message_id, memory_revision, shown_paths) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (workspace_id, agent_id, channel_id, root_message_id) DO UPDATE SET \
             memory_revision = excluded.memory_revision, shown_paths = excluded.shown_paths",
        )
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .bind(channel_id.as_str())
        .bind(root_message_id.map_or("", MessageId::as_str))
        .bind(&cursor.memory_revision)
        .bind(shown)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }
}
