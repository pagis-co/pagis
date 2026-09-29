//! The Text Records (ADR-0020).
//!
//! One row per text the Agent sent or received. There is no
//! conversation table: a Text Conversation is the pair of one Agent
//! Phone Number and one counterpart number, and this store derives it
//! by grouping rows. The outbound row's `thread_id` is the reply
//! lookup, so texting needs no sent-texts table.
//!
//! Every read takes the Agent as well as the number, because a record
//! stays with the Agent that made it: a new holder of a number reads
//! none of the previous holder's conversations.

use async_trait::async_trait;
use pagis_core::{
    AgentId, ArtifactId, ChannelId, MessageId, PhoneNumberId, RunId, StoreError,
    TEXT_THREAD_WINDOW_MS, TextConversation, TextDeliveryStatus, TextDirection, TextRecord,
    TextRecordId, TextRecordStore, TrustTier, UnixMillis, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteTextRecordStore {
    pool: SqlitePool,
}

impl SqliteTextRecordStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, agent_id, phone_number_id, counterpart_e164, direction, \
     tier, body, segments, media_artifact_ids, carrier_message_id, run_id, channel_id, thread_id, \
     delivery_state, delivery_code, delivery_reason, occurred_at";

/// The media pointers go to the row as JSON, as the Call transcript
/// does.
fn media_json(ids: &[ArtifactId]) -> Result<String, StoreError> {
    serde_json::to_string(ids)
        .map_err(|error| StoreError::Corrupt(format!("the media list does not serialize: {error}")))
}

fn parse<T: std::str::FromStr<Err = String>>(value: &str) -> Result<T, StoreError> {
    value.parse().map_err(StoreError::Corrupt)
}

fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TextRecord, StoreError> {
    let delivery_state = row.get::<Option<String>, _>("delivery_state");
    let delivery_status = delivery_state
        .as_deref()
        .map(|state| {
            TextDeliveryStatus::parse(
                state,
                row.get::<Option<String>, _>("delivery_code").as_deref(),
                row.get::<Option<String>, _>("delivery_reason").as_deref(),
            )
        })
        .transpose()
        .map_err(StoreError::Corrupt)?;
    Ok(TextRecord {
        id: TextRecordId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        phone_number_id: PhoneNumberId::from(row.get::<String, _>("phone_number_id")),
        counterpart_e164: row.get("counterpart_e164"),
        direction: parse::<TextDirection>(&row.get::<String, _>("direction"))?,
        tier: parse::<TrustTier>(&row.get::<String, _>("tier"))?,
        body: row.get("body"),
        segments: row.get::<i64, _>("segments") as u32,
        media_artifact_ids: serde_json::from_str(&row.get::<String, _>("media_artifact_ids"))
            .map_err(|error| StoreError::Corrupt(format!("the media list is not JSON: {error}")))?,
        carrier_message_id: row.get("carrier_message_id"),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        channel_id: row
            .get::<Option<String>, _>("channel_id")
            .map(ChannelId::from),
        thread_id: row
            .get::<Option<String>, _>("thread_id")
            .map(MessageId::from),
        delivery_status,
        occurred_at: row.get("occurred_at"),
    })
}

fn conversation_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TextConversation, StoreError> {
    Ok(TextConversation {
        counterpart_e164: row.get("counterpart_e164"),
        last_at: row.get("occurred_at"),
        last_direction: parse::<TextDirection>(&row.get::<String, _>("direction"))?,
    })
}

#[async_trait]
impl TextRecordStore for SqliteTextRecordStore {
    async fn insert(&self, record: &TextRecord) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO text_records ({COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(record.id.as_str())
        .bind(record.workspace_id.as_str())
        .bind(record.agent_id.as_str())
        .bind(record.phone_number_id.as_str())
        .bind(&record.counterpart_e164)
        .bind(record.direction.as_str())
        .bind(record.tier.as_str())
        .bind(&record.body)
        .bind(i64::from(record.segments))
        .bind(media_json(&record.media_artifact_ids)?)
        .bind(&record.carrier_message_id)
        .bind(record.run_id.as_ref().map(RunId::as_str))
        .bind(record.channel_id.as_ref().map(ChannelId::as_str))
        .bind(record.thread_id.as_ref().map(MessageId::as_str))
        .bind(
            record
                .delivery_status
                .as_ref()
                .map(TextDeliveryStatus::as_str),
        )
        .bind(record.delivery_status.as_ref().and_then(|s| s.code()))
        .bind(record.delivery_status.as_ref().and_then(|s| s.reason()))
        .bind(record.occurred_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
    ) -> Result<Option<TextRecord>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM text_records WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(from_row).transpose()
    }

    async fn set_delivery_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
        status: &TextDeliveryStatus,
    ) -> Result<bool, StoreError> {
        // Only an outbound record has a delivery state, and the schema
        // holds that: the guard keeps an inbound row out.
        let updated = sqlx::query(
            "UPDATE text_records \
             SET delivery_state = ?, delivery_code = ?, delivery_reason = ? \
             WHERE id = ? AND workspace_id = ? AND delivery_state IS NOT NULL",
        )
        .bind(status.as_str())
        .bind(status.code())
        .bind(status.reason())
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn list_conversation(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        max: u32,
        before: Option<&TextRecordId>,
    ) -> Result<Vec<TextRecord>, StoreError> {
        // The page is keyed on the time and the id together, so two
        // texts of the same millisecond never hide each other.
        let cursor = match before {
            Some(_) => {
                " AND (occurred_at, id) < \
                        (SELECT occurred_at, id FROM text_records WHERE id = ?)"
            }
            None => "",
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM text_records \
             WHERE agent_id = ? AND workspace_id = ? \
             AND phone_number_id = ? AND counterpart_e164 = ?{cursor} \
             ORDER BY occurred_at DESC, id DESC LIMIT ?"
        );
        let mut query = sqlx::query(&sql)
            .bind(agent_id.as_str())
            .bind(workspace_id.as_str())
            .bind(phone_number_id.as_str())
            .bind(counterpart_e164);
        if let Some(before) = before {
            query = query.bind(before.as_str());
        }
        let rows = query
            .bind(i64::from(max))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.iter().map(from_row).collect()
    }

    async fn list_conversations(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        max: u32,
    ) -> Result<Vec<TextConversation>, StoreError> {
        // One row per counterpart: the newest text of the pair names
        // the time and the direction the desk lists.
        let rows = sqlx::query(
            "SELECT counterpart_e164, occurred_at, direction FROM ( \
                 SELECT counterpart_e164, occurred_at, direction, \
                        ROW_NUMBER() OVER ( \
                            PARTITION BY counterpart_e164 ORDER BY occurred_at DESC, id DESC \
                        ) AS row_rank \
                 FROM text_records \
                 WHERE agent_id = ? AND workspace_id = ? AND phone_number_id = ? \
             ) WHERE row_rank = 1 ORDER BY occurred_at DESC LIMIT ?",
        )
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(phone_number_id.as_str())
        .bind(i64::from(max))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(conversation_from_row).collect()
    }

    async fn landing_thread(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        now: UnixMillis,
    ) -> Result<Option<MessageId>, StoreError> {
        let row = sqlx::query(
            "SELECT thread_id FROM text_records \
             WHERE agent_id = ? AND workspace_id = ? \
               AND phone_number_id = ? AND counterpart_e164 = ? \
               AND direction = 'outbound' AND thread_id IS NOT NULL AND occurred_at > ? \
             ORDER BY occurred_at DESC, id DESC LIMIT 1",
        )
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(phone_number_id.as_str())
        .bind(counterpart_e164)
        .bind(now - TEXT_THREAD_WINDOW_MS)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|row| MessageId::from(row.get::<String, _>("thread_id"))))
    }
}
