//! The runner of answered calls (ADR-0020, ADR-0022): a call
//! the line answers runs under the Agent that holds the dialed
//! number, in a Run the daemon opens in the Agent's Thread with the
//! user, with the standing brief as its purpose and the Unknown tier
//! until the bridge settles it. The bridge is the scripted fake, so no
//! model session opens.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::ToolDef;
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, ArtifactId, ArtifactStore, Block, CallId, Channel,
    ChannelId, ChannelKind, ChannelStore, ConnectionId, Event, EventLog, KnownBlock, MessageStore,
    PhoneNumber, PhoneNumberId, PhoneNumberStore, PurchaseIntent, PurchaseIntentId,
    PurchaseIntentState, Run, RunId, RunState, RunStore, TriggerKind, WorkspaceId, WorkspaceStore,
    now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteArtifactStore, SqliteChannelStore, SqliteEventLog, SqliteMessageStore,
    SqlitePhoneNumberStore, SqliteRunStore, SqliteWorkspaceStore,
};
use pagis_telephony::fake::{
    FakeCallTransport, FakeNumberDirectory, PartyState, RemoteParty, TokioClock,
};
use pagis_telephony::session::{HANG_UP, SEND_DIGITS};
use pagis_telephony::{
    CALL_ENDED, CallDirection, CallEvents, CallIngestError, EndpointTask, FakeCallBridge,
    InboundCalls, InboundCallsDeps, InboundRuns, IncomingHub, LiveCalls, RegistrationState,
    RunTools, SipCredential, TrustTier,
};
use sqlx::SqlitePool;
use tokio::sync::mpsc;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+14155550199";

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: name.to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}

/// The Run holds one tool that needs no approval.
struct OneFreeTool;

#[async_trait]
impl RunTools for OneFreeTool {
    async fn free_tools(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Vec<ToolDef>, String> {
        Ok(vec![tool("memory_read")])
    }
}

/// The daemon's half: a Run in the Agent's Thread with the user, kept
/// so the test reads what was opened and how it closed.
struct RecordingRuns {
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
    runs: Arc<SqliteRunStore>,
    opened: Mutex<Vec<(AgentId, CallId)>>,
    closed: Mutex<Vec<(RunId, Option<String>)>>,
}

#[async_trait]
impl InboundRuns for RecordingRuns {
    async fn open(
        &self,
        _workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        call_id: &CallId,
    ) -> Result<Run, String> {
        let run = Run {
            id: RunId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: agent_id.clone(),
            channel_id: Some(self.channel_id.clone()),
            root_message_id: None,
            trigger_kind: TriggerKind::Event,
            trigger_ref: Some(call_id.to_string()),
            hop_count: 0,
            origin: None,
            state: RunState::Running,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at: Some(now_ms()),
            ended_at: None,
            created_at: now_ms(),
        };
        self.runs.create(&run).await.map_err(|e| e.to_string())?;
        self.opened
            .lock()
            .unwrap()
            .push((agent_id.clone(), call_id.clone()));
        Ok(run)
    }

    async fn close(&self, _workspace_id: &WorkspaceId, run_id: &RunId, error: Option<String>) {
        let mut run = self
            .runs
            .get(&self.workspace_id, run_id)
            .await
            .unwrap()
            .unwrap();
        run.state = if error.is_some() {
            RunState::Failed
        } else {
            RunState::Completed
        };
        run.error = error.clone();
        run.ended_at = Some(now_ms());
        self.runs.update(&run).await.unwrap();
        self.closed.lock().unwrap().push((run_id.clone(), error));
    }
}

/// The Trigger module's half: the batches one finished call hands
/// over, kept so the test reads what the Agent would wake on.
#[derive(Default)]
struct RecordingEvents {
    batches: Mutex<Vec<pagis_core::IngestBatch>>,
}

#[async_trait]
impl CallEvents for RecordingEvents {
    async fn ingest(&self, batch: pagis_core::IngestBatch) -> Result<(), CallIngestError> {
        self.batches.lock().unwrap().push(batch);
        Ok(())
    }
}

struct World {
    calls: InboundCalls,
    bridge: Arc<FakeCallBridge>,
    live: Arc<LiveCalls>,
    transport: Arc<FakeCallTransport>,
    numbers: Arc<SqlitePhoneNumberStore>,
    agents: Arc<SqliteAgentStore>,
    artifacts: Arc<SqliteArtifactStore>,
    messages: Arc<SqliteMessageStore>,
    runs: Arc<RecordingRuns>,
    run_records: Arc<SqliteRunStore>,
    events: Arc<SqliteEventLog>,
    call_events: Arc<RecordingEvents>,
    channel_id: ChannelId,
    workspace_id: WorkspaceId,
    agent: Agent,
}

impl World {
    async fn build(pool: SqlitePool, bridge: FakeCallBridge) -> Self {
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = pagis_core::Workspace {
            user_id: person.id.clone(),
            id: WorkspaceId::generate(),
            name: "Workspace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now_ms(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
        };
        workspaces.create(&workspace).await.unwrap();

        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: "Robin".to_string(),
            job: "assistant".to_string(),
            description: String::new(),
            personality: "warm".to_string(),
            model_alias: "default".to_string(),
            avatar: Default::default(),
            voice: Some("nova".to_string()),
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        agents.create(&agent).await.unwrap();

        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Dm,
            title: Some("Robin".to_string()),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        SqliteChannelStore::new(pool.clone())
            .create(&channel)
            .await
            .unwrap();

        let run_records = Arc::new(SqliteRunStore::new(pool.clone()));
        let runs = Arc::new(RecordingRuns {
            workspace_id: workspace.id.clone(),
            channel_id: channel.id.clone(),
            runs: Arc::clone(&run_records),
            opened: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
        });
        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let live = Arc::new(LiveCalls::default());
        let bridge = Arc::new(bridge);
        let messages = Arc::new(SqliteMessageStore::new(pool.clone()));
        let artifacts = Arc::new(SqliteArtifactStore::new(pool.clone()));
        let call_events = Arc::new(RecordingEvents::default());
        let calls = InboundCalls::new(InboundCallsDeps {
            numbers: Arc::clone(&numbers) as _,
            agents: Arc::clone(&agents) as _,
            runs: Arc::clone(&runs) as _,
            run_tools: Arc::new(OneFreeTool),
            run_records: Arc::clone(&run_records) as _,
            messages: Arc::clone(&messages) as _,
            transport: Arc::clone(&transport) as _,
            bridge: Arc::clone(&bridge) as _,
            live: Arc::clone(&live),
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::new(object_store::memory::InMemory::new()),
            bus,
            events: Arc::clone(&call_events) as _,
        });
        Self {
            calls,
            bridge,
            live,
            transport,
            numbers,
            agents,
            artifacts,
            messages,
            runs,
            run_records,
            events,
            call_events,
            channel_id: channel.id,
            workspace_id: workspace.id,
            agent,
        }
    }

    /// The Agent's own desk line, written the way a purchase writes it.
    async fn held_line(&self) -> PhoneNumber {
        let at = now_ms();
        let number = PhoneNumber::new(
            PhoneNumberId::generate(),
            self.workspace_id.clone(),
            ConnectionId::generate(),
            OWN.to_string(),
            "carrier-14155550123".to_string(),
            Some(self.agent.id.clone()),
            at,
        );
        let intent = PurchaseIntent {
            id: PurchaseIntentId::generate(),
            workspace_id: self.workspace_id.clone(),
            connection_id: number.connection_id.clone(),
            e164: OWN.to_string(),
            agent_id: Some(self.agent.id.clone()),
            state: PurchaseIntentState::Pending,
            created_at: at,
            settled_at: None,
        };
        self.numbers.create_intent(&intent).await.unwrap();
        self.numbers
            .confirm_purchase(&self.workspace_id, &intent.id, &number, at)
            .await
            .unwrap();
        number
    }

    /// A call from the Remote Party to the number, answered by the
    /// line, as the daemon hands it over.
    async fn ring(&self, number: &PhoneNumber) -> (IncomingHub, Arc<RemoteParty>, EndpointTask) {
        let (answered, mut incoming) = mpsc::channel(1);
        let task = EndpointTask::spawn(
            Arc::clone(&self.transport) as _,
            Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
            Arc::new(FakeNumberDirectory::with(vec![number.clone()])),
            answered,
        );
        task.watch()
            .wait_for(|state| *state == RegistrationState::Registered)
            .await
            .expect("the line registers");
        let party = self.transport.ring(OWN, REMOTE);
        let hub = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
            .await
            .expect("the task answers in time")
            .expect("the task answers the call");
        (hub, party, task)
    }

    /// The messages of the Agent's Thread with the user, oldest first.
    async fn strips(&self) -> Vec<pagis_core::Message> {
        let mut entries = self
            .messages
            .list_top_level(&self.workspace_id, &self.channel_id, None, 10)
            .await
            .unwrap();
        entries.reverse();
        entries.into_iter().map(|entry| entry.message).collect()
    }

    async fn events_of(&self, event_type: &str) -> Vec<Event> {
        self.events
            .list_after(None, 0, 100)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == event_type)
            .collect()
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_answered_call_runs_under_the_agent_that_holds_the_number(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("I would like a table.")),
    )
    .await;
    let number = world.held_line().await;
    let mut agent = world.agent.clone();
    agent.standing_brief = Some("Book appointments for the clinic.".to_string());
    world.agents.update(&agent).await.unwrap();
    let (incoming, party, _task) = world.ring(&number).await;

    let call_id = world.calls.answer(incoming).await.expect("the call runs");

    // The bridge was handed the call under the standing brief, with
    // the Unknown tier until the challenge proves another.
    let answered = world.bridge.answered();
    assert_eq!(answered.len(), 1);
    let placed = &answered[0];
    assert_eq!(placed.id, call_id);
    assert_eq!(placed.workspace_id, world.workspace_id);
    assert_eq!(placed.brief.direction, CallDirection::Inbound);
    assert_eq!(placed.brief.agent_id, world.agent.id);
    assert_eq!(placed.brief.phone_number_id, number.id);
    assert_eq!(placed.brief.own_e164, OWN);
    assert_eq!(placed.brief.remote_e164, REMOTE);
    assert_eq!(placed.brief.purpose, "Book appointments for the clinic.");
    assert_eq!(placed.brief.tier, TrustTier::Unknown);
    assert_eq!(names(&placed.brief.tools), vec!["memory_read"]);
    assert_eq!(names(&placed.tools), vec![HANG_UP, SEND_DIGITS]);

    // The Run was opened for the Agent, referenced to the Call, and
    // completed once the call settled.
    let opened = world.runs.opened.lock().unwrap().clone();
    assert_eq!(opened, vec![(world.agent.id.clone(), call_id.clone())]);
    let run = world
        .run_records
        .get(&world.workspace_id, &placed.run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(run.error, None);
    assert!(run.ended_at.is_some());
    let closed = world.runs.closed.lock().unwrap().clone();
    assert_eq!(closed, vec![(placed.run_id.clone(), None)]);

    // The `call` block landed in the Agent's Thread with the user.
    let strips = world.strips().await;
    assert_eq!(strips.len(), 1);
    assert_eq!(strips[0].run_id, Some(placed.run_id.clone()));
    assert_eq!(
        strips[0].blocks,
        vec![Block::from(KnownBlock::Call {
            call_id: call_id.to_string(),
        })]
    );

    // The audit trail: placed as inbound, answered, ended with the
    // transcript Artifact.
    let placed_events = world.events_of("call.placed").await;
    assert_eq!(placed_events.len(), 1);
    assert_eq!(placed_events[0].payload["direction"], "inbound");
    assert_eq!(placed_events[0].payload["call_id"], call_id.as_str());
    assert_eq!(placed_events[0].payload["from"], OWN);
    assert_eq!(placed_events[0].payload["to"], REMOTE);
    assert_eq!(placed_events[0].run_id, Some(placed.run_id.clone()));
    assert_eq!(world.events_of("call.answered").await.len(), 1);
    let ended = world.events_of("call.ended").await;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].payload["outcome"], "answered");
    let transcript_id = ended[0].payload["transcript_artifact_id"]
        .as_str()
        .expect("the transcript was stored");
    let transcript = world
        .artifacts
        .get(
            &world.workspace_id,
            &ArtifactId::from(transcript_id.to_string()),
        )
        .await
        .unwrap()
        .expect("the transcript row exists");
    assert_eq!(transcript.kind, pagis_core::ArtifactKind::CallTranscript);
    assert_eq!(transcript.run_id, Some(placed.run_id.clone()));

    // The line is free again, and the Remote Party heard the end.
    assert!(world.live.session_tools(&number.id).is_none());
    assert!(matches!(party.state(), PartyState::Ended(_)));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_unassigned_after_the_line_routed_its_call_hangs_the_call_up(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    let number = world.held_line().await;
    let (incoming, party, _task) = world.ring(&number).await;
    // The number was unassigned after the line routed the call to it.
    world
        .numbers
        .unassign(&world.workspace_id, &number.id)
        .await
        .unwrap();

    let refused = world.calls.answer(incoming).await.unwrap_err();

    assert!(refused.0.contains("held by no Agent"), "{refused}");
    assert!(world.bridge.answered().is_empty());
    assert!(world.runs.opened.lock().unwrap().is_empty());
    assert!(world.events_of("call.placed").await.is_empty());
    assert!(
        matches!(party.state(), PartyState::Ended(_)),
        "the caller hears the end"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_bridge_that_fails_settles_the_call_and_the_run(pool: SqlitePool) {
    let world = World::build(pool, FakeCallBridge::failing("model_unavailable")).await;
    let number = world.held_line().await;
    let (incoming, party, _task) = world.ring(&number).await;

    let failed = world.calls.answer(incoming).await.unwrap_err();

    assert_eq!(failed.0, "model_unavailable");
    let closed = world.runs.closed.lock().unwrap().clone();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].1.as_deref(), Some("model_unavailable"));
    let run = world
        .run_records
        .get(&world.workspace_id, &closed[0].0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, RunState::Failed);
    let ended = world.events_of("call.ended").await;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].payload["outcome"], "failed");
    assert_eq!(ended[0].payload["ended_reason"], "model_unavailable");
    // The line is free again, and the caller hears the end.
    assert!(world.live.session_tools(&number.id).is_none());
    assert!(matches!(party.state(), PartyState::Ended(_)));
    assert_eq!(
        ended[0].payload["transcript_artifact_id"],
        serde_json::Value::Null
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_settled_call_reaches_the_trigger_module_as_an_envelope(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report(
            "the plan is still on, reach me on this number",
        )),
    )
    .await;
    let number = world.held_line().await;
    let (incoming, _party, _task) = world.ring(&number).await;

    let call_id = world.calls.answer(incoming).await.unwrap();

    // ADR-0021: the Agent takes the message, and the Run that reads it
    // afterwards reads data. This batch is what starts that Run.
    let batches = world.call_events.batches.lock().unwrap().clone();
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch.event_kind, CALL_ENDED);
    assert_eq!(batch.connection_id, number.connection_id);
    // The desk line is one Agent's own identity, so the pass carries
    // the Agent and no other Agent's rules read it.
    assert_eq!(batch.agent_id.as_ref(), Some(&world.agent.id));
    assert!(!batch.baseline);
    assert_eq!(batch.events.len(), 1);
    let event = &batch.events[0];
    // One call settles once, so a replay wakes nobody twice.
    assert_eq!(event.provider_event_id, call_id.to_string());
    assert_eq!(event.metadata["call_id"], call_id.to_string());
    assert_eq!(event.metadata["direction"], "inbound");
    assert_eq!(event.metadata["number"], OWN);
    assert_eq!(event.metadata["from"], REMOTE);
    assert_eq!(event.metadata["trust_tier"], "unknown");
    assert_eq!(event.metadata["outcome"], "answered");
    assert!(event.metadata["transcript_artifact_id"].is_string());
    // The envelope carries no words: the Agent reads those with
    // `call_transcript`.
    assert!(
        !event.metadata.to_string().contains("the plan is still on"),
        "the metadata carries the envelope alone: {}",
        event.metadata
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_the_bridge_never_ran_reports_nothing(pool: SqlitePool) {
    let world = World::build(pool, FakeCallBridge::failing("model_unavailable")).await;
    let number = world.held_line().await;
    let (incoming, _party, _task) = world.ring(&number).await;

    world.calls.answer(incoming).await.unwrap_err();

    // There is no message to take, so the Agent is not woken.
    assert!(world.call_events.batches.lock().unwrap().is_empty());
}
