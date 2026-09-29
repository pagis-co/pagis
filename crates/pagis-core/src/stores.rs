//! The store set: one handle that holds every repository of an
//! installation.
//!
//! `pagis-core` owns the traits, so it owns the set of them. A storage
//! crate builds one of these from its own pool, and the daemon reads
//! the fields. Nothing outside a storage crate names a pool type, so a
//! new backend is one more builder and no change anywhere else.

use std::sync::Arc;

use crate::continuation::ContinuationStore;
use crate::conversation_evidence::ConversationEvidenceStore;
use crate::forget::ForgetStore;
use crate::host::HostStore;
use crate::identity::{OrgStore, SessionStore, SignInLinkStore, UserStore};
use crate::keypad::KeypadFailureStore;
use crate::knowledge::KnowledgeStore;
use crate::memory::MemoryPageIndex;
use crate::usage::UsageStore;

use crate::store::{
    AgentMailboxStore, AgentStore, ArtifactStore, BriefStore, CallStore, CapabilitySnapshotStore,
    ChannelStore, ConnectionStore, ContributionStore, CredentialStore, EventLog,
    EventSubscriptionStore, GrantStore, MessageStore, ModelAliasStore, OnboardingStore,
    ParticipantStore, PendingEvidenceStore, PhoneNumberStore, PluginStore, PluginToolStore,
    RequestStore, RetentionPolicyStore, RunStore, ScheduleStore, SentMailStore, SoftwareStore,
    TextRecordStore, TriggerStore, TrustListStore, WorkspaceStore,
};

/// Every repository of one installation, behind its trait.
///
/// The fields are `Arc`, so a caller that needs one store for the life
/// of a task clones that field and holds nothing else.
#[derive(Clone)]
pub struct Stores {
    pub agent_mailboxes: Arc<dyn AgentMailboxStore>,
    pub agents: Arc<dyn AgentStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub briefs: Arc<dyn BriefStore>,
    pub calls: Arc<dyn CallStore>,
    pub capability_snapshots: Arc<dyn CapabilitySnapshotStore>,
    pub channels: Arc<dyn ChannelStore>,
    pub connections: Arc<dyn ConnectionStore>,
    pub continuations: Arc<dyn ContinuationStore>,
    pub contributions: Arc<dyn ContributionStore>,
    pub conversation_evidence: Arc<dyn ConversationEvidenceStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub events: Arc<dyn EventLog>,
    pub forget: Arc<dyn ForgetStore>,
    pub grants: Arc<dyn GrantStore>,
    pub hosts: Arc<dyn HostStore>,
    pub keypad_failures: Arc<dyn KeypadFailureStore>,
    pub knowledge: Arc<dyn KnowledgeStore>,
    pub memory_pages: Arc<dyn MemoryPageIndex>,
    pub messages: Arc<dyn MessageStore>,
    pub model_aliases: Arc<dyn ModelAliasStore>,
    pub onboarding: Arc<dyn OnboardingStore>,
    pub orgs: Arc<dyn OrgStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub pending_evidence: Arc<dyn PendingEvidenceStore>,
    pub phone_numbers: Arc<dyn PhoneNumberStore>,
    pub plugin_tools: Arc<dyn PluginToolStore>,
    pub plugins: Arc<dyn PluginStore>,
    pub requests: Arc<dyn RequestStore>,
    pub retention_policies: Arc<dyn RetentionPolicyStore>,
    pub runs: Arc<dyn RunStore>,
    pub schedules: Arc<dyn ScheduleStore>,
    pub sent_mail: Arc<dyn SentMailStore>,
    pub sessions: Arc<dyn SessionStore>,
    pub sign_in_links: Arc<dyn SignInLinkStore>,
    pub software: Arc<dyn SoftwareStore>,
    pub subscriptions: Arc<dyn EventSubscriptionStore>,
    pub text_records: Arc<dyn TextRecordStore>,
    pub triggers: Arc<dyn TriggerStore>,
    pub trust_list: Arc<dyn TrustListStore>,
    pub usage: Arc<dyn UsageStore>,
    pub users: Arc<dyn UserStore>,
    pub workspaces: Arc<dyn WorkspaceStore>,
}

/// Write the one Org of an installation and its administrator, and
/// answer the person. The daemon's seed calls it on an empty database,
/// and so does every test that writes a Workspace row of its own,
/// because a Workspace belongs to a person.
///
/// The person has no password: a local installation signs in with the
/// Client Credential.
pub async fn seed_org_and_administrator(
    orgs: &dyn OrgStore,
    users: &dyn UserStore,
    name: &str,
    now: crate::time::UnixMillis,
) -> Result<crate::identity::User, crate::store::StoreError> {
    let org = crate::identity::Org {
        id: crate::id::OrgId::generate(),
        name: name.to_string(),
        google_client_id: None,
        workspace_id: crate::id::WorkspaceId::generate(),
        created_at: now,
    };
    orgs.create(&org).await?;
    let user = crate::identity::User {
        id: crate::id::UserId::generate(),
        org_id: org.id,
        email: None,
        name: None,
        password_hash: None,
        role: crate::identity::UserRole::Administrator,
        disabled_at: None,
        last_signed_in_at: None,
        monthly_spend_cap_usd: None,
        created_at: now,
        updated_at: now,
    };
    users.create(&user).await?;
    Ok(user)
}

impl std::fmt::Debug for Stores {
    /// The set holds trait objects with nothing printable in them. The
    /// name is what a caller's `Debug` needs of it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stores")
    }
}
