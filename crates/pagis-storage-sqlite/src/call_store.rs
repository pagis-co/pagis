//! The Call records (ADR-0020).
//!
//! The bridge inserts the record when the call starts and writes it
//! again as the call moves: ringing, answered, and ended with a reason.
//! The record is what the UI and the Run read afterwards.

use async_trait::async_trait;
use pagis_core::{
    AgentId, ArtifactId, Call, CallDirection, CallId, CallOutcome, CallState, CallStore,
    Classification, PhoneNumberId, RunId, StoreError, TranscriptLine, TrustTier, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteCallStore {
    pool: SqlitePool,
}

impl SqliteCallStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, run_id, phone_number_id, direction, \
     remote_e164, agent_name, own_e164, purpose, tools, tier, state, outcome, ended_reason, \
     classification, message_left, transcript, recording_artifact_id, created_at, ringing_at, \
     answered_at, ended_at";

/// The transcript goes to the row as JSON, so its times survive.
fn transcript_json(lines: &[TranscriptLine]) -> Result<String, StoreError> {
    serde_json::to_string(lines)
        .map_err(|error| StoreError::Corrupt(format!("the transcript does not serialize: {error}")))
}

/// The tool names go to the row as JSON, because a name can hold
/// anything a separator could.
fn tools_json(tools: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(tools)
        .map_err(|error| StoreError::Corrupt(format!("the tool list does not serialize: {error}")))
}

fn parse<T: std::str::FromStr<Err = String>>(value: &str) -> Result<T, StoreError> {
    value.parse().map_err(StoreError::Corrupt)
}

fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Call, StoreError> {
    let outcome = row.get::<Option<String>, _>("outcome");
    let classification = row.get::<Option<String>, _>("classification");
    Ok(Call {
        id: CallId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        run_id: RunId::from(row.get::<String, _>("run_id")),
        phone_number_id: PhoneNumberId::from(row.get::<String, _>("phone_number_id")),
        direction: parse::<CallDirection>(&row.get::<String, _>("direction"))?,
        remote_e164: row.get("remote_e164"),
        agent_name: row.get("agent_name"),
        own_e164: row.get("own_e164"),
        purpose: row.get("purpose"),
        tools: serde_json::from_str(&row.get::<String, _>("tools"))
            .map_err(|error| StoreError::Corrupt(format!("the tool list is not JSON: {error}")))?,
        tier: parse::<TrustTier>(&row.get::<String, _>("tier"))?,
        state: parse::<CallState>(&row.get::<String, _>("state"))?,
        outcome: outcome.as_deref().map(parse::<CallOutcome>).transpose()?,
        ended_reason: row.get("ended_reason"),
        classification: classification
            .as_deref()
            .map(parse::<Classification>)
            .transpose()?,
        message_left: row.get::<i64, _>("message_left") != 0,
        transcript: serde_json::from_str(&row.get::<String, _>("transcript"))
            .map_err(|error| StoreError::Corrupt(format!("the transcript is not JSON: {error}")))?,
        recording_artifact_id: row
            .get::<Option<String>, _>("recording_artifact_id")
            .map(ArtifactId::from),
        created_at: row.get("created_at"),
        ringing_at: row.get("ringing_at"),
        answered_at: row.get("answered_at"),
        ended_at: row.get("ended_at"),
    })
}

#[async_trait]
impl CallStore for SqliteCallStore {
    async fn insert(&self, call: &Call) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO calls ({COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(call.id.as_str())
        .bind(call.workspace_id.as_str())
        .bind(call.agent_id.as_str())
        .bind(call.run_id.as_str())
        .bind(call.phone_number_id.as_str())
        .bind(call.direction.as_str())
        .bind(&call.remote_e164)
        .bind(&call.agent_name)
        .bind(&call.own_e164)
        .bind(&call.purpose)
        .bind(tools_json(&call.tools)?)
        .bind(call.tier.as_str())
        .bind(call.state.as_str())
        .bind(call.outcome.map(CallOutcome::as_str))
        .bind(call.ended_reason.as_deref())
        .bind(call.classification.map(Classification::as_str))
        .bind(i64::from(call.message_left))
        .bind(transcript_json(&call.transcript)?)
        .bind(call.recording_artifact_id.as_ref().map(ArtifactId::as_str))
        .bind(call.created_at)
        .bind(call.ringing_at)
        .bind(call.answered_at)
        .bind(call.ended_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update(&self, call: &Call) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE calls SET tier = ?, state = ?, outcome = ?, ended_reason = ?, \
             classification = ?, message_left = ?, transcript = ?, \
             recording_artifact_id = ?, ringing_at = ?, answered_at = ?, ended_at = ? \
             WHERE id = ? AND workspace_id = ?",
        )
        .bind(call.tier.as_str())
        .bind(call.state.as_str())
        .bind(call.outcome.map(CallOutcome::as_str))
        .bind(call.ended_reason.as_deref())
        .bind(call.classification.map(Classification::as_str))
        .bind(i64::from(call.message_left))
        .bind(transcript_json(&call.transcript)?)
        .bind(call.recording_artifact_id.as_ref().map(ArtifactId::as_str))
        .bind(call.ringing_at)
        .bind(call.answered_at)
        .bind(call.ended_at)
        .bind(call.id.as_str())
        .bind(call.workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CallId,
    ) -> Result<Option<Call>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM calls WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(from_row).transpose()
    }

    async fn list_unsettled(&self) -> Result<Vec<Call>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM calls WHERE state != 'ended' ORDER BY created_at"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(from_row).collect()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        direction: Option<CallDirection>,
        state: Option<CallState>,
        before: Option<&CallId>,
        limit: u32,
    ) -> Result<Vec<Call>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM calls \
             WHERE workspace_id = ? \
               AND (? IS NULL OR agent_id = ?) \
               AND (? IS NULL OR direction = ?) \
               AND (? IS NULL OR state = ?) \
               AND (? IS NULL OR id < ?) \
             ORDER BY id DESC LIMIT ?"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.map(|id| id.as_str()))
        .bind(agent_id.map(|id| id.as_str()))
        .bind(direction.map(CallDirection::as_str))
        .bind(direction.map(CallDirection::as_str))
        .bind(state.map(CallState::as_str))
        .bind(state.map(CallState::as_str))
        .bind(before.map(|id| id.as_str()))
        .bind(before.map(|id| id.as_str()))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(from_row).collect()
    }
}
