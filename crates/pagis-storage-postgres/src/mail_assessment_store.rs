//! Stored Spam Verdicts and Trust Tiers, scoped to a Workspace and mailbox.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{MailAssessment, MailAssessmentStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};
#[derive(Clone)]
pub struct PostgresMailAssessmentStore {
    pool: PgPool,
}
impl PostgresMailAssessmentStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait]
impl MailAssessmentStore for PostgresMailAssessmentStore {
    async fn get(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
    ) -> Result<Option<MailAssessment>, StoreError> {
        let row = sqlx::query("SELECT assessment FROM mail_assessments WHERE workspace_id = $1 AND mailbox = $2 AND message_id = $3")
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
        sqlx::query("INSERT INTO mail_assessments (workspace_id, mailbox, message_id, assessment) VALUES ($1, $2, $3, $4) ON CONFLICT (workspace_id, mailbox, message_id) DO UPDATE SET assessment = excluded.assessment")
            .bind(workspace.as_str()).bind(mailbox).bind(message).bind(value).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
}
