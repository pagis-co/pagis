//! The Workspace Mailbox record and its Address Ledger reservation.
use crate::{db_err, unique_violation};
use async_trait::async_trait;
use pagis_core::{
    ConnectionId, StoreError, WorkspaceId, WorkspaceMailbox, WorkspaceMailboxRecordStore,
};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteWorkspaceMailboxStore {
    pool: SqlitePool,
}
impl SqliteWorkspaceMailboxStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn record(row: &sqlx::sqlite::SqliteRow) -> Result<WorkspaceMailbox, StoreError> {
    Ok(WorkspaceMailbox {
        author: serde_json::from_str(row.get::<String, _>("author").as_str())
            .map_err(|error| StoreError::Corrupt(error.to_string()))?,
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        address: row.get("address"),
        visibility: row.get::<String, _>("visibility").parse()?,
        spam_mode: row.get::<String, _>("spam_mode").parse()?,
        outgoing_cap: row.get::<i64, _>("outgoing_cap") as u32,
        created_at: row.get("created_at"),
    })
}
#[async_trait]
impl WorkspaceMailboxRecordStore for SqliteWorkspaceMailboxStore {
    async fn calendar_cursor(
        &self,
        workspace: &WorkspaceId,
        folder: &str,
    ) -> Result<Option<pagis_core::MailboxCursor>, StoreError> {
        let row = sqlx::query("SELECT uid_validity, last_uid FROM workspace_mail_calendar_cursors WHERE workspace_id = ? AND folder = ?")
            .bind(workspace.as_str()).bind(folder).fetch_optional(&self.pool).await.map_err(db_err)?;
        Ok(row.map(|row| pagis_core::MailboxCursor {
            folder: folder.into(),
            uid_validity: row.get::<i64, _>("uid_validity") as u32,
            last_uid: row.get::<i64, _>("last_uid") as u32,
        }))
    }
    async fn set_calendar_cursor(
        &self,
        workspace: &WorkspaceId,
        cursor: &pagis_core::MailboxCursor,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mail_calendar_cursors (workspace_id, folder, uid_validity, last_uid) VALUES (?, ?, ?, ?) ON CONFLICT (workspace_id, folder) DO UPDATE SET uid_validity = excluded.uid_validity, last_uid = CASE WHEN workspace_mail_calendar_cursors.uid_validity = excluded.uid_validity THEN MAX(workspace_mail_calendar_cursors.last_uid, excluded.last_uid) ELSE excluded.last_uid END")
            .bind(workspace.as_str()).bind(&cursor.folder).bind(i64::from(cursor.uid_validity)).bind(i64::from(cursor.last_uid))
            .execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn set_visibility(
        &self,
        workspace: &WorkspaceId,
        visibility: pagis_core::MailVisibility,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspace_mailboxes SET visibility = ? WHERE workspace_id = ?")
            .bind(visibility.as_str())
            .bind(workspace.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
    async fn triage(
        &self,
        workspace: &WorkspaceId,
        message: &str,
    ) -> Result<Option<pagis_core::MailTriage>, StoreError> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT record FROM workspace_mail_triage WHERE workspace_id = ? AND message_id = ?",
        )
        .bind(workspace.as_str())
        .bind(message)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        value
            .map(|v| serde_json::from_str(&v).map_err(|e| StoreError::Corrupt(e.to_string())))
            .transpose()
    }
    async fn record_triage(
        &self,
        workspace: &WorkspaceId,
        message: &str,
        triage: &pagis_core::MailTriage,
    ) -> Result<(), StoreError> {
        triage.validate()?;
        if triage.outcome == pagis_core::MailTriageOutcome::Assigned && triage.assignee.is_none() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM mail_assignments WHERE workspace_id = ? AND message_key = ?",
            )
            .bind(workspace.as_str())
            .bind(message)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
            if count == 0 {
                return Err(StoreError::Conflict(
                    "Assigned mail needs an Agent or Mail Assignments.".into(),
                ));
            }
        }

        sqlx::query("INSERT INTO workspace_mail_triage (workspace_id, message_id, record) VALUES (?, ?, ?) ON CONFLICT (workspace_id, message_id) DO UPDATE SET record = excluded.record")
            .bind(workspace.as_str()).bind(message).bind(serde_json::to_string(triage).map_err(|e| StoreError::Corrupt(e.to_string()))?).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn relocate_triage(
        &self,
        workspace: &WorkspaceId,
        old_key: &str,
        new_key: &str,
        message_id: &str,
    ) -> Result<(), StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query("UPDATE workspace_mail_triage SET message_id = ?3, record = json_set(record, '$.message_id', ?4) WHERE workspace_id = ?1 AND message_id = ?2").bind(workspace.as_str()).bind(old_key).bind(new_key).bind(message_id).execute(&mut *transaction).await.map_err(db_err)?;
        sqlx::query("UPDATE workspace_mail_pending_triage SET message_key = ?3, host_id = ?4 WHERE workspace_id = ?1 AND message_key = ?2")
            .bind(workspace.as_str()).bind(old_key).bind(new_key).bind(message_id).execute(&mut *transaction).await.map_err(db_err)?;
        sqlx::query("UPDATE mail_assignments SET message_key = ?3, record = json_set(record, '$.message_key', ?3, '$.message_id', ?4) WHERE workspace_id = ?1 AND message_key = ?2")
            .bind(workspace.as_str()).bind(old_key).bind(new_key).bind(message_id).execute(&mut *transaction).await.map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }
    async fn open_needs_you(
        &self,
        workspace: &WorkspaceId,
        before: Option<(pagis_core::UnixMillis, &str)>,
        limit: u32,
    ) -> Result<Vec<(String, pagis_core::MailTriage)>, StoreError> {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT message_id, record FROM workspace_mail_triage WHERE workspace_id = ",
        );
        query.push_bind(workspace.as_str());
        query.push(" AND json_extract(record, '$.outcome') = 'needs_you' AND json_extract(record, '$.resolved_at') IS NULL");
        if let Some((at, key)) = before {
            query
                .push(" AND (json_extract(record, '$.recorded_at'), message_id) < (")
                .push_bind(at)
                .push(", ")
                .push_bind(key)
                .push(")");
        }
        query
            .push(" ORDER BY json_extract(record, '$.recorded_at') DESC, message_id DESC LIMIT ")
            .push_bind(i64::from(limit));
        let rows = query.build().fetch_all(&self.pool).await.map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.get("message_id"),
                    serde_json::from_str(&row.get::<String, _>("record"))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                ))
            })
            .collect()
    }
    async fn resolve_triage(
        &self,
        workspace: &WorkspaceId,
        message: &str,
        at: pagis_core::UnixMillis,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("UPDATE workspace_mail_triage SET record = json_set(record, '$.resolved_at', ?3) WHERE workspace_id = ?1 AND message_id = ?2 AND json_extract(record, '$.outcome') = 'needs_you' AND json_extract(record, '$.resolved_at') IS NULL")
            .bind(workspace.as_str()).bind(message).bind(at).execute(&self.pool).await.map_err(db_err)?;
        Ok(result.rows_affected() != 0)
    }
    async fn put_pending_triage(
        &self,
        workspace: &WorkspaceId,
        pending: &pagis_core::PendingMailTriage,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mail_pending_triage (workspace_id, message_key, host_id, wakeup_id) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (workspace_id, message_key) DO UPDATE SET host_id = excluded.host_id, wakeup_id = excluded.wakeup_id")
            .bind(workspace.as_str()).bind(&pending.key).bind(&pending.message_id).bind(pending.wakeup_id.as_str()).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn pending_triage(
        &self,
        workspace: &WorkspaceId,
        wakeup: Option<&pagis_core::WakeupId>,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<pagis_core::PendingMailTriage>, StoreError> {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "SELECT message_key, host_id, wakeup_id FROM workspace_mail_pending_triage WHERE workspace_id = ",
        );
        query.push_bind(workspace.as_str());
        if let Some(wakeup) = wakeup {
            query.push(" AND wakeup_id = ").push_bind(wakeup.as_str());
        }
        if let Some(after) = after {
            query.push(" AND message_key > ").push_bind(after);
        }
        query
            .push(" ORDER BY message_key LIMIT ")
            .push_bind(i64::from(limit));
        let rows = query.build().fetch_all(&self.pool).await.map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|row| pagis_core::PendingMailTriage {
                key: row.get("message_key"),
                message_id: row.get("host_id"),
                wakeup_id: pagis_core::WakeupId::from(row.get::<String, _>("wakeup_id")),
            })
            .collect())
    }
    async fn remove_pending_triage(
        &self,
        workspace: &WorkspaceId,
        key: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM workspace_mail_pending_triage WHERE workspace_id = ?1 AND message_key = ?2").bind(workspace.as_str()).bind(key).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn triage_rules(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<pagis_core::MailTriageRule>, StoreError> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT rules FROM workspace_mail_triage_rules WHERE workspace_id = ?",
        )
        .bind(workspace.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        value
            .map(|v| serde_json::from_str(&v).map_err(|e| StoreError::Corrupt(e.to_string())))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    async fn replace_triage_rules(
        &self,
        workspace: &WorkspaceId,
        rules: &[pagis_core::MailTriageRule],
    ) -> Result<(), StoreError> {
        pagis_core::validate_triage_rules(rules)?;
        sqlx::query("INSERT INTO workspace_mail_triage_rules (workspace_id, rules) VALUES (?, ?) ON CONFLICT (workspace_id) DO UPDATE SET rules = excluded.rules")
            .bind(workspace.as_str()).bind(serde_json::to_string(rules).map_err(|e| StoreError::Corrupt(e.to_string()))?).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn triage_cursor(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Option<pagis_core::MailboxCursor>, StoreError> {
        let row = sqlx::query("SELECT uid_validity, last_uid FROM workspace_mail_triage_cursors WHERE workspace_id = ?").bind(workspace.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        Ok(row.map(|row| pagis_core::MailboxCursor {
            folder: "INBOX".into(),
            uid_validity: row.get::<i64, _>("uid_validity") as u32,
            last_uid: row.get::<i64, _>("last_uid") as u32,
        }))
    }
    async fn set_triage_cursor(
        &self,
        workspace: &WorkspaceId,
        cursor: &pagis_core::MailboxCursor,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mail_triage_cursors (workspace_id, uid_validity, last_uid) VALUES (?, ?, ?) ON CONFLICT (workspace_id) DO UPDATE SET uid_validity = excluded.uid_validity, last_uid = CASE WHEN workspace_mail_triage_cursors.uid_validity = excluded.uid_validity THEN MAX(workspace_mail_triage_cursors.last_uid, excluded.last_uid) ELSE excluded.last_uid END")
            .bind(workspace.as_str()).bind(i64::from(cursor.uid_validity)).bind(i64::from(cursor.last_uid)).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn set_spam_mode(
        &self,
        workspace: &WorkspaceId,
        mode: pagis_core::MailScoringMode,
    ) -> Result<(), StoreError> {
        let result =
            sqlx::query("UPDATE workspace_mailboxes SET spam_mode = ? WHERE workspace_id = ?")
                .bind(mode.as_str())
                .bind(workspace.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict("mail record does not exist".into()));
        }
        Ok(())
    }
    async fn reserve_send(&self, workspace: &WorkspaceId, day: i64) -> Result<bool, StoreError> {
        let result = sqlx::query("UPDATE workspace_mailboxes SET sends_today = CASE WHEN send_day = ? THEN sends_today + 1 ELSE 1 END, send_day = ? WHERE workspace_id = ? AND (send_day <> ? OR sends_today < outgoing_cap)")
            .bind(day).bind(day).bind(workspace.as_str()).bind(day).execute(&self.pool).await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
    async fn release_send(&self, workspace: &WorkspaceId, day: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspace_mailboxes SET sends_today = sends_today - 1 WHERE workspace_id = ? AND send_day = ? AND sends_today > 0")
            .bind(workspace.as_str()).bind(day).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceMailbox>, StoreError> {
        let row = sqlx::query("SELECT author, workspace_id, connection_id, address, outgoing_cap, spam_mode, visibility, created_at FROM workspace_mailboxes WHERE workspace_id = ?")
            .bind(workspace_id.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        row.as_ref().map(record).transpose()
    }
    async fn create(&self, mailbox: &WorkspaceMailbox) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mailboxes (author, workspace_id, connection_id, address, created_at, outgoing_cap, spam_mode, visibility) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(serde_json::to_string(&mailbox.author).map_err(|error| StoreError::Corrupt(error.to_string()))?).bind(mailbox.workspace_id.as_str()).bind(mailbox.connection_id.as_str())
            .bind(&mailbox.address).bind(mailbox.created_at).bind(i64::from(mailbox.outgoing_cap)).bind(mailbox.spam_mode.as_str()).bind(mailbox.visibility.as_str())
            .execute(&self.pool).await.map_err(|error| if unique_violation(&error) {
                StoreError::Conflict("that Workspace or address already has a mailbox".into())
            } else { db_err(error) })?;
        Ok(())
    }
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError> {
        let result =
            sqlx::query("DELETE FROM workspace_mailboxes WHERE workspace_id = ? AND address = ?")
                .bind(workspace_id.as_str())
                .bind(address)
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }
    async fn count_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS count FROM workspace_mailboxes WHERE workspace_id = ? AND connection_id = ?",
        )
        .bind(workspace_id.as_str())
        .bind(connection_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>("count") as u32)
    }
}
