//! The Agent Mailbox records and the Address Ledger (ADR-0019).
//!
//! The schema owns the two rules the desk must never break. A unique
//! index on `address`, with no exception for tombstones, is the Address
//! Ledger, so an address Pagis ever made or assigned is never given out
//! again. A partial unique index on `agent_id` gives one Agent one live
//! mailbox. Both refusals arrive at the caller as
//! [`StoreError::Conflict`].

use async_trait::async_trait;
use pagis_core::{
    AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, AgentMailboxStore, ConnectionId,
    MailboxCursor, StoreError, UnixMillis, WorkspaceId, day_start,
};
use sqlx::{PgPool, Row};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct PostgresAgentMailboxStore {
    pool: PgPool,
}

impl PostgresAgentMailboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, connection_id, address, state, reason, \
     outgoing_cap, allow_rules, cursor_folder, cursor_uid_validity, cursor_last_uid, sends_day, \
     sends_today, created_at, deleted_at";

fn mailbox_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentMailbox, StoreError> {
    let state: String = row.get("state");
    let cursor = row
        .get::<Option<String>, _>("cursor_folder")
        .map(|folder| MailboxCursor {
            folder,
            uid_validity: row.get::<i64, _>("cursor_uid_validity") as u32,
            last_uid: row.get::<i64, _>("cursor_last_uid") as u32,
        });
    Ok(AgentMailbox {
        id: AgentMailboxId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        address: row.get("address"),
        state: state.parse().map_err(StoreError::Corrupt)?,
        reason: row.get("reason"),
        outgoing_cap: row.get::<i64, _>("outgoing_cap") as u32,
        allow_rules: serde_json::from_str(row.get::<&str, _>("allow_rules"))
            .map_err(|error| StoreError::Corrupt(error.to_string()))?,
        cursor,
        sends_day: row.get("sends_day"),
        sends_today: row.get::<i64, _>("sends_today") as u32,
        created_at: row.get("created_at"),
        deleted_at: row.get("deleted_at"),
    })
}

/// The allow rules as the row stores them: one JSON array of
/// registrable domains.
fn allow_rules_json(rules: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(rules).map_err(|error| StoreError::Corrupt(error.to_string()))
}

/// The ledger or the one-mailbox rule refused the row. The message is
/// the one the creation form shows.
fn reservation_conflict(error: sqlx::Error) -> StoreError {
    match unique_violation(&error) {
        true => StoreError::Conflict(
            "that address is taken, or that Agent already holds a mailbox".to_string(),
        ),
        false => db_err(error),
    }
}

#[async_trait]
impl AgentMailboxStore for PostgresAgentMailboxStore {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO agent_mailboxes ({COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"
        ))
        .bind(mailbox.id.as_str())
        .bind(mailbox.workspace_id.as_str())
        .bind(mailbox.agent_id.as_str())
        .bind(mailbox.connection_id.as_str())
        .bind(&mailbox.address)
        .bind(mailbox.state.as_str())
        .bind(mailbox.reason.as_deref())
        .bind(i64::from(mailbox.outgoing_cap))
        .bind(allow_rules_json(&mailbox.allow_rules)?)
        .bind(mailbox.cursor.as_ref().map(|cursor| cursor.folder.clone()))
        .bind(
            mailbox
                .cursor
                .as_ref()
                .map(|cursor| i64::from(cursor.uid_validity)),
        )
        .bind(
            mailbox
                .cursor
                .as_ref()
                .map(|cursor| i64::from(cursor.last_uid)),
        )
        .bind(mailbox.sends_day)
        .bind(i64::from(mailbox.sends_today))
        .bind(mailbox.created_at)
        .bind(mailbox.deleted_at)
        .execute(&self.pool)
        .await
        .map_err(reservation_conflict)?;
        Ok(())
    }

    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<bool, StoreError> {
        // Only a mailbox the host never made goes away without a
        // tombstone. Every other end keeps its row in the ledger.
        let deleted = sqlx::query(
            "DELETE FROM agent_mailboxes WHERE id = $1 AND workspace_id = $2 AND state = $3",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Provisioning.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM agent_mailboxes WHERE workspace_id = $1 AND id = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(mailbox_from_row).transpose()
    }

    async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM agent_mailboxes \
             WHERE agent_id = $1 AND workspace_id = $2 AND state <> $3"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(mailbox_from_row).transpose()
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<AgentMailbox>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM agent_mailboxes \
             WHERE workspace_id = $1 AND state <> $2 ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(mailbox_from_row).collect()
    }

    async fn address_taken(&self, address: &str) -> Result<bool, StoreError> {
        // The whole ledger answers: every Connection, and the
        // tombstones too.
        let row = sqlx::query("SELECT 1 FROM agent_mailboxes WHERE address = $1 LIMIT 1")
            .bind(address.to_lowercase())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row.is_some())
    }

    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        state: AgentMailboxState,
        reason: Option<&str>,
    ) -> Result<bool, StoreError> {
        if state == AgentMailboxState::Deleted {
            return Err(StoreError::Conflict(
                "a tombstone is written by a delete, not by a state change".to_string(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE agent_mailboxes SET state = $1, reason = $2 \
             WHERE id = $3 AND workspace_id = $4 AND state <> $5",
        )
        .bind(state.as_str())
        .bind(reason)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        cursor: &MailboxCursor,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE agent_mailboxes \
             SET cursor_folder = $1, cursor_uid_validity = $2, cursor_last_uid = $3 \
             WHERE id = $4 AND workspace_id = $5 AND state <> $6",
        )
        .bind(&cursor.folder)
        .bind(i64::from(cursor.uid_validity))
        .bind(i64::from(cursor.last_uid))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn make_dormant_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        let Some(mailbox) = self.for_agent(workspace_id, agent_id).await? else {
            return Ok(None);
        };
        self.set_state(workspace_id, &mailbox.id, AgentMailboxState::Dormant, None)
            .await?;
        Ok(Some(AgentMailbox {
            state: AgentMailboxState::Dormant,
            reason: None,
            ..mailbox
        }))
    }

    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        // The tombstone keeps the address, the Agent and the time, and
        // loses the cursor and the reason: nothing reads a deleted
        // mailbox but the ledger.
        let updated = sqlx::query(
            "UPDATE agent_mailboxes \
             SET state = $1, reason = NULL, deleted_at = $2, \
                 cursor_folder = NULL, cursor_uid_validity = NULL, cursor_last_uid = NULL \
             WHERE id = $3 AND workspace_id = $4 AND state <> $5",
        )
        .bind(AgentMailboxState::Deleted.as_str())
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn note_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<u32, StoreError> {
        let day = day_start(at);
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(
            "UPDATE agent_mailboxes \
             SET sends_today = CASE WHEN sends_day = $1 THEN sends_today + 1 ELSE 1 END, \
                 sends_day = $2 \
             WHERE id = $3 AND workspace_id = $4 AND state <> $5",
        )
        .bind(day)
        .bind(day)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let count = sqlx::query(
            "SELECT sends_today FROM agent_mailboxes WHERE id = $1 AND workspace_id = $2",
        )
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

    async fn add_allow_rules(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let row = sqlx::query(
            "SELECT allow_rules FROM agent_mailboxes \
                 WHERE id = $1 AND workspace_id = $2 AND state <> $3",
        )
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            StoreError::Conflict("that mailbox is gone, so it takes no rules".to_string())
        })?;
        let mut held: Vec<String> = serde_json::from_str(row.get::<&str, _>("allow_rules"))
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        for rule in rules {
            if !held.contains(rule) {
                held.push(rule.clone());
            }
        }
        sqlx::query(
            "UPDATE agent_mailboxes SET allow_rules = $1 WHERE id = $2 AND workspace_id = $3",
        )
        .bind(allow_rules_json(&held)?)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(held)
    }

    async fn count_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS live FROM agent_mailboxes \
             WHERE connection_id = $1 AND workspace_id = $2 AND state <> $3",
        )
        .bind(connection_id.as_str())
        .bind(workspace_id.as_str())
        .bind(AgentMailboxState::Deleted.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>("live") as u32)
    }
}
