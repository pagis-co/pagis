use async_trait::async_trait;
use pagis_core::{
    AgentId, AuthorKind, Block, ChannelId, Message, MessageId, MessageStatus, MessageStore,
    REPLY_AUTHORS_MAX, ReplyAuthor, RunId, SendOutcome, StoreError, TimelineEntry, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, channel_id, parent_message_id, author_kind, \
     author_agent_id, run_id, status, blocks, text_content, pending_id, created_at, completed_at";

/// A message that is not a derived progress row, for the rows of
/// `table`. A progress row carries that one block, so its first block
/// names it.
fn not_progress(table: &str) -> String {
    format!("(({table}.blocks::jsonb -> 0) ->> 'type') IS DISTINCT FROM 'progress'")
}

#[derive(Clone)]
pub struct PostgresMessageStore {
    pool: PgPool,
}

impl PostgresMessageStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn get_by_pending(
        &self,
        channel_id: &ChannelId,
        pending_id: &str,
    ) -> Result<Option<Message>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM messages WHERE channel_id = $1 AND pending_id = $2"
        ))
        .bind(channel_id.as_str())
        .bind(pending_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_message).transpose()
    }

    /// The outcome of an insert: the new row, or the stored original
    /// when a `pending_id` deduplicated it.
    async fn outcome(&self, message: &Message, inserted: bool) -> Result<SendOutcome, StoreError> {
        if inserted {
            return Ok(SendOutcome::Created(message.clone()));
        }
        let pending_id = message.pending_id.as_deref().ok_or_else(|| {
            StoreError::Corrupt("message insert without pending_id affected no rows".to_string())
        })?;
        self.get_by_pending(&message.channel_id, pending_id)
            .await?
            .map(SendOutcome::Deduplicated)
            .ok_or_else(|| {
                StoreError::Corrupt("deduplicated message not found by pending_id".to_string())
            })
    }
}

/// Write the message row. `false` says a `pending_id` already held the
/// place, so nothing was written.
async fn insert_row(
    connection: &mut sqlx::PgConnection,
    message: &Message,
) -> Result<bool, StoreError> {
    let blocks = serde_json::to_string(&message.blocks)
        .map_err(|e| StoreError::Corrupt(format!("message blocks: {e}")))?;
    let result = sqlx::query(
        "INSERT INTO messages (id, workspace_id, channel_id, parent_message_id, \
         author_kind, author_agent_id, run_id, status, blocks, text_content, \
         pending_id, created_at, completed_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (channel_id, pending_id) WHERE pending_id IS NOT NULL DO NOTHING",
    )
    .bind(message.id.as_str())
    .bind(message.workspace_id.as_str())
    .bind(message.channel_id.as_str())
    .bind(
        message
            .parent_message_id
            .as_ref()
            .map(|m| m.as_str().to_owned()),
    )
    .bind(message.author_kind.as_str())
    .bind(
        message
            .author_agent_id
            .as_ref()
            .map(|a| a.as_str().to_owned()),
    )
    .bind(message.run_id.as_ref().map(|r| r.as_str().to_owned()))
    .bind(message.status.as_str())
    .bind(&blocks)
    .bind(&message.text_content)
    .bind(&message.pending_id)
    .bind(message.created_at)
    .bind(message.completed_at)
    .execute(connection)
    .await
    .map_err(db_err)?;
    Ok(result.rows_affected() == 1)
}

/// Replace one message's exposure stamp. The set row marks the stamp
/// known, even when the message drew on no source at all.
async fn stamp_exposures(
    connection: &mut sqlx::PgConnection,
    workspace_id: &WorkspaceId,
    id: &MessageId,
    exposures: &[pagis_core::MemoryExposure],
) -> Result<(), StoreError> {
    // The stamp tables carry no workspace of their own, so every write
    // reads the owning message row: a stamp never lands on another
    // Workspace's message.
    sqlx::query(
        "INSERT INTO message_exposure_sets(message_id) \
         SELECT id FROM messages WHERE id = $1 AND workspace_id = $2 \
         ON CONFLICT DO NOTHING",
    )
    .bind(id.as_str())
    .bind(workspace_id.as_str())
    .execute(&mut *connection)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM message_exposures WHERE message_id IN \
         (SELECT id FROM messages WHERE id = $1 AND workspace_id = $2)",
    )
    .bind(id.as_str())
    .bind(workspace_id.as_str())
    .execute(&mut *connection)
    .await
    .map_err(db_err)?;
    for exposure in exposures {
        sqlx::query(
            "INSERT INTO message_exposures(message_id, grant_id, grant_revision) \
             SELECT id, $1, $2 FROM messages WHERE id = $3 AND workspace_id = $4",
        )
        .bind(exposure.grant_id.as_str())
        .bind(exposure.revision)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// One row of the `reply_authors` JSON array the timeline query builds.
#[derive(serde::Deserialize)]
struct ReplyAuthorRow {
    kind: String,
    agent_id: Option<String>,
}

/// The reply authors of one timeline entry, the newest reply first.
/// `jsonb_agg` takes an explicit order, so the timeline query sorts the
/// authors on the `at` of each one and the array arrives in that order.
fn reply_authors(json: String) -> Result<Vec<ReplyAuthor>, StoreError> {
    let rows: Vec<ReplyAuthorRow> = serde_json::from_str(&json)
        .map_err(|e| StoreError::Corrupt(format!("reply_authors: {e}")))?;
    rows.into_iter()
        .map(|row| {
            Ok(ReplyAuthor {
                kind: row
                    .kind
                    .parse::<AuthorKind>()
                    .map_err(StoreError::Corrupt)?,
                agent_id: row.agent_id.map(AgentId::from),
            })
        })
        .collect()
}

fn row_to_message(row: &sqlx::postgres::PgRow) -> Result<Message, StoreError> {
    let blocks: String = row.get("blocks");
    let author_kind: String = row.get("author_kind");
    let status: String = row.get("status");
    Ok(Message {
        id: MessageId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
        parent_message_id: row
            .get::<Option<String>, _>("parent_message_id")
            .map(MessageId::from),
        author_kind: author_kind
            .parse::<AuthorKind>()
            .map_err(StoreError::Corrupt)?,
        author_agent_id: row
            .get::<Option<String>, _>("author_agent_id")
            .map(AgentId::from),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        status: status
            .parse::<MessageStatus>()
            .map_err(StoreError::Corrupt)?,
        blocks: serde_json::from_str(&blocks)
            .map_err(|e| StoreError::Corrupt(format!("message blocks: {e}")))?,
        text_content: row.get("text_content"),
        pending_id: row.get("pending_id"),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}

#[async_trait]
impl MessageStore for PostgresMessageStore {
    async fn insert(&self, message: &Message) -> Result<SendOutcome, StoreError> {
        let mut connection = self.pool.acquire().await.map_err(db_err)?;
        let inserted = insert_row(&mut connection, message).await?;
        drop(connection);
        self.outcome(message, inserted).await
    }

    async fn insert_stamped(
        &self,
        message: &Message,
        exposures: &[pagis_core::MemoryExposure],
    ) -> Result<SendOutcome, StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let inserted = insert_row(&mut transaction, message).await?;
        if inserted {
            stamp_exposures(
                &mut transaction,
                &message.workspace_id,
                &message.id,
                exposures,
            )
            .await?;
        }
        transaction.commit().await.map_err(db_err)?;
        self.outcome(message, inserted).await
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Message>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM messages WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_message).transpose()
    }

    async fn is_forgotten(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM forgotten_messages f \
             JOIN messages m ON m.id = f.message_id \
             WHERE f.message_id = $1 AND m.workspace_id = $2)",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    async fn set_exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        exposures: &[pagis_core::MemoryExposure],
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(db_err)?;
        stamp_exposures(&mut transaction, workspace_id, id, exposures).await?;
        transaction.commit().await.map_err(db_err)
    }

    async fn exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Vec<pagis_core::MemoryExposure>>, StoreError> {
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM message_exposure_sets s \
             JOIN messages m ON m.id = s.message_id \
             WHERE s.message_id = $1 AND m.workspace_id = $2 \
             AND s.message_id NOT IN (SELECT message_id FROM forgotten_messages))",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        if !known {
            return Ok(None);
        }
        let rows = sqlx::query(
            "SELECT e.grant_id, e.grant_revision FROM message_exposures e \
             JOIN messages m ON m.id = e.message_id \
             WHERE e.message_id = $1 AND m.workspace_id = $2 ORDER BY e.grant_id",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(Some(
            rows.into_iter()
                .map(|row| pagis_core::MemoryExposure {
                    grant_id: pagis_core::GrantId::from(row.get::<String, _>("grant_id")),
                    revision: row.get("grant_revision"),
                })
                .collect(),
        ))
    }

    async fn list_thread(
        &self,
        workspace_id: &WorkspaceId,
        root_id: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM messages \
             WHERE workspace_id = $1 AND (id = $2 OR parent_message_id = $3) \
             ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .bind(root_id.as_str())
        .bind(root_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_message).collect()
    }

    async fn latest_agent_message_of_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<Message>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM messages \
             WHERE run_id = $1 AND workspace_id = $2 \
             AND author_kind = 'agent' AND status = 'complete' \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_message).transpose()
    }

    async fn finalize(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        status: MessageStatus,
        blocks: &[Block],
        text_content: &str,
        completed_at: i64,
    ) -> Result<(), StoreError> {
        let blocks = serde_json::to_string(blocks)
            .map_err(|e| StoreError::Corrupt(format!("message blocks: {e}")))?;
        sqlx::query(
            "UPDATE messages SET status = $1, blocks = $2, text_content = $3, completed_at = $4 \
             WHERE id = $5 AND workspace_id = $6",
        )
        .bind(status.as_str())
        .bind(&blocks)
        .bind(text_content)
        .bind(completed_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn fail_streaming(&self, completed_at: i64) -> Result<u64, StoreError> {
        let result = sqlx::query(
            "UPDATE messages SET status = 'failed', completed_at = $1 WHERE status = 'streaming'",
        )
        .bind(completed_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected())
    }

    async fn list_agent_elsewhere(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        exclude_channel: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        // A progress row is daemon state the channel view hides,
        // so it is not this agent's word anywhere else either.
        let not_progress = not_progress("messages");
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM messages \
             WHERE author_agent_id = $1 AND workspace_id = $2 \
             AND channel_id != $3 AND status = 'complete' \
             AND {not_progress} \
             AND ($4 IS NULL OR id < $5) \
             ORDER BY id DESC LIMIT $6"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(exclude_channel.as_str())
        .bind(before.map(|m| m.as_str().to_owned()))
        .bind(before.map(|m| m.as_str().to_owned()))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_message).collect()
    }

    async fn list_top_level(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        // The rollup counts conversation: a derived progress row
        // is daemon state in the thread, not a reply of its own. A
        // reply is of the root's own Workspace: a row of another
        // Workspace under the root is not a reply to it.
        let not_progress = not_progress("r");
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS}, \
             (SELECT COUNT(*) FROM messages r WHERE r.parent_message_id = messages.id \
                 AND r.workspace_id = messages.workspace_id \
                 AND {not_progress}) AS reply_count, \
             (SELECT MAX(r.created_at) FROM messages r WHERE r.parent_message_id = messages.id \
                 AND r.workspace_id = messages.workspace_id \
                 AND {not_progress}) AS last_reply_at, \
             (SELECT COALESCE(jsonb_agg(jsonb_build_object('kind', a.author_kind, \
                 'agent_id', a.author_agent_id) ORDER BY a.at DESC), '[]'::jsonb)::text \
                 FROM (SELECT r.author_kind, r.author_agent_id, MAX(r.created_at) AS at \
                     FROM messages r WHERE r.parent_message_id = messages.id \
                     AND r.workspace_id = messages.workspace_id \
                     AND {not_progress} \
                     GROUP BY r.author_kind, r.author_agent_id \
                     ORDER BY at DESC LIMIT {REPLY_AUTHORS_MAX}) a) AS reply_authors \
             FROM messages \
             WHERE channel_id = $1 AND workspace_id = $2 AND parent_message_id IS NULL \
             AND ($3 IS NULL OR id < $4) \
             ORDER BY id DESC LIMIT $5"
        ))
        .bind(channel_id.as_str())
        .bind(workspace_id.as_str())
        .bind(before.map(|m| m.as_str().to_owned()))
        .bind(before.map(|m| m.as_str().to_owned()))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|row| {
                Ok(TimelineEntry {
                    message: row_to_message(row)?,
                    reply_count: u32::try_from(row.get::<i64, _>("reply_count"))
                        .map_err(|e| StoreError::Corrupt(format!("reply_count: {e}")))?,
                    last_reply_at: row.get("last_reply_at"),
                    reply_authors: reply_authors(row.get("reply_authors"))?,
                })
            })
            .collect()
    }
}
