//! Domain records for the storage spine.

use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::coding_session::{SessionAllowRule, SessionApprovalMode};
use crate::id::{
    AgentId, ArtifactId, ChannelId, CodingSessionId, ConnectionId, EventSubscriptionId, GrantId,
    HostId, IncomingEventId, MessageId, ParticipantId, RequestId, RunId, ScheduleId,
    ScheduleOccurrenceId, SourceBatchId, UserId, WakeupId, WorkspaceId,
};
use crate::time::UnixMillis;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    /// The person this Workspace belongs to. One Workspace per
    /// person, and it stays their private scope.
    pub user_id: UserId,
    pub name: String,
    /// The IANA timezone copied into new wall-clock schedules.
    pub timezone: String,
    pub created_at: UnixMillis,
    /// When the user finished (or dismissed) the onboarding wizard;
    /// `None` keeps the wizard on the next visit.
    pub onboarded_at: Option<UnixMillis>,
    /// The one active Agent the shell pins: the Chief of Staff
    /// (ADR-0022). `None` when the Workspace has no active Agent. The
    /// designation grants nothing; it only decides what the shell shows
    /// first.
    pub chief_of_staff_agent_id: Option<AgentId>,
    /// The Schedule that makes the Chief of Staff write the Report for
    /// Home (ADR-0022). The seed creates it; Home runs it on demand.
    pub report_schedule_id: Option<ScheduleId>,
    /// The Person's Home Exit (ADR-0029): the one Host of this Workspace
    /// through which the Person's Computers on a Server reach the
    /// internet, or `None` when the Person has chosen none.
    pub home_exit_host_id: Option<HostId>,
}

/// The model alias every seeded Agent thinks on. The seed creates it,
/// onboarding names its one model, and the release evaluation prices it.
pub const DEFAULT_MODEL_ALIAS: &str = "default";

/// The server-owned proof that onboarding's key check passed: the
/// provider listed its models for the current credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingModelVerification {
    pub provider: String,
    /// How many models the provider listed for the key.
    pub available: i64,
    /// HMAC-SHA256 over the provider and credential source, keyed by the
    /// resolved credential. This value is never sent to a client.
    pub proof: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    Active,
    Paused,
    Completed,
    Blocked,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    OneShot,
    Cron,
    Interval,
}

impl ScheduleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Cron => "cron",
            Self::Interval => "interval",
        }
    }
}

impl std::str::FromStr for ScheduleKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "one_shot" => Ok(Self::OneShot),
            "cron" => Ok(Self::Cron),
            "interval" => Ok(Self::Interval),
            other => Err(format!("unknown schedule kind: {other}")),
        }
    }
}

impl ScheduleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Archived => "archived",
        }
    }
}

impl std::str::FromStr for ScheduleState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "blocked" => Ok(Self::Blocked),
            "archived" => Ok(Self::Archived),
            other => Err(format!("unknown schedule state: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: ScheduleId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    /// The private Subject Page from daemon-owned Schedule metadata.
    pub subject_page_path: Option<String>,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub kind: ScheduleKind,
    pub cron_expression: Option<String>,
    pub interval_ms: Option<UnixMillis>,
    pub anchor_at: Option<UnixMillis>,
    pub timezone: String,
    pub scheduled_at: UnixMillis,
    pub next_due_at: Option<UnixMillis>,
    /// The state of the most recently started Run, derived on read.
    pub last_result: Option<String>,
    pub state: ScheduleState,
    pub revision: u32,
    pub approved_revision: Option<u32>,
    pub creator: CreatorKind,
    pub creating_run_id: Option<RunId>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub archived_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRevision {
    pub schedule_id: ScheduleId,
    pub revision: u32,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    /// The private Subject Page from daemon-owned Schedule metadata.
    pub subject_page_path: Option<String>,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub kind: ScheduleKind,
    pub cron_expression: Option<String>,
    pub interval_ms: Option<UnixMillis>,
    pub anchor_at: Option<UnixMillis>,
    pub timezone: String,
    pub scheduled_at: UnixMillis,
    pub created_at: UnixMillis,
    pub creating_run_id: Option<RunId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleOccurrence {
    pub id: ScheduleOccurrenceId,
    pub workspace_id: WorkspaceId,
    pub schedule_id: ScheduleId,
    pub schedule_revision: u32,
    pub scheduled_at: UnixMillis,
    pub processed_at: UnixMillis,
    pub outcome: String,
    pub wakeup_id: Option<WakeupId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeupState {
    Pending,
    Started,
    Withdrawn,
}

impl WakeupState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Started => "started",
            Self::Withdrawn => "withdrawn",
        }
    }
}

impl std::str::FromStr for WakeupState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "started" => Ok(Self::Started),
            "withdrawn" => Ok(Self::Withdrawn),
            other => Err(format!("unknown wake-up state: {other}")),
        }
    }
}

/// The durable cause that a Wake-up delivers. A Schedule or Event
/// Subscription is a rule. A synced arrival batch is a direct cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeupRule {
    Schedule {
        schedule_id: ScheduleId,
    },
    EventSubscription {
        subscription_id: EventSubscriptionId,
    },
    /// One acquisition batch that changed these Subject Pages,
    /// or one batch of historical pages the backfill selected (ADR-0011).
    Arrival {
        subject_paths: Vec<String>,
        /// True while the pages hold historical arrivals alone.
        historical: bool,
    },
}

impl WakeupRule {
    /// The `source_kind` column: which table the rule id points at.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Schedule { .. } => "schedule",
            Self::EventSubscription { .. } => "event_subscription",
            Self::Arrival { .. } => "arrival",
        }
    }

    pub fn id_str(&self) -> &str {
        match self {
            Self::Schedule { schedule_id } => schedule_id.as_str(),
            Self::EventSubscription { subscription_id } => subscription_id.as_str(),
            Self::Arrival { .. } => "arrival",
        }
    }

    pub fn schedule_id(&self) -> Option<&ScheduleId> {
        match self {
            Self::Schedule { schedule_id } => Some(schedule_id),
            Self::EventSubscription { .. } => None,
            Self::Arrival { .. } => None,
        }
    }

    pub fn subscription_id(&self) -> Option<&EventSubscriptionId> {
        match self {
            Self::EventSubscription { subscription_id } => Some(subscription_id),
            Self::Schedule { .. } => None,
            Self::Arrival { .. } => None,
        }
    }

    pub fn subject_paths(&self) -> &[String] {
        match self {
            Self::Arrival { subject_paths, .. } => subject_paths,
            Self::Schedule { .. } | Self::EventSubscription { .. } => &[],
        }
    }

    /// True while the Subject Pages of an arrival Wake-up hold
    /// historical arrivals alone (ADR-0011).
    pub fn historical(&self) -> bool {
        match self {
            Self::Arrival { historical, .. } => *historical,
            Self::Schedule { .. } | Self::EventSubscription { .. } => false,
        }
    }

    /// The Run trigger kind a Wake-up from this rule starts.
    pub fn trigger_kind(&self) -> TriggerKind {
        match self {
            Self::Schedule { .. } => TriggerKind::Schedule,
            Self::EventSubscription { .. } => TriggerKind::Event,
            Self::Arrival { .. } => TriggerKind::Arrival,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wakeup {
    pub id: WakeupId,
    pub workspace_id: WorkspaceId,
    pub rule: WakeupRule,
    pub rule_revision: u32,
    /// The rule's name, snapshot at Wake-up creation.
    pub rule_name: String,
    pub agent_id: AgentId,
    /// The destination for a conversation Run. An arrival Run has no Channel.
    pub channel_id: Option<ChannelId>,
    pub root_message_id: Option<MessageId>,
    pub instruction: String,
    /// The delivery order key: a Schedule's due instant, or the receive
    /// time of the first Incoming Event that joined this Wake-up.
    pub scheduled_at: UnixMillis,
    pub state: WakeupState,
    pub run_id: Option<RunId>,
    /// How many source occurrences have joined, including the first.
    pub source_count: u32,
    pub created_at: UnixMillis,
    pub started_at: Option<UnixMillis>,
}

/// One source occurrence behind a Wake-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeupSource {
    ScheduleOccurrence { occurrence_id: ScheduleOccurrenceId },
    IncomingEvent { event_id: IncomingEventId },
}

impl WakeupSource {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::ScheduleOccurrence { .. } => "schedule_occurrence",
            Self::IncomingEvent { .. } => "incoming_event",
        }
    }

    pub fn id_str(&self) -> &str {
        match self {
            Self::ScheduleOccurrence { occurrence_id } => occurrence_id.as_str(),
            Self::IncomingEvent { event_id } => event_id.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeupSourceLink {
    pub workspace_id: WorkspaceId,
    pub wakeup_id: WakeupId,
    pub source: WakeupSource,
}

/// Where one Wake-up lands: the Channel it posts into and the Thread
/// it continues (ADR-0019). A rule carries one of these of its own;
/// an occurrence answers with one when it belongs somewhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeupLanding {
    pub channel_id: ChannelId,
    /// The message the conversation is rooted at, or `None` for the
    /// Channel itself.
    pub root_message_id: Option<MessageId>,
}

/// One immutable Incoming Event declaration from a Capability
/// Manifest (ADR-0006). It fixes the normalized metadata schema, the
/// typed filter schema, the capability an Agent must hold, the trusted
/// matcher, and the manifest version a subscription pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDeclaration {
    /// The qualified kind, for example `mail.message_received`.
    pub name: String,
    pub metadata_schema: serde_json::Value,
    pub filter_schema: serde_json::Value,
    /// The capability an Agent must hold on the Connection. It is
    /// empty where the Agent acts as its own identity and holds no
    /// Grant, as it does in its own mailbox (ADR-0019).
    pub required_capability: String,
    /// The name of the trusted matcher that evaluates the filter.
    pub matcher: String,
    pub source_version: String,
    /// The Connection provider that supplies this kind.
    pub provider: String,
    /// The synced resource whose Source Items the occurrences are. The
    /// provider event id of an occurrence is the id of its Source Item,
    /// so a Forget of the item also blocks the occurrence (ADR-0008).
    /// `None`: no Sync acquires the occurrences, and a Forget does not
    /// apply to them.
    pub source_resource: Option<String>,
}

/// Who created a proactive rule (ADR-0006). An Agent-created rule needs
/// the user's approval before it delivers anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorKind {
    User,
    Agent,
}

impl CreatorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

impl std::str::FromStr for CreatorKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            other => Err(format!("unknown creator kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSubscriptionState {
    Active,
    Paused,
    /// The Agent lost the grant the declaration requires, or the
    /// Connection needs reauthorization. No Wake-up is delivered.
    Blocked,
    Archived,
}

impl EventSubscriptionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Archived => "archived",
        }
    }
}

impl std::str::FromStr for EventSubscriptionState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "blocked" => Ok(Self::Blocked),
            "archived" => Ok(Self::Archived),
            other => Err(format!("unknown event subscription state: {other}")),
        }
    }
}

/// Where an Incoming Event comes from, and what an Event Subscription
/// listens to: a Connection, or a Coding Session (ADR-0006, ADR-0033).
/// A rule or an event has exactly one source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventSource {
    Connection {
        connection_id: ConnectionId,
    },
    /// A Coding Session raises its own events. It has no cursor, no
    /// baseline and no collector: one session event is one batch.
    CodingSession {
        coding_session_id: CodingSessionId,
    },
}

/// The provider of each Incoming Event kind that Pagis itself raises,
/// such as the events of a Coding Session (ADR-0033).
pub const PAGIS_PROVIDER: &str = "pagis";

impl EventSource {
    pub fn connection(connection_id: ConnectionId) -> Self {
        Self::Connection { connection_id }
    }

    pub fn coding_session(coding_session_id: CodingSessionId) -> Self {
        Self::CodingSession { coding_session_id }
    }

    /// The Connection, for a Connection source.
    pub fn connection_id(&self) -> Option<&ConnectionId> {
        match self {
            Self::Connection { connection_id } => Some(connection_id),
            Self::CodingSession { .. } => None,
        }
    }

    /// The Coding Session, for a Coding Session source.
    pub fn coding_session_id(&self) -> Option<&CodingSessionId> {
        match self {
            Self::Connection { .. } => None,
            Self::CodingSession { coding_session_id } => Some(coding_session_id),
        }
    }

    /// The source that two nullable columns hold. Exactly one of them
    /// holds a value, as the `CHECK` of each table says.
    pub fn from_columns(
        connection_id: Option<String>,
        coding_session_id: Option<String>,
    ) -> Result<Self, String> {
        match (connection_id, coding_session_id) {
            (Some(connection_id), None) => Ok(Self::connection(connection_id.into())),
            (None, Some(coding_session_id)) => Ok(Self::coding_session(coding_session_id.into())),
            (connection_id, coding_session_id) => Err(format!(
                "an event source names one of a Connection and a Coding Session, not \
                 connection_id={connection_id:?} and coding_session_id={coding_session_id:?}"
            )),
        }
    }
}

impl std::fmt::Display for EventSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection { connection_id } => write!(formatter, "connection:{connection_id}"),
            Self::CodingSession { coding_session_id } => {
                write!(formatter, "coding_session:{coding_session_id}")
            }
        }
    }
}

/// A durable rule that matches Incoming Events of one declared kind
/// from one source and asks one Agent to act (ADR-0006).
///
/// `source_version` pins the Capability Manifest version the filter and
/// the metadata were written against. `watermark_at` is the activation
/// watermark: provider data at or before it is never new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub id: EventSubscriptionId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub source: EventSource,
    pub event_kind: String,
    pub source_version: String,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    /// The typed filter, validated against the declaration's schema.
    pub filter: serde_json::Value,
    pub creator: CreatorKind,
    pub state: EventSubscriptionState,
    pub revision: u32,
    /// The revision the user approved. A rule delivers only when this
    /// equals `revision`.
    pub approved_revision: Option<u32>,
    pub watermark_at: Option<UnixMillis>,
    /// Why a `blocked` rule is blocked, and therefore what happens when
    /// it comes back.
    pub blocked_reason: Option<BlockReason>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub archived_at: Option<UnixMillis>,
}

/// What stopped a subscription, and what its return costs (ADR-0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// The Agent lost its grant on the Connection. A later grant
    /// restores delivery from that point forward only.
    GrantRevoked,
    /// The Connection itself needs reauthorization. The cursor is kept,
    /// so reauthorization runs one catch-up collection.
    ReauthRequired,
}

impl BlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GrantRevoked => "grant_revoked",
            Self::ReauthRequired => "reauth_required",
        }
    }
}

impl std::str::FromStr for BlockReason {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "grant_revoked" => Ok(Self::GrantRevoked),
            "reauth_required" => Ok(Self::ReauthRequired),
            other => Err(format!("unknown block reason: {other}")),
        }
    }
}

impl EventSubscription {
    /// True when the current revision carries the user's approval.
    pub fn is_approved(&self) -> bool {
        self.approved_revision == Some(self.revision)
    }

    /// True when this rule may match new Incoming Events.
    pub fn is_live(&self) -> bool {
        self.state == EventSubscriptionState::Active && self.is_approved()
    }
}

/// One normalized provider occurrence. It never stores an email body,
/// snippet, attachment, credential, or provider stderr (ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingEvent {
    pub id: IncomingEventId,
    pub workspace_id: WorkspaceId,
    pub source: EventSource,
    pub event_kind: String,
    /// The provider's own id. Unique with the source and the kind.
    pub provider_event_id: String,
    /// Untrusted normalized metadata, valid against the declaration.
    pub metadata: serde_json::Value,
    pub occurred_at: UnixMillis,
    pub received_at: UnixMillis,
    pub batch_id: SourceBatchId,
}

/// What one batch acquired: one collection pass of a Connection, or one
/// event of a Coding Session. For a Connection it is the collector
/// health record: the last successful collection, and the last failure
/// code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBatch {
    pub id: SourceBatchId,
    pub workspace_id: WorkspaceId,
    pub source: EventSource,
    pub event_kind: String,
    pub collected_at: UnixMillis,
    /// Occurrences the provider returned, before deduplication.
    pub collected_count: u32,
    /// Occurrences stored as new Incoming Events.
    pub stored_count: u32,
    pub wakeup_count: u32,
    pub outcome: SourceBatchOutcome,
    /// A stable failure code; never provider text.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceBatchOutcome {
    /// The first collection on this Connection: it set the watermark
    /// and emitted no Incoming Event.
    Baseline,
    Collected,
    Failed,
}

impl SourceBatchOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Collected => "collected",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for SourceBatchOutcome {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "collected" => Ok(Self::Collected),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown source batch outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeupClaim {
    pub wakeup: Wakeup,
    pub run: Run,
}

/// The free Run slots of one Agent, one pool for each kind of work. A
/// conversation Run answers a message or a Schedule in a Channel. An
/// arrival Run reflects synced Subject Pages and holds no Channel. The
/// pools are separate so that a sync in the background cannot fill the
/// slots a user message needs (ADR-0002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSlots {
    pub conversation: u32,
    pub arrival: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Active,
    Archived,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Active => "active",
            AgentStatus::Archived => "archived",
        }
    }
}

impl std::str::FromStr for AgentStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(AgentStatus::Active),
            "archived" => Ok(AgentStatus::Archived),
            other => Err(format!("unknown agent status: {other}")),
        }
    }
}

/// The channel's trigger contract: every user message in a DM
/// triggers its agent; a group channel triggers only on an @-mention,
/// plus the thread re-mention rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Dm,
    Group,
}

impl ChannelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelKind::Dm => "dm",
            ChannelKind::Group => "group",
        }
    }
}

impl std::str::FromStr for ChannelKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dm" => Ok(ChannelKind::Dm),
            "group" => Ok(ChannelKind::Group),
            other => Err(format!("unknown channel kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub kind: ChannelKind,
    pub title: Option<String>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    User,
    Agent,
    System,
}

impl AuthorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorKind::User => "user",
            AuthorKind::Agent => "agent",
            AuthorKind::System => "system",
        }
    }
}

impl std::str::FromStr for AuthorKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(AuthorKind::User),
            "agent" => Ok(AuthorKind::Agent),
            "system" => Ok(AuthorKind::System),
            other => Err(format!("unknown author kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Complete,
    Failed,
}

impl MessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageStatus::Streaming => "streaming",
            MessageStatus::Complete => "complete",
            MessageStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for MessageStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "streaming" => Ok(MessageStatus::Streaming),
            "complete" => Ok(MessageStatus::Complete),
            "failed" => Ok(MessageStatus::Failed),
            other => Err(format!("unknown message status: {other}")),
        }
    }
}

/// One message in a channel. `blocks` is the typed block array;
/// `text_content` is the plain-text projection used for search and
/// model context. `pending_id` is the client-generated send dedup key;
/// only user sends carry one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub parent_message_id: Option<MessageId>,
    pub author_kind: AuthorKind,
    pub author_agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub status: MessageStatus,
    pub blocks: Vec<Block>,
    pub text_content: String,
    pub pending_id: Option<String>,
    pub created_at: UnixMillis,
    pub completed_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantKind {
    User,
    Agent,
}

impl ParticipantKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParticipantKind::User => "user",
            ParticipantKind::Agent => "agent",
        }
    }
}

impl std::str::FromStr for ParticipantKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(ParticipantKind::User),
            "agent" => Ok(ParticipantKind::Agent),
            other => Err(format!("unknown participant kind: {other}")),
        }
    }
}

/// One member of a channel: the user, or an agent. A DM channel has the
/// user and exactly one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelParticipant {
    pub id: ParticipantId,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub kind: ParticipantKind,
    pub agent_id: Option<AgentId>,
    pub joined_at: UnixMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Reflecting,
    WaitingForUser,
    WaitingForApproval,
    Completed,
    Failed,
    Canceled,
}

impl RunState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Reflecting => "reflecting",
            RunState::WaitingForUser => "waiting_for_user",
            RunState::WaitingForApproval => "waiting_for_approval",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Canceled => "canceled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Completed | RunState::Failed | RunState::Canceled
        )
    }
}

impl std::str::FromStr for RunState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(RunState::Queued),
            "running" => Ok(RunState::Running),
            "reflecting" => Ok(RunState::Reflecting),
            "waiting_for_user" => Ok(RunState::WaitingForUser),
            "waiting_for_approval" => Ok(RunState::WaitingForApproval),
            "completed" => Ok(RunState::Completed),
            "failed" => Ok(RunState::Failed),
            "canceled" => Ok(RunState::Canceled),
            other => Err(format!("unknown run state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Message,
    Schedule,
    Event,
    Arrival,
    Review,
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerKind::Message => "message",
            TriggerKind::Schedule => "schedule",
            TriggerKind::Event => "event",
            TriggerKind::Arrival => "arrival",
            TriggerKind::Review => "review",
        }
    }
}

impl std::str::FromStr for TriggerKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "message" => Ok(TriggerKind::Message),
            "schedule" => Ok(TriggerKind::Schedule),
            "event" => Ok(TriggerKind::Event),
            "arrival" => Ok(TriggerKind::Arrival),
            "review" => Ok(TriggerKind::Review),
            other => Err(format!("unknown trigger kind: {other}")),
        }
    }
}

/// The conversation a delegation chain owes an answer to: the
/// agent that owes it, and the channel and thread it owes it in. A run
/// an agent message triggers inherits the origin of the chain
/// unchanged, so the answer knows where it belongs however many agents
/// the request passes through. A run the user triggered starts the
/// chain and has no origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOrigin {
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    /// The thread inside the channel; `None` is the top level.
    pub root_message_id: Option<MessageId>,
}

/// The cause recorded where a Run fails. The error holds the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    AgentMissing,
    ModelMissing,
    ContextFailed,
    ToolFailed,
    CallFailed,
    AccessChanged,
    PublicationRejected,
    LeaseFailed,
    DaemonRestarted,
    ModelFailed,
    TurnLimit,
    /// The Person is at their monthly Spend Cap. The run says so
    /// in the conversation and ends before it asks a model anything.
    SpendCapReached,
    Unknown,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentMissing => "agent_missing",
            Self::ModelMissing => "model_missing",
            Self::ContextFailed => "context_failed",
            Self::ToolFailed => "tool_failed",
            Self::CallFailed => "call_failed",
            Self::AccessChanged => "access_changed",
            Self::PublicationRejected => "publication_rejected",
            Self::LeaseFailed => "lease_failed",
            Self::DaemonRestarted => "daemon_restarted",
            Self::ModelFailed => "model_failed",
            Self::TurnLimit => "turn_limit",
            Self::SpendCapReached => "spend_cap_reached",
            Self::Unknown => "unknown",
        }
    }
}

impl std::str::FromStr for FailureKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent_missing" => Ok(Self::AgentMissing),
            "model_missing" => Ok(Self::ModelMissing),
            "context_failed" => Ok(Self::ContextFailed),
            "tool_failed" => Ok(Self::ToolFailed),
            "call_failed" => Ok(Self::CallFailed),
            "access_changed" => Ok(Self::AccessChanged),
            "publication_rejected" => Ok(Self::PublicationRejected),
            "lease_failed" => Ok(Self::LeaseFailed),
            "daemon_restarted" => Ok(Self::DaemonRestarted),
            "model_failed" => Ok(Self::ModelFailed),
            "turn_limit" => Ok(Self::TurnLimit),
            "spend_cap_reached" => Ok(Self::SpendCapReached),
            "unknown" => Ok(Self::Unknown),
            other => Err(format!("unknown failure kind: {other}")),
        }
    }
}

/// One unit of agent work: the loop from a trigger to a terminal
/// state. `root_message_id` binds the run to a thread; `None` binds it
/// to the channel's top level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: Option<ChannelId>,
    pub root_message_id: Option<MessageId>,
    pub trigger_kind: TriggerKind,
    pub trigger_ref: Option<String>,
    /// The count of agent-to-agent hops behind this run. A run
    /// triggered by the user has hop 0; a run triggered by a message
    /// another run sent has that run's hop plus one.
    pub hop_count: u32,
    /// The conversation waiting on this run's delegation chain.
    pub origin: Option<RunOrigin>,
    pub state: RunState,
    pub failure_kind: Option<FailureKind>,
    pub error: Option<String>,
    pub started_at: Option<UnixMillis>,
    pub ended_at: Option<UnixMillis>,
    pub created_at: UnixMillis,
    /// When the Person dismissed the Run from the Needs-You Queue.
    /// `RunStore::update` does not write it; only `RunStore::dismiss`
    /// does.
    pub dismissed_at: Option<UnixMillis>,
}

impl Run {
    /// The waiting conversation this run must reply into, rather
    /// than the channel that triggered it: the run answers a
    /// delegation its own agent started somewhere else. `None` keeps
    /// the reply in the run's own channel and thread.
    pub fn relay_target(&self) -> Option<&RunOrigin> {
        self.origin.as_ref().filter(|origin| {
            origin.agent_id == self.agent_id && Some(&origin.channel_id) != self.channel_id.as_ref()
        })
    }

    /// The channel this run's own reply lands in: the waiting
    /// conversation when the run relays, and the run's own channel
    /// otherwise. A run speaks once in a channel, so a message sent
    /// here would be a second one, and each message wakes the reader.
    pub fn reply_channel_id(&self) -> Option<&ChannelId> {
        match self.relay_target() {
            Some(origin) => Some(&origin.channel_id),
            None => self.channel_id.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    Approved,
    Denied,
    Expired,
    /// The user sent a message in place of a decision; the run
    /// read the message and did not run the action.
    Superseded,
}

impl RequestState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::Approved => "approved",
            RequestState::Denied => "denied",
            RequestState::Expired => "expired",
            RequestState::Superseded => "superseded",
        }
    }
}

impl std::str::FromStr for RequestState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(RequestState::Pending),
            "approved" => Ok(RequestState::Approved),
            "denied" => Ok(RequestState::Denied),
            "expired" => Ok(RequestState::Expired),
            "superseded" => Ok(RequestState::Superseded),
            other => Err(format!("unknown request state: {other}")),
        }
    }
}

/// One pending user decision. The row is the source of
/// truth for the state; the blocks that render it only reference
/// `request_id` and are never mutated.
///
/// `kind` names what the user decides:
/// - `tool_action` — a broker-gated tool call. `payload` holds
///   the trusted tool name, the validated arguments, the approval
///   presentation, and the optional allow-rule proposal.
/// - `credential_action` — a vault action, which reads its card
///   and its allow rule from the Credential record.
/// - `form` — `payload` holds the field schema and the display
///   fields; the decision records `values`.
/// - `choice` — `payload` holds the options; the decision
///   records the chosen value in `values`.
/// - `widget` (ADR-0016) — a tool that renders a Widget declared
///   `awaits_input`. `payload` holds the package, the version, the
///   Widget name, the tool call id and the author's projection; the
///   decision records the Widget's `text` and its optional `value`.
/// - `harness_permission` (ADR-0033) — a Harness Permission of a
///   Coding Session that Pagis policy does not allow. It has no Run.
///   `payload` holds the session, the machine, the directory, the tool
///   call, the card text and the allow-rule proposal of an `execute`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub run_id: Option<RunId>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub state: RequestState,
    /// What a `form` or a `choice` submitted with its decision. The
    /// daemon validates it against the field schema on `payload`,
    /// never against the block's denormalized copy.
    pub values: Option<serde_json::Value>,
    pub decided_at: Option<UnixMillis>,
    pub created_at: UnixMillis,
}

impl Request {
    /// A broker-gated tool call.
    pub const TOOL_ACTION_KIND: &'static str = "tool_action";
    /// A vault action.
    pub const CREDENTIAL_ACTION_KIND: &'static str = "credential_action";
    /// A form the user fills and submits.
    pub const FORM_KIND: &'static str = "form";
    /// A choice card the user taps.
    pub const CHOICE_KIND: &'static str = "choice";
    /// A Widget that asks the user (ADR-0016).
    pub const WIDGET_KIND: &'static str = "widget";
    /// A Harness Permission that asks the Person. It has no Run: it
    /// waits on its Coding Session (ADR-0033).
    pub const HARNESS_PERMISSION_KIND: &'static str = "harness_permission";

    /// True when the kind submits values with its decision. The
    /// daemon checks each kind's values against the payload it wrote,
    /// never against the block.
    pub fn takes_values(kind: &str) -> bool {
        kind == Self::FORM_KIND || kind == Self::CHOICE_KIND || kind == Self::WIDGET_KIND
    }
}

/// A scoped permission that lets one agent use one workspace resource.
/// A credential grant has no `resource_id` and holds allow rules. A host
/// grant names one Host and holds command allow rules, session allow
/// rules and the widest Session Approval Mode. A connection grant names
/// one connection and holds capabilities. A Plugin grant names one
/// Plugin. The off switch is `revoked_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub id: GrantId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub scope: serde_json::Value,
    /// Monotonic scope version. New grants start at 1.
    pub revision: i64,
    pub created_at: UnixMillis,
    pub revoked_at: Option<UnixMillis>,
}

impl Grant {
    /// The `resource_kind` of a host grant.
    pub const HOST_KIND: &'static str = "host";

    /// The `resource_kind` of a credential grant (ADR-0013): its
    /// allow rules are registrable domains the daemon read off a
    /// Credential record, never a string the agent supplied.
    pub const CREDENTIAL_KIND: &'static str = "credential";

    /// The `resource_kind` of a connection grant.
    pub const CONNECTION_KIND: &'static str = "connection";

    /// The `resource_kind` of a Plugin grant (ADR-0017). A
    /// Plugin is one trust unit, so the scope is empty: the Agent uses
    /// all of the Plugin or none of it. A call also needs the Grant on
    /// each bound Connection.
    pub const PLUGIN_KIND: &'static str = "plugin";

    /// The scope's allow rules; a malformed scope reads as none.
    pub fn allow_rules(&self) -> Vec<String> {
        self.scope["allow"]
            .as_array()
            .map(|rules| {
                rules
                    .iter()
                    .filter_map(|rule| rule.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The scope JSON for a list of allow rules.
    pub fn allow_scope(allow: &[String]) -> serde_json::Value {
        serde_json::json!({ "allow": allow })
    }

    /// The scope with these allow rules and every other field kept.
    pub fn with_allow_rules(&self, allow: &[String]) -> serde_json::Value {
        self.with_scope_field("allow", serde_json::json!(allow))
    }

    /// The widest Session Approval Mode that a host Grant gives its
    /// Agent on its machine (ADR-0033). An absent or unknown value, and
    /// a malformed scope, read as `person`.
    pub fn session_approval_mode(&self) -> SessionApprovalMode {
        self.scope["session_approval_mode"]
            .as_str()
            .and_then(|mode| mode.parse().ok())
            .unwrap_or(SessionApprovalMode::Person)
    }

    /// The scope with this widest Session Approval Mode and every other
    /// field kept.
    pub fn with_session_approval_mode(&self, mode: SessionApprovalMode) -> serde_json::Value {
        self.with_scope_field("session_approval_mode", serde_json::json!(mode.as_str()))
    }

    /// The session Allow Rules of a host Grant (ADR-0033). A malformed
    /// scope, and a malformed rule, read as none.
    pub fn session_allow_rules(&self) -> Vec<SessionAllowRule> {
        self.scope["sessions"]
            .as_array()
            .map(|rules| {
                rules
                    .iter()
                    .filter_map(|rule| serde_json::from_value(rule.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The scope with these session Allow Rules and every other field
    /// kept.
    pub fn with_session_allow_rules(&self, rules: &[SessionAllowRule]) -> serde_json::Value {
        self.with_scope_field("sessions", serde_json::json!(rules))
    }

    /// The scope with one field replaced. A malformed scope holds no
    /// field to keep, so the answer is a new object.
    fn with_scope_field(&self, name: &str, value: serde_json::Value) -> serde_json::Value {
        let mut scope = self.scope.as_object().cloned().unwrap_or_default();
        scope.insert(name.to_string(), value);
        serde_json::Value::Object(scope)
    }

    /// The named capabilities in a connection grant.
    pub fn capabilities(&self) -> Vec<String> {
        self.scope["capabilities"]
            .as_array()
            .map(|capabilities| {
                capabilities
                    .iter()
                    .filter_map(|capability| capability.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The scope JSON for named connection capabilities.
    pub fn connection_scope(capabilities: &[String]) -> serde_json::Value {
        serde_json::json!({ "capabilities": capabilities })
    }
}

/// One immutable capability snapshot stored by its canonical content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshotRecord {
    pub id: String,
    pub hash: String,
    pub content: serde_json::Value,
    pub created_at: UnixMillis,
}

/// The class of an Artifact. One retention policy applies to
/// one class, so the class is written when the row is made and is
/// never guessed from the filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A screen capture of an Agent Computer.
    Screenshot,
    /// The recorded audio of a Call.
    CallRecording,
    /// The written record of a Call.
    CallTranscript,
    /// Anything else: an upload, or a file an Agent wrote.
    File,
}

impl ArtifactKind {
    /// Every class, in the order the settings page shows them.
    pub const ALL: [ArtifactKind; 4] = [
        ArtifactKind::Screenshot,
        ArtifactKind::CallRecording,
        ArtifactKind::CallTranscript,
        ArtifactKind::File,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::Screenshot => "screenshot",
            ArtifactKind::CallRecording => "call_recording",
            ArtifactKind::CallTranscript => "call_transcript",
            ArtifactKind::File => "file",
        }
    }

    /// The class for one stored name, or `None` when the name is not a
    /// class the daemon knows.
    pub fn parse(value: &str) -> Option<Self> {
        ArtifactKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

impl std::fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How long one Artifact class is kept in one Workspace.
/// `retain_days` of `None` keeps the class for ever, which is the
/// default for every class (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub kind: ArtifactKind,
    pub retain_days: Option<i64>,
}

impl RetentionPolicy {
    /// The instant before which an Artifact of this class has expired,
    /// or `None` when the class is kept for ever.
    pub fn cutoff(&self, now: UnixMillis) -> Option<UnixMillis> {
        self.retain_days
            .map(|days| now - days * 24 * 60 * 60 * 1000)
    }
}

/// A binary asset stored outside the database. The row is the
/// metadata; the bytes live in the workspace blob store under
/// `storage_key`. Duplicate content dedups on (workspace_id, sha256).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub workspace_id: WorkspaceId,
    /// The retention class of the bytes.
    pub kind: ArtifactKind,
    pub creator_agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub filename: Option<String>,
    pub mime: String,
    pub size_bytes: i64,
    /// Lowercase hex SHA-256 of the bytes; the dedup key.
    pub sha256: String,
    /// The blob-store path of the bytes.
    pub storage_key: String,
    pub created_at: UnixMillis,
}

impl Artifact {
    /// True when the bytes are an image the model can see.
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }
}

/// One of the user's sprites. `voice` is the Agent Voice (ADR-0020):
/// one name from the catalogue the daemon knows, or `None`, which
/// declares no voice and lets the speech provider use its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub job: String,
    /// One line that says what to ask this Agent for. Every other
    /// Agent reads it in its sprite line, so a colleague can pick the
    /// Agent a request belongs to. Empty says nothing beyond the job.
    pub description: String,
    pub personality: String,
    pub model_alias: String,
    /// The sprite and its saved appearance.
    pub avatar: crate::AvatarAppearance,
    pub voice: Option<String>,
    /// The standing brief (ADR-0020): what an inbound call to the
    /// Agent's desk line is for. The daemon reads it at answer time.
    /// `None` answers with no purpose beyond taking a message.
    pub standing_brief: Option<String>,
    pub status: AgentStatus,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

impl Agent {
    /// The first message in the Agent's DM. It needs no model or Run.
    pub fn greeting(&self, channel_id: ChannelId) -> Message {
        let text = if self.job.is_empty() {
            format!("Hi, I'm {}.", self.name)
        } else {
            format!("Hi, I'm {}, your {}.", self.name, self.job)
        };
        Message {
            id: MessageId::generate(),
            workspace_id: self.workspace_id.clone(),
            channel_id,
            parent_message_id: None,
            author_kind: AuthorKind::Agent,
            author_agent_id: Some(self.id.clone()),
            run_id: None,
            status: MessageStatus::Complete,
            blocks: vec![Block::markdown(&text)],
            text_content: text,
            pending_id: None,
            created_at: self.created_at,
            completed_at: Some(self.created_at),
        }
    }
}

/// A workspace model name and its ordered provider/model fallbacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAlias {
    pub id: crate::ModelAliasId,
    pub workspace_id: WorkspaceId,
    pub alias: String,
    pub candidates: Vec<String>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

/// One live link to an external provider (ADR-0005). A Connection
/// belongs to the Workspace, and agents reach it through grants.
///
/// `alias` is the name a tool call uses to pick this Connection when
/// the agent holds a grant on more than one of the same provider. It is
/// model-visible, so it carries no ID and no secret.
///
/// `config` is the trusted binding the provider needs — for Google, the
/// account and the Desktop OAuth client. The user supplies it; nothing
/// an agent wrote ever reaches it, and it holds no token.
///
/// `auth_mode` records who supplies the OAuth client: `byo`, where the
/// user does, or `brokered`, where the installation does through its
/// Installation OAuth Client. The mode belongs to the connection, not to
/// the provider, because one installation can hold a client for a
/// provider whose people bring their own on another installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: crate::ConnectionId,
    pub workspace_id: WorkspaceId,
    pub provider: String,
    pub alias: String,
    pub display_name: String,
    pub status: String,
    pub auth_mode: String,
    /// The named capabilities accepted by the provider in the latest
    /// successful authorization.
    pub authorized_capabilities: Vec<String>,
    pub config: serde_json::Value,
    pub created_at: UnixMillis,
}

impl Connection {
    /// A record that carries its binding and has no live authorization.
    pub const DISCONNECTED: &'static str = "disconnected";
    /// The user is at Google, and the daemon waits on 127.0.0.1.
    pub const CONNECTING: &'static str = "connecting";
    /// The one status at which the broker offers a Connection's tools.
    pub const CONNECTED: &'static str = "connected";
    /// The provider refused the authorization it once accepted.
    pub const REAUTH_REQUIRED: &'static str = "reauth_required";
    /// The provider refused the credential the record holds. What the
    /// credential administers is out of reach until the user replaces
    /// it; work that uses another credential keeps running, as an Agent
    /// Mailbox does when the host API key is revoked (ADR-0019).
    pub const UNAVAILABLE: &'static str = "unavailable";
    /// Every status a Connection can hold. The store keeps each one.
    pub const STATUSES: [&'static str; 5] = [
        Self::DISCONNECTED,
        Self::CONNECTING,
        Self::CONNECTED,
        Self::REAUTH_REQUIRED,
        Self::UNAVAILABLE,
    ];

    /// The user supplies the OAuth client.
    pub const AUTH_MODE_BYO: &'static str = "byo";
    /// The installation supplies the OAuth client: its Installation OAuth
    /// Client.
    pub const AUTH_MODE_BROKERED: &'static str = "brokered";
}

/// One saved login in the Workspace vault (ADR-0013). The secret
/// and the TOTP seed are sealed: the row carries ciphertext, and only
/// `pagis-vault` holds the data key that opens it. Nothing else — not
/// this crate, not the API, not the model — can read them.
///
/// `login_url` is the one address a fill opens, and its registrable
/// domain must equal `domain`. The vault checks that on write and again
/// before every fill, so a fill is bound to its target by the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub id: crate::CredentialId,
    pub workspace_id: WorkspaceId,
    /// The registrable domain, from the public suffix list.
    pub domain: String,
    pub username: String,
    /// The one address the daemon opens before it types.
    pub login_url: String,
    /// The sealed secret. Opaque outside the vault.
    pub secret: SealedSecret,
    /// The sealed TOTP seed, when the user enrolled one.
    pub totp_seed: Option<SealedSecret>,
    /// The password recipe that minted the secret, in Apple's Password
    /// Rules grammar, so a later rotation reproduces it.
    pub recipe: String,
    /// The Agent that owns the record, or `None` for the user.
    /// Archiving an Agent clears this; the record stays usable.
    pub owner_agent_id: Option<AgentId>,
    pub provenance: CredentialProvenance,
    /// The Run that created it, for `agent_minted` records.
    pub created_run_id: Option<RunId>,
    pub created_at: UnixMillis,
}

/// Ciphertext for one secret field. It has no accessor that returns a
/// plaintext string, and its `Debug` never prints the bytes: a value
/// that cannot be read by accident cannot be logged by accident.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedSecret(pub Vec<u8>);

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SealedSecret({} bytes)", self.0.len())
    }
}

/// Where a Credential came from (ADR-0013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProvenance {
    /// The user typed it into Workspace settings.
    UserSupplied,
    /// The daemon minted it for an Agent at signup.
    AgentMinted,
}

impl CredentialProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialProvenance::UserSupplied => "user_supplied",
            CredentialProvenance::AgentMinted => "agent_minted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user_supplied" => Some(CredentialProvenance::UserSupplied),
            "agent_minted" => Some(CredentialProvenance::AgentMinted),
            _ => None,
        }
    }
}

/// One telephone number the Workspace owns: an Agent's desk line
/// (ADR-0018). `e164` is model-visible; `provider_number_id` is the
/// carrier's own handle and never reaches the model. Pagis stores the
/// two together or stores neither, as it does for the mailbox pair.
///
/// The record carries no price, because a price recorded at purchase
/// goes stale, and no health state, because registration health belongs
/// to the Connection: carrier trouble then shows one time, on the
/// carrier, and not one time for each Agent.
///
/// `agent_id` is the whole assignment. One Agent holds at most one
/// number and one number is held by at most one Agent, and a partial
/// unique index on this column holds both halves of that rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneNumber {
    pub id: crate::PhoneNumberId,
    pub workspace_id: WorkspaceId,
    /// The telephony Connection that sold the number and carries it.
    pub connection_id: crate::ConnectionId,
    pub e164: String,
    /// The carrier's own id for the number. Opaque outside the daemon.
    pub provider_number_id: String,
    /// The Agent that holds the line, when one does.
    pub agent_id: Option<AgentId>,
    pub status: PhoneNumberStatus,
    /// The most texts a day the number may send (ADR-0020). It counts
    /// texts, never segments, as the Outgoing Cap of a mailbox counts
    /// messages.
    pub outgoing_cap: u32,
    /// The counterpart numbers the user allowed from a send card
    /// (ADR-0020). A text to one of them runs with no card. The rules
    /// live on the record and not in a Grant, because an Agent holds
    /// its own number with no Grant.
    #[serde(default)]
    pub allow_rules: Vec<String>,
    /// The start of the day the send tally counts, or `None` while the
    /// number has sent nothing.
    pub sends_day: Option<UnixMillis>,
    /// How many texts the number sent on `sends_day`.
    pub sends_today: u32,
    /// Whether the carrier will deliver an outbound text (ADR-0020).
    pub messaging_readiness: MessagingReadiness,
    /// When the readiness was last read from the carrier.
    pub messaging_readiness_at: Option<UnixMillis>,
    /// Why the last readiness read failed, when one did. It travels
    /// beside the state, as a registration failure does on the SIP
    /// endpoint.
    pub messaging_readiness_error: Option<String>,
    /// Where the inbound collector reached (ADR-0020). The carrier
    /// decides what it holds, so it is one opaque string.
    pub text_cursor: Option<String>,
    /// The carrier's messaging object for this number: the Telnyx
    /// messaging profile id, or the Twilio Messaging Service the
    /// number sits in the sender pool of (ADR-0020).
    pub messaging_object_id: Option<String>,
    /// Where the Telnyx relay function stands (ADR-0020). It gates
    /// inbound collection only.
    pub relay_state: TelnyxRelayState,
    /// Whether the user let the daemon install the Telnyx CLI and ship
    /// the relay for this number (ADR-0020). The desk sets it before
    /// the download starts.
    pub relay_consent: bool,
    pub created_at: UnixMillis,
    pub assigned_at: Option<UnixMillis>,
}

impl PhoneNumber {
    /// The Outgoing Cap a number starts with (ADR-0020).
    pub const DEFAULT_OUTGOING_CAP: u32 = 50;

    /// A number the Workspace has just taken on, bought or adopted.
    /// Its texting fields start where every new number starts: the
    /// default cap with nothing spent, no allow rule, nothing read
    /// from the carrier and no relay.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: crate::PhoneNumberId,
        workspace_id: WorkspaceId,
        connection_id: crate::ConnectionId,
        e164: String,
        provider_number_id: String,
        agent_id: Option<AgentId>,
        at: UnixMillis,
    ) -> Self {
        let assigned = agent_id.is_some();
        Self {
            id,
            workspace_id,
            connection_id,
            e164,
            provider_number_id,
            agent_id,
            status: match assigned {
                true => PhoneNumberStatus::Assigned,
                false => PhoneNumberStatus::Unassigned,
            },
            outgoing_cap: Self::DEFAULT_OUTGOING_CAP,
            allow_rules: Vec::new(),
            sends_day: None,
            sends_today: 0,
            messaging_readiness: MessagingReadiness::Unknown,
            messaging_readiness_at: None,
            messaging_readiness_error: None,
            text_cursor: None,
            messaging_object_id: None,
            relay_state: TelnyxRelayState::Absent,
            relay_consent: false,
            created_at: at,
            assigned_at: assigned.then_some(at),
        }
    }

    /// How many texts the number has sent on one day. A tally from an
    /// earlier day counts for nothing.
    pub fn sends_on(&self, day: UnixMillis) -> u32 {
        match self.sends_day {
            Some(sends_day) if sends_day == day => self.sends_today,
            _ => 0,
        }
    }
}

/// Whether an Agent Phone Number may send a text (ADR-0020). The
/// carrier decides it: a number that is capable of SMS still does not
/// send until it is registered. Pagis reads the state, stores it,
/// shows it and refuses a send against it; it never registers the
/// number itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MessagingReadiness {
    /// No read has succeeded yet.
    Unknown,
    /// The number has no SMS feature.
    NotCapable,
    /// Capable, with no campaign and no verification.
    Unregistered,
    /// Registration or verification is in review, or the carrier is
    /// still propagating it.
    Pending,
    /// The carrier will deliver an outbound text.
    Ready,
    /// The carrier refused the registration, in its own words.
    Rejected { reason: String },
}

impl MessagingReadiness {
    /// The one word the row stores.
    pub fn as_str(&self) -> &'static str {
        match self {
            MessagingReadiness::Unknown => "unknown",
            MessagingReadiness::NotCapable => "not_capable",
            MessagingReadiness::Unregistered => "unregistered",
            MessagingReadiness::Pending => "pending",
            MessagingReadiness::Ready => "ready",
            MessagingReadiness::Rejected { .. } => "rejected",
        }
    }

    /// The carrier's words for a refusal, when the state carries them.
    pub fn reason(&self) -> Option<&str> {
        match self {
            MessagingReadiness::Rejected { reason } => Some(reason),
            _ => None,
        }
    }

    /// The state one stored word and its reason name.
    pub fn parse(state: &str, reason: Option<&str>) -> Result<Self, String> {
        match state {
            "unknown" => Ok(MessagingReadiness::Unknown),
            "not_capable" => Ok(MessagingReadiness::NotCapable),
            "unregistered" => Ok(MessagingReadiness::Unregistered),
            "pending" => Ok(MessagingReadiness::Pending),
            "ready" => Ok(MessagingReadiness::Ready),
            "rejected" => Ok(MessagingReadiness::Rejected {
                reason: reason.unwrap_or_default().to_string(),
            }),
            other => Err(format!("unknown messaging readiness: {other}")),
        }
    }

    /// Whether a send may leave this number.
    pub fn is_ready(&self) -> bool {
        matches!(self, MessagingReadiness::Ready)
    }
}

/// Where the Telnyx relay function of one number stands (ADR-0020).
/// Telnyx returns no inbound body, so an Edge Compute function writes
/// each inbound text to Telnyx KV and the collector drains it. The
/// daemon does not ship that function: that part is not built, so the
/// state stays `Absent` or `Failed`. The state gates inbound collection
/// only: a send does not wait for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TelnyxRelayState {
    /// Nothing is shipped yet.
    Absent,
    /// The daemon is downloading the CLI or shipping the function.
    Installing,
    /// The function answers and writes to KV.
    Ready,
    /// The CLI refused. The reason is for the user and `cli_lines`
    /// holds the last lines the CLI wrote, which the page shows
    /// behind a control.
    Failed {
        reason: String,
        cli_lines: Vec<String>,
    },
}

impl TelnyxRelayState {
    /// The one word the row stores.
    pub fn as_str(&self) -> &'static str {
        match self {
            TelnyxRelayState::Absent => "absent",
            TelnyxRelayState::Installing => "installing",
            TelnyxRelayState::Ready => "ready",
            TelnyxRelayState::Failed { .. } => "failed",
        }
    }

    /// Why the ship failed, when it did.
    pub fn reason(&self) -> Option<&str> {
        match self {
            TelnyxRelayState::Failed { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// The last lines the CLI wrote. Every state but a failure has
    /// none.
    pub fn cli_lines(&self) -> &[String] {
        match self {
            TelnyxRelayState::Failed { cli_lines, .. } => cli_lines,
            _ => &[],
        }
    }

    /// The state one stored word, its reason and its CLI lines name.
    pub fn parse(
        state: &str,
        reason: Option<&str>,
        cli_lines: Vec<String>,
    ) -> Result<Self, String> {
        match state {
            "absent" => Ok(TelnyxRelayState::Absent),
            "installing" => Ok(TelnyxRelayState::Installing),
            "ready" => Ok(TelnyxRelayState::Ready),
            "failed" => Ok(TelnyxRelayState::Failed {
                reason: reason.unwrap_or_default().to_string(),
                cli_lines,
            }),
            other => Err(format!("unknown telnyx relay state: {other}")),
        }
    }
}

/// Where one number stands (ADR-0018). To unassign is not to release:
/// an `Unassigned` number stays with the Workspace and stays paid for,
/// and a `Released` number went back to the carrier and never comes
/// back. The released record stays, so the Calls that point at it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneNumberStatus {
    Assigned,
    Unassigned,
    Released,
}

impl PhoneNumberStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PhoneNumberStatus::Assigned => "assigned",
            PhoneNumberStatus::Unassigned => "unassigned",
            PhoneNumberStatus::Released => "released",
        }
    }
}

impl std::str::FromStr for PhoneNumberStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "assigned" => Ok(PhoneNumberStatus::Assigned),
            "unassigned" => Ok(PhoneNumberStatus::Unassigned),
            "released" => Ok(PhoneNumberStatus::Released),
            other => Err(format!("unknown phone number status: {other}")),
        }
    }
}

/// The record the daemon writes before it asks the carrier to sell a
/// number (ADR-0018). The intent carries the idempotency key, so a
/// daemon that restarts in the middle of a purchase reconciles the
/// intent against the carrier's list and never buys a second time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseIntent {
    /// The id is the idempotency key the carrier sees.
    pub id: crate::PurchaseIntentId,
    pub workspace_id: WorkspaceId,
    pub connection_id: crate::ConnectionId,
    pub e164: String,
    /// The Agent the number is bought for, when the user bought it from
    /// an Agent's page. Reconciliation assigns it.
    pub agent_id: Option<AgentId>,
    pub state: PurchaseIntentState,
    pub created_at: UnixMillis,
    pub settled_at: Option<UnixMillis>,
}

/// One purchase intent's life. `Pending` is the only state that needs
/// reconciliation; the other two are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseIntentState {
    /// The daemon asked, and does not yet know the answer.
    Pending,
    /// The carrier sold the number and a record exists.
    Bought,
    /// The carrier does not hold the number, so nothing was bought.
    Abandoned,
}

impl PurchaseIntentState {
    pub fn as_str(self) -> &'static str {
        match self {
            PurchaseIntentState::Pending => "pending",
            PurchaseIntentState::Bought => "bought",
            PurchaseIntentState::Abandoned => "abandoned",
        }
    }
}

impl std::str::FromStr for PurchaseIntentState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(PurchaseIntentState::Pending),
            "bought" => Ok(PurchaseIntentState::Bought),
            "abandoned" => Ok(PurchaseIntentState::Abandoned),
            other => Err(format!("unknown purchase intent state: {other}")),
        }
    }
}

/// Which side dialed one Call (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDirection {
    Outbound,
    Inbound,
}

impl CallDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            CallDirection::Outbound => "outbound",
            CallDirection::Inbound => "inbound",
        }
    }
}

impl std::str::FromStr for CallDirection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "outbound" => Ok(CallDirection::Outbound),
            "inbound" => Ok(CallDirection::Inbound),
            other => Err(format!("unknown call direction: {other}")),
        }
    }
}

/// What the words of one Remote Party are worth (ADR-0021). The tier
/// gates authority and never access: Pagis answers a call from any
/// number, and the tier decides whose words can move the Agent.
///
/// The order is the authority order, so `min` is the rule of ADR-0021:
/// a tier is the smaller of the tier caller ID proposes and the tier
/// the Keypad Code proves.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Unknown,
    Trusted,
    Owner,
}

impl TrustTier {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::Owner => "owner",
            TrustTier::Trusted => "trusted",
            TrustTier::Unknown => "unknown",
        }
    }

    /// A listed tier. A list carries `owner` and `trusted` only:
    /// `unknown` is what a number that is on no list gets.
    pub fn listed(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(TrustTier::Owner),
            "trusted" => Some(TrustTier::Trusted),
            _ => None,
        }
    }
}

impl std::str::FromStr for TrustTier {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "owner" => Ok(TrustTier::Owner),
            "trusted" => Ok(TrustTier::Trusted),
            "unknown" => Ok(TrustTier::Unknown),
            other => Err(format!("unknown trust tier: {other}")),
        }
    }
}

/// What one Trust List entry names (ADR-0021, ADR-0019). The list is
/// one list: a phone number for a Call, an email address or a bare
/// domain for mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustSubject {
    /// An E.164 number, e.g. `+14155550123`.
    Number,
    /// One email address, e.g. `clinic@example.com`.
    Address,
    /// A bare mail domain, e.g. `example.com`. It covers every address
    /// at that domain.
    Domain,
}

impl TrustSubject {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustSubject::Number => "number",
            TrustSubject::Address => "address",
            TrustSubject::Domain => "domain",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "number" => Some(TrustSubject::Number),
            "address" => Some(TrustSubject::Address),
            "domain" => Some(TrustSubject::Domain),
            _ => None,
        }
    }
}

/// What the user typed, read as a Trust List subject and put in the one
/// form the list stores (ADR-0019).
///
/// The three subjects tell themselves apart: an `@` makes an address, a
/// leading `+` makes a number, and everything else is a domain. The
/// list holds one form of each, so `A@Example.COM` and `a@example.com`
/// are one entry.
pub fn parse_trust_subject(value: &str) -> Result<(TrustSubject, String), String> {
    let value = value.trim();
    if value.contains('@') {
        let address = mail_address(value)
            .ok_or_else(|| format!("{value} is not an email address, such as name@example.com"))?;
        return Ok((TrustSubject::Address, address));
    }
    if value.starts_with('+') {
        let number = normalize_e164(value)
            .ok_or_else(|| format!("{value} is not an E.164 number, such as +14155550123"))?;
        return Ok((TrustSubject::Number, number));
    }
    let domain = mail_domain(value)
        .ok_or_else(|| format!("{value} is not a mail domain, such as example.com"))?;
    Ok((TrustSubject::Domain, domain))
}

/// One E.164 number, or `None` when the text is not one. It is the one
/// number rule of Pagis: the Trust List and the dial path share it.
/// The spaces, hyphens, dots and parentheses a person writes between
/// the digits are dropped; the leading `+` is required.
pub fn normalize_e164(value: &str) -> Option<String> {
    let value: String = value
        .trim()
        .chars()
        .filter(|character| !matches!(character, ' ' | '-' | '.' | '(' | ')'))
        .collect();
    let value = value.as_str();
    let valid = value.starts_with('+')
        && value.len() >= 8
        && value.len() <= 16
        && value[1..].chars().all(|digit| digit.is_ascii_digit())
        && !value[1..].starts_with('0');
    valid.then(|| value.to_string())
}

/// The bare address of `name@example.com`, lowercased. `None` when
/// either half is empty or the text holds a space.
pub fn mail_address(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let (local, domain) = value.split_once('@')?;
    if local.is_empty() || local.contains(char::is_whitespace) {
        return None;
    }
    let domain = mail_domain(domain)?;
    Some(format!("{local}@{domain}"))
}

/// A bare mail domain, lowercased. `None` when it holds no dot, an `@`,
/// a space or an empty label.
pub fn mail_domain(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let shaped = value.contains('.')
        && !value.contains('@')
        && !value.contains(char::is_whitespace)
        && value.split('.').all(|label| !label.is_empty());
    shaped.then_some(value)
}

/// One Trust List entry: who it names and the tier it proposes
/// (ADR-0021, ADR-0019). A row with no `agent_id` is the user's own
/// contact and is Workspace-wide; a row with one belongs to that
/// Agent's list. The shape follows `credentials.owner_agent_id`, which
/// already means "the Agent, or the user".
///
/// No tool writes this record. The user edits it through the daemon,
/// because a list an Agent can extend is a list a caller or a sender
/// can talk their way onto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    pub id: crate::TrustEntryId,
    pub workspace_id: WorkspaceId,
    /// `None` for the Workspace-wide list.
    pub agent_id: Option<AgentId>,
    pub subject: TrustSubject,
    /// The E.164 number, bare address or bare domain this entry names.
    pub value: String,
    /// `Owner` or `Trusted`. A row never carries `Unknown`.
    pub tier: TrustTier,
    /// What the user calls this contact, e.g. `Home`.
    pub label: String,
    pub created_at: UnixMillis,
}

/// Whether the call did what it was for (ADR-0020). It is not the
/// ended reason: a call can reach its purpose and still end because
/// the media stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Answered,
    NoAnswer,
    Busy,
    Voicemail,
    Failed,
}

impl CallOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            CallOutcome::Answered => "answered",
            CallOutcome::NoAnswer => "no_answer",
            CallOutcome::Busy => "busy",
            CallOutcome::Voicemail => "voicemail",
            CallOutcome::Failed => "failed",
        }
    }
}

impl std::str::FromStr for CallOutcome {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "answered" => Ok(CallOutcome::Answered),
            "no_answer" => Ok(CallOutcome::NoAnswer),
            "busy" => Ok(CallOutcome::Busy),
            "voicemail" => Ok(CallOutcome::Voicemail),
            "failed" => Ok(CallOutcome::Failed),
            other => Err(format!("unknown call outcome: {other}")),
        }
    }
}

/// The classify phase's verdict (ADR-0020): what answered the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Classification {
    Human,
    MachineIvr,
    MachineVm,
    MachineUnavailable,
    Uncertain,
}

impl Classification {
    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Human => "human",
            Classification::MachineIvr => "machine-ivr",
            Classification::MachineVm => "machine-vm",
            Classification::MachineUnavailable => "machine-unavailable",
            Classification::Uncertain => "uncertain",
        }
    }
}

impl std::str::FromStr for Classification {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "human" => Ok(Classification::Human),
            "machine-ivr" => Ok(Classification::MachineIvr),
            "machine-vm" => Ok(Classification::MachineVm),
            "machine-unavailable" => Ok(Classification::MachineUnavailable),
            "uncertain" => Ok(Classification::Uncertain),
            other => Err(format!("unknown classification: {other}")),
        }
    }
}

/// Where one Call stands (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Dialing,
    Live,
    Ended,
}

impl CallState {
    pub fn as_str(self) -> &'static str {
        match self {
            CallState::Dialing => "dialing",
            CallState::Live => "live",
            CallState::Ended => "ended",
        }
    }
}

impl std::str::FromStr for CallState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "dialing" => Ok(CallState::Dialing),
            "live" => Ok(CallState::Live),
            "ended" => Ok(CallState::Ended),
            other => Err(format!("unknown call state: {other}")),
        }
    }
}

/// Who said one line of a Call transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    /// The Remote Party, as the realtime session transcribed it.
    Caller,
    /// The Agent, as the realtime session transcribed it.
    Agent,
    /// The daemon, for what happened and nobody said: the tier, the
    /// keypad, the classify verdict.
    Daemon,
}

impl Speaker {
    pub fn as_str(self) -> &'static str {
        match self {
            Speaker::Caller => "caller",
            Speaker::Agent => "agent",
            Speaker::Daemon => "daemon",
        }
    }
}

/// One line of a Call transcript, with the time it was said. The same
/// line feeds the live `call` block in the Thread and the settled
/// record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptLine {
    pub at: UnixMillis,
    pub speaker: Speaker,
    pub text: String,
}

impl TranscriptLine {
    pub fn new(at: UnixMillis, speaker: Speaker, text: impl Into<String>) -> Self {
        Self {
            at,
            speaker,
            text: text.into(),
        }
    }
}

/// The transcript as prose, one line per turn. The model, the
/// transcript Artifact and the tool result read this.
pub fn render_transcript(lines: &[TranscriptLine]) -> String {
    lines
        .iter()
        .map(|line| format!("{}: {}", line.speaker.as_str(), line.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One telephone call, in either direction (ADR-0020). The record is
/// the one source of truth the UI reads, and it settles when the call
/// ends: every Call ends with a reason (ADR-0020).
///
/// `outcome` and `ended_reason` are two fields, because they answer two
/// questions. A call can reach its purpose and still end with
/// `media_timeout`. The recording is one Artifact, and the record
/// keeps its pointer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub id: crate::CallId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    /// The Run that stays alive for the length of the call.
    pub run_id: RunId,
    pub phone_number_id: crate::PhoneNumberId,
    pub direction: CallDirection,
    pub remote_e164: String,
    /// The Agent's external display name, and its own line. They come
    /// from the Call Brief, because a released number is a tombstone
    /// the Call still points at. The own line is the number that placed
    /// an outbound Call, and the number the Remote Party dialed for an
    /// inbound Call.
    pub agent_name: String,
    pub own_e164: String,
    /// What the call is for, from the Call Brief.
    pub purpose: String,
    /// The tools the call may use, by name.
    pub tools: Vec<String>,
    pub tier: TrustTier,
    pub state: CallState,
    pub outcome: Option<CallOutcome>,
    pub ended_reason: Option<String>,
    pub classification: Option<Classification>,
    /// True when a voicemail message was left.
    pub message_left: bool,
    /// The whole conversation, one line per turn, with the time each
    /// line was said.
    pub transcript: Vec<TranscriptLine>,
    /// The stereo WAV of the call: the Remote Party on the left and
    /// the Agent on the right. It is written when the call settles.
    pub recording_artifact_id: Option<ArtifactId>,
    pub created_at: UnixMillis,
    pub ringing_at: Option<UnixMillis>,
    pub answered_at: Option<UnixMillis>,
    pub ended_at: Option<UnixMillis>,
    /// When the Person dismissed the missed Call from the Needs-You
    /// Queue. `CallStore::update` does not write it; only
    /// `CallStore::dismiss` does.
    pub dismissed_at: Option<UnixMillis>,
}

/// Which side sent one Text Message (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextDirection {
    Outbound,
    Inbound,
}

impl TextDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            TextDirection::Outbound => "outbound",
            TextDirection::Inbound => "inbound",
        }
    }
}

impl std::str::FromStr for TextDirection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "outbound" => Ok(TextDirection::Outbound),
            "inbound" => Ok(TextDirection::Inbound),
            other => Err(format!("unknown text direction: {other}")),
        }
    }
}

/// Whether one outbound text arrived (ADR-0020). Thirteen carrier
/// states across three carriers answer the one question the Agent
/// asks, so Pagis stores these four. A carrier's `undelivered` is
/// `Failed`, and a text with no receipt stays `Sent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TextDeliveryStatus {
    /// The carrier accepted the send and has not carried it yet.
    Queued,
    /// The carrier handed the text to the network.
    Sent,
    /// The network reported the handset took it.
    Delivered,
    /// The text did not arrive, in the carrier's own code and words.
    Failed {
        code: Option<String>,
        reason: Option<String>,
    },
}

impl TextDeliveryStatus {
    /// The one word the row stores.
    pub fn as_str(&self) -> &'static str {
        match self {
            TextDeliveryStatus::Queued => "queued",
            TextDeliveryStatus::Sent => "sent",
            TextDeliveryStatus::Delivered => "delivered",
            TextDeliveryStatus::Failed { .. } => "failed",
        }
    }

    /// The carrier's code for a failure, when it gave one.
    pub fn code(&self) -> Option<&str> {
        match self {
            TextDeliveryStatus::Failed { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// The carrier's words for a failure, when it gave them.
    pub fn reason(&self) -> Option<&str> {
        match self {
            TextDeliveryStatus::Failed { reason, .. } => reason.as_deref(),
            _ => None,
        }
    }

    /// The status one stored word, its code and its reason name.
    pub fn parse(state: &str, code: Option<&str>, reason: Option<&str>) -> Result<Self, String> {
        match state {
            "queued" => Ok(TextDeliveryStatus::Queued),
            "sent" => Ok(TextDeliveryStatus::Sent),
            "delivered" => Ok(TextDeliveryStatus::Delivered),
            "failed" => Ok(TextDeliveryStatus::Failed {
                code: code.map(str::to_string),
                reason: reason.map(str::to_string),
            }),
            other => Err(format!("unknown text delivery status: {other}")),
        }
    }
}

/// How long an outbound text holds its Thread for the answer
/// (ADR-0020). An inbound text lands in the Thread of the last
/// outbound text of the same pair that is younger than this; an older
/// one lands in the Agent's own Thread with the user.
pub const TEXT_THREAD_WINDOW_MS: UnixMillis = 7 * 24 * 60 * 60 * 1000;

/// One text the Agent sent or received (ADR-0020).
///
/// Pagis keeps the body, because no carrier is a mailbox to fetch
/// from later: a carrier hands the body once or lists it with a
/// retention the daemon does not control. The record stays with the
/// Agent that sent or received it whatever happens to the number, as
/// a Call does, so a new holder of the number sees none of it.
///
/// An outbound record also carries the Run, the Channel and the
/// Thread it was sent from, as [`SentMail`] does, and the delivery
/// status. There is no separate sent-texts table: this record is the
/// reply lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRecord {
    pub id: crate::TextRecordId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub phone_number_id: crate::PhoneNumberId,
    /// The other end of the conversation, in E.164.
    pub counterpart_e164: String,
    pub direction: TextDirection,
    /// What the counterpart's words are worth, from the Trust List.
    pub tier: TrustTier,
    pub body: String,
    /// How many segments the carrier billed. The Outgoing Cap counts
    /// texts and never these.
    pub segments: u32,
    /// The MMS media, fetched at ingest and stored as Artifacts.
    #[serde(default)]
    pub media_artifact_ids: Vec<ArtifactId>,
    /// The carrier's own id for the message. Ingest deduplicates on
    /// it, and the delivery poll reads it.
    pub carrier_message_id: String,
    /// The Run that sent the text. An inbound record has none.
    pub run_id: Option<RunId>,
    /// The Channel the sending Run was working in.
    pub channel_id: Option<ChannelId>,
    /// The Thread the text was sent from: the message the sending
    /// Run's conversation is rooted at. It lands the answer.
    pub thread_id: Option<MessageId>,
    /// Whether the text arrived. An inbound record has no status.
    pub delivery_status: Option<TextDeliveryStatus>,
    /// When the text was received or sent.
    pub occurred_at: UnixMillis,
}

/// One Text Conversation as the desk lists it (ADR-0020). The
/// conversation is the pair of one Agent Phone Number and one
/// counterpart number, and it is derived from the records, never
/// stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextConversation {
    pub counterpart_e164: String,
    /// When the last text of the pair was received or sent.
    pub last_at: UnixMillis,
    /// Which side sent that last text.
    pub last_direction: TextDirection,
}

/// One Software Package in a Workspace's Software List
/// (ADR-0016). The record carries what a search reads; the files of
/// each Version live in the package's own bare repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftwarePackage {
    pub id: crate::SoftwarePackageId,
    pub workspace_id: WorkspaceId,
    /// The namespace the tools take. It is unique in the Workspace.
    pub name: String,
    /// The Agent that claimed the name. Only it publishes later
    /// Versions; another Agent forks the package.
    pub author_agent_id: AgentId,
    /// The package description of the latest Version's manifest.
    pub description: String,
    /// The keywords of the latest Version's manifest.
    pub keywords: Vec<String>,
    /// The tag of the latest Version: `v1`, `v2` and so on.
    pub latest_version: String,
    /// The package this one was forked from (ADR-0016). The
    /// fork's first publish writes it, and it never changes.
    pub origin_package_id: Option<crate::SoftwarePackageId>,
    /// The Version of the origin package the fork started from.
    pub origin_version: Option<String>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

/// What one Contribution asks for and where it ended
/// (ADR-0016). It is a record and a message, not a review surface: the
/// daemon computes the patch, the author agent reads it as a message,
/// merges it in its own working copy, and closes the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    pub id: crate::ContributionId,
    pub workspace_id: WorkspaceId,
    /// The origin package the change is offered to.
    pub package_id: crate::SoftwarePackageId,
    /// The Version the patch is against: the origin Version the Fork
    /// started from, or the Fork Version the last merged Contribution
    /// carried, so a later Contribution holds only the new change.
    pub base_version: String,
    /// The latest Version of the origin package when the record
    /// opened. A base behind it tells the author to expect conflicts.
    pub latest_at_open: String,
    pub fork_package_id: crate::SoftwarePackageId,
    /// The Version of the Fork the patch carries.
    pub fork_version: String,
    /// The unified diff between the two trees, as text.
    pub patch: String,
    /// What the forker says the change does.
    pub summary: String,
    pub status: ContributionStatus,
    /// What the author said when the record closed.
    pub outcome_reason: Option<String>,
    pub created_at: UnixMillis,
    pub closed_at: Option<UnixMillis>,
    /// The Run that opened the record.
    pub run_id: RunId,
}

/// Where one Contribution stands (ADR-0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContributionStatus {
    Open,
    Merged,
    Declined,
}

impl ContributionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Declined => "declined",
        }
    }

    /// The status one word names, or `None` when it names none.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "open" => Some(Self::Open),
            "merged" => Some(Self::Merged),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }
}

/// One published Version of a Software Package (ADR-0016). It is the
/// annotated tag plus the manifest, so a run start and a search never
/// open the repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftwareVersion {
    pub package_id: crate::SoftwarePackageId,
    /// The tag the daemon minted: `v1`, `v2` and so on.
    pub version: String,
    /// What the author said about this Version. It is the tag message.
    pub notes: String,
    /// The commit the tag marks.
    pub commit_id: String,
    /// The manifest and the argument schemas, as JSON.
    pub manifest: serde_json::Value,
    pub published_at: UnixMillis,
    /// The Run that published it.
    pub run_id: RunId,
}

/// One Plugin the Workspace installed (ADR-0017). It is its own
/// record, never a Connection: it consumes Connections and secrets
/// through [`PluginBinding`] and exposes neither.
///
/// The files of every installed state live in the Plugin's own bare
/// repository, and `installed_commit` is the state that runs now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    pub id: crate::PluginId,
    pub workspace_id: WorkspaceId,
    /// The name of `plugin.json`. It is the tool namespace, so it is
    /// unique in the Workspace and collides with no other namespace.
    pub name: String,
    pub source: PluginSource,
    /// The commit of the installed state in the Plugin's repository.
    pub installed_commit: String,
    /// The Capability Manifest version this state produced: `v1`, `v2`
    /// and so on. An update mints the next one, and a Run that started
    /// before it keeps its snapshot (ADR-0005).
    pub manifest_version: String,
    pub state: PluginState,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

/// Where the files of a Plugin came from (ADR-0017). An upload keeps
/// no address: the repository holds every installed state, so an
/// update of an uploaded Plugin needs a new upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginSource {
    Git {
        url: String,
        /// The branch, tag or commit the install read. `None` takes
        /// the default branch of the remote.
        #[serde(rename = "ref")]
        reference: Option<String>,
    },
    Upload,
}

/// Where one installed Plugin stands (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Installed, bound and ready to serve the Agents that hold a
    /// Grant on it.
    Enabled,
    /// Installed but not served: a required field has no binding, or
    /// a bound Connection is gone.
    Disabled,
    /// Its servers refused to start (ADR-0017). Only the user starts
    /// it again.
    Failed,
}

impl PluginState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }

    /// The state one word names, or `None` when it names none.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// What one config field a Plugin declares is bound to
/// (ADR-0017). The binding belongs to the Workspace and the user makes
/// it at install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginBinding {
    pub plugin_id: crate::PluginId,
    /// The field name under the `pagis` extension of `plugin.json`.
    pub field: String,
    pub value: PluginBindingValue,
}

/// The three things a field can be bound to (ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginBindingValue {
    /// One Connection of the Workspace, with the capabilities the
    /// Plugin declared it needs on it. An Agent that calls the Plugin
    /// must hold a Grant on the Connection with these capabilities.
    Connection {
        connection_id: crate::ConnectionId,
        capabilities: Vec<String>,
    },
    /// One secret in the daemon's secret store (ADR-0013). The record
    /// holds the name and never the value.
    Secret { secret_name: String },
    /// A plain `string`, `number` or `boolean` the user typed. It is
    /// package configuration, not a credential.
    Value { value: serde_json::Value },
}

/// One Agent Mailbox: the Agent's own email endpoint, and everything
/// its life needs (ADR-0019).
///
/// The record owns the address, the Mailbox Provider Connection, the
/// state and its reason, the Outgoing Cap, the collector cursor and the
/// two times. The Agent's address in the API is a read-through view of
/// this record: the Agent row holds no address of its own.
///
/// A deleted mailbox keeps its row as a tombstone. The rows together
/// are the Address Ledger, so an address Pagis ever made or assigned is
/// never given out a second time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMailbox {
    pub id: crate::AgentMailboxId,
    pub workspace_id: WorkspaceId,
    /// The Agent that holds the mailbox. It stays on the tombstone, so
    /// the ledger says who held the address.
    pub agent_id: AgentId,
    /// The Mailbox Provider Connection the mailbox lives on. There is
    /// no foreign key: a tombstone outlives the Connection the user
    /// later removes.
    pub connection_id: crate::ConnectionId,
    /// The full address, lowercased. It never changes.
    pub address: String,
    pub state: AgentMailboxState,
    /// Why the mailbox is `unavailable`, in the words the desk shows.
    pub reason: Option<String>,
    /// The most messages a day this mailbox may send.
    pub outgoing_cap: u32,
    /// The Mail Recipient Domain allow rules the user wrote from an
    /// approval card (ADR-0019). A send whose recipients all sit on
    /// these registrable domains runs with no card. The rules live on
    /// the record and not in a Grant, because the Agent holds its own
    /// mailbox with no Grant.
    #[serde(default)]
    pub allow_rules: Vec<String>,
    /// Where the collector reached. It is set when the login is first
    /// proven, so mail that arrived before then wakes nobody.
    pub cursor: Option<MailboxCursor>,
    /// The start of the day the send tally counts, or `None` while the
    /// mailbox has sent nothing.
    pub sends_day: Option<UnixMillis>,
    /// How many messages the mailbox sent on `sends_day`.
    pub sends_today: u32,
    pub created_at: UnixMillis,
    pub deleted_at: Option<UnixMillis>,
}

impl AgentMailbox {
    /// How many messages the mailbox has sent on one day. A tally from
    /// an earlier day counts for nothing.
    pub fn sends_on(&self, day: UnixMillis) -> u32 {
        match self.sends_day {
            Some(sends_day) if sends_day == day => self.sends_today,
            _ => 0,
        }
    }
}

/// The start of the UTC day one instant falls in. The Outgoing Cap is
/// a day's allowance, so the tally resets on this boundary.
pub fn day_start(at: UnixMillis) -> UnixMillis {
    const DAY: UnixMillis = 24 * 60 * 60 * 1000;
    at - at.rem_euclid(DAY)
}

/// Where a collector reached in one folder. UIDVALIDITY is part of it,
/// because a host that renumbers a folder invalidates every UID in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxCursor {
    pub folder: String,
    pub uid_validity: u32,
    /// The highest UID the collector has seen.
    pub last_uid: u32,
}

/// One message an Agent Mailbox sent, and the Thread the Run that
/// sent it was working in.
///
/// A reply comes back with the sent `Message-ID` in its `In-Reply-To`,
/// so this record is what lands the answer in the Thread that asked
/// for it. It holds no body: Pagis keeps no copy of message bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentMail {
    /// The `Message-ID` the send carried, in its header form.
    pub message_id: String,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub mailbox_id: crate::AgentMailboxId,
    pub run_id: RunId,
    /// The Channel the Run was working in.
    pub channel_id: Option<ChannelId>,
    /// The Thread: the message the Run's conversation is rooted at.
    /// `None` when the Run was not in a Thread.
    pub thread_id: Option<MessageId>,
    pub sent_at: UnixMillis,
}

/// Where one Agent Mailbox stands (ADR-0019).
///
/// `Provisioning` and `Active` are the working states, `Unavailable`
/// asks the user for a password reset, `Dormant` holds the mail of an
/// archived Agent, and `Deleted` is the tombstone that keeps the
/// address out of every later mailbox's reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMailboxState {
    /// The address is reserved and the host create succeeded. The
    /// first login is not yet proven.
    Provisioning,
    /// The login is proven: the tools work and IDLE runs.
    Active,
    /// The host refused the login or reports the mailbox gone. A
    /// password reset is the way back.
    Unavailable,
    /// The holding Agent is archived. The mail stays, nothing wakes
    /// and nothing sends.
    Dormant,
    /// A tombstone. The password secret and the host mailbox are gone;
    /// the address, the Agent and the time stay.
    Deleted,
}

impl AgentMailboxState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMailboxState::Provisioning => "provisioning",
            AgentMailboxState::Active => "active",
            AgentMailboxState::Unavailable => "unavailable",
            AgentMailboxState::Dormant => "dormant",
            AgentMailboxState::Deleted => "deleted",
        }
    }

    /// Whether the mailbox still points at its Connection. Every state
    /// but the tombstone does.
    pub fn is_live(self) -> bool {
        self != AgentMailboxState::Deleted
    }
}

impl std::str::FromStr for AgentMailboxState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provisioning" => Ok(AgentMailboxState::Provisioning),
            "active" => Ok(AgentMailboxState::Active),
            "unavailable" => Ok(AgentMailboxState::Unavailable),
            "dormant" => Ok(AgentMailboxState::Dormant),
            "deleted" => Ok(AgentMailboxState::Deleted),
            other => Err(format!("unknown agent mailbox state: {other}")),
        }
    }
}

/// The frozen tool catalog of one installed state of a Plugin
/// (ADR-0017). An install starts each declared server once, takes
/// `tools/list`, and writes the answer here with the Capability
/// Manifest version it produced. The daemon reads the catalog back at
/// every boot, so a server never has to run for the broker to know
/// which tools a Plugin offers.
///
/// The record is immutable except for [`PluginTools::tools_changed`]:
/// a later list is a proposal the user accepts through the update
/// flow, and never a change to the state that is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTools {
    pub plugin_id: crate::PluginId,
    /// The Capability Manifest version: `v1`, `v2` and so on.
    pub version: String,
    /// The commit of the installed state the freeze read.
    pub installed_commit: String,
    pub tools: Vec<PluginTool>,
    /// Whether a server has offered a different list since the freeze.
    /// The desk shows it; dispatch keeps refusing every tool outside
    /// the frozen list either way.
    pub tools_changed: bool,
    pub created_at: UnixMillis,
}

impl PluginTools {
    /// The frozen tool of one bare name, or `None` when the list does
    /// not carry it. A tool outside the list is refused at dispatch.
    pub fn tool(&self, name: &str) -> Option<&PluginTool> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

/// One tool a Plugin's server offered at the freeze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTool {
    /// The `mcp.json` name of the server that offers it.
    pub server: String,
    /// The bare name the server calls it, without the namespace.
    pub name: String,
    pub description: String,
    /// The argument schema, as the server declared it.
    pub schema: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_at_sign_makes_an_address_and_the_list_holds_one_form() {
        assert_eq!(
            parse_trust_subject("  Clinic@Example.COM "),
            Ok((TrustSubject::Address, "clinic@example.com".to_string()))
        );
    }

    #[test]
    fn a_leading_plus_makes_a_number() {
        assert_eq!(
            parse_trust_subject(" +14155550123 "),
            Ok((TrustSubject::Number, "+14155550123".to_string()))
        );
    }

    #[test]
    fn everything_else_is_a_domain() {
        assert_eq!(
            parse_trust_subject("Example.com"),
            Ok((TrustSubject::Domain, "example.com".to_string()))
        );
    }

    #[test]
    fn a_subject_that_reads_as_none_of_the_three_is_refused() {
        for value in ["@example.com", "+1", "example", "a b.com", "a@b", "a@.com"] {
            assert!(parse_trust_subject(value).is_err(), "{value} was accepted");
        }
    }

    #[test]
    fn a_listed_tier_is_owner_or_trusted() {
        assert_eq!(TrustTier::listed("owner"), Some(TrustTier::Owner));
        assert_eq!(TrustTier::listed("trusted"), Some(TrustTier::Trusted));
        assert_eq!(TrustTier::listed("unknown"), None);
    }

    #[test]
    fn a_subject_name_reads_back() {
        for subject in [
            TrustSubject::Number,
            TrustSubject::Address,
            TrustSubject::Domain,
        ] {
            assert_eq!(TrustSubject::parse(subject.as_str()), Some(subject));
        }
    }

    fn agent_with_job(job: &str) -> Agent {
        Agent {
            id: AgentId::generate(),
            workspace_id: WorkspaceId::generate(),
            name: "Pixie".to_string(),
            job: job.to_string(),
            description: String::new(),
            personality: String::new(),
            model_alias: "default".to_string(),
            avatar: Default::default(),
            voice: None,
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: 1,
            updated_at: 1,
        }
    }

    // The greeting names the job as the Product App shows it, in a
    // full sentence.
    #[test]
    fn the_greeting_names_the_job_in_a_full_sentence() {
        let greeting = agent_with_job("general assistant").greeting(ChannelId::generate());
        assert_eq!(
            greeting.text_content,
            "Hi, I'm Pixie, your general assistant."
        );
    }

    #[test]
    fn an_agent_with_no_job_gives_its_name_alone() {
        let greeting = agent_with_job("").greeting(ChannelId::generate());
        assert_eq!(greeting.text_content, "Hi, I'm Pixie.");
    }

    fn host_grant_with_scope(scope: serde_json::Value) -> Grant {
        Grant {
            id: GrantId::generate(),
            workspace_id: WorkspaceId::from("w".to_string()),
            agent_id: AgentId::from("a".to_string()),
            resource_kind: Grant::HOST_KIND.to_string(),
            resource_id: Some("h".to_string()),
            scope,
            revision: 1,
            created_at: 1,
            revoked_at: None,
        }
    }

    #[test]
    fn a_host_grant_reads_each_session_approval_mode() {
        for mode in [
            SessionApprovalMode::Person,
            SessionApprovalMode::Agent,
            SessionApprovalMode::Auto,
        ] {
            let grant = host_grant_with_scope(
                serde_json::json!({"allow": [], "session_approval_mode": mode.as_str()}),
            );
            assert_eq!(grant.session_approval_mode(), mode);
        }
    }

    #[test]
    fn an_absent_unknown_or_malformed_mode_reads_as_person() {
        for scope in [
            serde_json::json!({"allow": ["echo"]}),
            serde_json::json!({"allow": [], "session_approval_mode": "everything"}),
            serde_json::json!({"allow": [], "session_approval_mode": 2}),
            serde_json::json!(["not", "an", "object"]),
            serde_json::Value::Null,
        ] {
            assert_eq!(
                host_grant_with_scope(scope.clone()).session_approval_mode(),
                SessionApprovalMode::Person,
                "{scope}"
            );
        }
    }

    #[test]
    fn new_allow_rules_keep_the_session_approval_mode() {
        let grant = host_grant_with_scope(
            serde_json::json!({"allow": ["echo"], "session_approval_mode": "agent"}),
        );

        let scope = grant.with_allow_rules(&["git status".to_string()]);

        assert_eq!(
            scope,
            serde_json::json!({"allow": ["git status"], "session_approval_mode": "agent"})
        );
    }

    #[test]
    fn a_new_session_approval_mode_keeps_the_allow_rules() {
        let grant = host_grant_with_scope(serde_json::json!({"allow": ["echo"]}));

        let scope = grant.with_session_approval_mode(SessionApprovalMode::Auto);

        assert_eq!(
            scope,
            serde_json::json!({"allow": ["echo"], "session_approval_mode": "auto"})
        );
    }

    fn session_allow_rule() -> SessionAllowRule {
        SessionAllowRule::new("claude", "/work/pagis").expect("a valid rule")
    }

    #[test]
    fn new_session_allow_rules_keep_the_allow_rules_and_the_mode() {
        let grant = host_grant_with_scope(
            serde_json::json!({"allow": ["echo"], "session_approval_mode": "agent"}),
        );

        let scope = grant.with_session_allow_rules(&[session_allow_rule()]);

        assert_eq!(
            scope,
            serde_json::json!({
                "allow": ["echo"],
                "session_approval_mode": "agent",
                "sessions": [{"harness": "claude", "directory": "/work/pagis"}],
            })
        );
    }

    #[test]
    fn new_allow_rules_keep_the_session_allow_rules() {
        let grant = host_grant_with_scope(serde_json::json!({
            "allow": ["echo"],
            "sessions": [{"harness": "claude", "directory": "/work/pagis"}],
        }));

        let scope = grant.with_allow_rules(&["git status".to_string()]);

        assert_eq!(
            host_grant_with_scope(scope).session_allow_rules(),
            vec![session_allow_rule()]
        );
    }

    #[test]
    fn a_malformed_session_allow_rule_reads_as_none() {
        let grant = host_grant_with_scope(serde_json::json!({
            "sessions": [
                {"harness": "claude"},
                "claude in /work",
                {"harness": "claude", "directory": "/work/pagis"},
            ],
        }));

        assert_eq!(grant.session_allow_rules(), vec![session_allow_rule()]);
        assert!(
            host_grant_with_scope(serde_json::json!({"allow": []}))
                .session_allow_rules()
                .is_empty()
        );
    }

    #[test]
    fn a_field_written_on_a_malformed_scope_makes_an_object() {
        let grant = host_grant_with_scope(serde_json::json!(["not", "an", "object"]));

        assert_eq!(
            grant.with_session_approval_mode(SessionApprovalMode::Agent),
            serde_json::json!({"session_approval_mode": "agent"})
        );
    }
}
