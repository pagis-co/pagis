//! Postgres implementations of the pagis-core repository traits.
//!
//! The sibling of `pagis-storage-sqlite`: same module layout, same file
//! names, same order of items in each file, so the two crates read as
//! two columns of one table. What differs is the SQL dialect, the pool
//! type, and the full-text index, which is `tsvector` with a GIN index
//! here and FTS5 there (ADR-0008).
//!
//! One of the two crates that touches sqlx.

mod agent_mailbox_store;
mod agent_store;
mod artifact_store;
mod brief_store;
mod call_store;
mod capability_snapshot_store;
mod channel_store;
mod continuation_store;
mod contribution_store;
mod conversation_evidence_store;
mod event_log;
mod event_subscription_store;
mod forget_journal;
mod forget_memory;
mod forget_plan;
mod forget_purge;
mod forget_store;
mod grant_store;
mod host_store;
mod identity_store;
mod keypad_failure_store;
mod knowledge_events;
mod knowledge_store;
mod memory_page_index;
mod message_store;
mod model_alias_store;
mod model_request_capture_store;
mod onboarding_store;
mod participant_store;
mod pending_evidence_store;
mod phone_number_store;
mod plugin_store;
mod plugin_tool_store;
mod pool;
mod push_subscription_store;
mod request_store;
mod resource_store;
mod retention_policy_store;
mod run_store;
mod schedule_store;
mod sent_mail_store;
mod software_store;
mod text_record_store;
mod trigger_store;
mod trust_list_store;
mod usage_store;
mod workspace_store;

pub use agent_mailbox_store::PostgresAgentMailboxStore;
pub use agent_store::PostgresAgentStore;
pub use artifact_store::PostgresArtifactStore;
pub use brief_store::PostgresBriefStore;
pub use call_store::PostgresCallStore;
pub use capability_snapshot_store::PostgresCapabilitySnapshotStore;
pub use channel_store::PostgresChannelStore;
pub use continuation_store::PostgresContinuationStore;
pub use contribution_store::PostgresContributionStore;
pub use conversation_evidence_store::PostgresConversationEvidenceStore;
pub use event_log::PostgresEventLog;
pub use event_subscription_store::PostgresEventSubscriptionStore;
pub use forget_store::PostgresForgetStore;
pub use grant_store::PostgresGrantStore;
pub use host_store::PostgresHostStore;
pub use identity_store::{
    PostgresOrgStore, PostgresSessionStore, PostgresSignInLinkStore, PostgresUserStore,
    seed_org_and_administrator,
};
pub use keypad_failure_store::PostgresKeypadFailureStore;
pub use knowledge_store::PostgresKnowledgeStore;
pub use memory_page_index::PostgresMemoryPageIndex;
pub use message_store::PostgresMessageStore;
pub use model_alias_store::PostgresModelAliasStore;
pub use model_request_capture_store::PostgresModelRequestCaptureStore;
pub use onboarding_store::PostgresOnboardingStore;
pub use participant_store::PostgresParticipantStore;
pub use pending_evidence_store::PostgresPendingEvidenceStore;
pub use phone_number_store::PostgresPhoneNumberStore;
pub use plugin_store::PostgresPluginStore;
pub use plugin_tool_store::PostgresPluginToolStore;
pub use pool::{begin_write, connect};
pub use push_subscription_store::PostgresPushSubscriptionStore;
pub use request_store::PostgresRequestStore;
pub use resource_store::{PostgresConnectionStore, PostgresCredentialStore};
pub use retention_policy_store::PostgresRetentionPolicyStore;
pub use run_store::PostgresRunStore;
pub use schedule_store::PostgresScheduleStore;
pub use sent_mail_store::PostgresSentMailStore;
pub use software_store::PostgresSoftwareStore;
pub use text_record_store::PostgresTextRecordStore;
pub use trigger_store::PostgresTriggerStore;
pub use trust_list_store::PostgresTrustListStore;
pub use usage_store::PostgresUsageStore;
pub use workspace_store::PostgresWorkspaceStore;

use std::sync::Arc;

use sqlx::PgPool;

/// Embedded migrations, applied at daemon start and by the test
/// harness.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// Open the database the configuration names, apply the migrations, and
/// answer the store set. The daemon calls this or the SQLite one, and
/// names no pool type either way.
pub async fn open(url: &str) -> Result<pagis_core::Stores, sqlx::Error> {
    let pool = connect(url).await?;
    MIGRATOR.run(&pool).await?;
    Ok(stores(pool))
}

/// Every store of this backend, over one pool.
pub fn stores(pool: PgPool) -> pagis_core::Stores {
    pagis_core::Stores {
        agent_mailboxes: Arc::new(PostgresAgentMailboxStore::new(pool.clone())),
        agents: Arc::new(PostgresAgentStore::new(pool.clone())),
        artifacts: Arc::new(PostgresArtifactStore::new(pool.clone())),
        briefs: Arc::new(PostgresBriefStore::new(pool.clone())),
        calls: Arc::new(PostgresCallStore::new(pool.clone())),
        capability_snapshots: Arc::new(PostgresCapabilitySnapshotStore::new(pool.clone())),
        channels: Arc::new(PostgresChannelStore::new(pool.clone())),
        connections: Arc::new(PostgresConnectionStore::new(pool.clone())),
        continuations: Arc::new(PostgresContinuationStore::new(pool.clone())),
        contributions: Arc::new(PostgresContributionStore::new(pool.clone())),
        conversation_evidence: Arc::new(PostgresConversationEvidenceStore::new(pool.clone())),
        credentials: Arc::new(PostgresCredentialStore::new(pool.clone())),
        events: Arc::new(PostgresEventLog::new(pool.clone())),
        forget: Arc::new(PostgresForgetStore::new(pool.clone())),
        grants: Arc::new(PostgresGrantStore::new(pool.clone())),
        hosts: Arc::new(PostgresHostStore::new(pool.clone())),
        keypad_failures: Arc::new(PostgresKeypadFailureStore::new(pool.clone())),
        knowledge: Arc::new(PostgresKnowledgeStore::new(pool.clone())),
        memory_pages: Arc::new(PostgresMemoryPageIndex::new(pool.clone())),
        messages: Arc::new(PostgresMessageStore::new(pool.clone())),
        model_request_captures: Arc::new(PostgresModelRequestCaptureStore::new(pool.clone())),
        model_aliases: Arc::new(PostgresModelAliasStore::new(pool.clone())),
        onboarding: Arc::new(PostgresOnboardingStore::new(pool.clone())),
        orgs: Arc::new(PostgresOrgStore::new(pool.clone())),
        participants: Arc::new(PostgresParticipantStore::new(pool.clone())),
        pending_evidence: Arc::new(PostgresPendingEvidenceStore::new(pool.clone())),
        phone_numbers: Arc::new(PostgresPhoneNumberStore::new(pool.clone())),
        plugin_tools: Arc::new(PostgresPluginToolStore::new(pool.clone())),
        plugins: Arc::new(PostgresPluginStore::new(pool.clone())),
        push_subscriptions: Arc::new(PostgresPushSubscriptionStore::new(pool.clone())),
        requests: Arc::new(PostgresRequestStore::new(pool.clone())),
        retention_policies: Arc::new(PostgresRetentionPolicyStore::new(pool.clone())),
        runs: Arc::new(PostgresRunStore::new(pool.clone())),
        schedules: Arc::new(PostgresScheduleStore::new(pool.clone())),
        sent_mail: Arc::new(PostgresSentMailStore::new(pool.clone())),
        sessions: Arc::new(PostgresSessionStore::new(pool.clone())),
        sign_in_links: Arc::new(PostgresSignInLinkStore::new(pool.clone())),
        software: Arc::new(PostgresSoftwareStore::new(pool.clone())),
        subscriptions: Arc::new(PostgresEventSubscriptionStore::new(pool.clone())),
        text_records: Arc::new(PostgresTextRecordStore::new(pool.clone())),
        triggers: Arc::new(PostgresTriggerStore::new(pool.clone())),
        trust_list: Arc::new(PostgresTrustListStore::new(pool.clone())),
        usage: Arc::new(PostgresUsageStore::new(pool.clone())),
        users: Arc::new(PostgresUserStore::new(pool.clone())),
        workspaces: Arc::new(PostgresWorkspaceStore::new(pool)),
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> pagis_core::StoreError {
    pagis_core::StoreError::Database(Box::new(e))
}

/// Whether Postgres refused a write because a unique index already
/// holds the value. The caller turns that into a message for the user.
pub(crate) fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(error) if error.is_unique_violation())
}
