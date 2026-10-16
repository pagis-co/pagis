//! Junk policy, corrections and quarantine release.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{
    AuditEntry, JunkMessage, JunkStore, MailLists, OrgJunkPolicy, StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};
#[derive(Clone)]
pub struct PostgresJunkStore {
    pool: PgPool,
}
impl PostgresJunkStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    async fn policy<T: serde::de::DeserializeOwned + Default>(
        &self,
        scope: &str,
    ) -> Result<T, StoreError> {
        let row = sqlx::query("SELECT policy FROM junk_policies WHERE scope = $1")
            .bind(scope)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(|r| decode(&r.get::<String, _>("policy")))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    async fn set_policy(
        &self,
        scope: &str,
        policy: &impl serde::Serialize,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO junk_policies (scope, policy) VALUES ($1, $2) ON CONFLICT (scope) DO UPDATE SET policy = excluded.policy")
            .bind(scope).bind(encode(policy)?).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
}
fn encode(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    serde_json::from_str(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
fn message(row: &sqlx::postgres::PgRow) -> Result<JunkMessage, StoreError> {
    let mut message: JunkMessage = decode(&row.get::<String, _>("evidence"))?;
    message.person_mark = row.get::<Option<i32>, _>("person_mark").map(|v| v != 0);
    message.quarantined = row.get::<i32, _>("quarantined") != 0;
    message.released = row.get::<i32, _>("released") != 0;
    Ok(message)
}
#[async_trait]
impl JunkStore for PostgresJunkStore {
    async fn org_policy(&self) -> Result<OrgJunkPolicy, StoreError> {
        self.policy("").await
    }
    async fn set_org_policy(&self, policy: &OrgJunkPolicy) -> Result<(), StoreError> {
        policy.lists.validate()?;
        self.set_policy("", policy).await
    }
    async fn workspace_lists(&self, workspace: &WorkspaceId) -> Result<MailLists, StoreError> {
        self.policy(workspace.as_str()).await
    }
    async fn set_workspace_lists(
        &self,
        workspace: &WorkspaceId,
        lists: &MailLists,
    ) -> Result<(), StoreError> {
        lists.validate()?;
        self.set_policy(workspace.as_str(), lists).await
    }
    async fn message(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        key: &str,
    ) -> Result<Option<JunkMessage>, StoreError> {
        let row = sqlx::query("SELECT evidence, person_mark, released, quarantined FROM junk_messages WHERE workspace_id = $1 AND mailbox = $2 AND message_key = $3")
            .bind(workspace.as_str()).bind(mailbox).bind(key).fetch_optional(&self.pool).await.map_err(db_err)?;
        row.as_ref().map(message).transpose()
    }
    async fn observe(&self, message: &JunkMessage) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO junk_messages (workspace_id, mailbox, message_key, evidence, quarantined) SELECT $1, $2, $3, $4, $5 WHERE $5 = 1 OR EXISTS (SELECT 1 FROM junk_messages WHERE workspace_id = $1 AND mailbox = $2 AND message_key = $3) ON CONFLICT (workspace_id, mailbox, message_key) DO UPDATE SET evidence = CASE WHEN junk_messages.quarantined = 1 THEN jsonb_set(junk_messages.evidence::jsonb, '{host_id}', excluded.evidence::jsonb->'host_id')::text ELSE excluded.evidence END, quarantined = CASE WHEN junk_messages.quarantined = 1 THEN 1 ELSE excluded.quarantined END")
            .bind(message.workspace_id.as_str()).bind(&message.mailbox).bind(&message.message_key).bind(encode(message)?).bind(i32::from(message.quarantined)).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn relocate(&self, old_key: &str, message: &JunkMessage) -> Result<(), StoreError> {
        sqlx::query("UPDATE junk_messages SET message_key = $4, evidence = jsonb_set(jsonb_set(evidence::jsonb, '{host_id}', $5::text::jsonb->'host_id'), '{message_key}', to_jsonb($4::text))::text WHERE workspace_id = $1 AND mailbox = $2 AND message_key = $3")
            .bind(message.workspace_id.as_str()).bind(&message.mailbox).bind(old_key).bind(&message.message_key).bind(encode(message)?).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn mark(&self, message: &JunkMessage, spam: bool) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO junk_messages (workspace_id, mailbox, message_key, evidence, quarantined, person_mark) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (workspace_id, mailbox, message_key) DO UPDATE SET person_mark = excluded.person_mark")
            .bind(message.workspace_id.as_str()).bind(&message.mailbox).bind(&message.message_key).bind(encode(message)?).bind(i32::from(message.quarantined)).bind(i32::from(spam)).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn quarantine(&self) -> Result<Vec<JunkMessage>, StoreError> {
        sqlx::query("SELECT evidence, person_mark, released, quarantined FROM junk_messages WHERE quarantined = 1 AND released = 0 ORDER BY workspace_id, mailbox, message_key")
            .fetch_all(&self.pool).await.map_err(db_err)?.iter().map(message).collect()
    }
    async fn release(
        &self,
        mailbox: &str,
        key: &str,
        audit: &AuditEntry,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let result = sqlx::query("UPDATE junk_messages SET released = 1 WHERE workspace_id = $1 AND mailbox = $2 AND message_key = $3 AND quarantined = 1 AND released = 0")
            .bind(audit.subject_workspace_id.as_str()).bind(mailbox).bind(key).execute(&mut *tx).await.map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict("mail record does not exist".into()));
        }
        crate::audit_log::append(&mut tx, audit).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
