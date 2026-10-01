//! I/O-free domain types, ids, event types, errors, and the trait seams
//! (storage repositories, event bus) shared across all daemon modules.

pub mod avatar;
pub use avatar::AvatarAppearance;
pub mod block;
pub mod bus;
pub mod continuation;
pub mod conversation_evidence;
pub mod domain;
pub mod event;
pub mod exposure;
pub mod host;
pub mod id;
pub mod identity;
pub mod keypad;
pub mod knowledge;
pub mod memory;
pub mod memory_page;
pub mod pending_evidence;
pub mod reflection_filter;
pub mod seal;
pub mod secrets;
pub mod skill;
pub mod store;
pub mod stores;
pub mod subject_page;
pub mod time;
pub mod untrusted;
pub mod usage;
pub mod values;

pub use block::{
    Block, ChoiceOption, FormField, FormFieldKind, KnownBlock, MailDirection, TableAlign,
    TableCell, TableColumn, blocks_text,
};
pub use bus::{EventBus, EventScope, EventStream};
pub use continuation::{
    ContinuationCheckpoint, ContinuationKey, ContinuationState, ContinuationStore,
};
pub use conversation_evidence::{
    ConversationEvidenceHit, ConversationEvidenceMessage, ConversationEvidenceStore,
    ConversationScope, ConversationToolEvidence, EvidenceArtifactRef, RetainedToolEvidence,
};
pub use domain::{
    Agent, AgentMailbox, AgentMailboxState, AgentStatus, Artifact, ArtifactKind, AuthorKind,
    BlockReason, Call, CallDirection, CallOutcome, CallState, CapabilitySnapshotRecord, Channel,
    ChannelKind, ChannelParticipant, Classification, Connection, Contribution, ContributionStatus,
    CreatorKind, Credential, CredentialProvenance, DEFAULT_MODEL_ALIAS, EventDeclaration,
    EventSubscription, EventSubscriptionState, FailureKind, Grant, IncomingEvent, MailboxCursor,
    Message, MessageStatus, MessagingReadiness, ModelAlias, OnboardingModelVerification,
    ParticipantKind, PhoneNumber, PhoneNumberStatus, Plugin, PluginBinding, PluginBindingValue,
    PluginSource, PluginState, PluginTool, PluginTools, PurchaseIntent, PurchaseIntentState,
    Request, RequestState, RetentionPolicy, Run, RunOrigin, RunSlots, RunState, Schedule,
    ScheduleKind, ScheduleOccurrence, ScheduleRevision, ScheduleState, SealedSecret, SentMail,
    SoftwarePackage, SoftwareVersion, SourceBatch, SourceBatchOutcome, Speaker,
    TEXT_THREAD_WINDOW_MS, TelnyxRelayState, TextConversation, TextDeliveryStatus, TextDirection,
    TextRecord, TranscriptLine, TriggerKind, TrustEntry, TrustSubject, TrustTier, Wakeup,
    WakeupClaim, WakeupLanding, WakeupRule, WakeupSource, WakeupSourceLink, WakeupState, Workspace,
    day_start, mail_address, mail_domain, normalize_e164, parse_trust_subject, render_transcript,
};
pub use event::{Event, NewEvent};
pub use exposure::message_source_is_live;
pub use host::{Host, HostStore, SHELL_CAPABILITY};
pub use id::{
    AgentId, AgentMailboxId, ArtifactId, CallId, ChannelId, ConnectionId, ContributionId,
    CredentialId, EventId, EventSubscriptionId, GrantId, HostId, IncomingEventId, MessageId,
    ModelAliasId, OrgId, ParticipantId, PendingEvidenceId, PhoneNumberId, PluginId,
    PurchaseIntentId, RequestId, RunId, ScheduleId, ScheduleOccurrenceId, SessionId, SignInLinkId,
    SoftwarePackageId, SourceBatchId, TextRecordId, TrustEntryId, UsageId, UserId, WakeupId,
    WorkspaceId,
};
pub use identity::{
    CLIENT_LINK_LIFETIME_MS, ClientKind, GOOGLE_WEB_CLIENT_SECRET, INVITE_LINK_LIFETIME_MS, Org,
    OrgStore, SESSION_LIFETIME_MS, START_LINK_LIFETIME_MS, Session, SessionStore, SignInLink,
    SignInLinkKind, SignInLinkStore, User, UserRole, UserStore,
};
pub use keypad::{KeypadFailureStore, KeypadFailures};
pub use memory::{
    BriefEntries, IndexedPage, MAX_MEMORY_FILE_BYTES, MEMORY_INDEX_FILE, MemoryAccess,
    MemoryAuthor, MemoryChangeset, MemoryCommitDiff, MemoryContentKind, MemoryError,
    MemoryExposure, MemoryFile, MemoryFileDiff, MemoryHunk, MemoryIndexes, MemoryPageCounts,
    MemoryPageEntry, MemoryPageIndex, MemoryPageList, MemoryScope, MemorySearchHit, MemoryStore,
    PageCursor, PageIndexHead, PageIndexUpdate, PageListQuery, PageSearchHit, ScopedPath,
    VolatilePageIndex, validate_content,
};
pub use pending_evidence::{
    PendingEvidence, PendingEvidenceRecord, PendingEvidenceState, PendingReviewClaim,
    PendingUrgency,
};
pub use seal::{DataKey, SealError, TenantKeys, data_key_name};
pub use secrets::{
    KeySource, MemorySecretStore, PROVIDERS, Provider, ProviderKeyStatus, ProviderKeys,
    SecretError, SecretStore, workspace_secret_name,
};
pub use skill::{
    FIRST_PARTY_SKILLS, MAX_SKILL_DESCRIPTION, NoSkills, SKILL_SEPARATOR, Skill, SkillMount, Skills,
};
pub use store::{
    AgentMailboxStore, AgentStore, ArrivalRun, ArtifactOutcome, ArtifactStore, BriefCursor,
    BriefStore, CallStore, CapabilitySnapshotStore, ChannelStore, CollectorTarget, ConnectionStore,
    ContributionStore, CredentialStore, DecideOutcome, DueBatch, EventCatalog, EventLog,
    EventMatcher, EventSubscriptionStore, EventWakeupContext, GrantStore, IngestBatch,
    IngestOutcome, MessageStore, ModelAliasStore, NormalizedEvent, OnboardingStore,
    ParticipantStore, PendingEvidenceStore, PhoneNumberStore, PluginStore, PluginToolStore,
    REPLY_AUTHORS_MAX, ReplyAuthor, RequestStore, RetentionPolicyStore, RunStore, ScheduleStore,
    SendOutcome, SentMailStore, SoftwareStore, StoreError, TextRecordStore, TimelineEntry,
    TriggerStore, TrustListStore, WorkspaceStore,
};
pub use stores::{Stores, seed_org_and_administrator};
pub use time::{Clock, SystemClock, UnixMillis, local_date, now_ms};
pub use untrusted::{ENVELOPE_RULE, Untrusted, agent_source, tool_source, wrap as wrap_untrusted};
pub use usage::{RunUsage, UsagePeriod, UsageRecord, UsageStore, UsageTotal, WorkspaceUsage};
pub use values::validate_values;

mod forget;
pub use forget::*;
