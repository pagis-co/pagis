//! Durable source acquisition and revision-checked knowledge (ADR-0011).

mod invalidation;
pub use invalidation::KnowledgeInvalidation;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::reflection_filter::{
    Catalogue, FilterPreview, PageSignals, ReflectionFilter, Selection,
};
use crate::{AgentId, ConnectionId, StoreError, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SourceKey {
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    #[schema(value_type = String)]
    pub connection_id: ConnectionId,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SyncConfig {
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    #[schema(value_type = String)]
    pub connection_id: ConnectionId,
    pub resource: String,
    #[schema(value_type = String)]
    pub agent_id: AgentId,
    /// The adapter's read capability, set by the daemon, never the model.
    pub required_capability: String,
    pub enabled: bool,
    pub since: i64,
    /// The rules that decide which Subject Pages reflect (ADR-0011).
    pub filter: ReflectionFilter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SyncStatus {
    pub config: SyncConfig,
    pub revision: i64,
    pub cursor_revision: i64,
    pub checkpoint: serde_json::Value,
    pub caught_up: bool,
    pub rebuilding: bool,
    pub processed: i64,
    pub arrival_pending: i64,
    pub arrival_processed: i64,
    /// Arrivals that a permanent rejection or five attempts stopped.
    pub arrival_skipped: i64,
    /// The revision of the Reflection Filter these counts belong to.
    pub filter_revision: i64,
    /// Pages the current filter revision sent to reflection.
    pub reflected_pages: i64,
    /// Pages the current filter revision held back.
    pub skipped_pages: i64,
    /// Backfill pages that reflect and wait for a Run (ADR-0011).
    pub backfill_pending: i64,
    /// Backfill pages that a Run already reflects.
    pub backfill_reflected: i64,
    /// Backfill pages whose Runs failed at the attempt limit (ADR-0011).
    pub backfill_failed: i64,
    /// True while the budget of the backfill window is spent.
    pub backfill_capped: bool,
    /// Upsert versions that hold no metadata yet (ADR-0011).
    pub incomplete_versions: i64,
    pub acquisition_error: Option<String>,
    pub arrival_error: Option<String>,
    pub updated_at: i64,
}

impl SyncStatus {
    pub fn key(&self) -> SourceKey {
        SourceKey {
            workspace_id: self.config.workspace_id.clone(),
            connection_id: self.config.connection_id.clone(),
            resource: self.config.resource.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceOperation {
    Upsert,
    Deleted,
    AccessLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceChange {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub parent: Option<String>,
    pub source_at: Option<i64>,
    pub operation: SourceOperation,
    /// The stored facts of this version, from which the resource
    /// computes Page Signals. It never holds source words (ADR-0011).
    pub metadata: serde_json::Value,
}

/// The synced source content that one call of a Run read, in one
/// resource of the Connection of the call (ADR-0008). A call that reads
/// one Source Item names the item. A call that reads every item of one
/// parent, such as the messages of one mail thread, names the parent.
/// A Forget of a Source Item reaches each Run that read the item or its
/// parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceRead {
    Item { resource: String, id: String },
    Parent { resource: String, id: String },
}

/// One filter decision about one Subject Page (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageReflection {
    pub page_path: String,
    /// The source subject of the page, such as a mail thread id.
    pub page_subject: String,
    pub filter_revision: i64,
    pub decided_by: ReflectionPass,
    pub selection: Selection,
    pub decided_at: i64,
}

/// The pass that decided about one Subject Page (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionPass {
    /// Delivery of the live arrivals of one acquisition batch.
    Live,
    /// The Backfill Reflection of the historical pages of one resource.
    Backfill,
}

impl ReflectionPass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Backfill => "backfill",
        }
    }
}

/// One historical Subject Page that the Backfill Reflection can decide
/// about (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillPage {
    /// The source subject, such as a mail thread id.
    pub subject: String,
    /// The time of the newest historical arrival of the page.
    pub newest_at: i64,
}

/// How many Subject Pages one backfill Run reflects (ADR-0011).
pub const BACKFILL_PAGES_PER_RUN: u32 = 20;
/// How many backfill Runs one resource starts inside one window.
pub const BACKFILL_RUNS_PER_WINDOW: i64 = 50;
/// The budget window of the Backfill Reflection.
pub const BACKFILL_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// How many failed Runs one page waits through before it stops
/// (ADR-0011).
pub const BACKFILL_ATTEMPTS_PER_PAGE: i64 = 3;

/// The Page Signals of one resource. One implementation serves one
/// resource, so another resource can offer its own signals without a
/// change to the filter (ADR-0011).
#[async_trait]
pub trait SignalSource: Send + Sync {
    /// The signals of the page of one subject, from stored metadata.
    async fn signals(&self, subject: &str, now: i64) -> Result<PageSignals, StoreError>;
}

/// What one resource offers the filter editor, for the settings API.
#[async_trait]
pub trait CatalogueProvider: Send + Sync {
    async fn catalogue(&self, key: &SourceKey) -> Result<Catalogue, StoreError>;
    /// The filter the resource has until the user writes their own.
    fn default_filter(&self, key: &SourceKey) -> ReflectionFilter;
    /// What `filter` decides about the stored pages of the resource,
    /// from stored metadata in one pass. No page body and no model.
    async fn preview(
        &self,
        key: &SourceKey,
        filter: &ReflectionFilter,
        now: i64,
    ) -> Result<FilterPreview, StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SourceBatch {
    pub historical: bool,
    #[schema(value_type = Vec<Object>)]
    pub arrivals: Vec<crate::NormalizedEvent>,
    pub expected_revision: i64,
    pub checkpoint: serde_json::Value,
    pub caught_up: bool,
    pub changes: Vec<SourceChange>,
}

/// One immutable acquisition.
#[derive(Debug, Clone)]
pub struct SourceVersion {
    pub sequence: i64,
    pub source: SourceChange,
    pub observed_at: i64,
    pub historical: bool,
    pub arrival: Option<crate::NormalizedEvent>,
}

/// Source content read for an arrival or a scoped foreground request.
#[derive(Debug, Clone)]
pub struct SourceContent {
    pub items: Vec<SourceChange>,
    pub contents: Vec<SourceText>,
}

/// One adapter-attributed item. Full text is transient.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceText {
    pub id: String,
    pub version: String,
    pub text: String,
    pub native_episode: Option<String>,
}

#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    async fn pending_invalidations(
        &self,
        workspace: &WorkspaceId,
        limit: u32,
    ) -> Result<Vec<KnowledgeInvalidation>, StoreError>;
    async fn acknowledge_invalidation(
        &self,
        workspace: &WorkspaceId,
        id: i64,
    ) -> Result<(), StoreError>;
    async fn configure(&self, config: SyncConfig, now: i64) -> Result<SyncStatus, StoreError>;
    /// The state of one resource. `now` measures the budget window of
    /// the Backfill Reflection (ADR-0011).
    async fn status(&self, key: &SourceKey, now: i64) -> Result<Option<SyncStatus>, StoreError>;
    async fn reset(
        &self,
        key: &SourceKey,
        expected_revision: i64,
        now: i64,
    ) -> Result<(), StoreError>;
    /// Write one batch and advance the checkpoint. A change of a source
    /// item that a Forget blocks writes no row, so no row holds its id
    /// or its record; `keys` gives the suppression key when the
    /// Workspace has a suppression row (ADR-0008).
    async fn acquire(
        &self,
        key: &SourceKey,
        batch: &SourceBatch,
        now: i64,
        keys: &dyn crate::ForgetKeys,
    ) -> Result<(), StoreError>;
    /// The oldest unacknowledged arrivals that are due for a new attempt.
    async fn arrivals(
        &self,
        key: &SourceKey,
        limit: u32,
        now: i64,
    ) -> Result<Vec<SourceVersion>, StoreError>;
    async fn versions(
        &self,
        key: &SourceKey,
        after_sequence: i64,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError>;
    /// Record one appended arrival.
    async fn acknowledge_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        now: i64,
    ) -> Result<(), StoreError>;
    /// Record one failed arrival and hold it until its retry time. A
    /// permanent failure, or the fifth attempt, skips the arrival instead
    /// so that the other arrivals in the pass continue.
    async fn fail_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        permanent: bool,
        reason: &str,
        now: i64,
    ) -> Result<(), StoreError>;
    async fn collection_error(
        &self,
        key: &SourceKey,
        arrival: bool,
        error: Option<&str>,
    ) -> Result<(), StoreError>;
    /// Record what the filter decided about one Subject Page. One
    /// decision per page and filter revision; a later pass replaces it.
    async fn record_reflection(
        &self,
        key: &SourceKey,
        reflection: &PageReflection,
    ) -> Result<(), StoreError>;
    /// The historical Subject Pages of one resource that no decision of
    /// `filter_revision` covers, newest arrival first (ADR-0011).
    async fn backfill_candidates(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<BackfillPage>, StoreError>;
    /// Return the pages of every backfill Run that ended without a
    /// reflection to the batch, and count the attempt on each page. The
    /// answer is the number of pages released (ADR-0011).
    async fn release_failed_backfill_runs(&self, key: &SourceKey) -> Result<u64, StoreError>;
    /// The paths of the backfill pages that reflect and wait for a Run
    /// under the attempt limit, newest decision first.
    async fn backfill_batch(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<String>, StoreError>;
    /// Record the Wake-up that reflects one batch of backfill pages.
    async fn record_backfill_run(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        page_paths: &[String],
        wakeup_id: &crate::WakeupId,
        now: i64,
    ) -> Result<(), StoreError>;
    /// The upsert versions that hold no metadata, oldest first
    /// (ADR-0011).
    async fn versions_without_metadata(
        &self,
        key: &SourceKey,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError>;
    /// Write the metadata of one version. The backfill decisions of the
    /// version's subject that no Run reflected yet are dropped, so the
    /// filter decides about the page with its signals (ADR-0011).
    async fn complete_metadata(
        &self,
        key: &SourceKey,
        sequence: i64,
        metadata: &serde_json::Value,
    ) -> Result<(), StoreError>;
    /// The stored metadata of every acquired version of one parent, such
    /// as the messages of one mail thread.
    async fn parent_metadata(
        &self,
        key: &SourceKey,
        parent: &str,
    ) -> Result<Vec<serde_json::Value>, StoreError>;
    /// The stored metadata of every parent of one resource, grouped by
    /// parent and ordered by sequence inside each parent. One pass
    /// serves the filter preview.
    async fn parents_metadata(
        &self,
        key: &SourceKey,
    ) -> Result<Vec<(String, Vec<serde_json::Value>)>, StoreError>;
    /// The number of distinct parents whose metadata field holds `value`
    /// and whose newest source time is at or after `since`.
    async fn parents_with_metadata(
        &self,
        key: &SourceKey,
        field: &str,
        value: &str,
        since: i64,
    ) -> Result<i64, StoreError>;
}
