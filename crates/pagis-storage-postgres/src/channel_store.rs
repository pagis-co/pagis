use async_trait::async_trait;
use pagis_core::{AgentId, Channel, ChannelId, ChannelStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};

const COLUMNS: &str = "id, workspace_id, kind, title, created_at, updated_at";

use crate::db_err;

#[derive(Clone)]
pub struct PostgresChannelStore {
    pool: PgPool,
}

impl PostgresChannelStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_channel(row: &sqlx::postgres::PgRow) -> Result<Channel, StoreError> {
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
impl ChannelStore for PostgresChannelStore {
    async fn get_or_create_conversation(
        &self,
        workspace_id: &WorkspaceId,
        participants: &[WorkspaceId],
    ) -> Result<Channel, StoreError> {
        let ids: std::collections::BTreeSet<_> = participants.iter().collect();
        if ids.len() < 2 {
            return Err(StoreError::Conflict(
                "a conversation needs two Workspaces".into(),
            ));
        }
        let key = serde_json::to_string(&ids).expect("Workspace ids serialize");
        let id = ChannelId::generate();
        let now = pagis_core::now_ms();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let inserted = sqlx::query(
            "INSERT INTO channels (id, workspace_id, kind, created_at, updated_at, conversation_key) \
             VALUES ($1, $2, 'conversation', $3, $4, $5) \
             ON CONFLICT (workspace_id, conversation_key) DO NOTHING",
        ).bind(id.as_str()).bind(workspace_id.as_str()).bind(now).bind(now).bind(&key)
            .execute(&mut *tx).await.map_err(db_err)?.rows_affected();
        if inserted == 1 {
            for participant in ids {
                sqlx::query("INSERT INTO channel_participants \
                    (id, workspace_id, channel_id, participant_kind, participant_workspace_id, joined_at) \
                    VALUES ($1, $2, $3, 'workspace', $4, $5)")
                    .bind(pagis_core::ParticipantId::generate().as_str())
                    .bind(workspace_id.as_str()).bind(id.as_str()).bind(participant.as_str()).bind(now)
                    .execute(&mut *tx).await.map_err(db_err)?;
            }
        }
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM channels WHERE workspace_id = $1 AND conversation_key = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(&key)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        let channel = row_to_channel(&row)?;
        tx.commit().await.map_err(db_err)?;
        Ok(channel)
    }

    async fn create(&self, channel: &Channel) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
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
            "SELECT {COLUMNS} FROM channels WHERE id = $1 AND workspace_id = $2"
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
            "SELECT {COLUMNS} FROM channels WHERE workspace_id = $1 \
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
             WHERE c.workspace_id = $1 AND c.kind = 'dm' \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.participant_kind = 'person') \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = $2) \
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
             WHERE c.workspace_id = $1 AND c.kind = 'dm' \
             AND NOT EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.participant_kind = 'person') \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = $2) \
             AND EXISTS (SELECT 1 FROM channel_participants p \
                 WHERE p.channel_id = c.id AND p.agent_id = $3) \
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
