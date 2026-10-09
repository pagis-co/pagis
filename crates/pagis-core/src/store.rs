//! Repository trait seams. The SQLite and Postgres stores implement them.

use async_trait::async_trait;

use crate::block::Block;
use crate::domain::{
    Agent, AgentMailbox, AgentMailboxState, Artifact, ArtifactKind, AuthorKind, Call,
    CapabilitySnapshotRecord, Channel, ChannelParticipant, Connection, Contribution,
    ContributionStatus, Credential, EventDeclaration, EventSubscription, Grant, IncomingEvent,
    MailboxCursor, Message, MessageStatus, MessagingReadiness, ModelAlias,
    OnboardingModelVerification, PhoneNumber, Plugin, PluginBinding, PluginState, PluginTools,
    PurchaseIntent, Request, RequestState, RetentionPolicy, Run, RunSlots, Schedule,
    ScheduleOccurrence, ScheduleRevision, SentMail, SoftwarePackage, SoftwareVersion, SourceBatch,
    TelnyxRelayState, TextConversation, TextDeliveryStatus, TextRecord, Wakeup, WakeupClaim,
    WakeupLanding, Workspace,
};
use crate::event::{Event, NewEvent};
use crate::id::{
    AgentId, AgentMailboxId, ArtifactId, CallId, ChannelId, ConnectionId, ContributionId, EventId,
    EventSubscriptionId, GrantId, HostId, IncomingEventId, MessageId, PhoneNumberId, PluginId,
    PurchaseIntentId, RequestId, RunId, ScheduleId, ScheduleOccurrenceId, SoftwarePackageId,
    TextRecordId, UserId, WakeupId, WorkspaceId,
};
use crate::time::UnixMillis;

#[async_trait]
pub trait PendingEvidenceStore: Send + Sync {
    async fn record(
        &self,
        evidence: crate::PendingEvidence,
    ) -> Result<Option<crate::PendingEvidenceRecord>, StoreError>;
    async fn review_cursor(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
        subject: &str,
    ) -> Result<Option<MessageId>, StoreError>;
    /// Mark one fixed compaction range as reviewed. The store also settles
    /// pending work whose complete source set is inside that range. Evidence
    /// that arrived after the fixed upper cursor stays pending.
    async fn settle_compaction(
        &self,
        evidence: &crate::PendingEvidence,
        memory_revision: Option<&str>,
        now: UnixMillis,
    ) -> Result<(), StoreError>;
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError>;
    /// The Agents with pending work, each with the Workspace that owns
    /// it. A claim needs both, and the sweep crosses every Workspace.
    async fn pending_agents(
        &self,
        now: UnixMillis,
    ) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError>;
    async fn claim(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        arrival_slots: u32,
        now: UnixMillis,
    ) -> Result<Vec<crate::PendingReviewClaim>, StoreError>;
    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<crate::PendingEvidenceRecord>, StoreError>;
    /// Leases left by a stopped daemon, for cross-store recovery.
    async fn leased(&self) -> Result<Vec<crate::PendingEvidenceRecord>, StoreError>;
    async fn complete(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        memory_revision: Option<&str>,
        now: UnixMillis,
    ) -> Result<bool, StoreError>;
    async fn fail(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        error: &str,
        now: UnixMillis,
    ) -> Result<bool, StoreError>;
    async fn invalidate(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        now: UnixMillis,
    ) -> Result<bool, StoreError>;
    async fn retry(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::PendingEvidenceId,
        now: UnixMillis,
    ) -> Result<bool, StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("corrupt record: {0}")]
    Corrupt(String),
    /// A uniqueness the schema owns, refused by the schema: a duplicate
    /// Connection alias, for one. The caller reports it to the user
    /// rather than logging an internal failure.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The Tenant Data Key that a key of the store derives from cannot
    /// be read or made.
    #[error(transparent)]
    Key(#[from] crate::SealError),
}

/// A Workspace here is a person's. The Org's Workspace
/// ([`crate::Org::workspace_id`]) belongs to no person, so this store
/// never answers it: no per-Workspace sweep, seed or Tenant Data Key
/// reaches it.
#[async_trait]
pub trait WorkspaceStore: Send + Sync {
    async fn create(&self, workspace: &Workspace) -> Result<(), StoreError>;
    async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, StoreError>;
    /// The Workspace one person owns. This is how a request
    /// resolves its tenant: the signed-in person names the Workspace.
    async fn for_user(&self, user_id: &UserId) -> Result<Option<Workspace>, StoreError>;
    /// Every person's Workspace.
    async fn list(&self) -> Result<Vec<Workspace>, StoreError>;
    /// Record onboarding completion once; later calls keep the first
    /// timestamp.
    async fn set_onboarded(&self, id: &WorkspaceId, at: UnixMillis) -> Result<(), StoreError>;
    async fn set_timezone(&self, id: &WorkspaceId, timezone: &str) -> Result<(), StoreError>;
    /// Name the Chief of Staff, or clear it (ADR-0022).
    async fn set_chief_of_staff(
        &self,
        id: &WorkspaceId,
        agent_id: Option<&AgentId>,
    ) -> Result<(), StoreError>;
    /// Name the Schedule that writes the Report, or clear it
    /// (ADR-0022).
    async fn set_report_schedule(
        &self,
        id: &WorkspaceId,
        schedule_id: Option<&ScheduleId>,
    ) -> Result<(), StoreError>;
    /// Name one Host of this Workspace as the Person's Home Exit, or
    /// clear it (ADR-0029). A Host of another Workspace is never the
    /// Home Exit of this one: the write then changes nothing and answers
    /// `false`, as it does for a Host or a Workspace that does not exist.
    async fn set_home_exit(
        &self,
        id: &WorkspaceId,
        host_id: Option<&HostId>,
    ) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait OnboardingStore: Send + Sync {
    /// The key check of each provider the model step checked.
    async fn model_verifications(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<OnboardingModelVerification>, StoreError>;
    /// Record one provider's key check, in place of its earlier one.
    async fn set_model_verification(
        &self,
        workspace_id: &WorkspaceId,
        verification: &OnboardingModelVerification,
    ) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueBatch {
    pub blocked_schedules: Vec<Schedule>,
    pub withdrawn_wakeups: Vec<Wakeup>,
    pub occurrences: Vec<ScheduleOccurrence>,
    pub wakeups: Vec<Wakeup>,
}

#[async_trait]
pub trait ScheduleStore: Send + Sync {
    async fn create(&self, schedule: &Schedule) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Option<Schedule>, StoreError>;
    async fn edit(
        &self,
        previous: &Schedule,
        replacement: &Schedule,
    ) -> Result<Option<Schedule>, StoreError>;
    async fn list_revisions(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Vec<ScheduleRevision>, StoreError>;
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        before: Option<&ScheduleId>,
        limit: u32,
    ) -> Result<Vec<Schedule>, StoreError>;
    /// Open schedules whose metadata names one Subject Page.
    async fn list_open_for_subject_page(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<Vec<crate::subject_page::OpenSchedule>, StoreError>;
    async fn archive(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError>;
    async fn pause(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError>;
    async fn resume(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        next_due_at: UnixMillis,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError>;
    async fn skip_next(
        &self,
        schedule: &Schedule,
        expected_due_at: UnixMillis,
        next_due_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<ScheduleOccurrence, StoreError>;
    async fn list_occurrences(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&ScheduleOccurrenceId>,
        limit: u32,
    ) -> Result<Vec<ScheduleOccurrence>, StoreError>;
    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BriefCursor {
    pub initialized: bool,
    pub memory_revision: Option<String>,
    pub shown_paths: Vec<String>,
}

#[async_trait]
pub trait BriefStore: Send + Sync {
    async fn load(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &crate::ChannelId,
        root_message_id: Option<&crate::MessageId>,
    ) -> Result<BriefCursor, StoreError>;

    async fn save(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &crate::ChannelId,
        root_message_id: Option<&crate::MessageId>,
        cursor: &BriefCursor,
    ) -> Result<(), StoreError>;
}

/// One provider occurrence, already normalized by its collector. The
/// collector stops here: nothing below this type knows the transport.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedEvent {
    /// The provider's own id for the occurrence.
    pub provider_event_id: String,
    /// Untrusted metadata, valid against the declared schema.
    pub metadata: serde_json::Value,
    pub occurred_at: UnixMillis,
    /// Where a Wake-up this occurrence makes must land, when the
    /// occurrence decides it for itself. `None` lands the Wake-up in
    /// the rule's own conversation, which is what a Schedule and most
    /// events do. A reply to mail the Agent sent lands in the Thread
    /// that sent it (ADR-0019).
    pub landing: Option<WakeupLanding>,
}

/// One collection pass submitted to the Trigger module. Cursor commit,
/// deduplication, matching, and Wake-up creation happen together, so a
/// batch that fails changes nothing (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestBatch {
    pub workspace_id: WorkspaceId,
    pub source: crate::EventSource,
    /// The Agent this pass belongs to, where the source is the Agent's
    /// own identity and not an account the Workspace shares. An Agent
    /// Mailbox is one Agent's mail, so only that Agent's rules see the
    /// pass, even where two mailboxes sit on one Connection
    /// (ADR-0019). `None` is a shared account: every rule on the
    /// Connection sees the pass.
    pub agent_id: Option<AgentId>,
    pub event_kind: String,
    /// The opaque next cursor. The Trigger module stores it and never
    /// reads inside it. A Coding Session source has none.
    pub cursor: Option<String>,
    pub events: Vec<NormalizedEvent>,
    pub received_at: UnixMillis,
    /// True for the first collection on this Connection: it sets the
    /// cursor and emits no Incoming Event.
    pub baseline: bool,
}

/// The reflection-only Run that one synced acquisition batch requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrivalRun {
    pub agent_id: AgentId,
    pub subject_paths: Vec<String>,
    /// True for a Backfill Reflection: the pages hold historical
    /// arrivals, so the briefing tells the Run (ADR-0011).
    pub historical: bool,
}

/// What one `ingest` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    pub batch: SourceBatch,
    /// Incoming Events stored by this batch; a repeat of a provider id
    /// already stored is absent.
    pub events: Vec<IncomingEvent>,
    pub created: Vec<Wakeup>,
    /// Pending Wake-ups this batch joined more sources onto.
    pub combined: Vec<Wakeup>,
}

/// Decides whether one Incoming Event satisfies one typed filter. The
/// declaration names its matcher; only a trusted, in-process matcher is
/// ever registered, and it reads normalized metadata only (ADR-0006).
pub trait EventMatcher: Send + Sync {
    fn matches(&self, filter: &serde_json::Value, metadata: &serde_json::Value) -> bool;
}

/// What a proactive Run needs to brief an Agent on an event Wake-up:
/// the source it came from and the occurrences that joined it. The
/// metadata is untrusted provider text, and it carries no body,
/// snippet, or attachment (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventWakeupContext {
    pub subscription_id: EventSubscriptionId,
    pub source: crate::EventSource,
    /// The alias of the Connection, for a Connection source alone.
    pub connection_alias: Option<String>,
    pub event_kind: String,
    pub events: Vec<IncomingEvent>,
}

/// The installed Incoming Event declarations. The Capability Broker
/// owns the manifests; the Trigger module only reads the declaration
/// it needs, so a kind no manifest declares — a Pagis audit event, for
/// one — has no subscription source at all (ADR-0006).
///
/// Several Connection providers can supply one kind, and each says
/// for itself what an Agent must hold to act on it, so the lookup
/// takes the provider beside the name (ADR-0019).
pub trait EventCatalog: Send + Sync {
    /// What one event kind of one provider declares, for this Workspace.
    /// The installed Capability Manifests are a tenant's own, so
    /// the Workspace is an argument and not a property of the catalogue.
    fn declaration(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
        provider: &str,
    ) -> Option<EventDeclaration>;
}

/// One Connection a collector must poll: it has at least one live
/// Event Subscription of this kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectorTarget {
    pub workspace_id: WorkspaceId,
    pub connection_id: ConnectionId,
    pub event_kind: String,
}

#[async_trait]
pub trait TriggerStore: Send + Sync {
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError>;
    /// The Agents with pending Wake-ups, each with the Workspace that
    /// owns them. A claim needs both, and the sweep crosses every
    /// Workspace.
    async fn pending_agents(&self) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError>;
    async fn process_due(&self, now: UnixMillis) -> Result<DueBatch, StoreError>;
    /// Wake one active Schedule at once, outside its cadence: the user
    /// asked for its work now (ADR-0022). The Schedule keeps its own
    /// next time. `None` when the Schedule is gone or not active; the
    /// pending Wake-up it already has when one waits.
    async fn wake_now(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        now: UnixMillis,
    ) -> Result<Option<Wakeup>, StoreError>;
    /// Start pending Wake-ups as Runs: arrival Wake-ups fill the arrival
    /// pool, every other Wake-up fills the conversation pool.
    async fn claim_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        slots: RunSlots,
        now: UnixMillis,
    ) -> Result<Vec<WakeupClaim>, StoreError>;
    async fn get_wakeup(
        &self,
        workspace_id: &WorkspaceId,
        id: &WakeupId,
    ) -> Result<Option<Wakeup>, StoreError>;
    /// Commit one collection pass. `eligible` is the live subscriptions
    /// the caller already checked the grant of, in delivery order;
    /// `matcher` runs inside the same transaction so a batch commits
    /// its cursor, its Incoming Events, and its Wake-ups at once.
    ///
    /// `source_resource` is the synced resource whose Source Items the
    /// events are, from the declaration of the kind. An event whose
    /// Source Item a Forget blocks writes no row and wakes nothing: the
    /// store drops it. `keys` gives the suppression key only when the
    /// Connection has a suppression row. `None` checks no event
    /// (ADR-0008).
    async fn ingest(
        &self,
        batch: IngestBatch,
        eligible: &[EventSubscription],
        matcher: &dyn EventMatcher,
        arrival: Option<&ArrivalRun>,
        source_resource: Option<&str>,
        keys: &dyn crate::ForgetKeys,
    ) -> Result<IngestOutcome, StoreError>;
    /// The opaque cursor a collector left, if any.
    async fn cursor(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<Option<String>, StoreError>;
    /// Record one failed collection pass without touching the cursor.
    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        code: &str,
        at: UnixMillis,
    ) -> Result<SourceBatch, StoreError>;
    /// The Incoming Events one Wake-up combines, oldest first, with
    /// the account they arrived on.
    async fn event_context(
        &self,
        workspace_id: &WorkspaceId,
        wakeup_id: &WakeupId,
    ) -> Result<Option<EventWakeupContext>, StoreError>;
}

#[async_trait]
pub trait EventSubscriptionStore: Send + Sync {
    async fn create(&self, subscription: &EventSubscription) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
    ) -> Result<Option<EventSubscription>, StoreError>;
    /// One page of the Connection subscriptions in a workspace, newest
    /// first. A rule of a Coding Session source is daemon housekeeping,
    /// so the list leaves it out (ADR-0033).
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        before: Option<&EventSubscriptionId>,
        limit: u32,
    ) -> Result<Vec<EventSubscription>, StoreError>;
    /// Write the mutable fields: name, instruction, destination,
    /// filter, state, revision, approval, watermark, timestamps.
    async fn update(&self, subscription: &EventSubscription) -> Result<bool, StoreError>;
    /// Write the mutable fields as `update` does, and move every pending
    /// Wake-up of the rule to `withdrawn`, in one transaction. No reader
    /// sees the written rule with pending work of the rule before it.
    /// Answer the withdrawn Wake-ups.
    async fn update_and_withdraw(
        &self,
        subscription: &EventSubscription,
    ) -> Result<Vec<Wakeup>, StoreError>;
    /// Every Connection with at least one live subscription: the
    /// collector work list. A Connection with none stops collecting.
    async fn collector_targets(&self) -> Result<Vec<CollectorTarget>, StoreError>;
    /// The live subscriptions of one source and event kind, oldest
    /// first.
    async fn live_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &crate::EventSource,
        event_kind: &str,
    ) -> Result<Vec<EventSubscription>, StoreError>;
    /// Every subscription of one source in the given states.
    async fn list_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &crate::EventSource,
        states: &[&str],
    ) -> Result<Vec<EventSubscription>, StoreError>;
    /// One page of the Incoming Events this subscription matched.
    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&IncomingEventId>,
        limit: u32,
    ) -> Result<Vec<IncomingEvent>, StoreError>;
    /// One page of this subscription's Wake-ups, newest first.
    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError>;
    /// The newest collection pass on one Connection and event kind,
    /// and the newest successful one: the collector health record.
    async fn collector_health(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<(Option<SourceBatch>, Option<SourceBatch>), StoreError>;
}

#[async_trait]
pub trait AgentStore: Send + Sync {
    async fn create(&self, agent: &Agent) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError>;
    async fn list_by_workspace(&self, workspace_id: &WorkspaceId)
    -> Result<Vec<Agent>, StoreError>;
    /// Write the mutable agent fields: name, job, personality, voice,
    /// standing_brief, status, updated_at.
    async fn update(&self, agent: &Agent) -> Result<(), StoreError>;
    /// Change appearance without overwriting profile edits.
    async fn update_avatar(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
        avatar: &crate::AvatarAppearance,
        updated_at: i64,
    ) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait ModelAliasStore: Send + Sync {
    async fn create(&self, model_alias: &ModelAlias) -> Result<(), StoreError>;
    async fn get_by_alias(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError>;
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError>;
    async fn update_candidates(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
        candidates: &[String],
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError>;
    async fn delete(&self, workspace_id: &WorkspaceId, alias: &str) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait ConnectionStore: Send + Sync {
    /// Insert one Connection. A workspace alias is taken once, so a
    /// duplicate is a [`StoreError::Conflict`], never a silent second
    /// row: the alias is how a tool call picks the account.
    async fn create(&self, connection: &Connection) -> Result<(), StoreError>;
    /// Move one Connection to a new status. Returns false when the
    /// connection is missing.
    async fn set_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError>;
    /// Record one successful authorization and its complete named
    /// capability set.
    async fn set_authorization(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError>;
    /// Replace the trusted binding the provider needs. The user
    /// supplies it, as at creation; it holds no secret. Returns false
    /// when the connection is missing.
    async fn set_config(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError>;
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
    ) -> Result<Option<Connection>, StoreError>;
    /// Delete the connection and revoke every dependent live grant in
    /// one transaction. Returns false when the connection is missing.
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Keep the sealed OAuth refresh token of one Google Connection,
    /// or clear it with `None`.
    ///
    /// The bytes are sealed with the Tenant Data Key of this Workspace,
    /// so a read of another tenant's row returns ciphertext it
    /// holds no key for. The token is not on [`Connection`], because
    /// every list and every card would then carry it; one caller writes
    /// it and one caller reads it.
    async fn set_refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
        token: Option<&crate::SealedSecret>,
    ) -> Result<bool, StoreError>;
    /// The sealed refresh token of one Connection, when it holds one.
    async fn refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::ConnectionId,
    ) -> Result<Option<crate::SealedSecret>, StoreError>;
}

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn create(&self, credential: &Credential) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::CredentialId,
    ) -> Result<Option<Credential>, StoreError>;
    /// Every credential in one workspace, newest first. `domain`
    /// filters to one registrable domain.
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        domain: Option<&str>,
    ) -> Result<Vec<Credential>, StoreError>;
    /// Delete the credential and revoke every dependent live grant in
    /// one transaction. Returns false when the credential is missing.
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::CredentialId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Hand one Agent's minted credentials to the user. Archiving
    /// an Agent must not destroy account access: there is no backup and
    /// no export, so the records stay and only the owner changes.
    async fn release_owner(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u64, StoreError>;
}

/// The Trust List (ADR-0021, ADR-0019): which numbers, addresses and
/// domains propose which tier.
///
/// No tool writes here. The daemon writes what the user edits in the
/// Trusted contacts tab, and the call and mail paths only read.
#[async_trait]
pub trait TrustListStore: Send + Sync {
    /// Add one row, or move an existing row to a new tier and label.
    /// One `(agent_id, subject, value)` triple has one row.
    async fn upsert(&self, row: &crate::TrustEntry) -> Result<(), StoreError>;
    /// Every row of one Workspace, the Workspace-wide rows first.
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<crate::TrustEntry>, StoreError>;
    /// `false` when no such row is in this Workspace.
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::TrustEntryId,
    ) -> Result<bool, StoreError>;
    /// The tier the list proposes for one subject on one Agent's line:
    /// the highest tier of the rows that match, over the Workspace-wide
    /// list and that Agent's list. `None` is a subject on no list.
    async fn candidate(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        subject: crate::TrustSubject,
        value: &str,
    ) -> Result<Option<crate::TrustTier>, StoreError>;
}

#[async_trait]
pub trait ChannelStore: Send + Sync {
    async fn create(&self, channel: &Channel) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ChannelId,
    ) -> Result<Option<Channel>, StoreError>;
    /// Channels in one workspace, most recently updated first.
    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Channel>, StoreError>;
    /// The DM channel between the user and one agent: kind `dm`,
    /// a user participant, and this agent.
    async fn find_user_dm(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<Channel>, StoreError>;
    /// The DM channel between two agents: kind `dm`, no user
    /// participant, and both agents.
    async fn find_agent_dm(
        &self,
        workspace_id: &WorkspaceId,
        a: &AgentId,
        b: &AgentId,
    ) -> Result<Option<Channel>, StoreError>;
}

/// The result of a message insert with send dedup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Created(Message),
    /// A message with the same (channel_id, pending_id) already exists;
    /// this is that original message.
    Deduplicated(Message),
}

/// One top-level timeline entry: the message plus its thread rollup,
/// computed in the query. A message without replies has
/// `reply_count` 0, no `last_reply_at` and no `reply_authors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineEntry {
    pub message: Message,
    pub reply_count: u32,
    pub last_reply_at: Option<UnixMillis>,
    /// The different authors of the replies, the newest reply first,
    /// at most [`REPLY_AUTHORS_MAX`].
    pub reply_authors: Vec<ReplyAuthor>,
}

/// The number of reply authors a timeline entry names.
pub const REPLY_AUTHORS_MAX: usize = 3;

/// One author of the replies in a thread: the user, or one Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyAuthor {
    pub kind: AuthorKind,
    pub agent_id: Option<AgentId>,
}

#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Insert one message. When the message carries a `pending_id` that
    /// already exists in its channel, nothing is written and the stored
    /// original is returned.
    async fn insert(&self, message: &Message) -> Result<SendOutcome, StoreError>;
    /// Insert one generated message together with its exposure stamp,
    /// in one write. A reader that finds a stored message without its
    /// stamp cannot verify the source access behind it and withholds
    /// the prose, so the two never reach the database apart.
    async fn insert_stamped(
        &self,
        message: &Message,
        exposures: &[crate::MemoryExposure],
    ) -> Result<SendOutcome, StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Message>, StoreError>;
    /// True when Forget has invalidated this original message.
    async fn is_forgotten(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<bool, StoreError>;
    /// Daemon-derived source permissions that influenced retained prose.
    async fn set_exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        exposures: &[crate::MemoryExposure],
    ) -> Result<(), StoreError>;
    /// `None` is a message with no stamp: a generated message the daemon
    /// has not stamped yet. A reader cannot verify its scope and
    /// withholds its prose.
    async fn exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Vec<crate::MemoryExposure>>, StoreError>;
    /// One timeline page of top-level messages (no thread replies) with
    /// their thread rollups, newest first. `before` is an exclusive
    /// ULID cursor.
    async fn list_top_level(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<TimelineEntry>, StoreError>;
    /// The newest top-level message of one channel, which the
    /// Conversations list shows. A thread reply and a derived progress
    /// row are not conversation, so they are never the last message.
    async fn last_message(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Option<Message>, StoreError>;
    /// One page of an agent's complete messages outside one channel,
    /// newest first — the source rows of the DM pointer entries.
    /// `before` is an exclusive ULID cursor.
    async fn list_agent_elsewhere(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        exclude_channel: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError>;
    /// One thread: the root message plus its replies, oldest first.
    async fn list_thread(
        &self,
        workspace_id: &WorkspaceId,
        root_id: &MessageId,
    ) -> Result<Vec<Message>, StoreError>;
    /// The last complete message one Run's Agent wrote, or `None` when
    /// the Run said nothing. A proactive Run speaks at the top level of
    /// its channel, so its words are found by the Run, not by a thread.
    async fn latest_agent_message_of_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<Message>, StoreError>;
    /// Settle a streaming message: final blocks, text, and status.
    async fn finalize(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        status: MessageStatus,
        blocks: &[Block],
        text_content: &str,
        completed_at: UnixMillis,
    ) -> Result<(), StoreError>;
    /// Mark every `streaming` message `failed`. The boot recovery sweep
    /// uses this: a daemon restart orphans in-flight streams.
    async fn fail_streaming(&self, completed_at: UnixMillis) -> Result<u64, StoreError>;
}

#[async_trait]
pub trait ParticipantStore: Send + Sync {
    async fn create(&self, participant: &ChannelParticipant) -> Result<(), StoreError>;
    /// The agent participants of one channel.
    async fn agents_in_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<AgentId>, StoreError>;
    /// Every participant of one channel, oldest first.
    async fn list_for_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<ChannelParticipant>, StoreError>;
}

#[async_trait]
pub trait RunStore: Send + Sync {
    async fn create(&self, run: &Run) -> Result<(), StoreError>;
    async fn get(&self, workspace_id: &WorkspaceId, id: &RunId) -> Result<Option<Run>, StoreError>;
    /// Write the mutable run fields: root_message_id, state, error,
    /// started_at, and ended_at.
    async fn update(&self, run: &Run) -> Result<(), StoreError>;
    /// Runs in a non-terminal state, oldest first.
    async fn list_unfinished(&self) -> Result<Vec<Run>, StoreError>;
    /// One page of runs in a workspace, newest first. Every optional
    /// filter is combined with the others. `before` is exclusive. A run
    /// matches `states` when its state is in the set; an empty set
    /// keeps every state.
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
        states: &[crate::domain::RunState],
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<Run>, StoreError>;
    /// Mark the Run dismissed from the Needs-You Queue at `at`. A Run
    /// that is already dismissed keeps its first time. `false` when the
    /// Workspace holds no such Run.
    async fn dismiss(
        &self,
        workspace_id: &WorkspaceId,
        id: &RunId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Record the synced source content that the Run read through one
    /// Connection (ADR-0008). A Forget of a Source Item that the Run
    /// read forgets each message of the Run and deletes its retained
    /// tool results. A read that is already recorded changes nothing.
    async fn record_source_reads(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        connection_id: &ConnectionId,
        reads: &[crate::knowledge::SourceRead],
    ) -> Result<(), StoreError>;
}

/// The result of a decision write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideOutcome {
    /// The request was pending; it now carries the decision.
    Decided(Request),
    /// The request was not pending; this is its current record.
    NotPending(Request),
}

#[async_trait]
pub trait RequestStore: Send + Sync {
    async fn create(&self, request: &Request) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
    ) -> Result<Option<Request>, StoreError>;
    /// Requests of one workspace in one state, newest first, narrowed
    /// to one `kind` when given.
    async fn list_by_state(
        &self,
        workspace_id: &WorkspaceId,
        state: RequestState,
        kind: Option<&str>,
    ) -> Result<Vec<Request>, StoreError>;
    /// Write a decision onto a pending request. The write is
    /// conditional on `state = pending`, so two racing decisions
    /// resolve to one `Decided` and one `NotPending`. `values` carries
    /// what a form or a choice submitted, already validated against
    /// the schema on the row.
    async fn decide(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
        state: RequestState,
        values: Option<serde_json::Value>,
        decided_at: UnixMillis,
    ) -> Result<DecideOutcome, StoreError>;
    /// Mark every pending request of one run `expired`. The run
    /// failure paths (cancel, restart recovery) call this.
    async fn expire_pending_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        decided_at: UnixMillis,
    ) -> Result<u64, StoreError>;
}

#[async_trait]
pub trait GrantStore: Send + Sync {
    async fn create(&self, grant: &Grant) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
    ) -> Result<Option<Grant>, StoreError>;
    /// Live (unrevoked) grants in one workspace, oldest first.
    async fn list_live(&self, workspace_id: &WorkspaceId) -> Result<Vec<Grant>, StoreError>;
    /// The agent's live grant for one resource kind, when one exists.
    /// The partial unique index allows at most one.
    async fn live_grant(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
    ) -> Result<Option<Grant>, StoreError>;
    /// One agent's live grants, oldest first.
    async fn list_live_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Vec<Grant>, StoreError>;
    /// One live grant for an exact agent and workspace resource.
    async fn live_for_resource(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<Option<Grant>, StoreError>;
    /// Replace a live grant's scope. `false` when the grant is missing
    /// or revoked.
    async fn set_scope(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        scope: &serde_json::Value,
    ) -> Result<bool, StoreError>;
    /// Revoke a grant once. `false` when it is missing or already
    /// revoked.
    async fn revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait CapabilitySnapshotStore: Send + Sync {
    /// Store one immutable snapshot. Repeating the same content-addressed
    /// record is a no-op.
    async fn put(&self, snapshot: &CapabilitySnapshotRecord) -> Result<(), StoreError>;
    /// Bind one run to its snapshot. A run can have only one binding.
    async fn bind_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        snapshot_id: &str,
    ) -> Result<(), StoreError>;
    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<CapabilitySnapshotRecord>, StoreError>;
}

/// The result of an artifact insert with content dedup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactOutcome {
    Created(Artifact),
    /// An artifact with the same (workspace_id, sha256) already exists;
    /// this is that original artifact.
    Deduplicated(Artifact),
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Insert one artifact. When the workspace already holds the same
    /// sha256, nothing is written and the stored original is returned.
    async fn insert(&self, artifact: &Artifact) -> Result<ArtifactOutcome, StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ArtifactId,
    ) -> Result<Option<Artifact>, StoreError>;
    /// Artifacts of one class made before `cutoff`, oldest first — the
    /// retention sweep's work list.
    async fn expired_before(
        &self,
        workspace_id: &WorkspaceId,
        kind: ArtifactKind,
        cutoff: UnixMillis,
        limit: u32,
    ) -> Result<Vec<Artifact>, StoreError>;
    /// Delete one artifact row. `false` when it is already gone.
    async fn delete(&self, workspace_id: &WorkspaceId, id: &ArtifactId)
    -> Result<bool, StoreError>;
}

/// The retention window the user sets for each Artifact class.
/// A class with no stored row is kept for ever.
#[async_trait]
pub trait RetentionPolicyStore: Send + Sync {
    /// The policy for every class, in `ArtifactKind::ALL` order. A
    /// class the user has not set reads back as "keep for ever".
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<RetentionPolicy>, StoreError>;
    /// Set one class's window. `retain_days` of `None` returns the
    /// class to "keep for ever".
    async fn set(
        &self,
        workspace_id: &WorkspaceId,
        kind: ArtifactKind,
        retain_days: Option<i64>,
        now: UnixMillis,
    ) -> Result<(), StoreError>;
}

/// The append-only events table. The event bus builds on this.
#[async_trait]
pub trait EventLog: Send + Sync {
    /// Append one event; the log assigns id, seq, and created_at.
    async fn append(&self, event: NewEvent) -> Result<Event, StoreError>;
    /// Events with seq strictly greater than `after_seq`, oldest first.
    /// `workspace_id` filters to one Workspace; `None` reads every
    /// Workspace, which only a daemon-lifetime consumer asks for.
    async fn list_after(
        &self,
        workspace_id: Option<&WorkspaceId>,
        after_seq: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError>;
    /// The seq of the newest event, or 0 for an empty log.
    async fn latest_seq(&self) -> Result<i64, StoreError>;
    /// One page of events of the given types, newest first. `before`
    /// is an exclusive ULID cursor. The learning feed is this
    /// query over memory commits, reverts, and wake-only Schedule creation.
    async fn list_by_types(
        &self,
        workspace_id: &WorkspaceId,
        event_types: &[&str],
        before: Option<&EventId>,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError>;
    /// Every event for one run, oldest first.
    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<Event>, StoreError>;
}

/// The Agent Phone Number records and the purchase intents that make
/// them (ADR-0018). The schema owns the one-to-one rule: a partial
/// unique index on `agent_id` refuses a second number for one Agent and
/// a second Agent for one number, and the refusal arrives here as
/// [`StoreError::Conflict`].
#[async_trait]
pub trait PhoneNumberStore: Send + Sync {
    /// Record the intent before the carrier is asked. The id is the
    /// idempotency key, so the record exists before the money moves.
    async fn create_intent(&self, intent: &PurchaseIntent) -> Result<(), StoreError>;
    /// Every intent that still waits for an answer, over all
    /// Workspaces: the reconciliation reads this at start-up.
    async fn list_pending_intents(&self) -> Result<Vec<PurchaseIntent>, StoreError>;
    /// The carrier sold the number: write the record and settle the
    /// intent together, so a failure leaves neither.
    async fn confirm_purchase(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        number: &PhoneNumber,
        at: UnixMillis,
    ) -> Result<(), StoreError>;
    /// Record a number the account already held. There is no
    /// intent, because nothing was bought. `Conflict` when the
    /// Workspace already holds a live record for the number, or the
    /// Agent already holds a number.
    async fn create(&self, number: &PhoneNumber) -> Result<(), StoreError>;
    /// The carrier does not hold the number, so nothing was bought.
    async fn abandon_intent(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<Option<PhoneNumber>, StoreError>;
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, StoreError>;
    /// The number one Agent holds, when it holds one.
    async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError>;
    /// Every record of one E.164 number that is not released, over all
    /// Workspaces. The carrier's line reads it to find the Workspace
    /// and the Agent of an inbound Call by the dialed number (ADR-0020).
    async fn live_for_e164(&self, e164: &str) -> Result<Vec<PhoneNumber>, StoreError>;
    /// Give the line to an Agent. `false` when no such number is on
    /// hand, and `Conflict` when the Agent or the number is taken.
    async fn assign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        agent_id: &AgentId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Take the line back. The Workspace keeps the number and keeps
    /// paying for it.
    async fn unassign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<bool, StoreError>;
    /// Take back whatever line an Agent holds, and say which it was.
    /// Archiving an Agent uses this.
    async fn unassign_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError>;
    /// The number went back to the carrier. The record stays, for the
    /// Calls that point at it.
    async fn release(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<bool, StoreError>;
    /// Whether a number that is not released still points at this
    /// Connection. Deleting the carrier Connection refuses while one
    /// does.
    async fn any_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<bool, StoreError>;

    /// Set the most texts a day the number may send (ADR-0020).
    /// `false` when no such number exists.
    async fn set_outgoing_cap(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cap: u32,
    ) -> Result<bool, StoreError>;

    /// Add counterpart number allow rules and answer the rules the
    /// record holds after the write (ADR-0020). A rule already there
    /// is not written twice, and the order the user made them in is
    /// kept.
    async fn add_allow_rules(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError>;

    /// Count one text against the day's tally and answer the new
    /// count. The tally starts again when the day changes, so the
    /// Outgoing Cap is a day's allowance.
    async fn note_text_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        at: UnixMillis,
    ) -> Result<u32, StoreError>;

    /// Write what the carrier said about sending (ADR-0020), the time
    /// of the read, and the error of the read that failed. `error`
    /// replaces the old one, so a read that succeeds clears it.
    async fn set_messaging_readiness(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        readiness: &MessagingReadiness,
        at: UnixMillis,
        error: Option<&str>,
    ) -> Result<bool, StoreError>;

    /// Remember where the inbound collector reached (ADR-0020).
    async fn set_text_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cursor: &str,
    ) -> Result<bool, StoreError>;

    /// Remember the carrier's messaging object for this number: the
    /// Telnyx messaging profile, or the Twilio Messaging Service the
    /// number sits in the sender pool of (ADR-0020).
    async fn set_messaging_object_id(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        object_id: &str,
    ) -> Result<bool, StoreError>;

    /// Write where the Telnyx relay function stands (ADR-0020).
    async fn set_relay_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        state: &TelnyxRelayState,
    ) -> Result<bool, StoreError>;

    /// Record that the user let the daemon install the Telnyx CLI and
    /// ship the relay for this number (ADR-0020). The desk sets it
    /// before the download starts.
    async fn set_relay_consent(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        consent: bool,
    ) -> Result<bool, StoreError>;
}

/// The Text Records (ADR-0020): every text an Agent sent or received.
///
/// There is no conversation table and no sent-texts table. A Text
/// Conversation is the pair of one Agent Phone Number and one
/// counterpart number, and this store derives it by grouping records.
/// The outbound record's Thread field is the reply lookup that the
/// mail collector needs a `SentMailStore` for.
///
/// Every read takes the Agent as well as the number, because records
/// stay with the Agent that made them: a new holder of a number sees
/// none of the previous holder's conversations.
#[async_trait]
pub trait TextRecordStore: Send + Sync {
    /// Write one record. The collector writes an inbound record after
    /// it stored the media; the send tool writes an outbound one.
    async fn insert(&self, record: &TextRecord) -> Result<(), StoreError>;

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
    ) -> Result<Option<TextRecord>, StoreError>;

    /// Write what the carrier now says about one outbound text.
    /// `false` when no such record exists.
    async fn set_delivery_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
        status: &TextDeliveryStatus,
    ) -> Result<bool, StoreError>;

    /// One Text Conversation, newest first. `before` is an exclusive
    /// cursor: the page holds the records older than the record it
    /// names.
    async fn list_conversation(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        max: u32,
        before: Option<&TextRecordId>,
    ) -> Result<Vec<TextRecord>, StoreError>;

    /// The recent counterparts of one number, the newest conversation
    /// first, with the time and the direction of its last text.
    async fn list_conversations(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        max: u32,
    ) -> Result<Vec<TextConversation>, StoreError>;

    /// Where an inbound text of this pair lands (ADR-0020): the
    /// Thread of the last outbound record of the pair that carries
    /// one and is younger than [`TEXT_THREAD_WINDOW_MS`]. `None`
    /// sends the text to the Agent's own Thread with the user.
    async fn landing_thread(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        now: UnixMillis,
    ) -> Result<Option<MessageId>, StoreError>;
}

/// The Agent Mailbox records (ADR-0019). One record per mailbox holds
/// the address, the Mailbox Provider Connection and the state, and a
/// deleted mailbox stays as a tombstone so its address is never reused.
///
/// Removing a Mailbox Provider Connection reads the count here: a
/// Connection that a mailbox still points at is not removed.
///
/// The schema owns the two rules the desk must never break. A unique
/// index on the address, tombstones included, is the Address Ledger; a
/// partial unique index on the Agent gives one Agent one live mailbox.
/// Both refusals arrive here as [`StoreError::Conflict`].
#[async_trait]
pub trait AgentMailboxStore: Send + Sync {
    /// Reserve the address in the Address Ledger. The desk writes the
    /// record before it asks the host, so two forms cannot reserve one
    /// address. An address the ledger already holds, or an Agent that
    /// already holds a live mailbox, is a [`StoreError::Conflict`].
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StoreError>;
    /// Take the reservation back. The host refused the create, so the
    /// row never became a mailbox and its address stays free. Only a
    /// `provisioning` record is dropped this way; every other end is a
    /// tombstone.
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<bool, StoreError>;

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<Option<AgentMailbox>, StoreError>;
    /// The mailbox one Agent holds now. A tombstone is not one.
    async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError>;
    /// The mailboxes of one Workspace that are not deleted, oldest
    /// first.
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<AgentMailbox>, StoreError>;
    /// Whether the Address Ledger already holds this address, deleted
    /// ones and every Connection included. The creation form asks this
    /// as the user types.
    async fn address_taken(&self, address: &str) -> Result<bool, StoreError>;

    /// Move the mailbox to a state and say why. `reason` replaces the
    /// old one, so a state that needs no reason clears it. `false`
    /// when no such record exists or it is already a tombstone.
    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        state: AgentMailboxState,
        reason: Option<&str>,
    ) -> Result<bool, StoreError>;
    /// Remember where the collector reached. A reset leaves it alone.
    async fn set_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        cursor: &MailboxCursor,
    ) -> Result<bool, StoreError>;
    /// The Agent is archived, so the mailbox it holds sleeps with it.
    /// It says which mailbox, or `None` when the Agent holds none.
    async fn make_dormant_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError>;
    /// Write the tombstone. The address, the Agent and the time stay,
    /// so the address is never reused.
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;

    /// Count one send against the day's tally and answer the new
    /// count. The tally starts again when the day changes, so the
    /// Outgoing Cap is a day's allowance.
    async fn note_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<u32, StoreError>;

    /// Add Mail Recipient Domain allow rules to the record and answer
    /// the rules it holds after the write (ADR-0019). A rule already
    /// there is not written twice, and the order the user made them in
    /// is kept.
    async fn add_allow_rules(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError>;

    /// How many mailboxes that are not deleted point at one Connection.
    async fn count_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError>;
}

/// The messages the Agent Mailboxes sent (ADR-0019).
///
/// One row joins a `Message-ID` to the Thread its Run was working in,
/// so a reply that carries the id in `In-Reply-To` lands where the
/// send came from. The row holds no body.
#[async_trait]
pub trait SentMailStore: Send + Sync {
    /// Remember one sent message. A `Message-ID` written twice keeps
    /// the first row: the id is the message, and a send makes a new one.
    async fn record(&self, sent: &SentMail) -> Result<(), StoreError>;

    /// The send one `Message-ID` names, when this Workspace made it.
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        message_id: &str,
    ) -> Result<Option<SentMail>, StoreError>;
}

/// The Call records (ADR-0020). The bridge writes the record when the
/// call starts and settles it when the call ends, so the UI and the
/// Run read one source of truth.
#[async_trait]
pub trait CallStore: Send + Sync {
    async fn insert(&self, call: &Call) -> Result<(), StoreError>;
    /// Write the record again, with the state it has reached. `false`
    /// when no such record exists.
    async fn update(&self, call: &Call) -> Result<bool, StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CallId,
    ) -> Result<Option<Call>, StoreError>;
    /// The records that never reached `ended`, oldest first. The daemon
    /// reads them at startup: a call does not survive a restart, so
    /// each of these settles with `daemon_restart` (ADR-0020).
    async fn list_unsettled(&self) -> Result<Vec<Call>, StoreError>;
    /// One page of Calls in a Workspace, newest first. Every optional
    /// filter is combined with the others. `before` is exclusive.
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        direction: Option<crate::domain::CallDirection>,
        state: Option<crate::domain::CallState>,
        before: Option<&CallId>,
        limit: u32,
    ) -> Result<Vec<Call>, StoreError>;
    /// Mark the Call dismissed from the Needs-You Queue at `at`. A Call
    /// that is already dismissed keeps its first time. `false` when the
    /// Workspace holds no such Call.
    async fn dismiss(
        &self,
        workspace_id: &WorkspaceId,
        id: &CallId,
        at: UnixMillis,
    ) -> Result<bool, StoreError>;
}

/// The Software List (ADR-0016): which packages a Workspace
/// holds, who wrote them, and what each published Version carries.
///
/// The files live in the package's own bare repository. This store
/// holds the manifest of every Version as JSON, so a run start and a
/// search never open the repository.
#[async_trait]
pub trait SoftwareStore: Send + Sync {
    /// Claim a package name for its author. A name the Workspace
    /// already holds is a [`StoreError::Conflict`].
    async fn create_package(&self, package: &SoftwarePackage) -> Result<(), StoreError>;
    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<SoftwarePackage>, StoreError>;
    /// Every package of the Workspace, by name.
    async fn list_packages(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<SoftwarePackage>, StoreError>;
    /// Write one Version and replace the package row with the one that
    /// names it. The two writes are one transaction: a package never
    /// names a Version that is not stored.
    async fn add_version(
        &self,
        package: &SoftwarePackage,
        version: &SoftwareVersion,
    ) -> Result<(), StoreError>;
    /// Every Version of one package, oldest first.
    async fn list_versions(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<SoftwareVersion>, StoreError>;
}

/// The Contributions of one Workspace (ADR-0016): what a Fork
/// offers its origin package, and how the author answered.
#[async_trait]
pub trait ContributionStore: Send + Sync {
    async fn create(&self, contribution: &Contribution) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
    ) -> Result<Option<Contribution>, StoreError>;
    /// Every Contribution one Fork opened, oldest first.
    async fn list_by_fork(
        &self,
        workspace_id: &WorkspaceId,
        fork_package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError>;
    /// Every Contribution offered to one origin package, oldest first.
    async fn list_by_package(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<Contribution>, StoreError>;
    /// Write the status, the reason and the close time of one record.
    async fn close(
        &self,
        workspace_id: &WorkspaceId,
        id: &ContributionId,
        status: ContributionStatus,
        reason: &str,
        closed_at: UnixMillis,
    ) -> Result<(), StoreError>;
}

/// The Plugins of one Workspace (ADR-0017): the record, its
/// Bindings, and the states an install and an update move it through.
///
/// The files of every installed state live in the Plugin's own bare
/// repository; this store holds only what the desk and the dispatch
/// path read.
#[async_trait]
pub trait PluginStore: Send + Sync {
    /// Write one Plugin and its Bindings in one transaction. A name
    /// the Workspace already holds is a [`StoreError::Conflict`].
    async fn create(&self, plugin: &Plugin, bindings: &[PluginBinding]) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Option<Plugin>, StoreError>;
    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<Plugin>, StoreError>;
    /// Every Plugin of the Workspace, by name.
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Plugin>, StoreError>;
    /// Move one Plugin to the state it has reached. `false` when the
    /// Plugin is missing.
    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        state: PluginState,
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Write the state one update installed: the new commit and the
    /// Capability Manifest version it produced. `false` when the
    /// Plugin is missing.
    async fn set_installed(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        installed_commit: &str,
        manifest_version: &str,
        state: PluginState,
        updated_at: UnixMillis,
    ) -> Result<bool, StoreError>;
    /// Write one Binding, replacing the one the field already has.
    async fn put_binding(
        &self,
        workspace_id: &WorkspaceId,
        binding: &PluginBinding,
    ) -> Result<(), StoreError>;
    /// Every Binding of one Plugin, by field.
    async fn list_bindings(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Vec<PluginBinding>, StoreError>;
    /// Forget the Binding of one field. An update calls it for a field
    /// the new state no longer declares.
    async fn delete_binding(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        field: &str,
    ) -> Result<(), StoreError>;
    /// Delete the Plugin with its Bindings and revoke every live Grant
    /// on it, in one transaction (ADR-0017). The Plugin is the Org's,
    /// so the Grants of every person's Workspace go. The audit rows
    /// stand alone and are not touched. `false` when the Plugin is
    /// missing.
    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError>;
}

/// The frozen tool catalogs of the Plugins (ADR-0017). One row
/// belongs to one Capability Manifest version, so an old Run can still
/// read the catalog its snapshot names.
#[async_trait]
pub trait PluginToolStore: Send + Sync {
    /// Write the catalog of one version. A second write of the same
    /// version replaces it: only an install or an update writes here,
    /// and both own the version they mint.
    async fn put(&self, tools: &PluginTools) -> Result<(), StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
    ) -> Result<Option<PluginTools>, StoreError>;
    /// Record that a server offered a different list than the frozen
    /// one, or that the user has accepted the difference.
    async fn set_tools_changed(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
        version: &str,
        changed: bool,
    ) -> Result<(), StoreError>;
    /// Forget every catalog of one Plugin. An uninstall calls it.
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        plugin_id: &PluginId,
    ) -> Result<(), StoreError>;
}
