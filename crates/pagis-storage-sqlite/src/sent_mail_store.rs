//! The messages the Agent Mailboxes sent (ADR-0019).
//!
//! One row joins a `Message-ID` to the Thread the sending Run was
//! working in. A reply carries that id in `In-Reply-To`, so this is
//! what lands the answer where the send came from. The row holds no
//! body: Pagis keeps no copy of message bodies.

use async_trait::async_trait;
use pagis_core::{
    AgentId, AgentMailboxId, ChannelId, MessageId, RunId, SentMail, SentMailStore, StoreError,
    WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteSentMailStore {
    pool: SqlitePool,
}

impl SqliteSentMailStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str =
    "message_id, workspace_id, agent_id, mailbox_id, run_id, channel_id, thread_id, sent_at";

fn sent_from_row(row: &sqlx::sqlite::SqliteRow) -> SentMail {
    SentMail {
        message_id: row.get("message_id"),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        mailbox_id: AgentMailboxId::from(row.get::<String, _>("mailbox_id")),
        run_id: RunId::from(row.get::<String, _>("run_id")),
        channel_id: row
            .get::<Option<String>, _>("channel_id")
            .map(ChannelId::from),
        thread_id: row
            .get::<Option<String>, _>("thread_id")
            .map(MessageId::from),
        sent_at: row.get("sent_at"),
    }
}

#[async_trait]
impl SentMailStore for SqliteSentMailStore {
    async fn record(&self, sent: &SentMail) -> Result<(), StoreError> {
        // A `Message-ID` names one message, so a second write of the
        // same id is the same send and keeps the first row.
        sqlx::query(&format!(
            "INSERT INTO sent_mail ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (message_id) DO NOTHING"
        ))
        .bind(&sent.message_id)
        .bind(sent.workspace_id.as_str())
        .bind(sent.agent_id.as_str())
        .bind(sent.mailbox_id.as_str())
        .bind(sent.run_id.as_str())
        .bind(sent.channel_id.as_ref().map(|id| id.as_str()))
        .bind(sent.thread_id.as_ref().map(|id| id.as_str()))
        .bind(sent.sent_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        message_id: &str,
    ) -> Result<Option<SentMail>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM sent_mail WHERE message_id = ? AND workspace_id = ?"
        ))
        .bind(message_id)
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.as_ref().map(sent_from_row))
    }
}
