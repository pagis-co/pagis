//! The Push Subscriptions of a Person (ADR-0030).

use async_trait::async_trait;
use pagis_core::{
    PushSubscription, PushSubscriptionId, PushSubscriptionStore, SessionId, StoreError, UnixMillis,
    WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

const COLUMNS: &str =
    "id, workspace_id, session_id, endpoint, p256dh, auth, created_at, last_sent_at";

#[derive(Clone)]
pub struct PostgresPushSubscriptionStore {
    pool: PgPool,
}

impl PostgresPushSubscriptionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_subscription(row: &sqlx::postgres::PgRow) -> PushSubscription {
    PushSubscription {
        id: PushSubscriptionId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        session_id: SessionId::from(row.get::<String, _>("session_id")),
        endpoint: row.get("endpoint"),
        p256dh: row.get("p256dh"),
        auth: row.get("auth"),
        created_at: row.get("created_at"),
        last_sent_at: row.get("last_sent_at"),
    }
}

#[async_trait]
impl PushSubscriptionStore for PostgresPushSubscriptionStore {
    async fn upsert(
        &self,
        subscription: &PushSubscription,
    ) -> Result<PushSubscription, StoreError> {
        // The endpoint is the identity of the row. A move to another
        // Session is a new subscription of that Session, so the row
        // takes the new time and forgets the pushes of the old Session.
        // Each right-hand side reads the row as it was before the update.
        let row = sqlx::query(&format!(
            "INSERT INTO push_subscriptions ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (endpoint) DO UPDATE SET \
             workspace_id = excluded.workspace_id, session_id = excluded.session_id, \
             p256dh = excluded.p256dh, auth = excluded.auth, \
             created_at = CASE WHEN push_subscriptions.session_id = excluded.session_id \
             THEN push_subscriptions.created_at ELSE excluded.created_at END, \
             last_sent_at = CASE WHEN push_subscriptions.session_id = excluded.session_id \
             THEN push_subscriptions.last_sent_at ELSE excluded.last_sent_at END \
             RETURNING {COLUMNS}"
        ))
        .bind(subscription.id.as_str())
        .bind(subscription.workspace_id.as_str())
        .bind(subscription.session_id.as_str())
        .bind(&subscription.endpoint)
        .bind(&subscription.p256dh)
        .bind(&subscription.auth)
        .bind(subscription.created_at)
        .bind(subscription.last_sent_at)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row_to_subscription(&row))
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PushSubscription>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM push_subscriptions WHERE workspace_id = $1 ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.iter().map(row_to_subscription).collect())
    }

    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &PushSubscriptionId,
    ) -> Result<bool, StoreError> {
        let deleted =
            sqlx::query("DELETE FROM push_subscriptions WHERE id = $1 AND workspace_id = $2")
                .bind(id.as_str())
                .bind(workspace_id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn delete_by_endpoint(
        &self,
        workspace_id: &WorkspaceId,
        endpoint: &str,
    ) -> Result<bool, StoreError> {
        let deleted =
            sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1 AND workspace_id = $2")
                .bind(endpoint)
                .bind(workspace_id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn mark_sent(
        &self,
        workspace_id: &WorkspaceId,
        id: &PushSubscriptionId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE push_subscriptions SET last_sent_at = $1 \
             WHERE id = $2 AND workspace_id = $3",
        )
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() == 1)
    }
}
