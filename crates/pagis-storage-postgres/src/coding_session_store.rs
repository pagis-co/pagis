//! The Coding Session records and their transcripts (ADR-0033).
//!
//! The daemon inserts the record when the session starts and writes it
//! again whole as the session moves. An update of the harness goes to
//! the transcript through [`pagis_core::coding_session::fold`], which
//! the SQLite store applies too: the last row is read and the writes
//! are made in one write transaction, so two updates of one session
//! never read the same last row.

use async_trait::async_trait;
use pagis_core::coding_session::{TranscriptWrite, fold};
use pagis_core::{
    AgentId, ChannelId, CodingSession, CodingSessionEvent, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, CodingSessionStore, CodingSessionUsage, HostId, MessageId,
    NewCodingSessionEvent, RunId, StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresCodingSessionStore {
    pool: PgPool,
}

impl PostgresCodingSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, harness_id, harness_version, place, host_id, \
     directory, working_directory, worktree_branch, approval_mode, title, state, end_reason, \
     end_detail, acp_session_id, channel_id, root_message_id, message_id, run_id, context_used, \
     context_size, cost_amount, cost_currency, created_at, updated_at, ended_at";

const EVENT_COLUMNS: &str = "workspace_id, coding_session_id, seq, at, kind, payload";

/// The terminal states, as the SQL of `count_open` and `list_open`
/// names them. `CodingSessionState::is_terminal` is the rule.
const TERMINAL: &str = "('closed', 'failed')";

fn parse<T: std::str::FromStr<Err = String>>(value: &str) -> Result<T, StoreError> {
    value.parse().map_err(StoreError::Corrupt)
}

fn count_to_db(value: Option<u64>) -> Result<Option<i64>, StoreError> {
    value
        .map(|count| {
            i64::try_from(count)
                .map_err(|_| StoreError::Corrupt(format!("{count} does not fit a token count")))
        })
        .transpose()
}

fn count_from_db(value: Option<i64>) -> Result<Option<u64>, StoreError> {
    value
        .map(|count| {
            u64::try_from(count)
                .map_err(|_| StoreError::Corrupt(format!("{count} is not a token count")))
        })
        .transpose()
}

fn from_row(row: &sqlx::postgres::PgRow) -> Result<CodingSession, StoreError> {
    Ok(CodingSession {
        id: CodingSessionId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        harness_id: row.get("harness_id"),
        harness_version: row.get("harness_version"),
        place: parse(&row.get::<String, _>("place"))?,
        host_id: row.get::<Option<String>, _>("host_id").map(HostId::from),
        directory: row.get("directory"),
        working_directory: row.get("working_directory"),
        worktree_branch: row.get("worktree_branch"),
        approval_mode: parse(&row.get::<String, _>("approval_mode"))?,
        title: row.get("title"),
        state: parse(&row.get::<String, _>("state"))?,
        end_reason: row.get("end_reason"),
        end_detail: row.get("end_detail"),
        acp_session_id: row.get("acp_session_id"),
        channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
        root_message_id: MessageId::from(row.get::<String, _>("root_message_id")),
        message_id: MessageId::from(row.get::<String, _>("message_id")),
        run_id: RunId::from(row.get::<String, _>("run_id")),
        usage: CodingSessionUsage {
            context_used: count_from_db(row.get("context_used"))?,
            context_size: count_from_db(row.get("context_size"))?,
            cost_amount: row.get("cost_amount"),
            cost_currency: row.get("cost_currency"),
        },
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        ended_at: row.get("ended_at"),
    })
}

fn event_from_row(row: &sqlx::postgres::PgRow) -> Result<CodingSessionEvent, StoreError> {
    Ok(CodingSessionEvent {
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        coding_session_id: CodingSessionId::from(row.get::<String, _>("coding_session_id")),
        seq: row.get("seq"),
        at: row.get("at"),
        kind: parse(&row.get::<String, _>("kind"))?,
        payload: serde_json::from_str(&row.get::<String, _>("payload")).map_err(|error| {
            StoreError::Corrupt(format!("a transcript payload is not JSON: {error}"))
        })?,
    })
}

fn payload_json(payload: &serde_json::Value) -> Result<String, StoreError> {
    serde_json::to_string(payload).map_err(|error| {
        StoreError::Corrupt(format!("a transcript payload does not serialize: {error}"))
    })
}

#[async_trait]
impl CodingSessionStore for PostgresCodingSessionStore {
    async fn insert(&self, session: &CodingSession) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO coding_sessions ({COLUMNS}) VALUES \
             ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
             $19, $20, $21, $22, $23, $24, $25, $26, $27)"
        ))
        .bind(session.id.as_str())
        .bind(session.workspace_id.as_str())
        .bind(session.agent_id.as_str())
        .bind(&session.harness_id)
        .bind(&session.harness_version)
        .bind(session.place.as_str())
        .bind(session.host_id.as_ref().map(HostId::as_str))
        .bind(&session.directory)
        .bind(session.working_directory.as_deref())
        .bind(session.worktree_branch.as_deref())
        .bind(session.approval_mode.as_str())
        .bind(&session.title)
        .bind(session.state.as_str())
        .bind(session.end_reason.as_deref())
        .bind(session.end_detail.as_deref())
        .bind(session.acp_session_id.as_deref())
        .bind(session.channel_id.as_str())
        .bind(session.root_message_id.as_str())
        .bind(session.message_id.as_str())
        .bind(session.run_id.as_str())
        .bind(count_to_db(session.usage.context_used)?)
        .bind(count_to_db(session.usage.context_size)?)
        .bind(session.usage.cost_amount)
        .bind(session.usage.cost_currency.as_deref())
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(session.ended_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update(&self, session: &CodingSession) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE coding_sessions SET agent_id = $1, harness_id = $2, harness_version = $3, \
             place = $4, host_id = $5, directory = $6, working_directory = $7, worktree_branch = $8, \
             approval_mode = $9, title = $10, state = $11, end_reason = $12, end_detail = $13, \
             acp_session_id = $14, channel_id = $15, root_message_id = $16, message_id = $17, \
             run_id = $18, context_used = $19, context_size = $20, cost_amount = $21, \
             cost_currency = $22, created_at = $23, updated_at = $24, ended_at = $25 \
             WHERE id = $26 AND workspace_id = $27",
        )
        .bind(session.agent_id.as_str())
        .bind(&session.harness_id)
        .bind(&session.harness_version)
        .bind(session.place.as_str())
        .bind(session.host_id.as_ref().map(HostId::as_str))
        .bind(&session.directory)
        .bind(session.working_directory.as_deref())
        .bind(session.worktree_branch.as_deref())
        .bind(session.approval_mode.as_str())
        .bind(&session.title)
        .bind(session.state.as_str())
        .bind(session.end_reason.as_deref())
        .bind(session.end_detail.as_deref())
        .bind(session.acp_session_id.as_deref())
        .bind(session.channel_id.as_str())
        .bind(session.root_message_id.as_str())
        .bind(session.message_id.as_str())
        .bind(session.run_id.as_str())
        .bind(count_to_db(session.usage.context_used)?)
        .bind(count_to_db(session.usage.context_size)?)
        .bind(session.usage.cost_amount)
        .bind(session.usage.cost_currency.as_deref())
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(session.ended_at)
        .bind(session.id.as_str())
        .bind(session.workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CodingSessionId,
    ) -> Result<Option<CodingSession>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM coding_sessions WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(from_row).transpose()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        state: Option<CodingSessionState>,
        before: Option<&CodingSessionId>,
        limit: u32,
    ) -> Result<Vec<CodingSession>, StoreError> {
        // A filter that the caller left out is a null bind. Postgres
        // reads the type of a placeholder from its use, and `IS NULL`
        // alone names none, so every guard casts its own placeholder.
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM coding_sessions \
             WHERE workspace_id = $1 \
               AND ($2::text IS NULL OR agent_id = $3) \
               AND ($4::text IS NULL OR state = $5) \
               AND ($6::text IS NULL OR id < $7) \
             ORDER BY id DESC LIMIT $8"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.map(AgentId::as_str))
        .bind(agent_id.map(AgentId::as_str))
        .bind(state.map(CodingSessionState::as_str))
        .bind(state.map(CodingSessionState::as_str))
        .bind(before.map(CodingSessionId::as_str))
        .bind(before.map(CodingSessionId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(from_row).collect()
    }

    async fn count_open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u32, StoreError> {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM coding_sessions \
             WHERE workspace_id = $1 AND agent_id = $2 AND state NOT IN {TERMINAL}"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        u32::try_from(count)
            .map_err(|_| StoreError::Corrupt(format!("{count} is not a count of sessions")))
    }

    async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM coding_sessions WHERE state NOT IN {TERMINAL} ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(from_row).collect()
    }

    async fn append_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        event: NewCodingSessionEvent,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        // The lock on the session row holds from the read of the last
        // row to the commit, so a second update of the session waits
        // here and then reads the row that this one wrote.
        let mut tx = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let held = sqlx::query(
            "SELECT 1 FROM coding_sessions WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(coding_session_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if held.is_none() {
            return Ok(Vec::new());
        }
        let last = sqlx::query(&format!(
            "SELECT {EVENT_COLUMNS} FROM coding_session_events \
             WHERE coding_session_id = $1 ORDER BY seq DESC LIMIT 1"
        ))
        .bind(coding_session_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .as_ref()
        .map(event_from_row)
        .transpose()?;

        let mut written = Vec::new();
        for write in fold(last.as_ref(), event) {
            let row = match write {
                TranscriptWrite::Replace { seq, payload } => {
                    sqlx::query(
                        "UPDATE coding_session_events SET payload = $1 \
                         WHERE coding_session_id = $2 AND seq = $3",
                    )
                    .bind(payload_json(&payload)?)
                    .bind(coding_session_id.as_str())
                    .bind(seq)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                    let replaced = last.clone().ok_or_else(|| {
                        StoreError::Corrupt("the transcript rewrote a row it does not hold".into())
                    })?;
                    CodingSessionEvent {
                        payload,
                        ..replaced
                    }
                }
                TranscriptWrite::Append {
                    seq,
                    at,
                    kind,
                    payload,
                } => {
                    sqlx::query(&format!(
                        "INSERT INTO coding_session_events ({EVENT_COLUMNS}) \
                         VALUES ($1, $2, $3, $4, $5, $6)"
                    ))
                    .bind(workspace_id.as_str())
                    .bind(coding_session_id.as_str())
                    .bind(seq)
                    .bind(at)
                    .bind(kind.as_str())
                    .bind(payload_json(&payload)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                    CodingSessionEvent {
                        workspace_id: workspace_id.clone(),
                        coding_session_id: coding_session_id.clone(),
                        seq,
                        at,
                        kind,
                        payload,
                    }
                }
            };
            written.push(row);
        }
        tx.commit().await.map_err(db_err)?;
        Ok(written)
    }

    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {EVENT_COLUMNS} FROM coding_session_events \
             WHERE workspace_id = $1 AND coding_session_id = $2 AND seq > $3 \
             ORDER BY seq LIMIT $4"
        ))
        .bind(workspace_id.as_str())
        .bind(coding_session_id.as_str())
        .bind(after.unwrap_or(0))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(event_from_row).collect()
    }

    async fn latest_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        kinds: &[CodingSessionEventKind],
    ) -> Result<Option<CodingSessionEvent>, StoreError> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let marks = (3..3 + kinds.len())
            .map(|n| format!("${n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {EVENT_COLUMNS} FROM coding_session_events \
             WHERE workspace_id = $1 AND coding_session_id = $2 AND kind IN ({marks}) \
             ORDER BY seq DESC LIMIT 1"
        );
        let mut query = sqlx::query(&sql)
            .bind(workspace_id.as_str())
            .bind(coding_session_id.as_str());
        for kind in kinds {
            query = query.bind(kind.as_str());
        }
        let row = query.fetch_optional(&self.pool).await.map_err(db_err)?;
        row.as_ref().map(event_from_row).transpose()
    }
}
