use async_trait::async_trait;
use pagis_core::{AgentId, Channel, ChannelId, ChannelStore, StoreError, WorkspaceId};
use sqlx::{Row, SqlitePool};

const COLUMNS: &str = "id, workspace_id, kind, title, created_at, updated_at";

use crate::db_err;

#[derive(Clone)]
pub struct SqliteChannelStore {
    pool: SqlitePool,
}

impl SqliteChannelStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn row_to_channel(row: &sqlx::sqlite::SqliteRow) -> Result<Channel, StoreError> {
    let kind: String = row.get("kind");
    Ok(Channel {
        id: ChannelId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        kind: kind.parse().map_err(StoreError::Corrupt)?,
        title: row.get("title"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[async_trait]
impl ChannelStore for SqliteChannelStore {
    async fn create(&self, channel: &Channel) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(channel.id.as_str())
        .bind(channel.workspace_id.as_str())
        .bind(channel.kind.as_str())
        .bind(&channel.title)
        .bind(channel.created_at)
        .bind(channel.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ChannelId,
    ) -> Result<Option<Channel>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM channels WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_channel).transpose()
    }

    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Channel>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM channels WHERE workspace_id = ? \
             ORDER BY updated_at DESC, id DESC"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_channel).collect()
    }

    async fn find_user_dm(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM channels c \
             WHERE c.workspace_id = ? AND c.kind = 'dm' \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.participant_kind = 'user') \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = ?) \
             ORDER BY c.id LIMIT 1"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_channel).transpose()
    }

    async fn find_agent_dm(
        &self,
        workspace_id: &WorkspaceId,
        a: &AgentId,
        b: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM channels c \
             WHERE c.workspace_id = ? AND c.kind = 'dm' \
             AND NOT EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.participant_kind = 'user') \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = ?) \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = ?) \
             ORDER BY c.id LIMIT 1"
        ))
        .bind(workspace_id.as_str())
        .bind(a.as_str())
        .bind(b.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_channel).transpose()
    }
}
