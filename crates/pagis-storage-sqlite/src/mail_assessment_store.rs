//! Stored Spam Verdicts and Trust Tiers, scoped to a Workspace and mailbox.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{MailAssessment, MailAssessmentStore, StoreError, WorkspaceId};
use sqlx::{Row, SqlitePool};
#[derive(Clone)]
pub struct SqliteMailAssessmentStore {
    pool: SqlitePool,
}
impl SqliteMailAssessmentStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
#[async_trait]
impl MailAssessmentStore for SqliteMailAssessmentStore {
    async fn get(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
    ) -> Result<Option<MailAssessment>, StoreError> {
        let row = sqlx::query("SELECT assessment FROM mail_assessments WHERE workspace_id = ? AND mailbox = ? AND message_id = ?")
            .bind(workspace.as_str()).bind(mailbox).bind(message).fetch_optional(&self.pool).await.map_err(db_err)?;
        row.map(|row| {
            serde_json::from_str(&row.get::<String, _>("assessment"))
                .map_err(|e| StoreError::Corrupt(e.to_string()))
        })
        .transpose()
    }
    async fn put(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
        assessment: &MailAssessment,
    ) -> Result<(), StoreError> {
        let value =
            serde_json::to_string(assessment).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        sqlx::query("INSERT INTO mail_assessments (workspace_id, mailbox, message_id, assessment) VALUES (?, ?, ?, ?) ON CONFLICT (workspace_id, mailbox, message_id) DO UPDATE SET assessment = excluded.assessment")
            .bind(workspace.as_str()).bind(mailbox).bind(message).bind(value).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
}
