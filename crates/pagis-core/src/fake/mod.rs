//! In-memory stores of one installation. A store set shares state, so
//! writes that touch several records are one operation.

use crate::knowledge::{
    BACKFILL_ATTEMPTS_PER_PAGE, BACKFILL_RUNS_PER_WINDOW, BACKFILL_WINDOW_MS, BackfillPage,
    CatalogueProvider, KnowledgeInvalidation, KnowledgeStore, PageReflection, ReflectionPass,
    SourceKey, SourceOperation, SourceRead, SourceVersion, SyncConfig, SyncStatus,
};
use crate::reflection_filter::*;
use crate::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

mod audit;
mod automation;
mod calendar;
mod conversation;
mod events;
mod evidence;
mod forget;
mod identity;
mod knowledge;
mod memory;
mod resources;
mod telephony;
pub use events::FakeEventBus;
pub use memory::FakeMemoryStore;
// These existing in-memory implementations are also part of the fake set.
pub use crate::{MemorySecretStore, VolatilePageIndex};

type Shared = Arc<Mutex<Data>>;

/// A set of related fake stores. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeDatabase(Shared);

#[derive(Clone, Default)]
pub struct FakeAuditLog(Shared);

/// The in-memory Calendar store.
#[derive(Clone, Default)]
pub struct FakeCalendarStore(Shared);

/// Build every repository over one in-memory database.
pub fn stores() -> Stores {
    FakeDatabase::default().stores()
}

struct Table<K, V>(BTreeMap<(WorkspaceId, K), V>);
impl<K, V> Default for Table<K, V> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}
impl<K: Ord + Clone, V> Table<K, V> {
    fn get(&self, workspace: &WorkspaceId, key: &K) -> Option<&V> {
        self.0.get(&(workspace.clone(), key.clone()))
    }
    fn get_mut(&mut self, workspace: &WorkspaceId, key: &K) -> Option<&mut V> {
        self.0.get_mut(&(workspace.clone(), key.clone()))
    }
    fn insert(&mut self, workspace: &WorkspaceId, key: K, value: V) -> Result<(), StoreError> {
        match self.0.entry((workspace.clone(), key)) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(value);
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(_) => Err(conflict("record exists")),
        }
    }
    fn put(&mut self, workspace: &WorkspaceId, key: K, value: V) {
        self.0.insert((workspace.clone(), key), value);
    }
    fn remove(&mut self, workspace: &WorkspaceId, key: &K) -> Option<V> {
        self.0.remove(&(workspace.clone(), key.clone()))
    }
    fn rows<'a>(&'a self, workspace: &'a WorkspaceId) -> impl Iterator<Item = &'a V> {
        self.0
            .iter()
            .filter(move |((w, _), _)| w == workspace)
            .map(|(_, v)| v)
    }
    fn rows_mut<'a>(&'a mut self, workspace: &'a WorkspaceId) -> impl Iterator<Item = &'a mut V> {
        self.0
            .iter_mut()
            .filter(move |((w, _), _)| w == workspace)
            .map(|(_, v)| v)
    }
}
fn conflict(message: &str) -> StoreError {
    StoreError::Conflict(message.to_string())
}
fn page<K: Ord, V>(mut rows: Vec<(K, V)>, before: Option<&K>, limit: u32) -> Vec<V> {
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter()
        .filter(|(key, _)| before.is_none_or(|before| key < before))
        .take(limit as usize)
        .map(|(_, row)| row)
        .collect()
}
// Store handles.
/// The in-memory `WorkspaceStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeWorkspaceStore(Shared);
#[derive(Clone)]
pub struct FakeWorkspaceMemberStore(Shared);
/// The in-memory `OnboardingStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeOnboardingStore(Shared);
/// The in-memory `OrgStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeOrgStore(Shared);
/// The in-memory `UserStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeUserStore(Shared);
/// The in-memory `SessionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeSessionStore(Shared);
/// The in-memory `SignInLinkStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeSignInLinkStore(Shared);
/// The in-memory `HostStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeHostStore(Shared);
/// The in-memory `PushSubscriptionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakePushSubscriptionStore(Shared);
/// The in-memory `AgentStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeAgentStore(Shared);
/// The in-memory `ModelAliasStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeModelAliasStore(Shared);
/// The in-memory `ConnectionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeConnectionStore(Shared);
/// The in-memory `CredentialStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeCredentialStore(Shared);
/// The in-memory `GrantStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeGrantStore(Shared);
/// The in-memory `CapabilitySnapshotStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeCapabilitySnapshotStore(Shared);
/// The in-memory `FileStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeFileStore(Shared);
/// The in-memory `RetentionPolicyStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeRetentionPolicyStore(Shared);
/// The in-memory `SoftwareStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeSoftwareStore(Shared);
/// The in-memory `ContributionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeContributionStore(Shared);
/// The in-memory `PluginStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakePluginStore(Shared);
/// The in-memory `PluginToolStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakePluginToolStore(Shared);
/// The in-memory `ChannelStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeChannelStore(Shared);
/// The in-memory `ParticipantStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeParticipantStore(Shared);
/// The in-memory `MessageStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeMessageStore(Shared);
/// The in-memory `RunStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeRunStore(Shared);
/// The in-memory `RequestStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeRequestStore(Shared);
/// The in-memory `UsageStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeUsageStore(Shared);
/// The in-memory `ModelRequestCaptureStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeModelRequestCaptureStore(Shared);
/// The in-memory `CodingSessionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeCodingSessionStore(Shared);
/// The in-memory `ConversationEvidenceStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeConversationEvidenceStore(Shared);
/// The in-memory `RunJournal`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeRunJournal(Shared);
/// The in-memory `PhoneNumberStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakePhoneNumberStore(Shared);
/// The in-memory `AgentMailboxStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeAgentMailboxStore(Shared);
/// The in-memory `SentMailStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeSentMailStore(Shared);
/// The in-memory `CallStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeCallStore(Shared);
/// The in-memory `TextRecordStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeTextRecordStore(Shared);
/// The in-memory `TrustListStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeTrustListStore(Shared);
/// The in-memory `KeypadFailureStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeKeypadFailureStore(Shared);
/// The in-memory `BriefStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeBriefStore(Shared);
/// The in-memory `ContinuationStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeContinuationStore(Shared);
/// The in-memory `PendingEvidenceStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakePendingEvidenceStore(Shared);
/// The in-memory `ScheduleStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeScheduleStore(Shared);
/// The in-memory `TriggerStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeTriggerStore(Shared);
/// The in-memory `EventSubscriptionStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeEventSubscriptionStore(Shared);
/// The in-memory `KnowledgeStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeKnowledgeStore(Shared);
/// The in-memory `CatalogueProvider`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeCatalogueProvider(Shared);
/// The in-memory `ForgetStore`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeForgetStore(Shared);
/// The in-memory `ForgetKeys`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeForgetKeys(Shared);
/// The in-memory `EventLog`. Clones share their records.
#[derive(Clone, Default)]
pub struct FakeEventLog(Shared);

impl FakeDatabase {
    pub fn bus(&self) -> FakeEventBus {
        FakeEventBus::new(FakeEventLog(self.0.clone()))
    }
    pub fn stores(&self) -> Stores {
        Stores {
            mail_assessments: Arc::new(FakeMailAssessmentStore(self.0.clone())),
            audit: Arc::new(FakeAuditLog(self.0.clone())),
            workspace_mailboxes: Arc::new(FakeWorkspaceMailboxStore(self.0.clone())),
            agent_mailboxes: Arc::new(FakeAgentMailboxStore(self.0.clone())),
            agents: Arc::new(FakeAgentStore(self.0.clone())),
            files: Arc::new(FakeFileStore(self.0.clone())),
            briefs: Arc::new(FakeBriefStore(self.0.clone())),
            calls: Arc::new(FakeCallStore(self.0.clone())),
            capability_snapshots: Arc::new(FakeCapabilitySnapshotStore(self.0.clone())),
            channels: Arc::new(FakeChannelStore(self.0.clone())),
            coding_sessions: Arc::new(FakeCodingSessionStore(self.0.clone())),
            connections: Arc::new(FakeConnectionStore(self.0.clone())),
            continuations: Arc::new(FakeContinuationStore(self.0.clone())),
            contributions: Arc::new(FakeContributionStore(self.0.clone())),
            conversation_evidence: Arc::new(FakeConversationEvidenceStore(self.0.clone())),
            credentials: Arc::new(FakeCredentialStore(self.0.clone())),
            events: Arc::new(FakeEventLog(self.0.clone())),
            forget: Arc::new(FakeForgetStore(self.0.clone())),
            grants: Arc::new(FakeGrantStore(self.0.clone())),
            hosts: Arc::new(FakeHostStore(self.0.clone())),
            keypad_failures: Arc::new(FakeKeypadFailureStore(self.0.clone())),
            knowledge: Arc::new(FakeKnowledgeStore(self.0.clone())),
            memory_pages: self.0.lock().unwrap().memory_pages.clone(),
            messages: Arc::new(FakeMessageStore(self.0.clone())),
            model_aliases: Arc::new(FakeModelAliasStore(self.0.clone())),
            model_request_captures: Arc::new(FakeModelRequestCaptureStore(self.0.clone())),
            onboarding: Arc::new(FakeOnboardingStore(self.0.clone())),
            orgs: Arc::new(FakeOrgStore(self.0.clone())),
            participants: Arc::new(FakeParticipantStore(self.0.clone())),
            pending_evidence: Arc::new(FakePendingEvidenceStore(self.0.clone())),
            phone_numbers: Arc::new(FakePhoneNumberStore(self.0.clone())),
            plugin_tools: Arc::new(FakePluginToolStore(self.0.clone())),
            plugins: Arc::new(FakePluginStore(self.0.clone())),
            calendar: Arc::new(FakeCalendarStore(self.0.clone())),
            push_subscriptions: Arc::new(FakePushSubscriptionStore(self.0.clone())),
            requests: Arc::new(FakeRequestStore(self.0.clone())),
            retention_policies: Arc::new(FakeRetentionPolicyStore(self.0.clone())),
            runs: Arc::new(FakeRunStore(self.0.clone())),
            run_journal: Arc::new(FakeRunJournal(self.0.clone())),
            schedules: Arc::new(FakeScheduleStore(self.0.clone())),
            sent_mail: Arc::new(FakeSentMailStore(self.0.clone())),
            sessions: Arc::new(FakeSessionStore(self.0.clone())),
            sign_in_links: Arc::new(FakeSignInLinkStore(self.0.clone())),
            software: Arc::new(FakeSoftwareStore(self.0.clone())),
            subscriptions: Arc::new(FakeEventSubscriptionStore(self.0.clone())),
            text_records: Arc::new(FakeTextRecordStore(self.0.clone())),
            triggers: Arc::new(FakeTriggerStore(self.0.clone())),
            trust_list: Arc::new(FakeTrustListStore(self.0.clone())),
            usage: Arc::new(FakeUsageStore(self.0.clone())),
            users: Arc::new(FakeUserStore(self.0.clone())),
            workspaces: Arc::new(FakeWorkspaceStore(self.0.clone())),
            workspace_members: Arc::new(FakeWorkspaceMemberStore(self.0.clone())),
        }
    }
}

#[derive(Default)]
struct Data {
    audit: BTreeMap<AuditEntryId, AuditEntry>,
    workspace_mail_sends: BTreeMap<WorkspaceId, (i64, u32)>,
    mail_assessments: BTreeMap<(WorkspaceId, String, String), crate::MailAssessment>,
    workspace_mailboxes: BTreeMap<WorkspaceId, crate::WorkspaceMailbox>,
    agents: Table<AgentId, Agent>,
    model_aliases: Table<String, ModelAlias>,
    connections: Table<ConnectionId, Connection>,
    credentials: Table<CredentialId, Credential>,
    grants: Table<GrantId, Grant>,
    files: Table<FileId, File>,
    software: Table<SoftwarePackageId, SoftwarePackage>,
    contributions: Table<ContributionId, Contribution>,
    plugins: Table<PluginId, Plugin>,
    channels: Table<ChannelId, Channel>,
    participants: Table<ParticipantId, ChannelParticipant>,
    messages: Table<MessageId, Message>,
    runs: Table<RunId, Run>,
    requests: Table<RequestId, Request>,
    coding_sessions: Table<CodingSessionId, CodingSession>,
    phone_numbers: Table<PhoneNumberId, PhoneNumber>,
    agent_mailboxes: Table<AgentMailboxId, AgentMailbox>,
    sent_mail: Table<String, SentMail>,
    calls: Table<CallId, Call>,
    text_records: Table<TextRecordId, TextRecord>,
    trust_list: Table<TrustEntryId, TrustEntry>,
    schedules: Table<ScheduleId, Schedule>,
    subscriptions: Table<EventSubscriptionId, EventSubscription>,
    calendar: Table<CalendarEventId, CalendarEvent>,
    push_subscriptions: BTreeMap<PushSubscriptionId, PushSubscription>,

    workspaces: BTreeMap<WorkspaceId, Workspace>,
    workspace_members: BTreeMap<(WorkspaceId, UserId), crate::WorkspaceMember>,
    orgs: BTreeMap<OrgId, Org>,
    users: BTreeMap<UserId, User>,
    sessions: BTreeMap<SessionId, Session>,
    sign_in_links: BTreeMap<String, SignInLink>,
    hosts: Table<HostId, Host>,
    onboarding: Table<String, OnboardingModelVerification>,
    refresh_tokens: Table<ConnectionId, SealedSecret>,
    snapshots: BTreeMap<String, CapabilitySnapshotRecord>,
    snapshot_runs: Table<RunId, String>,
    retention: Table<String, Option<i64>>,
    software_versions: Table<(SoftwarePackageId, String), SoftwareVersion>,
    plugin_bindings: Table<(PluginId, String), PluginBinding>,
    plugin_tools: BTreeMap<(PluginId, String), PluginTools>,
    message_exposures: Table<MessageId, Vec<MemoryExposure>>,
    forgotten_messages: BTreeSet<(WorkspaceId, MessageId)>,
    source_reads: Table<(RunId, ConnectionId), Vec<SourceRead>>,
    usage: Table<UsageId, UsageRecord>,
    captures: Table<ModelRequestCaptureId, ModelRequestCapture>,
    coding_events: Table<CodingSessionId, Vec<CodingSessionEvent>>,
    model_tokens: Table<CodingSessionId, String>,
    tool_evidence: Table<String, (String, RetainedToolEvidence)>,
    journal: BTreeMap<RunId, RunJournalEntry>,
    purchase_intents: Table<PurchaseIntentId, PurchaseIntent>,
    keypad: BTreeMap<WorkspaceId, KeypadFailures>,
    briefs: Table<(AgentId, ChannelId, Option<MessageId>), BriefCursor>,
    continuations: Table<(AgentId, ChannelId, Option<MessageId>), ContinuationCheckpoint>,
    pending_evidence: Table<PendingEvidenceId, PendingEvidenceRecord>,
    review_cursors: Table<(AgentId, ChannelId, Option<MessageId>, String), MessageId>,
    review_leases: Table<PendingEvidenceId, i64>,
    schedule_revisions: Table<(ScheduleId, u32), ScheduleRevision>,
    occurrences: Table<ScheduleOccurrenceId, ScheduleOccurrence>,
    wakeups: Table<WakeupId, Wakeup>,
    collector_cursors: Table<(ConnectionId, String), String>,
    collector_backoffs: Table<(ConnectionId, String), CollectorTarget>,
    incoming_events: Table<IncomingEventId, IncomingEvent>,
    batches: Table<SourceBatchId, SourceBatch>,
    wakeup_events: Table<WakeupId, Vec<IncomingEventId>>,
    subscription_events: Table<EventSubscriptionId, Vec<IncomingEventId>>,
    knowledge: Table<(ConnectionId, String), knowledge::KnowledgeResource>,
    invalidations: Table<i64, KnowledgeInvalidation>,
    invalidation_sequence: i64,
    catalogues: Table<(ConnectionId, String), (Catalogue, ReflectionFilter, Vec<PageSignals>)>,
    forget_keys: BTreeMap<WorkspaceId, [u8; 32]>,
    forget_operations: Table<String, forget::ForgetRecord>,
    suppressions: Table<String, forget::Suppression>,
    raw_blocks: Table<String, ConnectionId>,
    forgotten_grants: Table<GrantId, String>,
    forgotten_runs: BTreeSet<(WorkspaceId, RunId)>,
    event_sequence: i64,
    events: Vec<Event>,
    subscribers: Vec<(
        EventScope,
        i64,
        futures::channel::mpsc::UnboundedSender<Event>,
    )>,
    memory_pages: Arc<VolatilePageIndex>,
}

fn queued_run(
    workspace: &WorkspaceId,
    agent: &AgentId,
    channel: Option<ChannelId>,
    root: Option<MessageId>,
    kind: TriggerKind,
    reference: Option<String>,
    at: i64,
) -> Run {
    Run {
        id: RunId::generate(),
        title: String::new(),
        workspace_id: workspace.clone(),
        agent_id: agent.clone(),
        channel_id: channel,
        root_message_id: root,
        trigger_kind: kind,
        trigger_ref: reference,
        hop_count: 0,
        origin: None,
        state: RunState::Queued,
        failure_kind: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: at,
        dismissed_at: None,
    }
}

/// The in-memory Workspace Mailbox records share the Address Ledger.
#[derive(Clone, Default)]
pub struct FakeWorkspaceMailboxStore(Shared);

#[derive(Clone, Default)]
pub struct FakeMailAssessmentStore(Shared);
#[async_trait::async_trait]
impl crate::MailAssessmentStore for FakeMailAssessmentStore {
    async fn get(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
    ) -> Result<Option<crate::MailAssessment>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assessments
            .get(&(workspace.clone(), mailbox.into(), message.into()))
            .cloned())
    }
    async fn put(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
        assessment: &crate::MailAssessment,
    ) -> Result<(), StoreError> {
        self.0.lock().unwrap().mail_assessments.insert(
            (workspace.clone(), mailbox.into(), message.into()),
            assessment.clone(),
        );
        Ok(())
    }
}
