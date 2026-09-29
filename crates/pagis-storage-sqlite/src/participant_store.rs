use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, ChannelParticipant, ParticipantId, ParticipantStore, StoreError,
    WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteParticipantStore {
    pool: SqlitePool,
}

impl SqliteParticipantStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ParticipantStore for SqliteParticipantStore {
    async fn create(&self, participant: &ChannelParticipant) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO channel_participants \
             (id, workspace_id, channel_id, participant_kind, agent_id, joined_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(participant.id.as_str())
        .bind(participant.workspace_id.as_str())
        .bind(participant.channel_id.as_str())
        .bind(participant.kind.as_str())
        .bind(participant.agent_id.as_ref().map(|a| a.as_str().to_owned()))
        .bind(participant.joined_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn agents_in_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<AgentId>, StoreError> {
        let rows = sqlx::query(
            "SELECT agent_id FROM channel_participants \
             WHERE channel_id = ? AND workspace_id = ? \
             AND participant_kind = 'agent' AND agent_id IS NOT NULL \
             ORDER BY joined_at, id",
        )
        .bind(channel_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| AgentId::from(row.get::<String, _>("agent_id")))
            .collect())
    }

    async fn list_for_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<ChannelParticipant>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, channel_id, participant_kind, agent_id, joined_at \
             FROM channel_participants WHERE channel_id = ? AND workspace_id = ? \
             ORDER BY joined_at, id",
        )
        .bind(channel_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|row| {
                let kind: String = row.get("participant_kind");
                Ok(ChannelParticipant {
                    id: ParticipantId::from(row.get::<String, _>("id")),
                    workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
                    channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
                    kind: kind.parse().map_err(StoreError::Corrupt)?,
                    agent_id: row.get::<Option<String>, _>("agent_id").map(AgentId::from),
                    joined_at: row.get("joined_at"),
                })
            })
            .collect()
    }
}
