//! Mail Assignment records and atomic open limits.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::*;
use sqlx::{Row, SqlitePool};

pub struct SqliteMailAssignmentStore {
    pool: SqlitePool,
}
impl SqliteMailAssignmentStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn decode(value: String) -> Result<MailAssignment, StoreError> {
    serde_json::from_str(&value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
fn encode(row: &MailAssignment) -> Result<String, StoreError> {
    serde_json::to_string(row).map_err(|e| StoreError::Corrupt(e.to_string()))
}
#[async_trait]
impl MailAssignmentStore for SqliteMailAssignmentStore {
    async fn find(&self, id: &MailAssignmentId) -> Result<Option<MailAssignment>, StoreError> {
        sqlx::query_scalar::<_, String>("SELECT record FROM mail_assignments WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .map(decode)
            .transpose()
    }
    async fn by_channel(&self, channel: &ChannelId) -> Result<Option<MailAssignment>, StoreError> {
        sqlx::query_scalar::<_, String>("SELECT record FROM mail_assignments WHERE channel_id = ?")
            .bind(channel.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .map(decode)
            .transpose()
    }
    async fn settings(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<MailAssignmentSettings, StoreError> {
        let row = sqlx::query("SELECT assignment_limit, assignment_timeout_minutes FROM mail_assignment_settings WHERE workspace_id = ?").bind(workspace.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        Ok(row
            .map(|r| MailAssignmentSettings {
                assignment_limit: r
                    .get::<Option<i64>, _>("assignment_limit")
                    .map(|n| n as u32),
                assignment_timeout_minutes: r.get::<i64, _>("assignment_timeout_minutes") as u32,
            })
            .unwrap_or_default())
    }
    async fn set_settings(
        &self,
        workspace: &WorkspaceId,
        settings: &MailAssignmentSettings,
    ) -> Result<(), StoreError> {
        settings.validate()?;
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query("UPDATE workspaces SET name = name WHERE id = ?")
            .bind(workspace.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("INSERT INTO mail_assignment_settings (workspace_id, assignment_limit, assignment_timeout_minutes) VALUES (?, ?, ?) ON CONFLICT (workspace_id) DO UPDATE SET assignment_limit = excluded.assignment_limit, assignment_timeout_minutes = excluded.assignment_timeout_minutes").bind(workspace.as_str()).bind(settings.assignment_limit.map(i64::from)).bind(i64::from(settings.assignment_timeout_minutes)).execute(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
    async fn create_batch(
        &self,
        workspace: &WorkspaceId,
        assignments: &[MailAssignment],
    ) -> Result<(), StoreError> {
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query("UPDATE workspaces SET name = name WHERE id = ?")
            .bind(workspace.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let limit: Option<i64> = sqlx::query_scalar(
            "SELECT assignment_limit FROM mail_assignment_settings WHERE workspace_id = ?",
        )
        .bind(workspace.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .flatten();
        for row in assignments {
            if &row.workspace_id != workspace {
                return Err(StoreError::Conflict(
                    "Mail Assignment belongs to another Workspace.".into(),
                ));
            }
            if let Some(limit) = limit {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mail_assignments WHERE workspace_id = ? AND assignee = ? AND state IN ('assigned', 'escalated')").bind(workspace.as_str()).bind(row.assignee.as_str()).fetch_one(&mut *tx).await.map_err(db_err)?;
                if count >= limit {
                    return Err(StoreError::Conflict(
                        "The member has reached the assignment limit. Select another member."
                            .into(),
                    ));
                }
            }
            sqlx::query("INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at, conversation_key) VALUES (?, ?, 'conversation', ?, ?, ?, ?)")
                .bind(row.channel_id.as_str()).bind(workspace.as_str()).bind(&row.summary)
                .bind(row.created_at).bind(row.created_at).bind(format!("mail_assignment:{}", row.id))
                .execute(&mut *tx).await.map_err(db_err)?;
            for participant in [workspace, &row.private_workspace_id] {
                sqlx::query("INSERT INTO channel_participants (id, workspace_id, channel_id, participant_kind, participant_workspace_id, joined_at) VALUES (?, ?, ?, 'workspace', ?, ?)")
                    .bind(ParticipantId::generate().as_str()).bind(workspace.as_str()).bind(row.channel_id.as_str())
                    .bind(participant.as_str()).bind(row.created_at).execute(&mut *tx).await.map_err(db_err)?;
            }
            sqlx::query("INSERT INTO mail_assignments (id, workspace_id, message_key, assignee, channel_id, state, due_at, record) VALUES (?, ?, ?, ?, ?, ?, ?, ?)").bind(row.id.as_str()).bind(workspace.as_str()).bind(&row.message_key).bind(row.assignee.as_str()).bind(row.channel_id.as_str()).bind(row.state.as_str()).bind(row.due_at).bind(encode(row)?).execute(&mut *tx).await.map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
    async fn list(
        &self,
        workspace: &WorkspaceId,
        message: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        let values: Vec<String> = if let Some(message) = message {
            sqlx::query_scalar("SELECT record FROM mail_assignments WHERE workspace_id = ? AND message_key = ? ORDER BY id DESC LIMIT ?").bind(workspace.as_str()).bind(message).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db_err)?
        } else {
            sqlx::query_scalar("SELECT record FROM mail_assignments WHERE workspace_id = ? ORDER BY id DESC LIMIT ?").bind(workspace.as_str()).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db_err)?
        };
        values.into_iter().map(decode).collect()
    }
    async fn for_person(
        &self,
        person: &UserId,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        sqlx::query_scalar::<_, String>("SELECT record FROM mail_assignments WHERE assignee = ? AND state IN ('assigned', 'escalated') ORDER BY id DESC LIMIT ?").bind(person.as_str()).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db_err)?.into_iter().map(decode).collect()
    }
    async fn overdue(&self, at: UnixMillis, limit: u32) -> Result<Vec<MailAssignment>, StoreError> {
        sqlx::query_scalar::<_, String>("SELECT record FROM mail_assignments WHERE state = 'assigned' AND due_at <= ? ORDER BY due_at, id LIMIT ?").bind(at).bind(i64::from(limit)).fetch_all(&self.pool).await.map_err(db_err)?.into_iter().map(decode).collect()
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
    ) -> Result<Option<MailAssignment>, StoreError> {
        sqlx::query_scalar::<_, String>(
            "SELECT record FROM mail_assignments WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .map(decode)
        .transpose()
    }
    async fn update(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
        expected: MailAssignmentState,
        state: MailAssignmentState,
        note: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let Some(mut row) = self.get(workspace, id).await? else {
            return Ok(false);
        };
        row.state = state;
        row.note = note.into();
        row.changed_at = at;
        let result = sqlx::query("UPDATE mail_assignments SET state = ?, record = ? WHERE workspace_id = ? AND id = ? AND state = ?").bind(state.as_str()).bind(encode(&row)?).bind(workspace.as_str()).bind(id.as_str()).bind(expected.as_str()).execute(&self.pool).await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}
