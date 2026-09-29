//! SQLite implementations of the pagis-core repository traits.
//! The only crate that touches sqlx.

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
mod grant_store;
mod host_store;
mod identity_store;
mod keypad_failure_store;
mod knowledge_events;
mod knowledge_store;
mod memory_page_index;
mod message_store;
mod model_alias_store;
mod onboarding_store;
mod participant_store;
mod pending_evidence_store;
mod phone_number_store;
mod plugin_store;
mod plugin_tool_store;
mod pool;
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

pub use agent_mailbox_store::SqliteAgentMailboxStore;
pub use agent_store::SqliteAgentStore;
pub use artifact_store::SqliteArtifactStore;
pub use brief_store::SqliteBriefStore;
pub use call_store::SqliteCallStore;
pub use capability_snapshot_store::SqliteCapabilitySnapshotStore;
pub use channel_store::SqliteChannelStore;
pub use continuation_store::SqliteContinuationStore;
pub use contribution_store::SqliteContributionStore;
pub use conversation_evidence_store::SqliteConversationEvidenceStore;
pub use event_log::SqliteEventLog;
pub use event_subscription_store::SqliteEventSubscriptionStore;
pub use grant_store::SqliteGrantStore;
pub use host_store::SqliteHostStore;
pub use identity_store::{
    SqliteOrgStore, SqliteSessionStore, SqliteSignInLinkStore, SqliteUserStore,
    seed_org_and_administrator,
};
pub use keypad_failure_store::SqliteKeypadFailureStore;
pub use knowledge_store::SqliteKnowledgeStore;
pub use memory_page_index::SqliteMemoryPageIndex;
pub use message_store::SqliteMessageStore;
pub use model_alias_store::SqliteModelAliasStore;
pub use onboarding_store::SqliteOnboardingStore;
pub use participant_store::SqliteParticipantStore;
pub use pending_evidence_store::SqlitePendingEvidenceStore;
pub use phone_number_store::SqlitePhoneNumberStore;
pub use plugin_store::SqlitePluginStore;
pub use plugin_tool_store::SqlitePluginToolStore;
pub use pool::{begin_write, connect, connect_memory};
pub use request_store::SqliteRequestStore;
pub use resource_store::{SqliteConnectionStore, SqliteCredentialStore};
pub use retention_policy_store::SqliteRetentionPolicyStore;
pub use run_store::SqliteRunStore;
pub use schedule_store::SqliteScheduleStore;
pub use sent_mail_store::SqliteSentMailStore;
pub use software_store::SqliteSoftwareStore;
pub use text_record_store::SqliteTextRecordStore;
pub use trigger_store::SqliteTriggerStore;
pub use trust_list_store::SqliteTrustListStore;
pub use usage_store::SqliteUsageStore;
pub use workspace_store::SqliteWorkspaceStore;

/// Embedded migrations, applied automatically at daemon start and by
/// `#[sqlx::test(migrations = ...)]` harnesses.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub(crate) fn db_err(e: sqlx::Error) -> pagis_core::StoreError {
    pagis_core::StoreError::Database(Box::new(e))
}

/// Whether SQLite refused a write because a unique index already holds
/// the value. The caller turns that into a message for the user.
pub(crate) fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(error) if error.is_unique_violation())
}

mod forget_store;
pub use forget_store::SqliteForgetStore;

mod forget_journal;
mod forget_memory;

mod forget_plan;
mod forget_purge;

use std::sync::Arc;

/// Open the database file the state directory holds, apply the
/// migrations, and answer the store set. The daemon calls this or the
/// Postgres one, and names no pool type either way.
pub async fn open(db_path: &std::path::Path) -> Result<pagis_core::Stores, sqlx::Error> {
    let pool = connect(db_path).await?;
    MIGRATOR.run(&pool).await?;
    Ok(stores(pool))
}

/// Every store of this backend, over one pool.
pub fn stores(pool: sqlx::SqlitePool) -> pagis_core::Stores {
    pagis_core::Stores {
        agent_mailboxes: Arc::new(SqliteAgentMailboxStore::new(pool.clone())),
        agents: Arc::new(SqliteAgentStore::new(pool.clone())),
        artifacts: Arc::new(SqliteArtifactStore::new(pool.clone())),
        briefs: Arc::new(SqliteBriefStore::new(pool.clone())),
        calls: Arc::new(SqliteCallStore::new(pool.clone())),
        capability_snapshots: Arc::new(SqliteCapabilitySnapshotStore::new(pool.clone())),
        channels: Arc::new(SqliteChannelStore::new(pool.clone())),
        connections: Arc::new(SqliteConnectionStore::new(pool.clone())),
        continuations: Arc::new(SqliteContinuationStore::new(pool.clone())),
        contributions: Arc::new(SqliteContributionStore::new(pool.clone())),
        conversation_evidence: Arc::new(SqliteConversationEvidenceStore::new(pool.clone())),
        credentials: Arc::new(SqliteCredentialStore::new(pool.clone())),
        events: Arc::new(SqliteEventLog::new(pool.clone())),
        forget: Arc::new(SqliteForgetStore::new(pool.clone())),
        grants: Arc::new(SqliteGrantStore::new(pool.clone())),
        hosts: Arc::new(SqliteHostStore::new(pool.clone())),
        keypad_failures: Arc::new(SqliteKeypadFailureStore::new(pool.clone())),
        knowledge: Arc::new(SqliteKnowledgeStore::new(pool.clone())),
        memory_pages: Arc::new(SqliteMemoryPageIndex::new(pool.clone())),
        messages: Arc::new(SqliteMessageStore::new(pool.clone())),
        model_aliases: Arc::new(SqliteModelAliasStore::new(pool.clone())),
        onboarding: Arc::new(SqliteOnboardingStore::new(pool.clone())),
        orgs: Arc::new(SqliteOrgStore::new(pool.clone())),
        participants: Arc::new(SqliteParticipantStore::new(pool.clone())),
        pending_evidence: Arc::new(SqlitePendingEvidenceStore::new(pool.clone())),
        phone_numbers: Arc::new(SqlitePhoneNumberStore::new(pool.clone())),
        plugin_tools: Arc::new(SqlitePluginToolStore::new(pool.clone())),
        plugins: Arc::new(SqlitePluginStore::new(pool.clone())),
        requests: Arc::new(SqliteRequestStore::new(pool.clone())),
        retention_policies: Arc::new(SqliteRetentionPolicyStore::new(pool.clone())),
        runs: Arc::new(SqliteRunStore::new(pool.clone())),
        schedules: Arc::new(SqliteScheduleStore::new(pool.clone())),
        sent_mail: Arc::new(SqliteSentMailStore::new(pool.clone())),
        sessions: Arc::new(SqliteSessionStore::new(pool.clone())),
        sign_in_links: Arc::new(SqliteSignInLinkStore::new(pool.clone())),
        software: Arc::new(SqliteSoftwareStore::new(pool.clone())),
        subscriptions: Arc::new(SqliteEventSubscriptionStore::new(pool.clone())),
        text_records: Arc::new(SqliteTextRecordStore::new(pool.clone())),
        triggers: Arc::new(SqliteTriggerStore::new(pool.clone())),
        trust_list: Arc::new(SqliteTrustListStore::new(pool.clone())),
        usage: Arc::new(SqliteUsageStore::new(pool.clone())),
        users: Arc::new(SqliteUserStore::new(pool.clone())),
        workspaces: Arc::new(SqliteWorkspaceStore::new(pool)),
    }
}
