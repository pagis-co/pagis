//! Resident actors and the trigger router: one lazily
//! spawned actor per agent routes DM triggers into runs, enforces the
//! concurrent-run cap and per-thread serialization, injects messages
//! into a thread's active run, handles cancel, and despawns after
//! idle.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use object_store::ObjectStore;
use pagis_broker::Broker;
use pagis_core::{
    AgentId, AgentStatus, AgentStore, Author, Block, BriefStore, ChannelKind, ChannelStore,
    ConnectionStore, EventBus, EventScope, FailureKind, FileStore, GrantStore, KnownBlock,
    MemoryStore, Message, MessageId, MessageStore, ModelAliasStore, NewEvent, ParticipantStore,
    RequestStore, Run, RunId, RunOrigin, RunSlots, RunState, RunStore, ScheduleStore, TriggerKind,
    TriggerStore, WorkspaceStore,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::admission::{Admission, AdmissionRun, CancelAction, Pool, ThreadKey};
use crate::brain::Brain;
use crate::hub::StreamHub;
use crate::progress::ProgressHub;
use crate::run::{RunOutcome, execute, fail, transition};
use crate::tool_runtime::CoreToolRuntime;
use crate::{RunQueue, RunSignals};

/// Loop tuning. Tests shrink the idle timeout.
#[derive(Debug, Clone, Copy)]
pub struct AgentLoopConfig {
    /// Concurrent conversation runs one agent executes; further runs
    /// queue. An arrival run does not take one of these slots.
    pub max_concurrent_runs: usize,
    /// Concurrent arrival runs one agent executes (ADR-0002). The pool
    /// is separate from `max_concurrent_runs` so a sync in the
    /// background cannot fill the slots a user message needs.
    pub max_arrival_runs: usize,
    /// An actor with no runs and empty queues despawns after this.
    pub idle_timeout: Duration,
    /// Top-level history window for a top-level run's context.
    pub context_messages: u32,
    /// Turn budget per run: the turns that may call tools. One answer
    /// turn with no tool calls follows the last of them, and the run
    /// then fails at the limit.
    pub max_turns: usize,
    /// The delegation hop cap: a message whose trigger chain
    /// already carries this many agent-to-agent hops triggers nothing.
    pub max_hops: u32,
    /// Whether the Agent has a Computer. A daemon without one removes
    /// its Computer tools from the run capability snapshot.
    pub computer: bool,
}

impl AgentLoopConfig {
    /// Every slot of both pools, the answer for an Agent with no actor.
    fn all_slots(&self) -> RunSlots {
        RunSlots {
            conversation: slot_count(self.max_concurrent_runs),
            arrival: slot_count(self.max_arrival_runs),
        }
    }
}

fn slot_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 3,
            max_arrival_runs: 1,
            idle_timeout: Duration::from_secs(600),
            context_messages: 50,
            max_turns: 50,
            max_hops: 8,
            computer: true,
        }
    }
}

/// Everything the loop reads and writes, behind the storage seams.
#[derive(Clone)]
pub struct AgentDeps {
    pub disclosure: Arc<dyn pagis_core::DisclosureStore>,
    pub person_activity: Arc<pagis_needs_you::PersonActivity>,
    pub run_journal: Arc<dyn pagis_core::RunJournal>,
    pub forget: Arc<dyn pagis_core::ForgetStore>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub users: Arc<dyn pagis_core::UserStore>,
    pub workspace_members: Arc<dyn pagis_core::WorkspaceMemberStore>,
    pub agents: Arc<dyn AgentStore>,
    pub channels: Arc<dyn ChannelStore>,
    pub messages: Arc<dyn MessageStore>,
    /// Text Message bodies, read only when their Incoming Events start a Run.
    pub text_records: Arc<dyn pagis_core::TextRecordStore>,
    pub runs: Arc<dyn RunStore>,
    pub model_aliases: Arc<dyn ModelAliasStore>,
    /// What each model call spent. The loop writes one Usage Record per call.
    pub usage: Arc<dyn pagis_core::UsageStore>,
    /// The live Model Request Capture setting (ADR-0031).
    pub capture: Arc<pagis_core::CaptureSetting>,
    /// Where the loop keeps a capture while the setting is on.
    pub model_request_captures: Arc<dyn pagis_core::ModelRequestCaptureStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub requests: Arc<dyn RequestStore>,
    pub grants: Arc<dyn GrantStore>,
    /// The alias of each Connection, which names a synced knowledge
    /// source in the turn's prompt.
    pub connections: Arc<dyn ConnectionStore>,
    pub files: Arc<dyn FileStore>,
    pub blobs: Arc<dyn ObjectStore>,
    pub memory: Arc<dyn MemoryStore>,
    /// Durable reasons for a later evidence review.
    pub pending_evidence: Arc<dyn pagis_core::PendingEvidenceStore>,
    /// The one durable working-context checkpoint per conversation.
    pub continuations: Arc<dyn pagis_core::ContinuationStore>,
    /// Per-conversation Brief cursors and shown-page sets.
    pub briefs: Arc<dyn BriefStore>,
    /// Schedules whose metadata can point at a Subject Page.
    pub schedules: Arc<dyn ScheduleStore>,
    /// The Skills the agent holds (ADR-0017): the prompt lists
    /// them, and `skill_load` reads one.
    pub skills: Arc<dyn pagis_core::Skills>,
    /// The agent computer lifecycle + screen, one manager per
    /// tenant. A Run resolves the manager of its own Workspace.
    pub computers: Arc<dyn pagis_core::Computers>,
    pub bus: Arc<dyn EventBus>,
    pub brain: Arc<dyn Brain>,
    /// The host's check before each model call.
    pub gate: Arc<dyn crate::ModelCallGate>,
    /// The Provider Model Lists, which give the context budget and the
    /// Spend Cap the metadata of a model the table does not know.
    pub models: Arc<crate::ModelCatalog>,
    pub broker: Arc<Broker>,
    pub tool_runtime: Arc<CoreToolRuntime>,
    pub hub: Arc<StreamHub>,
    /// The derived run progress: the live line per run.
    pub progress: Arc<ProgressHub>,
    /// Proactive source facts used to build the Schedule briefing.
    pub triggers: Arc<dyn TriggerStore>,
    /// The logical time for messages and fired Schedule Runs.
    pub clock: Arc<dyn pagis_core::Clock>,
    pub config: AgentLoopConfig,
}

/// The answer to a cancel request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The run was queued or running; cancellation is under way.
    Canceling,
    /// The run already ended in this state; nothing to cancel.
    AlreadyEnded(RunState),
    NotFound,
}

enum ActorMsg {
    Trigger(Box<Message>),
    Proactive(Box<Run>),
    Available(oneshot::Sender<RunSlots>),
    Cancel(RunId, oneshot::Sender<bool>),
    /// A run ended; its injection receiver comes back so the actor can
    /// requeue any message the run never consumed.
    Ended(RunId, mpsc::UnboundedReceiver<Message>),
}

type ActorMap = Arc<Mutex<HashMap<AgentId, mpsc::UnboundedSender<ActorMsg>>>>;

/// The trigger router. Holds the resident actors and routes triggers
/// and cancels to them.
pub struct AgentSystem {
    pub(crate) deps: Arc<AgentDeps>,
    actors: ActorMap,
}

impl AgentSystem {
    /// Start the system: subscribe to the event bus and route every
    /// user `message.completed` in a channel with agent participants.
    /// The subscription is live before this returns, so no message
    /// sent after startup misses its trigger.
    pub async fn start(deps: AgentDeps) -> Arc<Self> {
        let system = Arc::new(Self {
            deps: Arc::new(deps),
            actors: Arc::new(Mutex::new(HashMap::new())),
        });
        let events = system
            .deps
            .bus
            .subscribe(EventScope::Installation, None)
            .await;
        let dispatcher = Arc::clone(&system);
        pagis_rt::spawn(async move { dispatcher.dispatch(events).await });
        // The takeover audit block posts from its own subscriber.
        crate::takeover::spawn_notes(Arc::clone(&system.deps));
        match system.deps.runs.list_unfinished().await {
            Ok(runs) => {
                for run in runs.into_iter().filter(|run| run.state == RunState::Queued) {
                    system.enqueue_proactive(run);
                }
            }
            Err(error) => tracing::error!(%error, "Run queue load failed"),
        }
        system
    }

    /// The count of resident actors (test introspection).
    pub fn resident_agents(&self) -> usize {
        self.actors.lock().expect("actor map lock").len()
    }

    /// Free Run slots for one Agent, one count for each pool. Queued
    /// Runs reserve a slot so a scheduler pass cannot over-claim
    /// Wake-ups before the actor pumps.
    pub async fn available_slots(&self, agent_id: &AgentId) -> RunSlots {
        let sender = self
            .actors
            .lock()
            .expect("actor map lock")
            .get(agent_id)
            .cloned();
        let Some(sender) = sender else {
            return self.deps.config.all_slots();
        };
        let (reply, receive) = oneshot::channel();
        if sender.send(ActorMsg::Available(reply)).is_err() {
            return self.deps.config.all_slots();
        }
        receive.await.unwrap_or_default()
    }

    /// Whether the brain can think: a proactive Run waits while it
    /// cannot (see [`Brain::ready`]).
    pub fn brain_ready(&self) -> bool {
        self.deps.brain.ready()
    }

    /// Hand an already claimed proactive Run to its Agent actor.
    pub fn enqueue_proactive(&self, run: Run) {
        let agent_id = run.agent_id.clone();
        self.send_to_actor(&agent_id, ActorMsg::Proactive(Box::new(run)));
    }

    /// Cancel a run: a queued run cancels in place, a running run's
    /// stream is dropped and its partial persists as `failed`.
    pub async fn cancel(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        run_id: &RunId,
    ) -> CancelOutcome {
        let run = match self.deps.runs.get(workspace_id, run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return CancelOutcome::NotFound,
            Err(err) => {
                tracing::error!(error = %err, %run_id, "cancel lookup failed");
                return CancelOutcome::NotFound;
            }
        };
        if run.state.is_terminal() {
            return CancelOutcome::AlreadyEnded(run.state);
        }
        let sender = {
            let actors = self.actors.lock().expect("actor map lock");
            actors.get(&run.agent_id).cloned()
        };
        if let Some(sender) = sender {
            let (reply, on_reply) = oneshot::channel();
            if sender.send(ActorMsg::Cancel(run_id.clone(), reply)).is_ok()
                && on_reply.await.unwrap_or(false)
            {
                return CancelOutcome::Canceling;
            }
        }
        // No live actor knows the run: the store state is authoritative.
        match self.deps.runs.get(workspace_id, run_id).await {
            Ok(Some(run)) if run.state.is_terminal() => CancelOutcome::AlreadyEnded(run.state),
            Ok(Some(run)) => {
                tracing::warn!(%run_id, state = run.state.as_str(), "unowned non-terminal run");
                CancelOutcome::AlreadyEnded(run.state)
            }
            _ => CancelOutcome::NotFound,
        }
    }

    /// The dispatcher: live events only; messages sent while the
    /// daemon was down do not trigger.
    async fn dispatch(self: Arc<Self>, mut events: pagis_core::EventStream) {
        while let Some(event) = events.next().await {
            if event.event_type == "conversation.message" {
                if let Err(error) = self.deliver_conversation(&event).await {
                    tracing::error!(%error, "conversation delivery failed");
                }
                continue;
            }
            if event.event_type != "message.completed" {
                continue;
            }
            // User and agent messages trigger; system messages
            // (approval cards, takeover notes) never do.
            let author_kind = event.payload["author"]["kind"].clone();
            if author_kind != serde_json::json!("person")
                && author_kind != serde_json::json!("agent")
            {
                continue;
            }
            let Some(message_id) = event.payload["message_id"].as_str() else {
                continue;
            };
            let message = match self
                .deps
                .messages
                .get(
                    &event.workspace_id,
                    &MessageId::from(message_id.to_string()),
                )
                .await
            {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(err) => {
                    tracing::error!(error = %err, message_id, "trigger message load failed");
                    continue;
                }
            };
            // A message whose source scope cannot be verified carries
            // no words a reader may see (ADR-0004). Waking an Agent on
            // it starts a Run with an empty request, which answers the
            // sender out of nothing.
            match pagis_core::message_source_is_live(
                self.deps.messages.as_ref(),
                self.deps.grants.as_ref(),
                &message,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(message_id, "trigger message is unreadable");
                    continue;
                }
                Err(err) => {
                    tracing::error!(error = %err, message_id, "trigger readability failed");
                    continue;
                }
            }
            let targets = match self.trigger_targets(&message).await {
                Ok(targets) => targets,
                Err(err) => {
                    tracing::error!(error = %err, "trigger target lookup failed");
                    continue;
                }
            };
            for agent_id in targets {
                self.send_to_actor(&agent_id, ActorMsg::Trigger(Box::new(message.clone())));
            }
        }
    }

    /// The agents this message triggers. A DM triggers
    /// every active agent participant except the author. A group
    /// channel triggers an active participant only on an @-mention of
    /// its name — or, for a user thread reply, when the agent already
    /// posted in that thread. A message the daemon addressed to the
    /// user — an `approval_card`, a progress row —
    /// triggers nothing. Archived agents never trigger.
    async fn trigger_targets(
        &self,
        message: &Message,
    ) -> Result<Vec<AgentId>, Box<dyn std::error::Error + Send + Sync>> {
        if addressed_to_the_user(message) {
            return Ok(Vec::new());
        }
        let channel = self
            .deps
            .channels
            .get(&message.workspace_id, &message.channel_id)
            .await?
            .ok_or("trigger channel missing")?;
        let participant_ids = self
            .deps
            .participants
            .agents_in_channel(&message.workspace_id, &message.channel_id)
            .await?;
        // The in-thread follow-up rule applies to user messages only:
        // an agent reply re-triggering every thread author would
        // ping-pong; agent-to-agent group triggers are
        // mention-based.
        let thread_authors = match (&channel.kind, &message.author, &message.parent_message_id) {
            (ChannelKind::Group, Author::Person(_), Some(root_id)) => self
                .deps
                .messages
                .list_thread(&message.workspace_id, root_id)
                .await?
                .into_iter()
                .filter_map(|m| m.author.agent_id().cloned())
                .collect(),
            _ => HashSet::new(),
        };
        let mut targets = Vec::new();
        for agent_id in participant_ids {
            if message.author.agent_id() == Some(&agent_id) {
                continue;
            }
            let Some(agent) = self
                .deps
                .agents
                .get(&message.workspace_id, &agent_id)
                .await?
            else {
                continue;
            };
            if agent.status == AgentStatus::Archived {
                continue;
            }
            let triggered = match channel.kind {
                ChannelKind::Conversation => false,
                ChannelKind::Dm => true,
                ChannelKind::Group => {
                    crate::mention::is_mentioned(&message.text_content, &agent.name)
                        || thread_authors.contains(&agent.id)
                }
            };
            if triggered {
                targets.push(agent.id);
            }
        }
        Ok(targets)
    }

    /// Route one message to an agent's actor, spawning it when absent
    /// or already despawned (lazy residency).
    fn send_to_actor(&self, agent_id: &AgentId, mut msg: ActorMsg) {
        loop {
            let sender = {
                let mut actors = self.actors.lock().expect("actor map lock");
                actors
                    .entry(agent_id.clone())
                    .or_insert_with(|| {
                        spawn_actor(
                            agent_id.clone(),
                            Arc::clone(&self.deps),
                            Arc::clone(&self.actors),
                        )
                    })
                    .clone()
            };
            match sender.send(msg) {
                Ok(()) => return,
                // The actor despawned between the lookup and the send:
                // drop its stale entry and spawn a fresh one.
                Err(mpsc::error::SendError(unsent)) => {
                    msg = unsent;
                    let mut actors = self.actors.lock().expect("actor map lock");
                    if actors
                        .get(agent_id)
                        .is_some_and(|s| s.same_channel(&sender))
                    {
                        actors.remove(agent_id);
                    }
                }
            }
        }
    }
}

/// True when the message is addressed to the Person and triggers no
/// agent: an `approval_card`, or a derived progress row.
/// Both view a row the daemon owns; neither is conversation.
fn addressed_to_the_user(message: &Message) -> bool {
    message.blocks.iter().any(|block| {
        matches!(
            block,
            Block::Known(KnownBlock::ApprovalCard { .. } | KnownBlock::Progress { .. })
        )
    })
}

/// The delegation chain a run triggered by this message inherits:
/// the hop count, and the conversation waiting on the chain.
///
/// A user message starts a fresh chain at hop 0 with nothing waiting.
/// An agent message continues the sending run's chain: a chain that
/// carries an origin passes it on unchanged, and a chain without one
/// takes the sending run's own channel and thread — the conversation
/// the sender left to ask elsewhere, and the one its answer must
/// reach.
pub(crate) async fn chain_of(deps: &AgentDeps, message: &Message) -> (u32, Option<RunOrigin>) {
    if message.author.agent_id().is_none() {
        return (0, None);
    }
    let Some(run_id) = &message.run_id else {
        return (1, None);
    };
    let sender = match deps
        .runs
        .get(
            message
                .author_workspace_id
                .as_ref()
                .unwrap_or(&message.workspace_id),
            run_id,
        )
        .await
    {
        Ok(Some(sender)) => sender,
        Ok(None) => return (1, None),
        Err(err) => {
            tracing::error!(error = %err, %run_id, "delegation chain lookup failed");
            return (1, None);
        }
    };
    let origin = sender.origin.clone().or_else(|| {
        sender.channel_id.clone().map(|channel_id| RunOrigin {
            agent_id: sender.agent_id.clone(),
            channel_id,
            root_message_id: sender.root_message_id.clone(),
        })
    });
    (sender.hop_count.saturating_add(1), origin)
}

fn thread_key(message: &Message) -> ThreadKey {
    ThreadKey::Conversation {
        channel_id: message.channel_id.clone(),
        root_message_id: message.parent_message_id.clone(),
    }
}

/// The cancel token and the message channel the Run drains each Turn.
struct RunHandle {
    cancel: CancellationToken,
    inject: mpsc::UnboundedSender<Message>,
}

impl RunSignals for HashMap<RunId, RunHandle> {
    fn cancel(&self, run_id: &RunId) -> bool {
        let Some(handle) = self.get(run_id) else {
            return false;
        };
        handle.cancel.cancel();
        true
    }

    fn inject(&self, run_id: &RunId, message: Message) -> Option<Message> {
        let Some(handle) = self.get(run_id) else {
            return Some(message);
        };
        handle.inject.send(message).err().map(|error| error.0)
    }
}

struct Actor {
    agent_id: AgentId,
    deps: Arc<AgentDeps>,
    self_tx: mpsc::UnboundedSender<ActorMsg>,
    admission: Admission,
    queue: VecDeque<Run>,
    signals: HashMap<RunId, RunHandle>,
}

fn spawn_actor(
    agent_id: AgentId,
    deps: Arc<AgentDeps>,
    actors: ActorMap,
) -> mpsc::UnboundedSender<ActorMsg> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut actor = Actor {
        agent_id: agent_id.clone(),
        admission: Admission::new(&deps.config),
        deps,
        self_tx: tx.clone(),
        queue: VecDeque::new(),
        signals: HashMap::new(),
    };
    pagis_rt::spawn(async move {
        loop {
            let idle = actor.admission.is_empty();
            let msg = if idle {
                match pagis_rt::timeout(actor.deps.config.idle_timeout, rx.recv()).await {
                    Ok(msg) => msg,
                    Err(_) => {
                        // Idle timeout: despawn, unless a message
                        // arrived while we were deciding. The map lock
                        // orders this against `send_to_actor`.
                        let mut actors = actors.lock().expect("actor map lock");
                        match rx.try_recv() {
                            Ok(msg) => {
                                drop(actors);
                                Some(msg)
                            }
                            Err(_) => {
                                actors.remove(&actor.agent_id);
                                return;
                            }
                        }
                    }
                }
            } else {
                rx.recv().await
            };
            let Some(msg) = msg else { return };
            actor.handle(msg).await;
        }
    });
    tx
}

impl Actor {
    async fn handle(&mut self, msg: ActorMsg) {
        match msg {
            ActorMsg::Trigger(message) => self.trigger(*message).await,
            ActorMsg::Proactive(run) => self.proactive(*run).await,
            ActorMsg::Available(reply) => {
                let _ = reply.send(RunSlots {
                    conversation: slot_count(self.admission.free_slots(Pool::Conversation)),
                    arrival: slot_count(self.admission.free_slots(Pool::Arrival)),
                });
            }
            ActorMsg::Cancel(run_id, reply) => {
                let _ = reply.send(self.cancel(&run_id).await);
            }
            ActorMsg::Ended(run_id, mut leftover) => {
                self.signals.remove(&run_id);
                self.admission.end(&run_id);
                // The handle's sender is dropped: no further message
                // can land in `leftover`. An injection that raced the
                // run's end becomes a new queued run — the
                // message is never lost.
                while let Ok(message) = leftover.try_recv() {
                    self.trigger(message).await;
                }
                self.pump().await;
            }
        }
    }

    async fn proactive(&mut self, run: Run) {
        self.admission.enqueue(AdmissionRun::from(&run));
        self.queue.enqueue(run);
        self.pump().await;
    }

    /// A DM trigger: a message over the hop cap is
    /// dropped; a message for a thread with an active run injects into
    /// that run; otherwise create the run row (`queued`,
    /// `run.created`) and pump the queue.
    async fn trigger(&mut self, message: Message) {
        // The hop cap applies before the message can reach a run:
        // a delegation chain at the cap neither starts a run nor joins
        // one that still runs, so a loop of agents messaging each other
        // stops whatever the timing.
        let (hop_count, origin) = chain_of(&self.deps, &message).await;
        if hop_count > self.deps.config.max_hops {
            tracing::warn!(
                agent_id = %self.agent_id,
                message_id = %message.id,
                hop_count,
                "delegation hop cap reached; message triggers no run"
            );
            return;
        }
        let message = match self.inject(message).await {
            Some(message) => message,
            None => return,
        };
        let run = Run {
            id: RunId::generate(),
            title: pagis_core::run_title(pagis_core::RunTitleSource::Message(
                &message.text_content,
            )),
            workspace_id: message.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            channel_id: Some(message.channel_id.clone()),
            root_message_id: message.parent_message_id.clone(),
            trigger_kind: TriggerKind::Message,
            trigger_ref: Some(message.id.to_string()),
            hop_count,
            origin,
            state: RunState::Queued,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at: None,
            ended_at: None,
            // A run's own bookkeeping, read back as the time the run
            // happened, so it follows the injected clock.
            created_at: self.deps.clock.now_ms(),
        };
        if let Err(err) = self.deps.runs.create(&run).await {
            tracing::error!(error = %err, "run create failed");
            return;
        }
        let created = NewEvent {
            workspace_id: run.workspace_id.clone(),
            event_type: "run.created".to_string(),
            agent_id: Some(run.agent_id.clone()),
            run_id: Some(run.id.clone()),
            channel_id: run.channel_id.clone(),
            payload: serde_json::json!({
                "trigger_kind": run.trigger_kind,
                "trigger_ref": run.trigger_ref,
                "root_message_id": run.root_message_id.as_ref().map(|m| m.as_str()),
                // The Desk Panel reads the waiting conversation to
                // show the Desk of a delegate (ADR-0022).
                "origin_channel_id": run
                    .origin
                    .as_ref()
                    .map(|origin| origin.channel_id.as_str()),
            }),
        };
        if let Err(err) = self.deps.bus.publish(created).await {
            tracing::error!(error = %err, run_id = %run.id, "run.created publish failed");
        }
        self.admission.enqueue(AdmissionRun::from(&run));
        self.queue.enqueue(run);
        self.pump().await;
    }

    /// Send the message into its thread's active run, when one exists.
    /// Returns the message back when no run absorbed it.
    async fn inject(&mut self, message: Message) -> Option<Message> {
        let Some(run_id) = self.admission.active(&thread_key(&message)) else {
            return Some(message);
        };
        let message_id = message.id.clone();
        let channel_id = message.channel_id.clone();
        let workspace_id = message.workspace_id.clone();
        let root_message_id = message.parent_message_id.clone();
        if let Some(unsent) = self.signals.inject(&run_id, message) {
            // The Run stopped reading; queue a fresh Run instead.
            return Some(unsent);
        }
        let injected = NewEvent {
            workspace_id,
            event_type: "run.message_injected".to_string(),
            agent_id: Some(self.agent_id.clone()),
            run_id: Some(run_id.clone()),
            channel_id: Some(channel_id),
            payload: serde_json::json!({
                "message_id": message_id.as_str(),
                "root_message_id": root_message_id,
            }),
        };
        if let Err(err) = self.deps.bus.publish(injected).await {
            tracing::error!(error = %err, %run_id, "run.message_injected publish failed");
        }
        None
    }

    /// Start queued runs while their pool has a slot and their thread
    /// is free. A user message goes before a proactive run.
    async fn pump(&mut self) {
        while let Some(run_id) = self.admission.next() {
            let mut run =
                RunQueue::remove(&mut self.queue, &run_id).expect("admitted Run is queued");

            let agent = match self
                .deps
                .agents
                .get(&run.workspace_id, &self.agent_id)
                .await
            {
                Ok(Some(agent)) => agent,
                Ok(None) => {
                    self.admission.cancel(&run_id);
                    run.error = Some("agent not found".to_string());
                    fail(
                        &self.deps,
                        &mut run,
                        FailureKind::AgentMissing,
                        "agent not found",
                    )
                    .await;
                    continue;
                }
                Err(err) => {
                    self.admission.cancel(&run_id);
                    run.error = Some(err.to_string());
                    fail(
                        &self.deps,
                        &mut run,
                        FailureKind::AgentMissing,
                        "agent load failed",
                    )
                    .await;
                    continue;
                }
            };

            transition(&self.deps, &mut run, RunState::Running, "run started").await;
            let token = CancellationToken::new();
            let (inject_tx, mut inject_rx) = mpsc::unbounded_channel();
            assert!(self.admission.start(&run_id), "next Run can start");
            self.signals.insert(
                run.id.clone(),
                RunHandle {
                    cancel: token.clone(),
                    inject: inject_tx,
                },
            );
            let deps = Arc::clone(&self.deps);
            let done = self.self_tx.clone();
            let run_id = run.id.clone();
            pagis_rt::spawn(async move {
                let _outcome: RunOutcome = execute(deps, agent, run, token, &mut inject_rx).await;
                let _ = done.send(ActorMsg::Ended(run_id, inject_rx));
            });
        }
    }

    /// True when the run was ours to cancel.
    async fn cancel(&mut self, run_id: &RunId) -> bool {
        match self.admission.cancel(run_id) {
            CancelAction::Running => self.signals.cancel(run_id),
            CancelAction::Queued => {
                let mut run =
                    RunQueue::remove(&mut self.queue, run_id).expect("canceled Run is queued");
                transition(&self.deps, &mut run, RunState::Canceled, "canceled by user").await;
                true
            }
            CancelAction::Unknown => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunSignals;

    fn message() -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "message",
            "workspace_id": "workspace",
            "channel_id": "channel",
            "author": {"kind": "person", "person_id": "person"},
            "status": "complete",
            "blocks": [],
            "text_content": "Continue",
            "created_at": 0,
        }))
        .expect("Message")
    }

    #[test]
    fn memory_signals_cancel_only_the_named_run() {
        let mut signals = HashMap::new();
        let tokens = [CancellationToken::new(), CancellationToken::new()];
        let mut receivers = Vec::new();
        for (name, token) in ["a", "b"].into_iter().zip(&tokens) {
            let (inject, receiver) = mpsc::unbounded_channel();
            receivers.push(receiver);
            signals.insert(
                RunId::from(name.to_string()),
                RunHandle {
                    cancel: token.clone(),
                    inject,
                },
            );
        }
        assert!(!signals.cancel(&RunId::from("absent".to_string())));
        assert!(signals.cancel(&RunId::from("a".to_string())));
        assert!(signals.cancel(&RunId::from("a".to_string())));
        assert!(tokens[0].is_cancelled());
        assert!(!tokens[1].is_cancelled());
    }

    #[test]
    fn memory_signals_deliver_in_order_and_return_unsent_messages() {
        let mut signals = HashMap::new();
        let run_id = RunId::from("run".to_string());
        let (inject, mut receiver) = mpsc::unbounded_channel();
        signals.insert(
            run_id.clone(),
            RunHandle {
                cancel: CancellationToken::new(),
                inject,
            },
        );
        let first = message();
        let mut second = first.clone();
        second.id = MessageId::from("second".to_string());
        assert_eq!(
            signals.inject(&RunId::from("absent".to_string()), first.clone()),
            Some(first.clone())
        );
        assert_eq!(signals.inject(&run_id, first.clone()), None);
        assert_eq!(signals.inject(&run_id, second.clone()), None);
        assert_eq!(receiver.try_recv().expect("first message"), first);
        assert_eq!(receiver.try_recv().expect("second message"), second);
        drop(receiver);
        assert_eq!(signals.inject(&run_id, first.clone()), Some(first));
    }
}
