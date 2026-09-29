use async_trait::async_trait;
use pagis_core::knowledge::SourceRead;
use pagis_core::{
    AgentId, ChannelId, ConnectionId, MessageId, Run, RunId, RunOrigin, RunState, RunStore,
    StoreError, TriggerKind, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, agent_id, channel_id, root_message_id, trigger_kind, \
     trigger_ref, hop_count, origin_agent_id, origin_channel_id, origin_root_message_id, \
     state, error, failure_kind, started_at, ended_at, created_at";

#[derive(Clone)]
pub struct SqliteRunStore {
    pool: SqlitePool,
}

impl SqliteRunStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn row_to_run(row: &sqlx::sqlite::SqliteRow) -> Result<Run, StoreError> {
    let trigger_kind: String = row.get("trigger_kind");
    let state: String = row.get("state");
    Ok(Run {
        id: RunId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        channel_id: row
            .get::<Option<String>, _>("channel_id")
            .map(ChannelId::from),
        root_message_id: row
            .get::<Option<String>, _>("root_message_id")
            .map(MessageId::from),
        trigger_kind: trigger_kind
            .parse::<TriggerKind>()
            .map_err(StoreError::Corrupt)?,
        trigger_ref: row.get("trigger_ref"),
        hop_count: u32::try_from(row.get::<i64, _>("hop_count"))
            .map_err(|e| StoreError::Corrupt(format!("hop_count: {e}")))?,
        // The three origin columns are written together, so the agent
        // and the channel are either both present or both absent.
        origin: row
            .get::<Option<String>, _>("origin_agent_id")
            .zip(row.get::<Option<String>, _>("origin_channel_id"))
            .map(|(agent_id, channel_id)| RunOrigin {
                agent_id: AgentId::from(agent_id),
                channel_id: ChannelId::from(channel_id),
                root_message_id: row
                    .get::<Option<String>, _>("origin_root_message_id")
                    .map(MessageId::from),
            }),
        state: state.parse::<RunState>().map_err(StoreError::Corrupt)?,
        failure_kind: row
            .get::<Option<String>, _>("failure_kind")
            .map(|kind| kind.parse())
            .transpose()
            .map_err(StoreError::Corrupt)?,
        error: row.get("error"),
        started_at: row.get("started_at"),
        ended_at: row.get("ended_at"),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl RunStore for SqliteRunStore {
    async fn create(&self, run: &Run) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO runs (id, workspace_id, agent_id, channel_id, root_message_id, \
             trigger_kind, trigger_ref, hop_count, origin_agent_id, origin_channel_id, \
             origin_root_message_id, state, error, failure_kind, started_at, ended_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(run.id.as_str())
        .bind(run.workspace_id.as_str())
        .bind(run.agent_id.as_str())
        .bind(run.channel_id.as_ref().map(|c| c.as_str().to_owned()))
        .bind(run.root_message_id.as_ref().map(|m| m.as_str().to_owned()))
        .bind(run.trigger_kind.as_str())
        .bind(&run.trigger_ref)
        .bind(i64::from(run.hop_count))
        .bind(run.origin.as_ref().map(|o| o.agent_id.as_str().to_owned()))
        .bind(
            run.origin
                .as_ref()
                .map(|o| o.channel_id.as_str().to_owned()),
        )
        .bind(
            run.origin
                .as_ref()
                .and_then(|o| o.root_message_id.as_ref())
                .map(|m| m.as_str().to_owned()),
        )
        .bind(run.state.as_str())
        .bind(&run.error)
        .bind(run.failure_kind.map(|kind| kind.as_str()))
        .bind(run.started_at)
        .bind(run.ended_at)
        .bind(run.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(&self, workspace_id: &WorkspaceId, id: &RunId) -> Result<Option<Run>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM runs WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_run).transpose()
    }

    async fn update(&self, run: &Run) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE runs SET root_message_id = ?, state = ?, error = ?, failure_kind = ?, started_at = ?, ended_at = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(run.root_message_id.as_ref().map(|id| id.as_str()))
        .bind(run.state.as_str())
        .bind(&run.error)
        .bind(run.failure_kind.map(|kind| kind.as_str()))
        .bind(run.started_at)
        .bind(run.ended_at)
        .bind(run.id.as_str())
        .bind(run.workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_unfinished(&self) -> Result<Vec<Run>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM runs \
             WHERE state NOT IN ('completed', 'failed', 'canceled') \
             ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_run).collect()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
        states: &[RunState],
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<Run>, StoreError> {
        // An empty set keeps every state, so the clause disappears.
        let state_filter = if states.is_empty() {
            String::new()
        } else {
            let placeholders = vec!["?"; states.len()].join(", ");
            format!(" AND state IN ({placeholders})")
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM runs \
             WHERE workspace_id = ? \
               AND (? IS NULL OR agent_id = ?) \
               AND (? IS NULL OR channel_id = ?){state_filter} \
               AND (? IS NULL OR id < ?) \
             ORDER BY id DESC LIMIT ?"
        );
        let mut query = sqlx::query(&sql)
            .bind(workspace_id.as_str())
            .bind(agent_id.map(|id| id.as_str()))
            .bind(agent_id.map(|id| id.as_str()))
            .bind(channel_id.map(|id| id.as_str()))
            .bind(channel_id.map(|id| id.as_str()));
        for state in states {
            query = query.bind(state.as_str());
        }
        let rows = query
            .bind(before.map(|id| id.as_str()))
            .bind(before.map(|id| id.as_str()))
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.iter().map(row_to_run).collect()
    }

    async fn record_source_reads(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        connection_id: &ConnectionId,
        reads: &[SourceRead],
    ) -> Result<(), StoreError> {
        if reads.is_empty() {
            return Ok(());
        }
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ? AND workspace_id = ?)",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(db_err)?;
        if !known {
            return Err(StoreError::Conflict("run unavailable".into()));
        }
        // A Forget blocks each new retrieval through its Connection, and
        // a read with no record must not reach the model (ADR-0008). The
        // write lock orders this check against the start of a Forget.
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM forget_suppressions WHERE workspace_id = ? AND connection_id = ?)",
        )
        .bind(workspace_id.as_str())
        .bind(connection_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(db_err)?;
        if blocked {
            return Err(StoreError::Conflict(
                "a Forget blocks the reads of this Connection".into(),
            ));
        }
        for read in reads {
            let (kind, resource, source_id) = match read {
                SourceRead::Item { resource, id } => ("item", resource, id),
                SourceRead::Parent { resource, id } => ("parent", resource, id),
            };
            sqlx::query(
                "INSERT INTO run_source_reads (workspace_id, run_id, connection_id, resource, kind, source_id) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            )
            .bind(workspace_id.as_str())
            .bind(run_id.as_str())
            .bind(connection_id.as_str())
            .bind(resource)
            .bind(kind)
            .bind(source_id)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        transaction.commit().await.map_err(db_err)
    }
}
