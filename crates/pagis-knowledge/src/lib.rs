//! Source acquisition and versioning (ADR-0011).

use async_trait::async_trait;
use pagis_core::knowledge::*;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] pagis_core::StoreError),
    #[error("source requires authorization")]
    Unauthorized,
    #[error("source item is unavailable")]
    NotFound,
    #[error("source checkpoint expired")]
    ResetRequired,
    #[error("source is temporarily unavailable")]
    Unavailable,
    #[error("the source rejected this item: {0}")]
    Rejected(String),
}

impl Error {
    /// A permanent failure gives the same answer on every retry, so the
    /// caller must skip the item instead of holding it.
    pub fn permanent(&self) -> bool {
        matches!(self, Self::Rejected(_) | Self::NotFound)
    }
}

pub struct SourcePage {
    pub historical: bool,
    pub arrivals: Vec<pagis_core::NormalizedEvent>,
    pub checkpoint: Value,
    pub caught_up: bool,
    pub changes: Vec<SourceChange>,
}

#[async_trait]
pub trait Source: Send + Sync {
    async fn collect(&self, checkpoint: &Value, since: i64) -> Result<SourcePage, Error>;
    async fn read(&self, item: &SourceChange) -> Result<SourceContent, Error>;
    /// The metadata of one version the store holds without it (ADR-0011).
    async fn metadata(&self, item: &SourceChange) -> Result<Value, Error>;
}

/// How many incomplete versions one collect pass completes. Each one is
/// one provider read (ADR-0011).
pub const COMPLETIONS_PER_PASS: u32 = 100;

/// Acquires source versions without asking a model to interpret them.
pub struct Knowledge {
    store: Arc<dyn KnowledgeStore>,
    /// The source of the suppression key that blocks a forgotten item.
    forget_keys: Arc<dyn pagis_core::ForgetKeys>,
}

impl Knowledge {
    pub fn new(
        store: Arc<dyn KnowledgeStore>,
        forget_keys: Arc<dyn pagis_core::ForgetKeys>,
    ) -> Self {
        Self { store, forget_keys }
    }

    async fn enabled(&self, key: &SourceKey, now: i64) -> Result<SyncStatus, Error> {
        self.store
            .status(key, now)
            .await?
            .filter(|state| state.config.enabled)
            .ok_or(Error::Unauthorized)
    }

    /// Acquires one source page and advances its provider checkpoint.
    pub async fn collect(
        &self,
        key: &SourceKey,
        source: &dyn Source,
        now: i64,
    ) -> Result<(), Error> {
        let state = self.enabled(key, now).await?;
        let page = match source.collect(&state.checkpoint, state.config.since).await {
            Ok(page) => page,
            Err(Error::ResetRequired) => {
                self.store.reset(key, state.cursor_revision, now).await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if page.changes.is_empty()
            && page.checkpoint == state.checkpoint
            && page.caught_up == state.caught_up
        {
            return Ok(());
        }
        self.store
            .acquire(
                key,
                &SourceBatch {
                    historical: page.historical,
                    arrivals: page.arrivals,
                    expected_revision: state.cursor_revision,
                    checkpoint: page.checkpoint,
                    caught_up: page.caught_up,
                    changes: page.changes,
                },
                now,
                self.forget_keys.as_ref(),
            )
            .await?;
        Ok(())
    }

    /// Completes the metadata of the versions the store holds without it,
    /// oldest first. A lost item completes with empty metadata, so the
    /// pass moves on; a transient failure stops the pass, and the next
    /// pass continues from the same version (ADR-0011).
    pub async fn complete(
        &self,
        key: &SourceKey,
        source: &dyn Source,
        now: i64,
    ) -> Result<(), Error> {
        self.enabled(key, now).await?;
        for version in self
            .store
            .versions_without_metadata(key, COMPLETIONS_PER_PASS)
            .await?
        {
            let metadata = match source.metadata(&version.source).await {
                Ok(metadata) => metadata,
                Err(error) if error.permanent() => {
                    tracing::warn!(
                        id = %version.source.id,
                        %error,
                        "the source holds no metadata for the version"
                    );
                    Value::Object(Default::default())
                }
                Err(error) => return Err(error),
            };
            self.store
                .complete_metadata(key, version.sequence, &metadata)
                .await?;
        }
        Ok(())
    }
}
