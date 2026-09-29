use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, ContinuationCheckpoint, ContinuationKey, ContinuationState,
    ContinuationStore, GrantId, MemoryExposure, MessageId, StoreError, WorkspaceId,
};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteContinuationStore {
    pool: SqlitePool,
}

impl SqliteContinuationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn root_key(key: &ContinuationKey) -> &str {
    key.root_message_id.as_ref().map_or("", MessageId::as_str)
}

async fn replace_dependencies(
    transaction: &mut Transaction<'_, Sqlite>,
    checkpoint: &ContinuationCheckpoint,
) -> Result<(), StoreError> {
    let key = &checkpoint.key;
    for table in [
        "continuation_checkpoint_messages",
        "continuation_checkpoint_exposures",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE workspace_id=? AND agent_id=? AND channel_id=? AND root_key=?"
        ))
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .execute(&mut **transaction)
        .await
        .map_err(db_err)?;
    }
    for message_id in &checkpoint.source_message_ids {
        sqlx::query(
            "INSERT INTO continuation_checkpoint_messages(workspace_id,agent_id,channel_id,root_key,message_id) VALUES(?,?,?,?,?)",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .bind(message_id.as_str())
        .execute(&mut **transaction)
        .await
        .map_err(db_err)?;
    }
    for exposure in &checkpoint.exposures {
        sqlx::query(
            "INSERT INTO continuation_checkpoint_exposures(workspace_id,agent_id,channel_id,root_key,grant_id,grant_revision) VALUES(?,?,?,?,?,?)",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .bind(exposure.grant_id.as_str())
        .bind(exposure.revision)
        .execute(&mut **transaction)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

async fn validate_dependencies(
    transaction: &mut Transaction<'_, Sqlite>,
    checkpoint: &ContinuationCheckpoint,
) -> Result<(), StoreError> {
    for message_id in &checkpoint.source_message_ids {
        let forgotten: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM forgotten_messages WHERE message_id = ?)",
        )
        .bind(message_id.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(db_err)?;
        if forgotten {
            return Err(StoreError::Conflict(format!(
                "continuation source message {} was forgotten",
                message_id.as_str()
            )));
        }
    }
    for exposure in &checkpoint.exposures {
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM grants WHERE id=? AND revision=? AND revoked_at IS NULL)",
        )
        .bind(exposure.grant_id.as_str())
        .bind(exposure.revision)
        .fetch_one(&mut **transaction)
        .await
        .map_err(db_err)?;
        if !live {
            return Err(StoreError::Conflict(format!(
                "continuation source grant {} revision {} is no longer live",
                exposure.grant_id.as_str(),
                exposure.revision
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl ContinuationStore for SqliteContinuationStore {
    async fn get(
        &self,
        key: &ContinuationKey,
    ) -> Result<Option<ContinuationCheckpoint>, StoreError> {
        let Some(row) = sqlx::query(
            "SELECT revision,after_exclusive,through_inclusive,state,created_at FROM continuation_checkpoints WHERE workspace_id=? AND agent_id=? AND channel_id=? AND root_key=?",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)? else {
            return Ok(None);
        };
        let source_message_ids = sqlx::query_scalar::<_, String>(
            "SELECT message_id FROM continuation_checkpoint_messages WHERE workspace_id=? AND agent_id=? AND channel_id=? AND root_key=? ORDER BY message_id",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(MessageId::from)
        .collect();
        let exposures = sqlx::query(
            "SELECT grant_id,grant_revision FROM continuation_checkpoint_exposures WHERE workspace_id=? AND agent_id=? AND channel_id=? AND root_key=? ORDER BY grant_id,grant_revision",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.agent_id.as_str())
        .bind(key.channel_id.as_str())
        .bind(root_key(key))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|row| MemoryExposure {
            grant_id: GrantId::from(row.get::<String, _>("grant_id")),
            revision: row.get("grant_revision"),
        })
        .collect();
        let state: String = row.get("state");
        Ok(Some(ContinuationCheckpoint {
            key: ContinuationKey {
                workspace_id: WorkspaceId::from(key.workspace_id.to_string()),
                agent_id: AgentId::from(key.agent_id.to_string()),
                channel_id: ChannelId::from(key.channel_id.to_string()),
                root_message_id: key.root_message_id.clone(),
            },
            revision: u32::try_from(row.get::<i64, _>("revision"))
                .map_err(|error| StoreError::Corrupt(format!("continuation revision: {error}")))?,
            after_exclusive: row
                .get::<Option<String>, _>("after_exclusive")
                .map(MessageId::from),
            through_inclusive: MessageId::from(row.get::<String, _>("through_inclusive")),
            state: serde_json::from_str::<ContinuationState>(&state)
                .map_err(|error| StoreError::Corrupt(format!("continuation state: {error}")))?,
            source_message_ids,
            exposures,
            created_at: row.get("created_at"),
        }))
    }

    async fn replace(
        &self,
        checkpoint: &ContinuationCheckpoint,
        expected_revision: Option<u32>,
    ) -> Result<bool, StoreError> {
        let required_revision = expected_revision.map_or(1, |revision| revision.saturating_add(1));
        if checkpoint.revision != required_revision {
            return Err(StoreError::Corrupt(format!(
                "continuation revision {} does not follow expected revision {:?}",
                checkpoint.revision, expected_revision
            )));
        }
        let key = &checkpoint.key;
        let state = serde_json::to_string(&checkpoint.state)
            .map_err(|error| StoreError::Corrupt(format!("continuation state: {error}")))?;
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        validate_dependencies(&mut transaction, checkpoint).await?;
        let changed = match expected_revision {
            None => sqlx::query(
                "INSERT INTO continuation_checkpoints(workspace_id,agent_id,channel_id,root_message_id,root_key,revision,after_exclusive,through_inclusive,state,created_at) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT DO NOTHING",
            )
            .bind(key.workspace_id.as_str())
            .bind(key.agent_id.as_str())
            .bind(key.channel_id.as_str())
            .bind(key.root_message_id.as_ref().map(MessageId::as_str))
            .bind(root_key(key))
            .bind(i64::from(checkpoint.revision))
            .bind(checkpoint.after_exclusive.as_ref().map(MessageId::as_str))
            .bind(checkpoint.through_inclusive.as_str())
            .bind(&state)
            .bind(checkpoint.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?
            .rows_affected() == 1,
            Some(expected) => sqlx::query(
                "UPDATE continuation_checkpoints SET revision=?,after_exclusive=?,through_inclusive=?,state=?,created_at=? WHERE workspace_id=? AND agent_id=? AND channel_id=? AND root_key=? AND revision=?",
            )
            .bind(i64::from(checkpoint.revision))
            .bind(checkpoint.after_exclusive.as_ref().map(MessageId::as_str))
            .bind(checkpoint.through_inclusive.as_str())
            .bind(&state)
            .bind(checkpoint.created_at)
            .bind(key.workspace_id.as_str())
            .bind(key.agent_id.as_str())
            .bind(key.channel_id.as_str())
            .bind(root_key(key))
            .bind(i64::from(expected))
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?
            .rows_affected() == 1,
        };
        if !changed {
            transaction.rollback().await.map_err(db_err)?;
            return Ok(false);
        }
        replace_dependencies(&mut transaction, checkpoint).await?;
        transaction.commit().await.map_err(db_err)?;
        Ok(true)
    }
}
