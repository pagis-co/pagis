//! The daemon driver of the release evaluation. It replays one
//! chronology through a daemon booted on its own clean directory: the
//! fixture source serves the evidence in acquisition order, and each
//! probe reaches Sage through the shipped conversation path.
//!
//! The driver records what the daemon showed, what it did, what it
//! spent, and which capability a probe did not get. A different route
//! proposes each probe grade for the owner.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use pagis_agent::{Brain, JsonSchemaFormat, TurnDelta, TurnMessage, TurnRequest};
use pagis_core::{
    AgentId, AgentStore, AuthorKind, Channel, ChannelId, ChannelKind, ChannelParticipant,
    ChannelStore, Connection, ConnectionId, ConnectionStore, EventLog, Grant, GrantId, GrantStore,
    Message, MessageId, MessageStatus, MessageStore, ModelAliasStore, ParticipantId,
    ParticipantKind, ParticipantStore, PendingEvidenceStore, RequestId, RequestStore, RunId,
    RunState, RunStore, TriggerStore, WorkspaceId, WorkspaceStore,
    knowledge::{KnowledgeStore, SourceKey, SyncConfig},
    now_ms,
    subject_page::repair_json,
};
use pagis_evaluation::pricing::RouteRate;
use pagis_evaluation::{
    Authorization, Chronology, ChronologyDriver, DeliveredIntervention, DriverResult, Evidence,
    InterventionPipeline, Probe, ProbeObservation, RunStatus, ScheduleObservation,
    StoreObservability, SystemFailures, Usage,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteChannelStore, SqliteConnectionStore, SqliteEventLog, SqliteGrantStore,
    SqliteKnowledgeStore, SqliteMessageStore, SqliteModelAliasStore, SqliteParticipantStore,
    SqlitePendingEvidenceStore, SqliteRequestStore, SqliteRunStore, SqliteTriggerStore,
    SqliteWorkspaceStore,
};
use serde::Deserialize;

use super::import::{IMPORT_TIMEOUT, ImportWait};
use super::meter::{MeterCursor, MeteredBrain, ModelMeter};
use super::source::{FixtureClock, FixtureSource, mail_items, millis};

/// The input form the fixture source serves as mail. Every other form
/// reaches the daemon as an owner message in the conversation.
const MAIL_FORM: &str = "mail";

/// How long one delivery may take before the driver calls it a failure
/// rather than waiting for a daemon that is not going to answer.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(60);

/// The pre-grader needs room for reasoning before its short JSON reply.
const PRE_GRADE_OUTPUT_TOKENS: u32 = 4_096;

/// The event the daemon records when one owner message starts a run.
const RUN_CREATED: &str = "run.created";

/// The event the daemon records when one owner message reaches a run
/// that still holds the turn. Such a message starts no run of its own.
const MESSAGE_INJECTED: &str = "run.message_injected";

const SCHEDULE_CREATED: &str = "schedule.created";
const SCHEDULE_DECISION: &str = "schedule.decision";
const WAKEUP_STARTED: &str = "wakeup.started";

/// How many run events the driver reads back to find the run that
/// answered one message. One chronology holds at most six evidence
/// items and three probes, and each of them makes one run event.
const RUN_EVENT_LIMIT: u32 = 100;

/// How many intervention events one repeat can report. This is above
/// the release limit of 48 model calls per repeat.
const PIPELINE_EVENT_LIMIT: u32 = 1_000;

/// How many messages of the owner conversation the driver reads back.
/// One chronology holds at most six evidence items and three probes, so
/// this covers every message a run can produce.
const CONVERSATION_LIMIT: u32 = 200;

/// The spend the caller authorizes for this driver. The manifest is a
/// specification, never an authorization: the caller builds this value
/// and stays answerable for the cost.
#[derive(Clone, Debug)]
pub struct EvaluationSpend {
    pub authorization: Authorization,
    /// Worst-case cost reserved before one chronology repeat starts,
    /// normally the manifest's per-repeat ceiling.
    pub reserve_per_run_usd: f64,
    /// The rate the run settles its metered tokens at. A local route
    /// with no price may use zero and keeps its token and time caps.
    pub rate: RouteRate,
}

/// The replay engine owns one daemon for each chronology
/// repeat, with the caller's model, route and spend ledger.
struct Replayer {
    model: Arc<dyn Brain>,
    route: String,
    /// The ordered `provider/model` candidates the assistant thinks on.
    /// The driver writes them over the seeded alias of every daemon it
    /// boots, so the run uses the route the caller priced.
    candidates: Vec<String>,
    clock_version: String,
    zone_rule_version: String,
    spend: EvaluationSpend,
    /// How long one delivery may take. A release run keeps the default; the
    /// retained test shortens it, because a test cannot wait a minute
    /// for a daemon it made hang on purpose.
    delivery_timeout: Duration,
    /// How long the import of one mail evidence item may take. A
    /// release run keeps the default, which covers the two retry delays of the
    /// manifest; a retained test shortens it, because a test cannot
    /// wait out a retry schedule it made fail on purpose.
    import_timeout: Duration,
    /// The evaluation ledger: what this driver has committed so far.
    committed_usd: f64,
    /// The model route that proposes grades. It is set from the
    /// manifest before the first chronology starts.
    pre_grader_route: Option<String>,
}

impl Replayer {
    fn new(
        model: Arc<dyn Brain>,
        route: impl Into<String>,
        candidates: Vec<String>,
        spend: EvaluationSpend,
    ) -> Self {
        Self {
            model,
            route: route.into(),
            candidates,
            // One clock stamps and gates the source and drives every
            // background pass of the daemon, so the version
            // names one half.
            clock_version: "fixture-clock-v1".into(),
            zone_rule_version: format!("chrono-tz {}", chrono_tz::IANA_TZDB_VERSION),
            spend,
            delivery_timeout: DELIVERY_TIMEOUT,
            import_timeout: IMPORT_TIMEOUT,
            committed_usd: 0.0,
            pre_grader_route: None,
        }
    }

    /// The named capability a run is missing before it starts, if any.
    fn spend_refusal(&self) -> Option<String> {
        if !self.spend.authorization.priced_routes.contains(&self.route) {
            return Some(format!("priced and authorized model route: {}", self.route));
        }
        let left = self.spend.authorization.max_usd - self.committed_usd;
        if left < self.spend.reserve_per_run_usd {
            return Some(format!(
                "evaluation spend authorization: one run reserves ${:.2} and ${left:.2} remains",
                self.spend.reserve_per_run_usd
            ));
        }
        None
    }

    /// Replay one chronology repeat. The worst case is reserved on the
    /// evaluation ledger before the first model call and settled after
    /// the last one.
    async fn run(&mut self, case: &Chronology, repeat: u8) -> DriverResult {
        if let Some(missing) = self.spend_refusal() {
            return unscored(missing);
        }
        self.committed_usd += self.spend.reserve_per_run_usd;
        let started = Instant::now();
        let result = self.replay(case, repeat).await;
        let spent = result.usage.usd;
        self.committed_usd = self.committed_usd - self.spend.reserve_per_run_usd + spent;
        DriverResult {
            usage: Usage {
                elapsed_millis: started.elapsed().as_millis() as u64,
                ..result.usage
            },
            ..result
        }
    }
}

/// Replays one chronology through a real daemon that builds Subject Pages.
pub struct DaemonDriver(Replayer);

impl DaemonDriver {
    /// The model is the caller's: the retained test supplies a scripted
    /// one, and a release run supplies the responsible Agent's configured
    /// route.
    ///
    /// The model answers conversation and reflection turns. Acquisition
    /// starts a reflection-only Run after it stores an arrival.
    pub fn new(
        model: Arc<dyn Brain>,
        route: impl Into<String>,
        candidates: Vec<String>,
        spend: EvaluationSpend,
    ) -> Self {
        Self(Replayer::new(model, route, candidates, spend))
    }

    /// Wait `timeout` for one delivery instead of the default minute.
    pub fn with_delivery_timeout(mut self, timeout: Duration) -> Self {
        self.0.delivery_timeout = timeout;
        self
    }

    /// Wait `timeout` for one mail import instead of the manifest's
    /// retry budget.
    pub fn with_import_timeout(mut self, timeout: Duration) -> Self {
        self.0.import_timeout = timeout;
        self
    }
}

/// A run that did no work, with the capability it lacked.
fn unscored(missing: String) -> DriverResult {
    DriverResult {
        status: RunStatus::Unscored,
        missing_capabilities: vec![missing],
        observations: Vec::new(),
        delivered_interventions: Vec::new(),
        intervention_pipeline: InterventionPipeline::default(),
        system_failures: SystemFailures::default(),
        usage: Usage::default(),
    }
}

/// One evidence item with its position in acquisition order.
type Acquired<'a> = (usize, &'a Evidence);

/// The chronology's evidence in acquisition order, numbered from one.
fn acquisition_order(case: &Chronology) -> Vec<Acquired<'_>> {
    let mut order: Vec<&Evidence> = case.evidence.iter().collect();
    order.sort_by_key(|item| millis(&item.acquired_at).unwrap_or(i64::MAX));
    order
        .into_iter()
        .enumerate()
        .map(|(at, item)| (at + 1, item))
        .collect()
}

/// The daemon this run owns, with the identities the driver writes to.
struct Replay {
    daemon: crate::TestDaemon,
    /// The owner's own conversation with the assistant, seeded at boot.
    /// Every evidence item and fired Schedule message uses this conversation.
    dm: ChannelId,
    workspace: WorkspaceId,
    /// The assistant every probe channel holds as its participant.
    agent: AgentId,
    /// The imported source.
    source: SourceKey,
    meter: Arc<ModelMeter>,
    /// The same metered provider path that serves the daemon. A grade
    /// call only changes the candidate route.
    pre_grader: Arc<MeteredBrain>,
    fixture: Arc<FixtureSource>,
    clock: FixtureClock,
    delivery_timeout: Duration,
    import_timeout: Duration,
    initial_message_ids: Vec<MessageId>,
    /// What this replay counts against the brittleness gate.
    failures: Mutex<SystemFailures>,
}

impl Replay {
    /// Boot a daemon on a clean directory, hand it the fixture source,
    /// and enable the shipped Gmail import for the seeded assistant.
    async fn start(
        case: &Chronology,
        model: Arc<dyn Brain>,
        candidates: &[String],
        delivery_timeout: Duration,
        import_timeout: Duration,
    ) -> Result<Self, String> {
        let ordered = acquisition_order(case);
        let mail: Vec<Acquired<'_>> = ordered
            .iter()
            .filter(|(_, item)| item.input_form == MAIL_FORM)
            .copied()
            .collect();
        let items = mail_items(&mail, &case.zone)?;
        let start = ordered
            .first()
            .map(|(_, item)| millis(&item.acquired_at))
            .transpose()?
            .map(|at| at.saturating_sub(1))
            .unwrap_or_else(now_ms);
        let clock = FixtureClock::at(start);
        let fixture = Arc::new(FixtureSource::new(clock.clone(), items));
        let meter = Arc::new(ModelMeter::default());
        let metered = Arc::new(MeteredBrain::new(model, Arc::clone(&meter)));
        let daemon = crate::TestDaemon::start_with(crate::TestDaemonOptions {
            brain: Arc::clone(&metered) as Arc<dyn Brain>,
            gog: Some(Arc::clone(&fixture) as Arc<dyn pagis_google::GogRunner>),
            clock: Arc::new(clock.clone()) as Arc<dyn pagis_core::Clock>,
            // A release driver configures no Computer.
            agents: pagis_agent::AgentLoopConfig {
                computer: false,
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        let pool = daemon.pool().clone();
        let initial_message_ids = SqliteMessageStore::new(pool.clone())
            .list_top_level(
                &daemon.workspace_id,
                &ChannelId::from(daemon.dm_channel_id.clone()),
                None,
                CONVERSATION_LIMIT,
            )
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|entry| entry.message.id)
            .collect();
        let agent = SqliteAgentStore::new(pool.clone())
            .get(
                &daemon.workspace_id,
                &AgentId::from(daemon.agent_id.clone()),
            )
            .await
            .map_err(|error| error.to_string())?
            .ok_or("the daemon seeds one assistant")?;
        let routed = SqliteModelAliasStore::new(pool.clone())
            .update_candidates(
                &agent.workspace_id,
                &agent.model_alias,
                candidates,
                clock.now_ms(),
            )
            .await
            .map_err(|error| format!("the alias `{}`: {error}", agent.model_alias))?;
        if !routed {
            return Err(format!(
                "the seeded alias `{}` is missing",
                agent.model_alias
            ));
        }
        SqliteWorkspaceStore::new(pool.clone())
            .set_timezone(&agent.workspace_id, &case.zone)
            .await
            .map_err(|error| format!("the chronology zone {}: {error}", case.zone))?;
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: agent.workspace_id.clone(),
            provider: "google".into(),
            alias: "evaluation".into(),
            display_name: "Evaluation fixture".into(),
            status: Connection::CONNECTED.into(),
            auth_mode: Connection::AUTH_MODE_BYO.into(),
            authorized_capabilities: vec!["gmail_read".into()],
            config: serde_json::json!({"account":"owner@fixture.invalid","client":"evaluation"}),
            created_at: clock.now_ms(),
        };
        SqliteConnectionStore::new(pool.clone())
            .create(&connection)
            .await
            .map_err(|error| error.to_string())?;
        SqliteGrantStore::new(pool.clone())
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: agent.workspace_id.clone(),
                agent_id: agent.id.clone(),
                resource_kind: Grant::CONNECTION_KIND.into(),
                resource_id: Some(connection.id.to_string()),
                scope: Grant::connection_scope(&["gmail_read".into()]),
                revision: 1,
                created_at: clock.now_ms(),
                revoked_at: None,
            })
            .await
            .map_err(|error| error.to_string())?;
        // The import runs under the owner's explicit configuration, the
        // same call the Connections tab makes.
        let knowledge = SqliteKnowledgeStore::new(pool.clone());
        knowledge
            .configure(
                SyncConfig {
                    workspace_id: agent.workspace_id.clone(),
                    connection_id: connection.id.clone(),
                    resource: "gmail".into(),
                    agent_id: agent.id.clone(),
                    required_capability: "gmail_read".into(),
                    enabled: true,
                    since: 0,
                    filter: pagis_google::gmail_filter::default_filter(),
                },
                clock.now_ms(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let source = SourceKey {
            workspace_id: agent.workspace_id.clone(),
            connection_id: connection.id.clone(),
            resource: "gmail".into(),
        };
        // Finish the empty historical pass before the first evidence becomes
        // visible. The first item must enter as an arrival and write its Subject Page.
        ImportWait::new(&knowledge, &source, 0, import_timeout)
            .settled()
            .await?;
        Ok(Self {
            dm: ChannelId::from(daemon.dm_channel_id.clone()),
            workspace: agent.workspace_id.clone(),
            agent: agent.id.clone(),
            source,
            daemon,
            initial_message_ids,
            meter,
            pre_grader: metered,
            fixture,
            clock,
            delivery_timeout,
            import_timeout,
            failures: Mutex::new(SystemFailures::default()),
        })
    }

    /// The imported source of this replay.
    fn source(&self) -> &SourceKey {
        &self.source
    }

    /// Count one system failure of the given kind. `turn` names
    /// the delivery or the probe it happened in, so the record says
    /// which turn was lost and why.
    fn record(&self, turn: &str, stall: &Stall) {
        let mut failures = self.failures.lock().expect("failure lock");
        match stall {
            Stall::Hang(_) => failures.hangs += 1,
            Stall::Aborted(reason) => failures.aborted_turns.push(format!("{turn}: {reason}")),
        }
    }

    /// Count one aborted turn whose reply settled on a failed run.
    fn record_aborted(&self, turn: &str, reason: &str) {
        self.failures
            .lock()
            .expect("failure lock")
            .aborted_turns
            .push(format!("{turn}: {reason}"));
    }

    /// A fresh owner conversation for one probe.
    ///
    /// A reply turn rebuilds its context from the channel it is in, so
    /// a probe asked in a new channel reads no evidence delivery and no
    /// earlier probe: only the daemon's durable memory can answer it.
    /// The channel is a DM between the owner and the same assistant,
    /// made the way the daemon makes the owner's own at boot
    /// (`crates/pagis/src/boot.rs:220`), so the message triggers the
    /// assistant as every owner message does. Its id sorts after the
    /// seeded DM, where fired Subject Page Schedules deliver messages.
    async fn probe_channel(&self, probe_id: &str) -> Result<ChannelId, Stall> {
        let now = self.clock.now_ms();
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: self.workspace.clone(),
            kind: ChannelKind::Dm,
            title: Some(format!("probe {probe_id}")),
            created_at: now,
            updated_at: now,
        };
        SqliteChannelStore::new(self.daemon.pool().clone())
            .create(&channel)
            .await
            .map_err(|error| Stall::Aborted(error.to_string()))?;
        let participants = SqliteParticipantStore::new(self.daemon.pool().clone());
        for (kind, agent_id) in [
            (ParticipantKind::User, None),
            (ParticipantKind::Agent, Some(self.agent.clone())),
        ] {
            participants
                .create(&ChannelParticipant {
                    id: ParticipantId::generate(),
                    workspace_id: self.workspace.clone(),
                    channel_id: channel.id.clone(),
                    kind,
                    agent_id,
                    joined_at: now,
                })
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?;
        }
        Ok(channel.id)
    }

    fn messages(&self) -> SqliteMessageStore {
        SqliteMessageStore::new(self.daemon.pool().clone())
    }

    /// Send one owner message into `channel` through the daemon's own
    /// HTTP path, and return the message the daemon recorded. The reply
    /// is keyed by the run this message woke, so the driver keeps its
    /// identity.
    async fn say(
        &self,
        channel: &ChannelId,
        pending_id: &str,
        text: &str,
    ) -> Result<MessageId, Stall> {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/channels/{}/messages",
                self.daemon.base_url, channel
            ))
            .header("cookie", self.daemon.cookie())
            .json(&serde_json::json!({"pending_id": pending_id, "text": text}))
            .send()
            .await
            .map_err(|error| Stall::Aborted(error.to_string()))?;
        match response.status().as_u16() {
            201 => {
                let body: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|error| Stall::Aborted(error.to_string()))?;
                body["id"]
                    .as_str()
                    .map(|id| MessageId::from(id.to_string()))
                    .ok_or_else(|| Stall::Aborted("the daemon named no message id".into()))
            }
            status => Err(Stall::Aborted(format!(
                "the daemon refused the message with {status}"
            ))),
        }
    }

    /// The run that read one owner message. The daemon starts a run for
    /// a message that reaches a free thread, and injects a message that
    /// reaches a thread whose run still holds the turn; it records both
    /// in the event log, so the driver reads the answer there instead of
    /// guessing from the order of the conversation.
    async fn answering_run(&self, owner: &MessageId) -> Result<Option<RunId>, String> {
        let events = SqliteEventLog::new(self.daemon.pool().clone())
            .list_by_types(
                &self.daemon.workspace_id,
                &[RUN_CREATED, MESSAGE_INJECTED],
                None,
                RUN_EVENT_LIMIT,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(events
            .into_iter()
            .find(|event| {
                let payload = &event.payload;
                payload["trigger_ref"].as_str() == Some(owner.as_str())
                    || payload["message_id"].as_str() == Some(owner.as_str())
            })
            .and_then(|event| event.run_id))
    }

    /// The settled reply of the run that read one owner message.
    ///
    /// The reply must be the answer of that run, not the next agent
    /// message in the conversation. `seen` opens the window the failure
    /// line reads the last tool call from.
    async fn reply(
        &self,
        channel: &ChannelId,
        owner: &MessageId,
        seen: MeterCursor,
    ) -> Result<Message, Stall> {
        let deadline = Instant::now() + self.delivery_timeout;
        loop {
            if let Some(run) = self.answering_run(owner).await.map_err(Stall::Aborted)?
                && !self.busy(&run).await.map_err(Stall::Aborted)?
            {
                let entries = self
                    .messages()
                    .list_top_level(&self.daemon.workspace_id, channel, None, CONVERSATION_LIMIT)
                    .await
                    .map_err(|error| Stall::Aborted(error.to_string()))?;
                let answer = run_reply(entries.into_iter().map(|entry| entry.message), owner, &run);
                if let Some(message) = answer {
                    return Ok(message);
                }
            }
            if Instant::now() >= deadline {
                // The wait names what the daemon was doing, so a
                // report says whether the run was thinking, parked
                // or already over, and which tool it last called.
                return Err(Stall::Hang(format!(
                    "no settled reply within {} seconds: the run state is {} and the last tool call is {}",
                    self.delivery_timeout.as_secs(),
                    self.newest_run_state(channel)
                        .await
                        .unwrap_or_else(|| "none".to_string()),
                    self.meter
                        .last_tool_call_since(seen)
                        .unwrap_or_else(|| "none".to_string()),
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Whether one run still holds the turn.
    ///
    /// A run parked for the owner has given its answer: it shows an
    /// approval card or a question and waits. The driver decides
    /// nothing, so that run never finishes, and the parked state is
    /// the settled result of the probe.
    async fn busy(&self, run_id: &RunId) -> Result<bool, String> {
        Ok(SqliteRunStore::new(self.daemon.pool().clone())
            .list_unfinished()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .any(|run| {
                run.id == *run_id
                    && !matches!(
                        run.state,
                        RunState::WaitingForApproval | RunState::WaitingForUser
                    )
            }))
    }

    /// The state of the newest run of this conversation. A run that
    /// already ended keeps its state, so a wait that ran out of time
    /// names a failed run as well as a thinking one.
    async fn newest_run_state(&self, channel: &ChannelId) -> Option<String> {
        SqliteRunStore::new(self.daemon.pool().clone())
            .list(&self.workspace, None, Some(channel), &[], None, 1)
            .await
            .ok()?
            .first()
            .map(|run| run.state.as_str().to_string())
    }

    /// Wait until acquisition has stored everything the fixture source serves.
    async fn imported(&self) -> Result<(), Stall> {
        let store = SqliteKnowledgeStore::new(self.daemon.pool().clone());
        ImportWait::new(
            &store,
            self.source(),
            self.fixture.visible().len(),
            self.import_timeout,
        )
        .settled()
        .await
        .map_err(Stall::Hang)
    }

    /// Move fixture time through each due Schedule before it reaches the
    /// next evidence item or probe. The wait gives the scheduler and the
    /// fired Run time to settle at the due instant.
    async fn advance_to(&self, target: i64) -> Result<(), Stall> {
        let triggers = SqliteTriggerStore::new(self.daemon.pool().clone());
        loop {
            let due = triggers
                .next_due_at()
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?;
            let Some(due) = due.filter(|due| *due <= target) else {
                break;
            };
            self.clock.advance_to(due);
            self.wait_for_due_schedule(&triggers, due).await?;
        }
        self.clock.advance_to(target);
        Ok(())
    }

    async fn wait_for_due_schedule(
        &self,
        triggers: &SqliteTriggerStore,
        due_at: i64,
    ) -> Result<(), Stall> {
        let deadline = Instant::now() + self.delivery_timeout;
        loop {
            let next_due = triggers
                .next_due_at()
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?;
            let wakeup_is_pending = triggers
                .pending_agents()
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?
                .iter()
                .any(|(_, agent)| agent == &self.agent);
            let schedule_runs: Vec<_> = SqliteRunStore::new(self.daemon.pool().clone())
                .list(
                    &self.workspace,
                    Some(&self.agent),
                    Some(&self.dm),
                    &[],
                    None,
                    RUN_EVENT_LIMIT,
                )
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?
                .into_iter()
                .filter(|run| {
                    run.trigger_kind == pagis_core::TriggerKind::Schedule
                        && run.created_at >= due_at
                })
                .collect();
            let schedule_is_due = next_due.is_some_and(|due| due <= self.clock.now_ms());
            if let Some(run) = schedule_runs
                .iter()
                .find(|run| matches!(run.state, RunState::Failed | RunState::Canceled))
            {
                return Err(Stall::Aborted(format!(
                    "Schedule Run {} ended as {}: {}",
                    run.id,
                    run.state.as_str(),
                    run.error.as_deref().unwrap_or("no reason recorded")
                )));
            }
            if !schedule_is_due
                && !wakeup_is_pending
                && !schedule_runs.is_empty()
                && schedule_runs
                    .iter()
                    .all(|run| run.state == RunState::Completed)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let states = schedule_runs
                    .iter()
                    .map(|run| format!("{}:{}", run.id, run.state.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                let unfinished = SqliteRunStore::new(self.daemon.pool().clone())
                    .list_unfinished()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|run| run.agent_id == self.agent)
                    .map(|run| {
                        format!(
                            "{}:{}:{}:trigger={}:channel={}:root={}",
                            run.id,
                            run.trigger_kind.as_str(),
                            run.state.as_str(),
                            run.trigger_ref.as_deref().unwrap_or("none"),
                            run.channel_id
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "none".into()),
                            run.root_message_id
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "none".into()),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let requests = self.meter.recent_requests(8).join("; ");
                let tool_calls = self.meter.recent_tool_calls(8).join(", ");
                return Err(Stall::Hang(format!(
                    "the Schedule due at {due_at} did not complete within {} seconds: \
                     next_due_at={next_due:?}, wake_up_pending={wakeup_is_pending}, \
                     schedule_runs=[{states}], unfinished_runs=[{unfinished}], \
                     recent_model_requests=[{requests}], recent_tool_calls=[{tool_calls}]",
                    self.delivery_timeout.as_secs(),
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Wait until the Agent has no due Pending Evidence and no review
    /// Run that still holds a lease. Work that is not due yet stays
    /// pending, because the fixture clock does not move here.
    async fn reviewed(&self) -> Result<(), Stall> {
        let store = SqlitePendingEvidenceStore::new(self.daemon.pool().clone());
        let runs = SqliteRunStore::new(self.daemon.pool().clone());
        let deadline = Instant::now() + self.delivery_timeout;
        loop {
            let due = store
                .pending_agents(self.clock.now_ms())
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?
                .iter()
                .any(|(_, agent)| agent == &self.agent);
            let unfinished: HashSet<RunId> = runs
                .list_unfinished()
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?
                .into_iter()
                .map(|run| run.id)
                .collect();
            let reviewing = store
                .leased()
                .await
                .map_err(|error| Stall::Aborted(error.to_string()))?
                .iter()
                .filter(|record| record.agent_id == self.agent)
                .any(|record| {
                    record
                        .lease_run_id
                        .as_ref()
                        .is_none_or(|run| unfinished.contains(run))
                });
            if !due && !reviewing {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Stall::Hang(format!(
                    "the Pending Evidence review did not settle within {} seconds: \
                     due={due}, reviewing={reviewing}",
                    self.delivery_timeout.as_secs(),
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Deliver one evidence item at its acquisition point.
    async fn deliver(&self, position: usize, item: &Evidence) -> Result<(), Stall> {
        self.advance_to(millis(&item.acquired_at).map_err(Stall::Aborted)?)
            .await?;
        if item.input_form == MAIL_FORM {
            return self.imported().await;
        }
        let seen = self.meter.probe_start();
        let owner = self
            .say(&self.dm, &format!("evidence-{position}"), &item.text)
            .await?;
        // A reply does not reflect (ADR-0010). It can record Pending
        // Evidence, and a review Run reflects on it. The driver waits for
        // the reply and then for each due review, so the next item reads
        // the memory and Schedules this one produced.
        let message = self.reply(&self.dm, &owner, seen).await?;
        self.reviewed().await?;
        // A reply run that failed is an aborted turn. The chronology
        // continues, because the probes after it still show what the
        // daemon holds. The record names the delivery and the reason.
        if let Some(reason) = self.run_failure(&message).await {
            self.record_aborted(&delivery_turn(position), &reason);
        }
        Ok(())
    }

    /// Deliver one probe and record what the daemon showed and did.
    async fn probe(&self, case: &Chronology, probe: &Probe) -> ProbeObservation {
        let observation = |output, effects, failure| ProbeObservation {
            probe_id: probe.id.clone(),
            role: probe.role,
            observed_output: output,
            observed_effects: effects,
            failure,
            proposed_grade: None,
            proposed_reason: None,
        };
        let now = match millis(&probe.now) {
            Ok(now) => now,
            Err(reason) => return observation(None, Vec::new(), Some(reason)),
        };
        if let Err(stall) = self.advance_to(now).await {
            self.record(&probe_turn(&probe.id), &stall);
            return observation(None, Vec::new(), Some(stall.into_reason()));
        }
        let seen = self.meter.probe_start();
        // The daemon reads the fixture clock everywhere, the run loop
        // included. The probe still states its own
        // time in words, because the owner would say it.
        let text = format!(
            "{}\n\nThe current time is {} in {}.",
            probe.prompt, probe.now, case.zone
        );
        let turn = probe_turn(&probe.id);
        // Every probe is asked in a channel of its own, so the turn
        // reads no evidence delivery and no earlier probe.
        let channel = match self.probe_channel(&probe.id).await {
            Ok(channel) => channel,
            Err(stall) => {
                self.record(&turn, &stall);
                return observation(None, Vec::new(), Some(stall.into_reason()));
            }
        };
        let owner = match self
            .say(&channel, &format!("probe-{}", probe.id), &text)
            .await
        {
            Ok(owner) => owner,
            Err(stall) => {
                self.record(&turn, &stall);
                return observation(None, Vec::new(), Some(stall.into_reason()));
            }
        };
        match self.reply(&channel, &owner, seen).await {
            Err(stall) => {
                self.record(&turn, &stall);
                observation(
                    None,
                    self.effects(seen, None).await,
                    Some(stall.into_reason()),
                )
            }
            Ok(message) => {
                let failed = self.run_failure(&message).await;
                if let Some(reason) = failed.as_deref() {
                    self.record_aborted(&turn, reason);
                }
                let effects = self.effects(seen, Some(&message)).await;
                observation(Some(message.text_content.clone()), effects, failed)
            }
        }
    }

    /// Ask the configured different route for a proposed grade and
    /// reason for one probe.
    async fn pre_grade(
        &self,
        case: &Chronology,
        probe: &Probe,
        observation: &mut ProbeObservation,
        route: &str,
    ) -> Result<(), String> {
        let input = serde_json::json!({
            "case_id": &case.id,
            "probe": probe,
            "observed_output": &observation.observed_output,
            "observed_effects": &observation.observed_effects,
            "failure": &observation.failure,
        });
        let request = TurnRequest {
            model_alias: "evaluation-pre-grader".into(),
            model_candidates: vec![route.to_string()],
            system: "Grade one evaluation probe. Return one compact JSON object only: {\"grade\":\"pass\",\"reason\":\"short evidence-based reason\"}. The grade must be pass or fail.".into(),
            messages: vec![TurnMessage::user(input.to_string())],
            tools: Vec::new(),
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: Some(PRE_GRADE_OUTPUT_TOKENS),
            output_schema: Some(JsonSchemaFormat {
                name: "evaluation_pre_grade".into(),
                description: Some("A proposed owner grade for one probe".into()),
                schema: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["grade", "reason"],
                    "properties": {
                        "grade": {"type": "string", "enum": ["pass", "fail"]},
                        "reason": {"type": "string"}
                    }
                }),
            }),
        };
        let mut answer = self.pre_grade_answer(request.clone()).await?;
        if answer.trim().is_empty() {
            answer = self.pre_grade_answer(request).await?;
        }
        let proposal = parse_pre_grade(&answer)?;
        observation.proposed_grade = Some(proposal.grade);
        observation.proposed_reason = Some(proposal.reason);
        Ok(())
    }

    async fn pre_grade_answer(&self, request: TurnRequest) -> Result<String, String> {
        let mut stream = self
            .pre_grader
            .turn(request)
            .await
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        while let Some(delta) = stream.next().await {
            match delta.map_err(|error| error.to_string())? {
                TurnDelta::Text(text) => answer.push_str(&text),
                TurnDelta::Finish(_) => break,
                TurnDelta::ModelRetries(_) => {}
                TurnDelta::ToolCall(_) => {
                    return Err("the pre-grader called a tool".into());
                }
            }
        }
        Ok(answer)
    }

    /// Why the run behind one settled reply ended, when it failed. A
    /// failed run still settles its row, so the probe would otherwise
    /// record the daemon's bare `Failed` line and hide the cause.
    async fn run_failure(&self, message: &Message) -> Option<String> {
        let run = SqliteRunStore::new(self.daemon.pool().clone())
            .get(&self.daemon.workspace_id, message.run_id.as_ref()?)
            .await
            .ok()
            .flatten()?;
        (run.state == RunState::Failed).then(|| {
            format!(
                "the reply run failed: {}",
                run.error.as_deref().unwrap_or("no reason recorded")
            )
        })
    }

    /// What the run did: the tools it called, what each call returned,
    /// every visible block that is not prose, and every tool call the
    /// broker parked.
    ///
    /// The meter counts a tool call when the model emits it, which is
    /// before the broker gate. A parked call therefore also appears as
    /// `tool:<name>`. The `request:<name>` line is the one that says
    /// the effect waits for the owner and did not happen.
    ///
    /// A `result:<name>` line summarizes what one call returned.
    /// The last call of a run that never asks the model again has no
    /// result line, because the daemon shows the result to the model
    /// and keeps none.
    async fn effects(&self, seen: MeterCursor, message: Option<&Message>) -> Vec<String> {
        let mut effects: Vec<String> = self
            .meter
            .tool_calls_since(seen)
            .into_iter()
            .map(|name| format!("tool:{name}"))
            .collect();
        effects.extend(self.meter.tool_results_since(seen));
        for block in message
            .map(|message| message.blocks.as_slice())
            .unwrap_or_default()
        {
            let Ok(value) = serde_json::to_value(block) else {
                continue;
            };
            let Some(kind) = value["type"].as_str() else {
                continue;
            };
            if kind == "markdown" {
                continue;
            }
            effects.push(format!("block:{kind}"));
            if let Some(id) = value["request_id"].as_str()
                && let Some(tool) = self.requested_tool(id).await
            {
                effects.push(format!("request:{tool}"));
            }
        }
        effects
    }

    /// The tool one open Request asks the owner about. A card that
    /// gates no tool call, such as a form, names none.
    async fn requested_tool(&self, request_id: &str) -> Option<String> {
        SqliteRequestStore::new(self.daemon.pool().clone())
            .get(
                &self.daemon.workspace_id,
                &RequestId::from(request_id.to_string()),
            )
            .await
            .ok()
            .flatten()
            .and_then(|request| request.payload["tool_name"].as_str().map(str::to_string))
    }

    /// Every visible message from a fired Schedule Run. Messages from
    /// ordinary reply Runs and progress lines are not delivered
    /// interventions. Messages already present at startup, such
    /// as the greeting, are excluded.
    /// Each record also carries how much chronology evidence its
    /// timestamp made visible.
    async fn deliveries(&self, case: &Chronology) -> Vec<DeliveredIntervention> {
        let schedule_runs: HashSet<RunId> = SqliteRunStore::new(self.daemon.pool().clone())
            .list(
                &self.workspace,
                Some(&self.agent),
                Some(&self.dm),
                &[],
                None,
                RUN_EVENT_LIMIT,
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|run| run.trigger_kind == pagis_core::TriggerKind::Schedule)
            .map(|run| run.id)
            .collect();
        let mut entries = self
            .messages()
            .list_top_level(
                &self.daemon.workspace_id,
                &self.dm,
                None,
                CONVERSATION_LIMIT,
            )
            .await
            .unwrap_or_default();
        entries.reverse();
        let messages = self.messages();
        let mut conversation = Vec::new();
        for entry in entries {
            let root = entry.message.id.clone();
            conversation.extend(
                messages
                    .list_thread(&self.daemon.workspace_id, &root)
                    .await
                    .unwrap_or_else(|_| vec![entry.message]),
            );
        }
        conversation
            .into_iter()
            .filter(|message| {
                !self.initial_message_ids.contains(&message.id)
                    && delivered_schedule_message(message, &schedule_runs)
            })
            .map(|message| {
                let after_evidence = acquisition_order(case)
                    .into_iter()
                    .filter(|(_, evidence)| {
                        millis(&evidence.acquired_at)
                            .is_ok_and(|acquired_at| acquired_at <= message.created_at)
                    })
                    .count();
                DeliveredIntervention {
                    after_evidence,
                    at: message.created_at,
                    text: message.text_content,
                }
            })
            .collect()
    }

    /// The observed stages of the intervention pipeline for this repeat.
    async fn intervention_pipeline(&self) -> InterventionPipeline {
        let events = SqliteEventLog::new(self.daemon.pool().clone())
            .list_by_types(
                &self.daemon.workspace_id,
                &[SCHEDULE_CREATED, WAKEUP_STARTED, SCHEDULE_DECISION],
                None,
                PIPELINE_EVENT_LIMIT,
            )
            .await
            .unwrap_or_default();
        let fired: HashSet<_> = events
            .iter()
            .filter(|event| {
                event.event_type == WAKEUP_STARTED
                    && event.payload["source_kind"].as_str() == Some("schedule")
            })
            .filter_map(|event| event.payload["rule_id"].as_str())
            .collect();
        let mut schedules: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == SCHEDULE_CREATED)
            .filter_map(|event| {
                let schedule_id = event.payload["schedule_id"].as_str()?;
                Some(ScheduleObservation {
                    schedule_id: schedule_id.to_string(),
                    next_due_at: event.payload["next_due_at"].as_i64(),
                    fired: fired.contains(schedule_id),
                })
            })
            .collect();
        schedules.sort_by(|left, right| left.schedule_id.cmp(&right.schedule_id));
        let mut pipeline = InterventionPipeline {
            schedules_created: events
                .iter()
                .filter(|event| event.event_type == SCHEDULE_CREATED)
                .count(),
            wake_ups_fired: events
                .iter()
                .filter(|event| {
                    event.event_type == WAKEUP_STARTED
                        && event.payload["source_kind"].as_str() == Some("schedule")
                })
                .count(),
            schedules,
            ..Default::default()
        };
        for decision in events
            .iter()
            .filter(|event| event.event_type == SCHEDULE_DECISION)
            .filter_map(|event| event.payload["decision"].as_str())
        {
            match decision {
                "sent" => pipeline.decisions.sent += 1,
                "rescheduled" => pipeline.decisions.rescheduled += 1,
                "silent" => pipeline.decisions.silent += 1,
                _ => {}
            }
        }
        pipeline
    }

    /// The brittleness count of this replay.
    ///
    /// It holds no unscored metric: intervention precision has one
    /// denominator, the whole suite's, so a suite that delivered
    /// nothing counts it once, at the suite.
    async fn system_failures(&self) -> SystemFailures {
        self.failures.lock().expect("failure lock").clone()
    }
}

#[derive(Debug, Deserialize)]
struct PreGrade {
    grade: String,
    reason: String,
}

fn parse_pre_grade(answer: &str) -> Result<PreGrade, String> {
    let proposal = repair_json(answer)
        .map(|(proposal, _)| proposal)
        .or_else(|| extract_pre_grade(answer));
    let Some(proposal) = proposal else {
        let error = serde_json::from_str::<PreGrade>(answer)
            .expect_err("an unrepaired pre-grade must not parse");
        return Err(format!("invalid pre-grade: {error}"));
    };
    if !matches!(proposal.grade.as_str(), "pass" | "fail") {
        return Err("the pre-grader returned neither pass nor fail".into());
    }
    if proposal.reason.trim().is_empty() {
        return Err("the pre-grader returned no reason".into());
    }
    Ok(proposal)
}

/// Recover completed grade and reason fields from an object that lost
/// its closing quote or brace.
fn extract_pre_grade(answer: &str) -> Option<PreGrade> {
    let grade = regex::Regex::new(r#"(?s)"grade"\s*:\s*"(pass|fail)""#)
        .expect("fixed grade expression")
        .captures(answer)?
        .get(1)?
        .as_str()
        .to_string();
    let reason = regex::Regex::new(r#"(?s)"reason"\s*:\s*"((?:\\.|[^"])*)"#)
        .expect("fixed reason expression")
        .captures(answer)?
        .get(1)?
        .as_str();
    let reason = serde_json::from_str(&format!("\"{reason}\"")).ok()?;
    Some(PreGrade { grade, reason })
}

/// How the record names one evidence delivery.
fn delivery_turn(position: usize) -> String {
    format!("delivery {position}")
}

/// How the record names one probe.
fn probe_turn(probe_id: &str) -> String {
    format!("probe `{probe_id}`")
}

/// Why one delivery or one probe produced no settled reply. The
/// two kinds count separately against the brittleness gate.
enum Stall {
    /// No settled reply arrived inside the delivery timeout.
    Hang(String),
    /// The turn ended with no answer: the daemon refused the message,
    /// the import gave up, or the reply run failed.
    Aborted(String),
}

impl Stall {
    /// What the record says about this stall.
    fn into_reason(self) -> String {
        match self {
            Self::Hang(reason) | Self::Aborted(reason) => reason,
        }
    }
}

/// The settled reply of one run, from the conversation newest first.
///
/// It is the newest complete message of that run after the owner's own
/// message. A message the daemon published on its own carries no run,
/// so it never stands as a reply, and the progress line of the run is
/// older than its answer.
fn run_reply(
    newest_first: impl IntoIterator<Item = Message>,
    owner: &MessageId,
    run: &RunId,
) -> Option<Message> {
    newest_first.into_iter().find(|message| {
        message.author_kind == AuthorKind::Agent
            && message.status == MessageStatus::Complete
            && message.run_id.as_ref() == Some(run)
            && message.id.as_str() > owner.as_str()
    })
}

fn delivered_schedule_message(message: &Message, schedule_runs: &HashSet<RunId>) -> bool {
    message.author_kind == AuthorKind::Agent
        && message.status == MessageStatus::Complete
        && message
            .run_id
            .as_ref()
            .is_some_and(|run| schedule_runs.contains(run))
}

#[async_trait]
impl ChronologyDriver for DaemonDriver {
    async fn run(&mut self, case: &Chronology, repeat: u8) -> DriverResult {
        self.0.run(case, repeat).await
    }

    fn name(&self) -> &str {
        "daemon"
    }

    fn store(&self) -> StoreObservability {
        StoreObservability::SubjectPages
    }

    fn model_route(&self) -> &str {
        &self.0.route
    }

    fn routes_under_test(&self) -> Vec<String> {
        self.0.candidates.clone()
    }

    fn clock_version(&self) -> &str {
        &self.0.clock_version
    }

    fn zone_rule_version(&self) -> &str {
        &self.0.zone_rule_version
    }

    fn set_pre_grader_route(&mut self, route: &str) {
        self.0.pre_grader_route = Some(route.to_string());
    }

    fn run_context(&self) -> Vec<String> {
        Vec::new()
    }
}

impl Replayer {
    async fn replay(&self, case: &Chronology, repeat: u8) -> DriverResult {
        let replay = match Replay::start(
            case,
            Arc::clone(&self.model),
            &self.candidates,
            self.delivery_timeout,
            self.import_timeout,
        )
        .await
        {
            Ok(replay) => replay,
            Err(reason) => {
                return unscored(format!("{reason} (chronology {} repeat {repeat})", case.id));
            }
        };
        let mut observations = Vec::new();
        let mut failure = None;
        let mut delivered = 0;
        for (position, item) in acquisition_order(case) {
            for probe in case.probes.iter().filter(|p| p.after_evidence == delivered) {
                observations.push(replay.probe(case, probe).await);
            }
            if let Err(stall) = replay.deliver(position, item).await {
                replay.record(&delivery_turn(position), &stall);
                failure = Some(stall.into_reason());
                break;
            }
            delivered = position;
        }
        if failure.is_none() {
            for probe in case.probes.iter().filter(|p| p.after_evidence == delivered) {
                observations.push(replay.probe(case, probe).await);
            }
        }
        if failure.is_none()
            && let Some(window_end) = case
                .expected_interventions
                .iter()
                .filter_map(|expected| millis(&expected.not_after).ok())
                .max()
            && window_end > replay.clock.now_ms()
            && let Err(stall) = replay.advance_to(window_end).await
        {
            replay.record("final expected intervention window", &stall);
            failure = Some(stall.into_reason());
        }
        // A probe the driver never reached is recorded as a failure, not
        // left out: a missing row must not read as a pass.
        for probe in case.probes.iter() {
            if !observations.iter().any(|seen| seen.probe_id == probe.id) {
                observations.push(ProbeObservation {
                    probe_id: probe.id.clone(),
                    role: probe.role,
                    observed_output: None,
                    observed_effects: Vec::new(),
                    failure: Some(
                        failure
                            .clone()
                            .unwrap_or_else(|| "the run ended before this probe".into()),
                    ),
                    proposed_grade: None,
                    proposed_reason: None,
                });
            }
        }
        if let Some(route) = self.pre_grader_route.as_deref() {
            for observation in &mut observations {
                let Some(probe) = case
                    .probes
                    .iter()
                    .find(|probe| probe.id == observation.probe_id)
                else {
                    continue;
                };
                if let Err(reason) = replay.pre_grade(case, probe, observation, route).await {
                    observation.proposed_reason = Some(reason);
                }
            }
        }
        let delivered_interventions = replay.deliveries(case).await;
        let intervention_pipeline = replay.intervention_pipeline().await;
        let system_failures = replay.system_failures().await;
        let model = replay.meter.spend();
        let usage = Usage {
            model_calls: model.model_calls,
            input_tokens: model.input_tokens,
            output_tokens: model.output_tokens,
            source_reads: replay.fixture.reads(),
            elapsed_millis: 0,
            usd: self
                .spend
                .rate
                .cost(model.input_tokens, model.output_tokens),
        };
        let status = if failure.is_some() || observations.iter().any(|seen| seen.failure.is_some())
        {
            RunStatus::Failed
        } else {
            RunStatus::Complete
        };
        DriverResult {
            status,
            missing_capabilities: Vec::new(),
            observations,
            delivered_interventions,
            intervention_pipeline,
            system_failures,
            usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{Block, WorkspaceId};

    use crate::{Script, ScriptedBrain};

    const FILING_PAGE: &str = "private/subjects/messages/filing.md";

    struct DeadlineBrain {
        requests: Mutex<Vec<TurnRequest>>,
        schedule_local_time: String,
        moved_local_time: Option<String>,
        timezone: String,
        title: String,
        compiled_truth: String,
    }

    impl Default for DeadlineBrain {
        fn default() -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                schedule_local_time: "2026-01-01T10:00:00".into(),
                moved_local_time: None,
                timezone: "UTC".into(),
                title: "Filing conflict".into(),
                compiled_truth: "Deadline: resolve the filing conflict \
                    before 11:00 UTC. Act by: 2026-01-01 10:00 UTC. Wake at 2026-01-01 \
                    10:00 UTC because the owner needs time to resolve the conflict."
                    .into(),
            }
        }
    }

    impl DeadlineBrain {
        fn captured_requests(&self) -> usize {
            self.requests.lock().expect("request lock").len()
        }

        fn requests(&self) -> Vec<TurnRequest> {
            self.requests.lock().expect("request lock").clone()
        }

        fn listed_schedule(request: &TurnRequest) -> Option<(String, u64)> {
            request
                .messages
                .iter()
                .filter(|message| message.role == pagis_agent::TurnRole::Tool)
                .filter_map(|message| serde_json::from_str::<serde_json::Value>(&message.text).ok())
                .find_map(|result| {
                    // The Agent also owns Schedules for other matters,
                    // such as the Daily report. Only the Schedule of the
                    // filing page serves this matter.
                    let schedule = result["items"]
                        .as_array()?
                        .iter()
                        .find(|schedule| schedule["subject_page_path"] == FILING_PAGE)?;
                    Some((
                        schedule["schedule_id"].as_str()?.to_string(),
                        schedule["revision"].as_u64()?,
                    ))
                })
        }
    }

    #[async_trait]
    impl Brain for DeadlineBrain {
        async fn turn(
            &self,
            request: TurnRequest,
        ) -> Result<pagis_agent::TurnStream, pagis_agent::BrainError> {
            self.requests
                .lock()
                .expect("request lock")
                .push(request.clone());
            let scripted = ScriptedBrain::default();
            let is_reflection = request
                .messages
                .iter()
                .any(|message| message.text.starts_with("The run is over."));
            let is_probe = request
                .messages
                .iter()
                .any(|message| message.text.contains("The current time is"));
            let called = |name: &str| {
                request
                    .messages
                    .iter()
                    .flat_map(|message| &message.tool_calls)
                    .any(|call| call.name == name)
            };
            let is_schedule = request
                .messages
                .iter()
                .any(|message| message.text.contains("Schedule trigger:"));
            let response = if is_schedule && !is_reflection {
                if request
                    .tools
                    .iter()
                    .any(|tool| tool.name == pagis_broker::HOST_SHELL)
                {
                    Script::tool_call(
                        &[],
                        pagis_broker::HOST_SHELL,
                        serde_json::json!({"command": "date"}),
                    )
                } else {
                    Script::reply(&["The filing conflict begins in one hour."])
                }
            } else if is_reflection && !called("memory_write") {
                Script::tool_call(
                    &[],
                    "memory_write",
                    serde_json::json!({
                        "path": FILING_PAGE,
                        "content": pagis_core::subject_page::SubjectPage {
                            front_matter: pagis_core::subject_page::FrontMatter {
                                title: Some(self.title.clone()),
                                kind: Some("Event".into()),
                                ..Default::default()
                            },
                            truth: self.compiled_truth.clone(),
                            ..Default::default()
                        }.render(),
                        "expected_memory_revision": "missing"
                    }),
                )
            } else if is_reflection && !called("schedule_list") {
                Script::tool_call(&[], "schedule_list", serde_json::json!({}))
            } else if is_reflection {
                if let Some((schedule_id, revision)) = Self::listed_schedule(&request) {
                    if !called("schedule_update") {
                        Script::tool_call(
                            &[],
                            "schedule_update",
                            serde_json::json!({
                                "schedule_id": schedule_id,
                                "action": "edit",
                                "expected_revision": revision,
                                "kind": "one_shot",
                                "name": "Filing deadline",
                                "instruction": "Warn the owner before action is due",
                                "channel": "user",
                                "local_time": self.moved_local_time.as_deref()
                                    .unwrap_or(&self.schedule_local_time),
                                "timezone": self.timezone.clone(),
                                "wake_only": true,
                                "subject_page_path": FILING_PAGE
                            }),
                        )
                    } else {
                        Script::reply(&["Moved the filing warning."])
                    }
                } else if !called("schedule_create") {
                    Script::tool_call(
                        &[],
                        "schedule_create",
                        serde_json::json!({
                            "kind": "one_shot",
                            "name": "Filing deadline",
                            "instruction": "Warn the owner before action is due",
                            "local_time": self.schedule_local_time.clone(),
                            "timezone": self.timezone.clone(),
                            "wake_only": true,
                            "subject_page_path": FILING_PAGE
                        }),
                    )
                } else {
                    Script::reply(&["Scheduled the filing warning."])
                }
            } else if !is_probe && !called(pagis_broker::MEMORY_REVIEW) {
                // A reply does not reflect (ADR-0010). The deadline goes
                // to an urgent Pending Evidence review, and that review
                // Run makes the intervention decision.
                Script::tool_call(
                    &[],
                    pagis_broker::MEMORY_REVIEW,
                    serde_json::json!({
                        "subject": "filing",
                        "reason": "The evidence sets a deadline that needs a wake-up.",
                        "urgency": "urgent"
                    }),
                )
            } else {
                Script::reply(&["I will remember the filing deadline."])
            };
            scripted.push(response);
            scripted.turn(request).await
        }
    }

    fn deadline_chronology() -> Chronology {
        serde_json::from_value(serde_json::json!({
            "id": "filing-deadline",
            "partition": "development",
            "zone": "UTC",
            "evidence": [{
                "source_id": "owner-message-1",
                "source_version": "1",
                "source_ref": "fixture://filing-deadline/1",
                "input_form": "chat",
                "occurred_at": "2026-01-01T09:00:00Z",
                "valid_at": "2026-01-01T09:00:00Z",
                "acquired_at": "2026-01-01T09:00:00Z",
                "scope": "private",
                "text": "The filing cutoff and an appointment conflict today at 11:00 UTC."
            }],
            "probes": [{
                "id": "help",
                "role": "help",
                "after_evidence": 1,
                "now": "2026-01-01T09:30:00Z",
                "prompt": "What needs attention?",
                "permissible_conclusions": [],
                "required_support": [],
                "counterevidence": [],
                "expected_connections": [],
                "allowed_alternatives": [],
                "prohibited": [],
                "uncertainty": []
            }],
            "expected_interventions": [{
                "id": "filing-warning",
                "after_evidence": 1,
                "not_before": "2026-01-01T10:00:00Z",
                "not_after": "2026-01-01T10:59:00Z",
                "purpose": "Warn the owner before the same-day filing conflict.",
                "required_support": ["fixture://filing-deadline/1"]
            }]
        }))
        .expect("deadline chronology")
    }

    fn timing_chronology(id: &str, acquired_at: &str, evidence: &str) -> Chronology {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "partition": "development",
            "zone": "UTC",
            "evidence": [{
                "source_id": format!("owner-{id}"),
                "source_version": "1",
                "source_ref": format!("fixture://{id}/1"),
                "input_form": "chat",
                "occurred_at": acquired_at,
                "valid_at": acquired_at,
                "acquired_at": acquired_at,
                "scope": "private",
                "text": evidence
            }],
            "probes": [],
            "expected_interventions": []
        }))
        .expect("timing chronology")
    }

    fn moved_deadline_chronology() -> Chronology {
        serde_json::from_value(serde_json::json!({
            "id": "moved-deadline",
            "partition": "development",
            "zone": "UTC",
            "evidence": [
                {
                    "source_id": "owner-moved-deadline",
                    "source_version": "1",
                    "source_ref": "fixture://moved-deadline/1",
                    "input_form": "chat",
                    "occurred_at": "2026-01-05T09:00:00Z",
                    "valid_at": "2026-01-05T09:00:00Z",
                    "acquired_at": "2026-01-05T09:00:00Z",
                    "scope": "private",
                    "text": "The reply is due on Friday."
                },
                {
                    "source_id": "owner-moved-deadline",
                    "source_version": "2",
                    "source_ref": "fixture://moved-deadline/2",
                    "input_form": "chat",
                    "occurred_at": "2026-01-05T10:00:00Z",
                    "valid_at": "2026-01-05T10:00:00Z",
                    "acquired_at": "2026-01-05T10:00:00Z",
                    "scope": "private",
                    "text": "The reply is urgent. Act by Tuesday morning."
                }
            ],
            "probes": [],
            "expected_interventions": []
        }))
        .expect("moved deadline chronology")
    }

    fn driver_for(brain: Arc<DeadlineBrain>) -> DaemonDriver {
        let route = "scripted-timing";
        DaemonDriver::new(
            brain as Arc<dyn Brain>,
            route,
            vec!["anthropic/claude-haiku-4-5".to_string()],
            EvaluationSpend {
                authorization: Authorization {
                    max_usd: 0.0,
                    priced_routes: std::collections::BTreeSet::from([route.to_string()]),
                },
                reserve_per_run_usd: 0.0,
                rate: RouteRate {
                    input_usd_per_mtok: 0.0,
                    output_usd_per_mtok: 0.0,
                },
            },
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exhausted_router_failure_records_the_provider_error_in_the_aborted_turn() {
        let route = "scripted-rate-limit";
        let brain = Arc::new(ScriptedBrain::default());
        let error = llm_router::Error::Exhausted {
            model: "default".into(),
            attempts: 1,
            last: Box::new(llm_router::Error::Provider {
                provider: "openai".into(),
                status: 429,
                kind: llm_router::ErrorKind::RateLimit,
                message: "You have no credits remaining".into(),
                raw: Some(serde_json::json!({
                    "error": {"message": "You have no credits remaining"}
                })),
            }),
        };
        brain.push(Script::fail_after(&["Half"], &error.to_string()));
        let mut driver = DaemonDriver::new(
            brain as Arc<dyn Brain>,
            route,
            vec!["anthropic/claude-haiku-4-5".to_string()],
            EvaluationSpend {
                authorization: Authorization {
                    max_usd: 0.0,
                    priced_routes: std::collections::BTreeSet::from([route.to_string()]),
                },
                reserve_per_run_usd: 0.0,
                rate: RouteRate {
                    input_usd_per_mtok: 0.0,
                    output_usd_per_mtok: 0.0,
                },
            },
        );

        let result = driver
            .run(
                &timing_chronology(
                    "rate-limit",
                    "2026-01-01T09:00:00Z",
                    "Remember this evidence.",
                ),
                1,
            )
            .await;

        assert_eq!(
            result.system_failures.aborted_turns,
            [
                "delivery 1: the reply run failed: all candidates for model `default` failed: \
                 provider `openai` returned status 429 (RateLimit): You have no credits remaining"
            ]
        );
    }

    fn recorded_tool_arguments(brain: &DeadlineBrain, name: &str) -> serde_json::Value {
        brain
            .requests()
            .iter()
            .flat_map(|request| &request.messages)
            .flat_map(|message| &message.tool_calls)
            .find(|call| call.name == name)
            .map(|call| serde_json::from_str(&call.arguments).expect("tool arguments"))
            .unwrap_or_else(|| panic!("no recorded {name} call"))
    }

    /// One message of the owner conversation, with the run that wrote
    /// it. `None` is help the daemon published on its own.
    fn message(id: &str, author: AuthorKind, run: Option<&str>, text: &str) -> Message {
        Message {
            id: MessageId::from(id.to_string()),
            workspace_id: WorkspaceId::from("workspace".to_string()),
            channel_id: ChannelId::from("channel".to_string()),
            parent_message_id: None,
            author_kind: author,
            author_agent_id: None,
            run_id: run.map(|id| RunId::from(id.to_string())),
            status: MessageStatus::Complete,
            blocks: vec![Block::markdown(text.to_string())],
            text_content: text.to_string(),
            pending_id: None,
            created_at: 1,
            completed_at: Some(1),
        }
    }

    /// An unrelated message that lands during the run is not the probe's
    /// reply.
    #[test]
    fn an_unrelated_message_during_a_run_is_not_the_probe_s_reply() {
        let owner = MessageId::from("02-probe".to_string());
        let run = RunId::from("run-2".to_string());
        // The store lists a conversation newest first.
        let conversation = vec![
            message("05-answer", AuthorKind::Agent, Some("run-2"), "the reply"),
            message("04-other", AuthorKind::Agent, None, "another message"),
            message("03-progress", AuthorKind::Agent, Some("run-2"), "working"),
            message("02-probe", AuthorKind::User, None, "the probe"),
            message(
                "01-earlier",
                AuthorKind::Agent,
                Some("run-1"),
                "an earlier reply",
            ),
        ];

        let reply = run_reply(conversation, &owner, &run).expect("the run's own reply");

        assert_eq!(reply.text_content, "the reply");
    }

    /// A run that has written nothing has no reply, so the wait
    /// continues and another message in the conversation does not end it.
    #[test]
    fn a_run_that_has_written_nothing_has_no_reply_yet() {
        let owner = MessageId::from("02-probe".to_string());
        let run = RunId::from("run-2".to_string());
        let conversation = vec![
            message("04-other", AuthorKind::Agent, None, "another message"),
            message("02-probe", AuthorKind::User, None, "the probe"),
        ];

        assert!(run_reply(conversation, &owner, &run).is_none());
    }

    /// A message from a fired Schedule Run is a delivered intervention.
    /// A normal reply is not.
    #[test]
    fn a_fired_schedule_message_is_a_delivered_intervention() {
        let schedules = std::collections::HashSet::from([RunId::from("schedule-run".to_string())]);
        let delivered = message(
            "03-scheduled-help",
            AuthorKind::Agent,
            Some("schedule-run"),
            "The appointment is tomorrow.",
        );
        let reply = message(
            "02-reply",
            AuthorKind::Agent,
            Some("message-run"),
            "Here is your answer.",
        );

        assert!(delivered_schedule_message(&delivered, &schedules));
        assert!(!delivered_schedule_message(&reply, &schedules));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_same_day_conflict_schedules_that_morning_and_fired_run_offers_only_its_outcome_tool()
    {
        let route = "scripted-deadline";
        let brain = Arc::new(DeadlineBrain::default());
        let mut driver = DaemonDriver::new(
            Arc::clone(&brain) as Arc<dyn Brain>,
            route,
            vec!["anthropic/claude-haiku-4-5".to_string()],
            EvaluationSpend {
                authorization: Authorization {
                    max_usd: 0.0,
                    priced_routes: std::collections::BTreeSet::from([route.to_string()]),
                },
                reserve_per_run_usd: 0.0,
                rate: RouteRate {
                    input_usd_per_mtok: 0.0,
                    output_usd_per_mtok: 0.0,
                },
            },
        )
        .with_delivery_timeout(Duration::from_secs(2));

        let result = driver.run(&deadline_chronology(), 1).await;

        assert_eq!(
            result.delivered_interventions.len(),
            1,
            "intervention pipeline: schedules_created={}, wake_ups_fired={}, decisions={:?}; \
             captured_requests={}",
            result.intervention_pipeline.schedules_created,
            result.intervention_pipeline.wake_ups_fired,
            result.intervention_pipeline.decisions,
            brain.captured_requests(),
        );
        assert_eq!(result.delivered_interventions[0].at, 1_767_261_600_000);
        assert_eq!(result.delivered_interventions[0].after_evidence, 1);
        assert_eq!(result.intervention_pipeline.schedules_created, 1);
        assert_eq!(result.intervention_pipeline.wake_ups_fired, 1);
        assert_eq!(result.intervention_pipeline.schedules.len(), 1);
        assert_eq!(
            result.intervention_pipeline.schedules[0].next_due_at,
            Some(1_767_261_600_000)
        );
        assert!(result.intervention_pipeline.schedules[0].fired);
        assert_eq!(result.intervention_pipeline.decisions.sent, 1);
        assert_eq!(result.intervention_pipeline.decisions.rescheduled, 0);
        assert_eq!(result.intervention_pipeline.decisions.silent, 0);
        let schedule = recorded_tool_arguments(&brain, "schedule_create");
        assert_eq!(schedule["local_time"], "2026-01-01T10:00:00");
        let write = recorded_tool_arguments(&brain, "memory_write");
        assert!(
            write["content"]
                .as_str()
                .unwrap()
                .contains("because the owner needs time to resolve the conflict")
        );
        let decision_request = brain
            .requests()
            .into_iter()
            .find(|request| {
                request.messages.iter().any(|message| {
                    message.text.starts_with("The run is over.")
                        && message.text.contains("Current logical time: 1767258000000")
                        && message.text.contains("Owner time zone: UTC")
                })
            })
            .expect("reflection decision with logical time and time zone");
        assert!(decision_request.messages.iter().any(|message| {
            message.text.contains("first moment the owner must act")
                && message.text.contains("never the event itself")
        }),);
        let fired_request = brain
            .requests()
            .into_iter()
            .find(|request| {
                request
                    .messages
                    .iter()
                    .any(|message| message.text.contains("Schedule trigger:"))
            })
            .expect("fired Schedule request");
        assert!(!fired_request.messages.is_empty());
        assert!(
            fired_request.messages[0]
                .text
                .starts_with("System: Schedule trigger:")
        );
        assert!(!fired_request.system.contains("Schedule trigger:"));
        assert!(fired_request.system.contains("Filing"));
        assert_eq!(
            fired_request
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec![pagis_broker::SCHEDULE_UPDATE],
        );
        assert_eq!(
            pagis_evaluation::account_interventions(
                &deadline_chronology(),
                &result.delivered_interventions,
            ),
            pagis_evaluation::InterventionAccounting {
                expected: 1,
                delivered: 1,
                matched: 1,
                early: 0,
                late: 0,
                unexpected: 0,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_monday_summons_with_a_seven_day_reply_schedules_tuesday_morning() {
        let brain = Arc::new(DeadlineBrain {
            schedule_local_time: "2026-01-06T09:00:00".into(),
            title: "Jury summons".into(),
            compiled_truth: "Deadline: reply by 2026-01-12. Act by: \
                2026-01-12. Wake at 2026-01-06 09:00 UTC because the owner has not \
                been told. The jury date is not the deadline."
                .into(),
            ..Default::default()
        });
        let mut driver = driver_for(Arc::clone(&brain));

        let result = driver
            .run(
                &timing_chronology(
                    "jury-summons",
                    "2026-01-05T09:00:00Z",
                    "A jury summons arrived on Monday. Reply within seven days. The jury date is 20 January.",
                ),
                1,
            )
            .await;

        assert_eq!(result.status, RunStatus::Complete);
        assert_eq!(
            result.intervention_pipeline.schedules[0].next_due_at,
            Some(1_767_690_000_000),
        );
        let schedule = recorded_tool_arguments(&brain, "schedule_create");
        assert_eq!(schedule["local_time"], "2026-01-06T09:00:00");
        let write = recorded_tool_arguments(&brain, "memory_write");
        let truth = write["content"].as_str().unwrap();
        assert!(truth.contains("Deadline: reply by 2026-01-12"));
        assert!(truth.contains("Act by: 2026-01-12"));
        assert!(truth.contains("jury date is not the deadline"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_wednesday_delivery_and_lift_outage_schedule_the_next_morning_after_evidence() {
        let brain = Arc::new(DeadlineBrain {
            schedule_local_time: "2026-01-08T08:00:00".into(),
            title: "Delivery conflict".into(),
            compiled_truth: "Deadline: arrange access before the \
                Wednesday delivery. Act by: 2026-01-14. Wake at 2026-01-08 08:00 \
                UTC because this is the next morning after the evidence."
                .into(),
            ..Default::default()
        });
        let mut driver = driver_for(Arc::clone(&brain));

        let result = driver
            .run(
                &timing_chronology(
                    "delivery-lift-outage",
                    "2026-01-07T15:00:00Z",
                    "The delivery is Wednesday 14 January. The lift is out on Wednesday 14 January.",
                ),
                1,
            )
            .await;

        assert_eq!(result.status, RunStatus::Complete);
        assert_eq!(
            result.intervention_pipeline.schedules[0].next_due_at,
            Some(1_767_859_200_000),
        );
        let schedule = recorded_tool_arguments(&brain, "schedule_create");
        assert_eq!(schedule["local_time"], "2026-01-08T08:00:00");
        let write = recorded_tool_arguments(&brain, "memory_write");
        let truth = write["content"].as_str().unwrap();
        assert!(truth.contains("Deadline: arrange access"));
        assert!(truth.contains("Act by: 2026-01-14"));
        assert!(truth.contains("next morning after the evidence"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_with_an_existing_schedule_moves_it_instead_of_creating_a_second() {
        let brain = Arc::new(DeadlineBrain {
            schedule_local_time: "2026-01-09T09:00:00".into(),
            moved_local_time: Some("2026-01-06T09:00:00".into()),
            title: "Reply deadline".into(),
            compiled_truth: "Deadline: reply by Tuesday morning. \
                Act by: 2026-01-06 09:00 UTC."
                .into(),
            ..Default::default()
        });
        let mut driver = driver_for(Arc::clone(&brain));

        let result = driver.run(&moved_deadline_chronology(), 1).await;

        assert_eq!(result.status, RunStatus::Complete);
        assert_eq!(result.intervention_pipeline.schedules_created, 1);
        let moved = recorded_tool_arguments(&brain, "schedule_update");
        assert_eq!(moved["action"], "edit");
        assert_eq!(moved["local_time"], "2026-01-06T09:00:00");
        assert_eq!(moved["wake_only"], true);
        assert_eq!(moved["subject_page_path"], FILING_PAGE);
    }
}
