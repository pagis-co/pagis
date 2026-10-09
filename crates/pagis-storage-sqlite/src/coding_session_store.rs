//! The Coding Session records and their transcripts (ADR-0033).
//!
//! The daemon inserts the record when the session starts and writes it
//! again whole as the session moves. An update of the harness goes to
//! the transcript through [`pagis_core::coding_session::fold`], which
//! the Postgres store applies too: the last row is read and the writes
//! are made in one write transaction, so two updates of one session
//! never read the same last row.

use async_trait::async_trait;
use pagis_core::coding_session::{TranscriptWrite, fold};
use pagis_core::{
    AgentId, ChannelId, CodingSession, CodingSessionEvent, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, CodingSessionStore, CodingSessionUsage, HarnessSetting, HostId, MessageId,
    ModelTokenOwner, NewCodingSessionEvent, RunId, StoreError, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteCodingSessionStore {
    pool: SqlitePool,
}

impl SqliteCodingSessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, harness_id, harness_version, place, host_id, \
     directory, working_directory, worktree_branch, approval_mode, title, state, end_reason, \
     end_detail, acp_session_id, channel_id, root_message_id, message_id, run_id, context_used, \
     context_size, cost_amount, cost_currency, created_at, updated_at, ended_at, harness_mode, \
     harness_modes, model, thought_level";

const EVENT_COLUMNS: &str = "workspace_id, coding_session_id, seq, at, kind, payload";

/// The terminal states, as the SQL of `count_open` and `list_open`
/// names them. `CodingSessionState::is_terminal` is the rule.
const TERMINAL: &str = "('closed', 'failed')";

/// The states in which the harness of a session runs, so its token of
/// the Harness Model Endpoint is valid.
const HARNESS_RUNS: &str = "('starting', 'working', 'needs_decision', 'idle')";

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

fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<CodingSession, StoreError> {
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
        harness_mode: row.get("harness_mode"),
        harness_modes: serde_json::from_str(&row.get::<String, _>("harness_modes")).map_err(
            |error| StoreError::Corrupt(format!("the Harness Modes are not JSON: {error}")),
        )?,
        model: setting_from_db(row.get("model"))?,
        thought_level: setting_from_db(row.get("thought_level"))?,
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

fn event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<CodingSessionEvent, StoreError> {
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

fn modes_json(session: &CodingSession) -> Result<String, StoreError> {
    serde_json::to_string(&session.harness_modes).map_err(|error| {
        StoreError::Corrupt(format!("the Harness Modes do not serialize: {error}"))
    })
}

/// A Harness Setting as the JSON of its column, or `NULL` for none.
fn setting_json(setting: Option<&HarnessSetting>) -> Result<Option<String>, StoreError> {
    setting
        .map(|setting| {
            serde_json::to_string(setting).map_err(|error| {
                StoreError::Corrupt(format!("a Harness Setting does not serialize: {error}"))
            })
        })
        .transpose()
}

fn setting_from_db(value: Option<String>) -> Result<Option<HarnessSetting>, StoreError> {
    value
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                StoreError::Corrupt(format!("a Harness Setting is not JSON: {error}"))
            })
        })
        .transpose()
}

fn payload_json(payload: &serde_json::Value) -> Result<String, StoreError> {
    serde_json::to_string(payload).map_err(|error| {
        StoreError::Corrupt(format!("a transcript payload does not serialize: {error}"))
    })
}

#[async_trait]
impl CodingSessionStore for SqliteCodingSessionStore {
    async fn insert(&self, session: &CodingSession) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO coding_sessions ({COLUMNS}) VALUES \
             (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
             ?)"
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
        .bind(session.harness_mode.as_deref())
        .bind(modes_json(session)?)
        .bind(setting_json(session.model.as_ref())?)
        .bind(setting_json(session.thought_level.as_ref())?)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update(&self, session: &CodingSession) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE coding_sessions SET agent_id = ?, harness_id = ?, harness_version = ?, \
             place = ?, host_id = ?, directory = ?, working_directory = ?, worktree_branch = ?, \
             approval_mode = ?, title = ?, state = ?, end_reason = ?, end_detail = ?, \
             acp_session_id = ?, channel_id = ?, root_message_id = ?, message_id = ?, \
             run_id = ?, context_used = ?, context_size = ?, cost_amount = ?, \
             cost_currency = ?, created_at = ?, updated_at = ?, ended_at = ?, harness_mode = ?, \
             harness_modes = ?, model = ?, thought_level = ? WHERE id = ? AND workspace_id = ?",
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
        .bind(session.harness_mode.as_deref())
        .bind(modes_json(session)?)
        .bind(setting_json(session.model.as_ref())?)
        .bind(setting_json(session.thought_level.as_ref())?)
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
            "SELECT {COLUMNS} FROM coding_sessions WHERE id = ? AND workspace_id = ?"
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
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM coding_sessions \
             WHERE workspace_id = ? \
               AND (? IS NULL OR agent_id = ?) \
               AND (? IS NULL OR state = ?) \
               AND (? IS NULL OR id < ?) \
             ORDER BY id DESC LIMIT ?"
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
             WHERE workspace_id = ? AND agent_id = ? AND state NOT IN {TERMINAL}"
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
        // `BEGIN IMMEDIATE` holds the write lock from the read of the
        // last row to the commit, so no other update reads it between.
        let mut tx = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let held = sqlx::query("SELECT 1 FROM coding_sessions WHERE id = ? AND workspace_id = ?")
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
             WHERE coding_session_id = ? ORDER BY seq DESC LIMIT 1"
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
                        "UPDATE coding_session_events SET payload = ? \
                         WHERE coding_session_id = ? AND seq = ?",
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
                         VALUES (?, ?, ?, ?, ?, ?)"
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
             WHERE workspace_id = ? AND coding_session_id = ? AND seq > ? \
             ORDER BY seq LIMIT ?"
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
        let marks = vec!["?"; kinds.len()].join(", ");
        let sql = format!(
            "SELECT {EVENT_COLUMNS} FROM coding_session_events \
             WHERE workspace_id = ? AND coding_session_id = ? AND kind IN ({marks}) \
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

    async fn set_model_token(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        hash: &str,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE coding_sessions SET model_token_hash = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(hash)
        .bind(coding_session_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn model_token_owner(&self, hash: &str) -> Result<Option<ModelTokenOwner>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT workspace_id, agent_id, id, run_id FROM coding_sessions \
             WHERE model_token_hash = ? AND state IN {HARNESS_RUNS}"
        ))
        .bind(hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|row| ModelTokenOwner {
            workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
            agent_id: AgentId::from(row.get::<String, _>("agent_id")),
            session_id: CodingSessionId::from(row.get::<String, _>("id")),
            run_id: RunId::from(row.get::<String, _>("run_id")),
        }))
    }
}
