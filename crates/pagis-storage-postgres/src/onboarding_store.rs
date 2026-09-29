use async_trait::async_trait;
use pagis_core::{OnboardingModelVerification, OnboardingStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresOnboardingStore {
    pool: PgPool,
}

impl PostgresOnboardingStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OnboardingStore for PostgresOnboardingStore {
    async fn model_verification(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<OnboardingModelVerification>, StoreError> {
        let row = sqlx::query(
            "SELECT provider, available, proof \
             FROM onboarding_model_verifications WHERE workspace_id = $1",
        )
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|row| OnboardingModelVerification {
            provider: row.get("provider"),
            available: row.get("available"),
            proof: row.get("proof"),
        }))
    }

    async fn set_model_verification(
        &self,
        workspace_id: &WorkspaceId,
        verification: &OnboardingModelVerification,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO onboarding_model_verifications \
                 (workspace_id, provider, available, proof) VALUES ($1, $2, $3, $4) \
             ON CONFLICT(workspace_id) DO UPDATE SET provider = excluded.provider, \
                 available = excluded.available, proof = excluded.proof",
        )
        .bind(workspace_id.as_str())
        .bind(&verification.provider)
        .bind(verification.available)
        .bind(&verification.proof)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
