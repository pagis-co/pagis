//! Durable proactive trigger routing (ADR-0006).
//!
//! One module sits between provider acquisition and the agent system.
//! A collector stops at [`Trigger::ingest`]; the agent system starts at
//! [`Trigger::claim_wakeups`]. Neither provider transport nor Run
//! execution crosses this seam.

pub mod subscriptions;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{LocalResult, NaiveDateTime, TimeZone};
use pagis_core::{
    AgentId, AgentStatus, AgentStore, BlockReason, ChannelId, ChannelStore, CollectorTarget,
    Connection, ConnectionId, ConnectionStore, CreatorKind, EventBus, EventCatalog,
    EventDeclaration, EventMatcher, EventSubscription, EventSubscriptionId, EventSubscriptionState,
    EventSubscriptionStore, ForgetKeys, Grant, GrantStore, IncomingEvent, IncomingEventId,
    IngestBatch, IngestOutcome, MessageId, MessageStore, NewEvent, Schedule, ScheduleId,
    ScheduleKind, ScheduleOccurrence, ScheduleOccurrenceId, ScheduleRevision, ScheduleState,
    ScheduleStore, SourceBatch, StoreError, TriggerStore, Wakeup, WakeupClaim, WakeupId,
    WorkspaceId,
};

pub use pagis_core::ArrivalRun;

pub use subscriptions::{NewSubscription, SubscriptionAction};

/// The namespaces Pagis owns. A subscription never names one: an
/// internal audit Event is not a provider occurrence (ADR-0006).
const NATIVE_NAMESPACES: [&str; 3] = ["core", "ui", "vault"];
const SUBJECT_COOLDOWN_MS: i64 = 14 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone)]
pub struct NewOneShot {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub local_time: String,
    pub timezone: String,
    pub now: i64,
}

#[derive(Debug, Clone)]
pub struct NewInterval {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub every_ms: i64,
    pub anchor: i64,
    pub now: i64,
}

#[derive(Debug, Clone)]
pub struct NewCron {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub expression: String,
    pub timezone: String,
    pub now: i64,
}

#[derive(Debug, Clone)]
pub enum ScheduleTiming {
    OneShot {
        local_time: String,
        timezone: String,
    },
    Cron {
        expression: String,
        timezone: String,
    },
    Interval {
        every_ms: i64,
        anchor: i64,
    },
}

#[derive(Debug, Clone)]
pub struct ScheduleEdit {
    pub expected_revision: u32,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    /// The Thread. `None` keeps the current Thread while the Channel
    /// stays the same, and drops it when the Channel changes.
    /// `Some(None)` clears the Thread.
    pub root_message_id: Option<Option<MessageId>>,
    pub timing: ScheduleTiming,
}

#[derive(Debug, Clone)]
pub struct NewSchedule {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub name: String,
    pub instruction: String,
    pub subject_page_path: Option<String>,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub timing: ScheduleTiming,
    pub creator: CreatorKind,
    pub creating_run_id: Option<pagis_core::RunId>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessDueResult {
    pub created_occurrences: usize,
    pub created_wakeups: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("no connection named `{0}`")]
    ConnectionNotFound(String),
    #[error("`{0}` is not an incoming event Pagis can subscribe to")]
    UnknownEventKind(String),
    #[error("`{0}` is a Pagis event, not a provider occurrence")]
    NativeEventKind(String),
    #[error("the {0} connection does not supply `{1}`")]
    EventKindNotOnConnection(String, String),
    #[error("no matcher is registered for `{0}`")]
    UnknownMatcher(String),
    #[error("the filter does not match the declared schema: {0}")]
    InvalidFilter(String),
    #[error("event subscription not found")]
    SubscriptionNotFound,
    #[error("an archived subscription cannot change")]
    SubscriptionArchived,
    #[error("this agent has no live grant on that connection")]
    GrantMissing,
    #[error("invalid IANA timezone `{0}`")]
    InvalidTimezone(String),
    #[error("invalid local time; use YYYY-MM-DDTHH:MM:SS")]
    InvalidLocalTime,
    #[error("local time does not exist in this timezone")]
    NonexistentLocalTime,
    #[error("local time is ambiguous in this timezone")]
    AmbiguousLocalTime,
    #[error("scheduled time must be in the future")]
    PastInstant,
    #[error("interval must be at least one minute")]
    IntervalTooShort,
    #[error("invalid five-field cron expression")]
    InvalidCron,
    #[error("Schedule cannot resume because its one-shot time has passed")]
    CannotResume,
    #[error("name must not be empty")]
    EmptyName,
    #[error("instruction must not be empty")]
    EmptyInstruction,
    #[error("agent not found")]
    AgentNotFound,
    #[error("channel not found")]
    ChannelNotFound,
    #[error("thread root not found")]
    ThreadRootNotFound,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The Trigger module keeps schedule timing, occurrence creation, Wake-up
/// delivery, and Run creation behind one interface.
pub struct Trigger {
    schedules: Arc<dyn ScheduleStore>,
    store: Arc<dyn TriggerStore>,
    subscriptions: Arc<dyn EventSubscriptionStore>,
    connections: Arc<dyn ConnectionStore>,
    agents: Arc<dyn AgentStore>,
    channels: Arc<dyn ChannelStore>,
    messages: Arc<dyn MessageStore>,
    org_workspace_id: WorkspaceId,
    grants: Arc<dyn GrantStore>,
    /// The installed Incoming Event declarations.
    catalog: Arc<dyn EventCatalog>,
    /// The trusted matchers, by the name a declaration gives.
    matchers: HashMap<String, Arc<dyn EventMatcher>>,
    events: Arc<dyn EventBus>,
    /// The source of the suppression key that blocks an Incoming Event
    /// of a forgotten Source Item.
    forget_keys: Arc<dyn ForgetKeys>,
    changed: Arc<tokio::sync::Notify>,
}

/// Everything the Trigger module reads and writes. It is one struct so
/// the module keeps one constructor as it grows.
pub struct TriggerDeps {
    pub schedules: Arc<dyn ScheduleStore>,
    pub store: Arc<dyn TriggerStore>,
    pub subscriptions: Arc<dyn EventSubscriptionStore>,
    pub connections: Arc<dyn ConnectionStore>,
    /// The target of a rule: its Agent, its Channel and its Thread
    /// root. The Trigger module only reads them.
    pub agents: Arc<dyn AgentStore>,
    pub channels: Arc<dyn ChannelStore>,
    pub messages: Arc<dyn MessageStore>,
    /// The Org's Workspace, which holds the Installation Connections. A
    /// rule on an Agent's own number or mailbox names the Org's carrier
    /// or mail domain.
    pub org_workspace_id: WorkspaceId,
    pub grants: Arc<dyn GrantStore>,
    pub catalog: Arc<dyn EventCatalog>,
    pub matchers: HashMap<String, Arc<dyn EventMatcher>>,
    pub events: Arc<dyn EventBus>,
    /// The source of the suppression key that blocks an Incoming Event
    /// of a forgotten Source Item (ADR-0008).
    pub forget_keys: Arc<dyn ForgetKeys>,
}

impl Trigger {
    pub fn new(deps: TriggerDeps) -> Self {
        Self {
            schedules: deps.schedules,
            store: deps.store,
            subscriptions: deps.subscriptions,
            connections: deps.connections,
            agents: deps.agents,
            channels: deps.channels,
            messages: deps.messages,
            org_workspace_id: deps.org_workspace_id,
            grants: deps.grants,
            catalog: deps.catalog,
            matchers: deps.matchers,
            events: deps.events,
            forget_keys: deps.forget_keys,
            changed: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// The Connection a rule of one Workspace names: one of the
    /// Workspace's own, or an Installation Connection of the Org, which
    /// every person's Agents use for their own numbers and mailboxes.
    async fn connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        if let Some(connection) = self.connections.get(workspace_id, connection_id).await? {
            return Ok(Some(connection));
        }
        self.connections
            .get(&self.org_workspace_id, connection_id)
            .await
    }

    /// The one check of the target of a Schedule or an Event
    /// Subscription. Every create and every edit of both rules calls it
    /// (ADR-0006). In the rule's Workspace, the Agent must exist and be
    /// Active, the Channel must exist, and a Thread root must be a
    /// top-level message of that Channel. An id of another Workspace
    /// gets the same error as an id that names nothing, so a caller
    /// cannot find the ids of another Workspace.
    async fn check_target(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
    ) -> Result<(), TriggerError> {
        self.agents
            .get(workspace_id, agent_id)
            .await?
            .filter(|agent| agent.status == AgentStatus::Active)
            .ok_or(TriggerError::AgentNotFound)?;
        self.channels
            .get(workspace_id, channel_id)
            .await?
            .ok_or(TriggerError::ChannelNotFound)?;
        if let Some(root_message_id) = root_message_id {
            self.messages
                .get(workspace_id, root_message_id)
                .await?
                .filter(|root| root.channel_id == *channel_id && root.parent_message_id.is_none())
                .ok_or(TriggerError::ThreadRootNotFound)?;
        }
        Ok(())
    }

    pub async fn create_schedule(&self, input: NewSchedule) -> Result<Schedule, TriggerError> {
        self.create_schedule_identified(input, ScheduleId::generate())
            .await
    }

    async fn create_schedule_identified(
        &self,
        input: NewSchedule,
        id: ScheduleId,
    ) -> Result<Schedule, TriggerError> {
        if input.name.trim().is_empty() {
            return Err(TriggerError::EmptyName);
        }
        if input.instruction.trim().is_empty() {
            return Err(TriggerError::EmptyInstruction);
        }
        self.check_target(
            &input.workspace_id,
            &input.agent_id,
            &input.channel_id,
            input.root_message_id.as_ref(),
        )
        .await?;
        let resolved = resolve_timing(input.timing, input.now)?;
        if let Some(path) = input.subject_page_path.as_deref() {
            self.ensure_subject_cooldown(
                &input.workspace_id,
                &input.agent_id,
                path,
                resolved.scheduled_at,
            )
            .await?;
        }
        let approved_revision = input.subject_page_path.is_none().then_some(1);
        let schedule = Schedule {
            id,
            workspace_id: input.workspace_id,
            agent_id: input.agent_id,
            name: input.name.trim().to_string(),
            instruction: input.instruction.trim().to_string(),
            subject_page_path: input.subject_page_path,
            channel_id: input.channel_id,
            root_message_id: input.root_message_id,
            kind: resolved.kind,
            cron_expression: resolved.cron_expression,
            interval_ms: resolved.interval_ms,
            anchor_at: resolved.anchor_at,
            timezone: resolved.timezone,
            scheduled_at: resolved.scheduled_at,
            next_due_at: Some(resolved.scheduled_at),
            last_result: None,
            state: ScheduleState::Active,
            revision: 1,
            approved_revision,
            creator: input.creator,
            creating_run_id: input.creating_run_id,
            created_at: input.now,
            updated_at: input.now,
            archived_at: None,
        };
        self.schedules.create(&schedule).await?;
        self.changed.notify_one();
        self.publish(
            "schedule.created",
            &schedule.workspace_id,
            Some(&schedule.agent_id),
            Some(&schedule.channel_id),
            serde_json::json!({
                "schedule_id": schedule.id.as_str(),
                "state": schedule.state.as_str(),
                "next_due_at": schedule.next_due_at,
                "creator": schedule.creator.as_str(),
                "name": schedule.name,
                "instruction": schedule.instruction,
                "subject_page_path": schedule.subject_page_path,
                "run_id": schedule.creating_run_id,
            }),
        )
        .await?;
        Ok(schedule)
    }

    pub async fn create_one_shot(&self, input: NewOneShot) -> Result<Schedule, TriggerError> {
        self.create_schedule(NewSchedule {
            workspace_id: input.workspace_id,
            agent_id: input.agent_id,
            name: input.name,
            instruction: input.instruction,
            subject_page_path: None,
            channel_id: input.channel_id,
            root_message_id: input.root_message_id,
            timing: ScheduleTiming::OneShot {
                local_time: input.local_time,
                timezone: input.timezone,
            },
            creator: CreatorKind::User,
            creating_run_id: None,
            now: input.now,
        })
        .await
    }

    pub async fn create_interval(&self, input: NewInterval) -> Result<Schedule, TriggerError> {
        self.create_schedule(NewSchedule {
            workspace_id: input.workspace_id,
            agent_id: input.agent_id,
            name: input.name,
            instruction: input.instruction,
            subject_page_path: None,
            channel_id: input.channel_id,
            root_message_id: input.root_message_id,
            timing: ScheduleTiming::Interval {
                every_ms: input.every_ms,
                anchor: input.anchor,
            },
            creator: CreatorKind::User,
            creating_run_id: None,
            now: input.now,
        })
        .await
    }

    pub async fn create_cron(&self, input: NewCron) -> Result<Schedule, TriggerError> {
        self.create_schedule(NewSchedule {
            workspace_id: input.workspace_id,
            agent_id: input.agent_id,
            name: input.name,
            instruction: input.instruction,
            subject_page_path: None,
            channel_id: input.channel_id,
            root_message_id: input.root_message_id,
            timing: ScheduleTiming::Cron {
                expression: input.expression,
                timezone: input.timezone,
            },
            creator: CreatorKind::User,
            creating_run_id: None,
            now: input.now,
        })
        .await
    }

    pub async fn get_schedule(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Option<Schedule>, TriggerError> {
        Ok(self.schedules.get(workspace_id, id).await?)
    }

    pub async fn edit_schedule(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        edit: ScheduleEdit,
        at: i64,
    ) -> Result<Option<Schedule>, TriggerError> {
        self.edit_schedule_state(workspace_id, id, edit, at).await
    }

    async fn edit_schedule_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        edit: ScheduleEdit,
        at: i64,
    ) -> Result<Option<Schedule>, TriggerError> {
        if edit.name.trim().is_empty() {
            return Err(TriggerError::EmptyName);
        }
        if edit.instruction.trim().is_empty() {
            return Err(TriggerError::EmptyInstruction);
        }
        let Some(previous) = self.schedules.get(workspace_id, id).await? else {
            return Ok(None);
        };
        if previous.revision != edit.expected_revision {
            return Err(StoreError::Conflict("Schedule revision changed".to_string()).into());
        }
        let root_message_id = match edit.root_message_id {
            Some(root_message_id) => root_message_id,
            None => kept_thread(
                &previous.channel_id,
                &previous.root_message_id,
                &edit.channel_id,
            ),
        };
        self.check_target(
            workspace_id,
            &edit.agent_id,
            &edit.channel_id,
            root_message_id.as_ref(),
        )
        .await?;
        let pending = self.pending_schedule_wakeups(workspace_id, id).await?;
        let resolved = resolve_timing(edit.timing, at)?;
        if let Some(path) = previous.subject_page_path.as_deref() {
            self.ensure_subject_cooldown(
                &previous.workspace_id,
                &previous.agent_id,
                path,
                resolved.scheduled_at,
            )
            .await?;
        }
        let replacement = Schedule {
            agent_id: edit.agent_id,
            name: edit.name.trim().to_string(),
            instruction: edit.instruction.trim().to_string(),
            channel_id: edit.channel_id,
            root_message_id,
            kind: resolved.kind,
            cron_expression: resolved.cron_expression,
            interval_ms: resolved.interval_ms,
            anchor_at: resolved.anchor_at,
            timezone: resolved.timezone,
            scheduled_at: resolved.scheduled_at,
            next_due_at: Some(resolved.scheduled_at),
            state: ScheduleState::Active,
            revision: previous.revision.saturating_add(1),
            approved_revision: previous
                .subject_page_path
                .is_none()
                .then_some(previous.revision.saturating_add(1)),
            updated_at: at,
            archived_at: None,
            ..previous.clone()
        };
        let schedule = self.schedules.edit(&previous, &replacement).await?;
        self.changed.notify_one();
        if let Some(schedule) = &schedule {
            self.publish_withdrawn(pending).await?;
            self.publish(
                "schedule.updated",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                }),
            )
            .await?;
        }
        Ok(schedule)
    }

    async fn ensure_subject_cooldown(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        path: &str,
        scheduled_at: i64,
    ) -> Result<(), TriggerError> {
        let schedules = self.schedules.list(workspace_id, None, 1_000).await?;
        let mut last_fired = None;
        for schedule in schedules.into_iter().filter(|schedule| {
            &schedule.agent_id == agent_id && schedule.subject_page_path.as_deref() == Some(path)
        }) {
            if let Some(occurrence) = self
                .schedules
                .list_occurrences(workspace_id, &schedule.id, None, 1)
                .await?
                .first()
            {
                last_fired = last_fired.max(Some(occurrence.scheduled_at));
            }
        }
        if let Some(available_at) = last_fired.map(|fired| fired + SUBJECT_COOLDOWN_MS)
            && scheduled_at < available_at
        {
            let reason = format!(
                "the Subject Page has a 14-day cooldown; the next Schedule can start at {available_at}"
            );
            tracing::warn!(subject_page_path = path, %reason, "Subject Schedule reschedule refused");
            return Err(StoreError::Conflict(reason).into());
        }
        Ok(())
    }

    pub async fn list_revisions(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Vec<ScheduleRevision>, TriggerError> {
        Ok(self.schedules.list_revisions(workspace_id, id).await?)
    }

    pub async fn list_schedules(
        &self,
        workspace_id: &WorkspaceId,
        before: Option<&ScheduleId>,
        limit: u32,
    ) -> Result<Vec<Schedule>, TriggerError> {
        Ok(self.schedules.list(workspace_id, before, limit).await?)
    }

    pub async fn list_open_for_subject_page(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<Vec<pagis_core::subject_page::OpenSchedule>, TriggerError> {
        Ok(self
            .schedules
            .list_open_for_subject_page(workspace_id, agent_id, path)
            .await?)
    }

    pub async fn archive_schedule(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: i64,
    ) -> Result<Option<Schedule>, TriggerError> {
        let current = self.schedules.get(workspace_id, id).await?;
        let was_archived = current
            .as_ref()
            .is_some_and(|schedule| schedule.state == ScheduleState::Archived);
        let pending = self.pending_schedule_wakeups(workspace_id, id).await?;
        let schedule = self.schedules.archive(workspace_id, id, at).await?;
        self.changed.notify_one();
        if !was_archived && let Some(schedule) = &schedule {
            self.publish_withdrawn(pending).await?;
            self.publish(
                "schedule.updated",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                }),
            )
            .await?;
            self.publish(
                "schedule.state_changed",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                }),
            )
            .await?;
        }
        Ok(schedule)
    }

    pub async fn pause_schedule(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: i64,
    ) -> Result<Option<Schedule>, TriggerError> {
        let pending = self.pending_schedule_wakeups(workspace_id, id).await?;
        let schedule = self.schedules.pause(workspace_id, id, at).await?;
        self.changed.notify_one();
        if let Some(schedule) = &schedule {
            self.publish_withdrawn(pending).await?;
            self.publish(
                "schedule.updated",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                }),
            )
            .await?;
            self.publish(
                "schedule.state_changed",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                }),
            )
            .await?;
        }
        Ok(schedule)
    }

    pub async fn resume_schedule(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: i64,
    ) -> Result<Option<Schedule>, TriggerError> {
        let Some(schedule) = self.schedules.get(workspace_id, id).await? else {
            return Ok(None);
        };
        let next_due_at = next_after(&schedule, at)?.ok_or(TriggerError::CannotResume)?;
        let schedule = self
            .schedules
            .resume(workspace_id, id, next_due_at, at)
            .await?;
        self.changed.notify_one();
        if let Some(schedule) = &schedule {
            self.publish(
                "schedule.updated",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                    "next_due_at": schedule.next_due_at,
                }),
            )
            .await?;
            self.publish(
                "schedule.state_changed",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                serde_json::json!({
                    "schedule_id": schedule.id.as_str(),
                    "revision": schedule.revision,
                    "state": schedule.state.as_str(),
                    "next_due_at": schedule.next_due_at,
                }),
            )
            .await?;
        }
        Ok(schedule)
    }

    pub async fn skip_next(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        expected_due_at: i64,
        at: i64,
    ) -> Result<ScheduleOccurrence, TriggerError> {
        let schedule = self
            .schedules
            .get(workspace_id, id)
            .await?
            .ok_or_else(|| StoreError::Conflict("Schedule not found".to_string()))?;
        if schedule.next_due_at != Some(expected_due_at) {
            return Err(StoreError::Conflict(
                "the Schedule became due before skip-next completed".to_string(),
            )
            .into());
        }
        let next_due_at = next_after(&schedule, expected_due_at)?;
        let occurrence = self
            .schedules
            .skip_next(&schedule, expected_due_at, next_due_at, at)
            .await?;
        self.changed.notify_one();
        self.publish(
            "schedule.occurrence_recorded",
            &schedule.workspace_id,
            Some(&schedule.agent_id),
            Some(&schedule.channel_id),
            serde_json::json!({
                "schedule_id": schedule.id.as_str(),
                "occurrence_id": occurrence.id.as_str(),
                "scheduled_at": occurrence.scheduled_at,
            }),
        )
        .await?;
        Ok(occurrence)
    }

    pub async fn list_occurrences(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&ScheduleOccurrenceId>,
        limit: u32,
    ) -> Result<Vec<ScheduleOccurrence>, TriggerError> {
        Ok(self
            .schedules
            .list_occurrences(workspace_id, schedule_id, before, limit)
            .await?)
    }

    pub async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, TriggerError> {
        Ok(self
            .schedules
            .list_wakeups(workspace_id, schedule_id, before, limit)
            .await?)
    }

    pub async fn next_due_at(&self) -> Result<Option<i64>, TriggerError> {
        Ok(self.store.next_due_at().await?)
    }

    pub async fn pending_agents(&self) -> Result<Vec<(WorkspaceId, AgentId)>, TriggerError> {
        Ok(self.store.pending_agents().await?)
    }

    pub async fn changed(&self) {
        self.changed.notified().await;
    }

    pub async fn process_due(&self, now: i64) -> Result<ProcessDueResult, TriggerError> {
        let batch = self.store.process_due(now).await?;
        for wakeup in &batch.withdrawn_wakeups {
            self.publish_wakeup("wakeup.withdrawn", wakeup).await?;
        }
        for schedule in &batch.blocked_schedules {
            let payload = serde_json::json!({
                "schedule_id": schedule.id.as_str(),
                "revision": schedule.revision,
                "state": schedule.state.as_str(),
                "reason": "agent_unavailable",
            });
            self.publish(
                "schedule.updated",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                payload.clone(),
            )
            .await?;
            self.publish(
                "schedule.state_changed",
                &schedule.workspace_id,
                Some(&schedule.agent_id),
                Some(&schedule.channel_id),
                payload,
            )
            .await?;
        }
        for occurrence in &batch.occurrences {
            self.publish(
                "schedule.occurrence_recorded",
                &occurrence.workspace_id,
                None,
                None,
                serde_json::json!({
                    "occurrence_id": occurrence.id.as_str(),
                    "schedule_id": occurrence.schedule_id.as_str(),
                    "schedule_revision": occurrence.schedule_revision,
                    "scheduled_at": occurrence.scheduled_at,
                    "processed_at": occurrence.processed_at,
                    "outcome": occurrence.outcome,
                }),
            )
            .await?;
            if occurrence.outcome == "combined"
                && let Some(wakeup_id) = &occurrence.wakeup_id
                && let Some(wakeup) = self
                    .store
                    .get_wakeup(&occurrence.workspace_id, wakeup_id)
                    .await?
            {
                self.publish_wakeup("wakeup.combined", &wakeup).await?;
            }
        }
        for wakeup in &batch.wakeups {
            self.publish_wakeup("wakeup.created", wakeup).await?;
        }
        Ok(ProcessDueResult {
            created_occurrences: batch.occurrences.len(),
            created_wakeups: batch.wakeups.len(),
        })
    }

    /// Wake one Schedule at once, outside its cadence (ADR-0022). The
    /// Schedule keeps its own next time, so the daily Report still
    /// arrives in the morning. A Schedule that already has a Wake-up
    /// waiting returns that one: the work the user asked for is
    /// already on its way.
    pub async fn run_now(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        now: i64,
    ) -> Result<Option<Wakeup>, TriggerError> {
        let Some(wakeup) = self.store.wake_now(workspace_id, schedule_id, now).await? else {
            return Ok(None);
        };
        self.publish_wakeup("wakeup.created", &wakeup).await?;
        self.changed.notify_one();
        Ok(Some(wakeup))
    }

    pub async fn claim_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        slots: pagis_core::RunSlots,
        now: i64,
    ) -> Result<Vec<WakeupClaim>, TriggerError> {
        let claims = self
            .store
            .claim_wakeups(workspace_id, agent_id, slots, now)
            .await?;
        for claim in &claims {
            self.publish_wakeup("wakeup.started", &claim.wakeup).await?;
            self.events
                .publish(NewEvent {
                    workspace_id: claim.run.workspace_id.clone(),
                    event_type: "run.created".to_string(),
                    agent_id: Some(claim.run.agent_id.clone()),
                    run_id: Some(claim.run.id.clone()),
                    channel_id: claim.run.channel_id.clone(),
                    payload: serde_json::json!({
                        "trigger_kind": claim.run.trigger_kind,
                        "trigger_ref": claim.run.trigger_ref,
                        "root_message_id": claim.run.root_message_id.as_ref().map(MessageId::as_str),
                        "origin_channel_id": claim
                            .run
                            .origin
                            .as_ref()
                            .map(|origin| origin.channel_id.as_str()),
                    }),
                })
                .await?;
        }
        Ok(claims)
    }

    pub async fn get_wakeup(
        &self,
        workspace_id: &WorkspaceId,
        id: &WakeupId,
    ) -> Result<Option<Wakeup>, TriggerError> {
        Ok(self.store.get_wakeup(workspace_id, id).await?)
    }

    // ----- Event Subscriptions and incoming events -----

    /// The declaration one qualified kind resolves to on one
    /// Connection provider, after the two rules that never depend on a
    /// manifest: a Pagis namespace is not a provider source, and an
    /// undeclared kind has no source at all.
    pub fn declaration(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        event_kind: &str,
        provider: &str,
    ) -> Result<EventDeclaration, TriggerError> {
        let namespace = event_kind.split('.').next().unwrap_or_default();
        if NATIVE_NAMESPACES.contains(&namespace) {
            return Err(TriggerError::NativeEventKind(event_kind.to_string()));
        }
        self.catalog
            .declaration(workspace_id, event_kind, provider)
            .ok_or_else(|| TriggerError::UnknownEventKind(event_kind.to_string()))
    }

    pub async fn create_subscription(
        &self,
        input: NewSubscription,
    ) -> Result<EventSubscription, TriggerError> {
        let name = input.name.trim();
        let instruction = input.instruction.trim();
        if name.is_empty() {
            return Err(TriggerError::EmptyName);
        }
        if instruction.is_empty() {
            return Err(TriggerError::EmptyInstruction);
        }
        self.check_target(
            &input.workspace_id,
            &input.agent_id,
            &input.channel_id,
            input.root_message_id.as_ref(),
        )
        .await?;
        let connection = self
            .connection(&input.workspace_id, &input.connection_id)
            .await?
            .ok_or_else(|| TriggerError::ConnectionNotFound(input.connection_id.to_string()))?;
        // The provider decides the declaration, so a kind no manifest
        // declares and a kind this Connection does not supply read the
        // same way to the caller: this account cannot give you that.
        let declaration = self
            .declaration(&input.workspace_id, &input.event_kind, &connection.provider)
            .map_err(|error| match error {
                TriggerError::UnknownEventKind(kind) => {
                    TriggerError::EventKindNotOnConnection(connection.alias.clone(), kind)
                }
                other => other,
            })?;
        if !self.matchers.contains_key(&declaration.matcher) {
            return Err(TriggerError::UnknownMatcher(declaration.matcher));
        }
        validate_filter(&declaration.filter_schema, &input.filter)?;
        // The grant is checked here and again before every Wake-up: a
        // rule the Agent cannot act on is never created.
        if !self
            .authorized(
                &input.agent_id,
                &connection,
                &declaration.required_capability,
            )
            .await?
        {
            return Err(TriggerError::GrantMissing);
        }

        let subscription = EventSubscription {
            id: EventSubscriptionId::generate(),
            workspace_id: input.workspace_id,
            agent_id: input.agent_id,
            connection_id: input.connection_id,
            event_kind: input.event_kind,
            source_version: declaration.source_version,
            name: name.to_string(),
            instruction: instruction.to_string(),
            channel_id: input.channel_id,
            root_message_id: input.root_message_id,
            filter: input.filter,
            creator: input.creator,
            state: EventSubscriptionState::Active,
            revision: 1,
            // Both paths reach here past the user: REST is the user,
            // and the Agent tool is gated by a `tool_action` approval.
            approved_revision: Some(1),
            // The first collection establishes the real baseline; the
            // creation instant keeps mail from before it out even when
            // another rule already primed the cursor.
            watermark_at: Some(input.now),
            blocked_reason: None,
            created_at: input.now,
            updated_at: input.now,
            archived_at: None,
        };
        self.subscriptions.create(&subscription).await?;
        self.changed.notify_one();
        self.publish_subscription("event_subscription.created", &subscription)
            .await?;
        Ok(subscription)
    }

    pub async fn get_subscription(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
    ) -> Result<Option<EventSubscription>, TriggerError> {
        Ok(self.subscriptions.get(workspace_id, id).await?)
    }

    pub async fn list_subscriptions(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        before: Option<&EventSubscriptionId>,
        limit: u32,
    ) -> Result<Vec<EventSubscription>, TriggerError> {
        Ok(self
            .subscriptions
            .list(workspace_id, agent_id, before, limit)
            .await?)
    }

    /// Apply one validated action. A pause, an archive, or an edit
    /// withdraws the rule's pending Wake-up: the old revision's work
    /// is no longer what the user asked for.
    pub async fn update_subscription(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
        action: SubscriptionAction,
        now: i64,
    ) -> Result<EventSubscription, TriggerError> {
        let subscription = self
            .subscriptions
            .get(workspace_id, id)
            .await?
            .ok_or(TriggerError::SubscriptionNotFound)?;
        if !subscriptions::is_manageable(subscription.state) {
            return Err(TriggerError::SubscriptionArchived);
        }
        if let SubscriptionAction::Edit {
            filter: Some(filter),
            ..
        } = &action
        {
            let provider = self
                .connection(&subscription.workspace_id, &subscription.connection_id)
                .await?
                .ok_or_else(|| {
                    TriggerError::ConnectionNotFound(subscription.connection_id.to_string())
                })?
                .provider;
            let declaration = self.declaration(
                &subscription.workspace_id,
                &subscription.event_kind,
                &provider,
            )?;
            validate_filter(&declaration.filter_schema, filter)?;
        }
        let next = subscriptions::apply_action(&subscription, &action, now);
        if next.name.trim().is_empty() {
            return Err(TriggerError::EmptyName);
        }
        if next.instruction.trim().is_empty() {
            return Err(TriggerError::EmptyInstruction);
        }
        if matches!(action, SubscriptionAction::Edit { .. }) {
            self.check_target(
                &next.workspace_id,
                &next.agent_id,
                &next.channel_id,
                next.root_message_id.as_ref(),
            )
            .await?;
        }
        self.subscriptions.update(&next).await?;
        if !matches!(action, SubscriptionAction::Resume) {
            self.withdraw_pending(&next.workspace_id, next.id.as_str(), now)
                .await?;
        }
        self.changed.notify_one();
        self.publish_subscription("event_subscription.updated", &next)
            .await?;
        if next.state != subscription.state {
            self.publish_subscription("event_subscription.state_changed", &next)
                .await?;
        }
        Ok(next)
    }

    /// Every Connection a collector must poll this minute.
    pub async fn collector_targets(&self) -> Result<Vec<CollectorTarget>, TriggerError> {
        Ok(self.subscriptions.collector_targets().await?)
    }

    /// The opaque cursor a collector left. Nothing outside the
    /// collector reads inside it.
    pub async fn cursor(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<Option<String>, TriggerError> {
        Ok(self
            .store
            .cursor(workspace_id, connection_id, event_kind)
            .await?)
    }

    /// Commit one collection pass: the cursor, the deduplicated
    /// Incoming Events, the matches, and the Wake-ups, in one
    /// transaction. The live grant of every candidate rule is checked
    /// first, and a rule that lost it is blocked rather than delivered.
    /// An event of a Source Item that a Forget blocks is dropped: the
    /// declaration of the kind names the synced resource of the item
    /// (ADR-0008).
    pub async fn ingest(&self, batch: IngestBatch) -> Result<IngestOutcome, TriggerError> {
        self.ingest_with_arrival(batch, None).await
    }

    /// Commit a provider batch and request one reflection-only Run when the
    /// batch stores at least one new arrival.
    pub async fn ingest_arrivals(
        &self,
        batch: IngestBatch,
        arrival: ArrivalRun,
    ) -> Result<IngestOutcome, TriggerError> {
        self.ingest_with_arrival(batch, Some(arrival)).await
    }

    async fn ingest_with_arrival(
        &self,
        batch: IngestBatch,
        arrival: Option<ArrivalRun>,
    ) -> Result<IngestOutcome, TriggerError> {
        let connection = self
            .connection(&batch.workspace_id, &batch.connection_id)
            .await?
            .ok_or_else(|| TriggerError::ConnectionNotFound(batch.connection_id.to_string()))?;
        let declaration =
            self.declaration(&batch.workspace_id, &batch.event_kind, &connection.provider)?;
        let matcher = self
            .matchers
            .get(&declaration.matcher)
            .cloned()
            .ok_or_else(|| TriggerError::UnknownMatcher(declaration.matcher.clone()))?;
        let candidates = self
            .subscriptions
            .live_for_source(&batch.workspace_id, &batch.connection_id, &batch.event_kind)
            .await?;
        let mut eligible = Vec::new();
        for subscription in candidates {
            // A pass that belongs to one Agent's own identity reaches
            // that Agent's rules alone, so two Agent Mailboxes on one
            // Connection never read each other's mail (ADR-0019).
            if batch
                .agent_id
                .as_ref()
                .is_some_and(|agent_id| *agent_id != subscription.agent_id)
            {
                continue;
            }
            if self
                .authorized(
                    &subscription.agent_id,
                    &connection,
                    &declaration.required_capability,
                )
                .await?
            {
                eligible.push(subscription);
            } else {
                // Revocation blocks the rule, withdraws its pending
                // work, and stops future Wake-ups until a new grant.
                self.block(subscription, BlockReason::GrantRevoked, batch.received_at)
                    .await?;
            }
        }
        let outcome = self
            .store
            .ingest(
                batch,
                &eligible,
                matcher.as_ref(),
                arrival.as_ref(),
                declaration.source_resource.as_deref(),
                self.forget_keys.as_ref(),
            )
            .await?;
        for event in &outcome.events {
            self.publish(
                "incoming_event.received",
                &event.workspace_id,
                None,
                None,
                serde_json::json!({
                    "incoming_event_id": event.id.as_str(),
                    "connection_id": event.connection_id.as_str(),
                    "event_kind": event.event_kind,
                    "occurred_at": event.occurred_at,
                }),
            )
            .await?;
        }
        for wakeup in &outcome.created {
            self.publish_wakeup("wakeup.created", wakeup).await?;
        }
        for wakeup in &outcome.combined {
            self.publish_wakeup("wakeup.combined", wakeup).await?;
        }
        self.publish_collector_state(&outcome.batch).await?;
        if !outcome.created.is_empty() {
            self.changed.notify_one();
        }
        Ok(outcome)
    }

    /// Record one failed collection pass. The cursor stays where it
    /// was, so the next pass re-reads the same window.
    pub async fn record_collection_failure(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        code: &str,
        at: i64,
    ) -> Result<SourceBatch, TriggerError> {
        let batch = self
            .store
            .record_failure(workspace_id, connection_id, event_kind, code, at)
            .await?;
        self.publish_collector_state(&batch).await?;
        Ok(batch)
    }

    /// Block every live subscription on one Connection and withdraw
    /// their pending work. Connection-wide `reauth_required` uses this;
    /// the cursor is kept, so reauthorization catches up.
    pub async fn block_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        reason: BlockReason,
        now: i64,
    ) -> Result<usize, TriggerError> {
        let affected = self
            .subscriptions
            .list_for_connection(workspace_id, connection_id, &["active"])
            .await?;
        let count = affected.len();
        for subscription in affected {
            self.block(subscription, reason, now).await?;
        }
        Ok(count)
    }

    /// Reactivate the subscriptions one Connection blocked, for the
    /// Agents that can still act. What the return costs depends on why
    /// the rule stopped: a reauthorized Connection catches up once from
    /// its kept cursor, while a restored grant delivers from that point
    /// forward and never replays the gap (ADR-0006).
    pub async fn unblock_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        now: i64,
    ) -> Result<usize, TriggerError> {
        let blocked = self
            .subscriptions
            .list_for_connection(workspace_id, connection_id, &["blocked"])
            .await?;
        if blocked.is_empty() {
            return Ok(0);
        }
        let connection = self
            .connection(&blocked[0].workspace_id, connection_id)
            .await?
            .ok_or_else(|| TriggerError::ConnectionNotFound(connection_id.to_string()))?;
        let mut restored = 0;
        for subscription in blocked {
            let declaration = self.declaration(
                &subscription.workspace_id,
                &subscription.event_kind,
                &connection.provider,
            )?;
            if !self
                .authorized(
                    &subscription.agent_id,
                    &connection,
                    &declaration.required_capability,
                )
                .await?
            {
                continue;
            }
            let grant_gap = subscription.blocked_reason == Some(BlockReason::GrantRevoked);
            let next = EventSubscription {
                state: EventSubscriptionState::Active,
                blocked_reason: None,
                watermark_at: if grant_gap {
                    Some(now)
                } else {
                    subscription.watermark_at
                },
                updated_at: now,
                ..subscription
            };
            self.subscriptions.update(&next).await?;
            self.publish_subscription("event_subscription.state_changed", &next)
                .await?;
            restored += 1;
        }
        if restored > 0 {
            self.changed.notify_one();
        }
        Ok(restored)
    }

    pub async fn list_subscription_events(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
        before: Option<&IncomingEventId>,
        limit: u32,
    ) -> Result<Vec<IncomingEvent>, TriggerError> {
        Ok(self
            .subscriptions
            .list_events(workspace_id, id, before, limit)
            .await?)
    }

    pub async fn list_subscription_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, TriggerError> {
        Ok(self
            .subscriptions
            .list_wakeups(workspace_id, id, before, limit)
            .await?)
    }

    /// The last collection pass and the last successful one.
    pub async fn collector_health(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<(Option<SourceBatch>, Option<SourceBatch>), TriggerError> {
        Ok(self
            .subscriptions
            .collector_health(workspace_id, connection_id, event_kind)
            .await?)
    }

    async fn withdraw_pending(
        &self,
        workspace_id: &WorkspaceId,
        rule_id: &str,
        now: i64,
    ) -> Result<(), TriggerError> {
        for wakeup in self
            .store
            .withdraw_pending(workspace_id, rule_id, now)
            .await?
        {
            self.publish_wakeup("wakeup.withdrawn", &wakeup).await?;
        }
        Ok(())
    }

    /// Move one subscription to `blocked` and withdraw its pending
    /// work. The notice the user reads is the state change event.
    async fn block(
        &self,
        subscription: EventSubscription,
        reason: BlockReason,
        now: i64,
    ) -> Result<(), TriggerError> {
        let next = EventSubscription {
            state: EventSubscriptionState::Blocked,
            blocked_reason: Some(reason),
            updated_at: now,
            ..subscription
        };
        self.subscriptions.update(&next).await?;
        self.withdraw_pending(&next.workspace_id, next.id.as_str(), now)
            .await?;
        self.publish(
            "event_subscription.state_changed",
            &next.workspace_id,
            Some(&next.agent_id),
            Some(&next.channel_id),
            serde_json::json!({
                "event_subscription_id": next.id.as_str(),
                "state": next.state.as_str(),
                "reason": reason.as_str(),
            }),
        )
        .await?;
        Ok(())
    }

    /// The Agent's live grant on one Connection, when it carries the
    /// capability the declaration requires and the Connection can
    /// still serve it.
    async fn authorized(
        &self,
        agent_id: &AgentId,
        connection: &Connection,
        capability: &str,
    ) -> Result<bool, TriggerError> {
        // A declaration that names no capability is the Agent acting
        // as its own identity, which needs no Grant and does not stop
        // when the Connection's own credential does: an Agent Mailbox
        // keeps reading with a revoked host API key (ADR-0019).
        if capability.is_empty() {
            return Ok(true);
        }
        if connection.status != Connection::CONNECTED {
            return Ok(false);
        }
        let grant = self
            .grants
            .live_for_resource(
                &connection.workspace_id,
                agent_id,
                Grant::CONNECTION_KIND,
                connection.id.as_str(),
            )
            .await?;
        Ok(grant.is_some_and(|grant| grant.capabilities().iter().any(|held| held == capability)))
    }

    async fn publish_subscription(
        &self,
        event_type: &str,
        subscription: &EventSubscription,
    ) -> Result<(), TriggerError> {
        self.publish(
            event_type,
            &subscription.workspace_id,
            Some(&subscription.agent_id),
            Some(&subscription.channel_id),
            serde_json::json!({
                "event_subscription_id": subscription.id.as_str(),
                "event_kind": subscription.event_kind,
                "connection_id": subscription.connection_id.as_str(),
                "state": subscription.state.as_str(),
                "revision": subscription.revision,
            }),
        )
        .await
    }

    async fn publish_collector_state(&self, batch: &SourceBatch) -> Result<(), TriggerError> {
        self.publish(
            "collector.state_changed",
            &batch.workspace_id,
            None,
            None,
            serde_json::json!({
                "connection_id": batch.connection_id.as_str(),
                "event_kind": batch.event_kind,
                "outcome": batch.outcome.as_str(),
                "collected_count": batch.collected_count,
                "stored_count": batch.stored_count,
                "wakeup_count": batch.wakeup_count,
                "detail": batch.detail,
            }),
        )
        .await
    }

    async fn publish_wakeup(&self, event_type: &str, wakeup: &Wakeup) -> Result<(), TriggerError> {
        self.publish(
            event_type,
            &wakeup.workspace_id,
            Some(&wakeup.agent_id),
            wakeup.channel_id.as_ref(),
            serde_json::json!({
                "wakeup_id": wakeup.id.as_str(),
                "source_kind": wakeup.rule.kind_str(),
                "rule_id": wakeup.rule.id_str(),
                "rule_revision": wakeup.rule_revision,
                "source_count": wakeup.source_count,
                "state": wakeup.state.as_str(),
                "run_id": wakeup.run_id.as_ref().map(|id| id.as_str()),
            }),
        )
        .await
    }

    async fn pending_schedule_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
    ) -> Result<Vec<Wakeup>, TriggerError> {
        Ok(self
            .schedules
            .list_wakeups(workspace_id, schedule_id, None, 2)
            .await?
            .into_iter()
            .filter(|wakeup| wakeup.state == pagis_core::WakeupState::Pending)
            .collect())
    }

    async fn publish_withdrawn(&self, wakeups: Vec<Wakeup>) -> Result<(), TriggerError> {
        for mut wakeup in wakeups {
            wakeup.state = pagis_core::WakeupState::Withdrawn;
            self.publish_wakeup("wakeup.withdrawn", &wakeup).await?;
        }
        Ok(())
    }

    async fn publish(
        &self,
        event_type: &str,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
        payload: serde_json::Value,
    ) -> Result<(), TriggerError> {
        self.events
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: event_type.to_string(),
                agent_id: agent_id.cloned(),
                run_id: None,
                channel_id: channel_id.cloned(),
                payload,
            })
            .await
            .map(|_| ())
            .map_err(TriggerError::Store)
    }
}

/// The Thread a rule keeps after an edit that names no Thread root. A
/// Thread root is valid only inside its own Channel, so an edit that
/// moves the rule to another Channel drops it.
pub(crate) fn kept_thread(
    channel_id: &ChannelId,
    root_message_id: &Option<MessageId>,
    next_channel_id: &ChannelId,
) -> Option<MessageId> {
    if channel_id == next_channel_id {
        root_message_id.clone()
    } else {
        None
    }
}

/// Check one typed filter against the schema its declaration fixed.
/// The filter reaches Pagis from the model or the user, so it never
/// widens what the declaration allows.
fn validate_filter(
    schema: &serde_json::Value,
    filter: &serde_json::Value,
) -> Result<(), TriggerError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| TriggerError::InvalidFilter(error.to_string()))?;
    match validator.validate(filter) {
        Ok(()) => Ok(()),
        Err(error) => Err(TriggerError::InvalidFilter(error.to_string())),
    }
}

/// The first firing of a five-field cron expression after `now`, in
/// its own timezone. The seed reads it to give the Report Schedule its
/// first time (ADR-0022), and `resolve_timing` reads it for every
/// Schedule a user or an Agent creates.
pub fn next_cron_occurrence(
    expression: &str,
    timezone: &str,
    now: i64,
) -> Result<i64, TriggerError> {
    if expression.split_whitespace().count() != 5 {
        return Err(TriggerError::InvalidCron);
    }
    let zone = timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| TriggerError::InvalidTimezone(timezone.to_string()))?;
    let cron = croner::Cron::from_str(expression).map_err(|_| TriggerError::InvalidCron)?;
    let now = chrono::DateTime::from_timestamp_millis(now)
        .ok_or(TriggerError::InvalidCron)?
        .with_timezone(&zone);
    Ok(cron
        .find_next_occurrence(&now, false)
        .map_err(|_| TriggerError::InvalidCron)?
        .timestamp_millis())
}

fn next_after(schedule: &Schedule, after: i64) -> Result<Option<i64>, TriggerError> {
    match schedule.kind {
        ScheduleKind::OneShot => {
            Ok((schedule.scheduled_at > after).then_some(schedule.scheduled_at))
        }
        ScheduleKind::Interval => {
            let anchor = schedule
                .anchor_at
                .ok_or_else(|| StoreError::Corrupt("interval anchor is missing".to_string()))?;
            let interval_ms = schedule
                .interval_ms
                .ok_or_else(|| StoreError::Corrupt("interval cadence is missing".to_string()))?;
            if after < anchor {
                Ok(Some(anchor))
            } else {
                Ok(Some(
                    anchor + ((after - anchor) / interval_ms + 1) * interval_ms,
                ))
            }
        }
        ScheduleKind::Cron => {
            let expression = schedule
                .cron_expression
                .as_deref()
                .ok_or_else(|| StoreError::Corrupt("cron expression is missing".to_string()))?;
            let cron = croner::Cron::from_str(expression).map_err(|_| TriggerError::InvalidCron)?;
            let timezone = schedule
                .timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| TriggerError::InvalidTimezone(schedule.timezone.clone()))?;
            let after = chrono::DateTime::from_timestamp_millis(after)
                .ok_or(TriggerError::InvalidCron)?
                .with_timezone(&timezone);
            Ok(Some(
                cron.find_next_occurrence(&after, false)
                    .map_err(|_| TriggerError::InvalidCron)?
                    .timestamp_millis(),
            ))
        }
    }
}

struct ResolvedTiming {
    kind: ScheduleKind,
    cron_expression: Option<String>,
    interval_ms: Option<i64>,
    anchor_at: Option<i64>,
    timezone: String,
    scheduled_at: i64,
}

fn resolve_timing(timing: ScheduleTiming, now: i64) -> Result<ResolvedTiming, TriggerError> {
    match timing {
        ScheduleTiming::OneShot {
            local_time,
            timezone,
        } => {
            let zone = timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| TriggerError::InvalidTimezone(timezone.clone()))?;
            let local = NaiveDateTime::parse_from_str(&local_time, "%Y-%m-%dT%H:%M:%S")
                .map_err(|_| TriggerError::InvalidLocalTime)?;
            let scheduled_at = match zone.from_local_datetime(&local) {
                LocalResult::Single(value) => value.timestamp_millis(),
                LocalResult::None => return Err(TriggerError::NonexistentLocalTime),
                LocalResult::Ambiguous(_, _) => return Err(TriggerError::AmbiguousLocalTime),
            };
            if scheduled_at <= now {
                return Err(TriggerError::PastInstant);
            }
            Ok(ResolvedTiming {
                kind: ScheduleKind::OneShot,
                cron_expression: None,
                interval_ms: None,
                anchor_at: None,
                timezone,
                scheduled_at,
            })
        }
        ScheduleTiming::Cron {
            expression,
            timezone,
        } => {
            let scheduled_at = next_cron_occurrence(&expression, &timezone, now)?;
            Ok(ResolvedTiming {
                kind: ScheduleKind::Cron,
                cron_expression: Some(expression),
                interval_ms: None,
                anchor_at: None,
                timezone,
                scheduled_at,
            })
        }
        ScheduleTiming::Interval { every_ms, anchor } => {
            if every_ms < 60_000 {
                return Err(TriggerError::IntervalTooShort);
            }
            let scheduled_at = if now < anchor {
                anchor
            } else {
                anchor + ((now - anchor) / every_ms + 1) * every_ms
            };
            Ok(ResolvedTiming {
                kind: ScheduleKind::Interval,
                cron_expression: None,
                interval_ms: Some(every_ms),
                anchor_at: Some(anchor),
                timezone: "UTC".to_string(),
                scheduled_at,
            })
        }
    }
}
