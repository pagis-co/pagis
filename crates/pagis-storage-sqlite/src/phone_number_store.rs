//! The Agent Phone Number records and their purchase intents
//! (ADR-0018).
//!
//! Every write that changes who holds a line goes through this store,
//! and the schema keeps the one-Agent-one-number rule: a partial unique
//! index on `agent_id` refuses the second holder, and the refusal
//! arrives at the caller as [`StoreError::Conflict`].

use async_trait::async_trait;
use pagis_core::{
    AgentId, ConnectionId, MessagingReadiness, PhoneNumber, PhoneNumberId, PhoneNumberStatus,
    PhoneNumberStore, PurchaseIntent, PurchaseIntentId, StoreError, TelnyxRelayState, UnixMillis,
    WorkspaceId, day_start,
};
use sqlx::{Row, SqlitePool};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct SqlitePhoneNumberStore {
    pool: SqlitePool,
}

impl SqlitePhoneNumberStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const NUMBER_COLUMNS: &str = "id, workspace_id, connection_id, e164, provider_number_id, \
     agent_id, status, outgoing_cap, allow_rules, sends_day, sends_today, messaging_readiness, \
     messaging_readiness_reason, messaging_readiness_at, messaging_readiness_error, text_cursor, \
     messaging_object_id, relay_state, relay_reason, relay_cli_lines, relay_consent, created_at, \
     assigned_at";

/// The number of `?` placeholders one number row takes.
const NUMBER_VALUES: &str = "?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?";

/// A list of strings as the row stores it: one JSON array.
fn strings_json(values: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(values).map_err(|error| StoreError::Corrupt(error.to_string()))
}

fn strings_from_json(text: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(text).map_err(|error| StoreError::Corrupt(error.to_string()))
}

const INTENT_COLUMNS: &str =
    "id, workspace_id, connection_id, e164, agent_id, state, created_at, settled_at";

fn number_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<PhoneNumber, StoreError> {
    let status: String = row.get("status");
    Ok(PhoneNumber {
        id: PhoneNumberId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        e164: row.get("e164"),
        provider_number_id: row.get("provider_number_id"),
        agent_id: row.get::<Option<String>, _>("agent_id").map(AgentId::from),
        status: status.parse().map_err(StoreError::Corrupt)?,
        outgoing_cap: row.get::<i64, _>("outgoing_cap") as u32,
        allow_rules: strings_from_json(row.get::<&str, _>("allow_rules"))?,
        sends_day: row.get("sends_day"),
        sends_today: row.get::<i64, _>("sends_today") as u32,
        messaging_readiness: MessagingReadiness::parse(
            row.get::<&str, _>("messaging_readiness"),
            row.get::<Option<String>, _>("messaging_readiness_reason")
                .as_deref(),
        )
        .map_err(StoreError::Corrupt)?,
        messaging_readiness_at: row.get("messaging_readiness_at"),
        messaging_readiness_error: row.get("messaging_readiness_error"),
        text_cursor: row.get("text_cursor"),
        messaging_object_id: row.get("messaging_object_id"),
        relay_state: TelnyxRelayState::parse(
            row.get::<&str, _>("relay_state"),
            row.get::<Option<String>, _>("relay_reason").as_deref(),
            strings_from_json(row.get::<&str, _>("relay_cli_lines"))?,
        )
        .map_err(StoreError::Corrupt)?,
        relay_consent: row.get::<i64, _>("relay_consent") != 0,
        created_at: row.get("created_at"),
        assigned_at: row.get("assigned_at"),
    })
}

/// Every value of one number row, in the order [`NUMBER_COLUMNS`]
/// names them. Two inserts write a number, so they bind it once.
fn bind_number<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    number: &'q PhoneNumber,
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>, StoreError> {
    Ok(query
        .bind(number.id.as_str())
        .bind(number.workspace_id.as_str())
        .bind(number.connection_id.as_str())
        .bind(&number.e164)
        .bind(&number.provider_number_id)
        .bind(number.agent_id.as_ref().map(AgentId::as_str))
        .bind(number.status.as_str())
        .bind(i64::from(number.outgoing_cap))
        .bind(strings_json(&number.allow_rules)?)
        .bind(number.sends_day)
        .bind(i64::from(number.sends_today))
        .bind(number.messaging_readiness.as_str())
        .bind(number.messaging_readiness.reason().map(str::to_string))
        .bind(number.messaging_readiness_at)
        .bind(number.messaging_readiness_error.clone())
        .bind(number.text_cursor.clone())
        .bind(number.messaging_object_id.clone())
        .bind(number.relay_state.as_str())
        .bind(number.relay_state.reason().map(str::to_string))
        .bind(strings_json(number.relay_state.cli_lines())?)
        .bind(i64::from(number.relay_consent))
        .bind(number.created_at)
        .bind(number.assigned_at))
}

fn intent_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<PurchaseIntent, StoreError> {
    let state: String = row.get("state");
    Ok(PurchaseIntent {
        id: PurchaseIntentId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        e164: row.get("e164"),
        agent_id: row.get::<Option<String>, _>("agent_id").map(AgentId::from),
        state: state.parse().map_err(StoreError::Corrupt)?,
        created_at: row.get("created_at"),
        settled_at: row.get("settled_at"),
    })
}

/// A number the schema refused belongs to an Agent or a Workspace that
/// already has one. The message is the one the user reads.
fn assignment_conflict(error: sqlx::Error) -> StoreError {
    match unique_violation(&error) {
        true => StoreError::Conflict(
            "that Agent already holds a number, or that number is already held".to_string(),
        ),
        false => db_err(error),
    }
}

#[async_trait]
impl PhoneNumberStore for SqlitePhoneNumberStore {
    async fn create_intent(&self, intent: &PurchaseIntent) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO phone_number_purchase_intents ({INTENT_COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(intent.id.as_str())
        .bind(intent.workspace_id.as_str())
        .bind(intent.connection_id.as_str())
        .bind(&intent.e164)
        .bind(intent.agent_id.as_ref().map(AgentId::as_str))
        .bind(intent.state.as_str())
        .bind(intent.created_at)
        .bind(intent.settled_at)
        .execute(&self.pool)
        .await
        .map_err(|error| match unique_violation(&error) {
            true => StoreError::Conflict(format!("{} is already being bought", intent.e164)),
            false => db_err(error),
        })?;
        Ok(())
    }

    async fn list_pending_intents(&self) -> Result<Vec<PurchaseIntent>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {INTENT_COLUMNS} FROM phone_number_purchase_intents \
             WHERE state = 'pending' ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(intent_from_row).collect()
    }

    async fn confirm_purchase(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        number: &PhoneNumber,
        at: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let sql = format!("INSERT INTO phone_numbers ({NUMBER_COLUMNS}) VALUES ({NUMBER_VALUES})");
        bind_number(sqlx::query(&sql), number)?
            .execute(&mut *transaction)
            .await
            .map_err(assignment_conflict)?;
        sqlx::query(
            "UPDATE phone_number_purchase_intents SET state = 'bought', settled_at = ? \
             WHERE id = ? AND workspace_id = ? AND state = 'pending'",
        )
        .bind(at)
        .bind(intent_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn create(&self, number: &PhoneNumber) -> Result<(), StoreError> {
        let sql = format!("INSERT INTO phone_numbers ({NUMBER_COLUMNS}) VALUES ({NUMBER_VALUES})");
        bind_number(sqlx::query(&sql), number)?
            .execute(&self.pool)
            .await
            .map_err(assignment_conflict)?;
        Ok(())
    }

    async fn abandon_intent(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_number_purchase_intents SET state = 'abandoned', settled_at = ? \
             WHERE id = ? AND workspace_id = ? AND state = 'pending'",
        )
        .bind(at)
        .bind(intent_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {NUMBER_COLUMNS} FROM phone_numbers WHERE workspace_id = ? AND id = ?"
        ))
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(number_from_row).transpose()
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {NUMBER_COLUMNS} FROM phone_numbers WHERE workspace_id = ? ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(number_from_row).collect()
    }

    async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {NUMBER_COLUMNS} FROM phone_numbers WHERE agent_id = ? AND workspace_id = ?"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(number_from_row).transpose()
    }

    async fn live_for_e164(&self, e164: &str) -> Result<Vec<PhoneNumber>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {NUMBER_COLUMNS} FROM phone_numbers \
             WHERE e164 = ? AND status <> 'released' ORDER BY id"
        ))
        .bind(e164)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(number_from_row).collect()
    }

    async fn assign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        agent_id: &AgentId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET agent_id = ?, status = 'assigned', assigned_at = ? \
             WHERE id = ? AND workspace_id = ? AND status = 'unassigned'",
        )
        .bind(agent_id.as_str())
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(assignment_conflict)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn unassign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET agent_id = NULL, status = 'unassigned', assigned_at = NULL \
             WHERE id = ? AND workspace_id = ? AND status = 'assigned'",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn unassign_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        let Some(number) = self.for_agent(workspace_id, agent_id).await? else {
            return Ok(None);
        };
        self.unassign(workspace_id, &number.id).await?;
        Ok(Some(PhoneNumber {
            agent_id: None,
            status: PhoneNumberStatus::Unassigned,
            assigned_at: None,
            ..number
        }))
    }

    async fn release(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<bool, StoreError> {
        // A released number keeps its row and loses its holder: the
        // Calls that point at it still read, and the charge stops.
        let updated = sqlx::query(
            "UPDATE phone_numbers SET agent_id = NULL, status = 'released', assigned_at = NULL \
             WHERE id = ? AND workspace_id = ? AND status <> 'released'",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn any_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<bool, StoreError> {
        let row = sqlx::query(
            "SELECT 1 FROM phone_numbers \
             WHERE connection_id = ? AND workspace_id = ? AND status <> 'released' LIMIT 1",
        )
        .bind(connection_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.is_some())
    }

    async fn set_outgoing_cap(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cap: u32,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET outgoing_cap = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(i64::from(cap))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn add_allow_rules(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let row =
            sqlx::query("SELECT allow_rules FROM phone_numbers WHERE id = ? AND workspace_id = ?")
                .bind(id.as_str())
                .bind(workspace_id.as_str())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(db_err)?
                .ok_or_else(|| {
                    StoreError::Conflict("that number is gone, so it takes no rules".to_string())
                })?;
        let mut held = strings_from_json(row.get::<&str, _>("allow_rules"))?;
        for rule in rules {
            if !held.contains(rule) {
                held.push(rule.clone());
            }
        }
        sqlx::query("UPDATE phone_numbers SET allow_rules = ? WHERE id = ? AND workspace_id = ?")
            .bind(strings_json(&held)?)
            .bind(id.as_str())
            .bind(workspace_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(held)
    }

    async fn note_text_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        at: UnixMillis,
    ) -> Result<u32, StoreError> {
        let day = day_start(at);
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(
            "UPDATE phone_numbers \
             SET sends_today = CASE WHEN sends_day = ? THEN sends_today + 1 ELSE 1 END, \
                 sends_day = ? \
             WHERE id = ? AND workspace_id = ?",
        )
        .bind(day)
        .bind(day)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let count =
            sqlx::query("SELECT sends_today FROM phone_numbers WHERE id = ? AND workspace_id = ?")
                .bind(id.as_str())
                .bind(workspace_id.as_str())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(db_err)?
                .map(|row| row.get::<i64, _>("sends_today") as u32)
                .unwrap_or(0);
        transaction.commit().await.map_err(db_err)?;
        Ok(count)
    }

    async fn set_messaging_readiness(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        readiness: &MessagingReadiness,
        at: UnixMillis,
        error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers \
             SET messaging_readiness = ?, messaging_readiness_reason = ?, \
                 messaging_readiness_at = ?, messaging_readiness_error = ? \
             WHERE id = ? AND workspace_id = ?",
        )
        .bind(readiness.as_str())
        .bind(readiness.reason())
        .bind(at)
        .bind(error)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_text_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cursor: &str,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET text_cursor = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(cursor)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_messaging_object_id(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        object_id: &str,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET messaging_object_id = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(object_id)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_relay_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        state: &TelnyxRelayState,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers \
             SET relay_state = ?, relay_reason = ?, relay_cli_lines = ? \
             WHERE id = ? AND workspace_id = ?",
        )
        .bind(state.as_str())
        .bind(state.reason())
        .bind(strings_json(state.cli_lines())?)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_relay_consent(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        consent: bool,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE phone_numbers SET relay_consent = ? WHERE id = ? AND workspace_id = ?",
        )
        .bind(i64::from(consent))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }
}
