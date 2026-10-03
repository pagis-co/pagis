//! The call tool behind the broker (ADR-0020): the brief the
//! daemon writes, the tools a live call binds, the audit trail and the
//! result the Agent reads. The bridge is the scripted fake, so no call
//! is placed and no socket opens.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::{AuthorizedCall, CoreTool, ToolDef, ToolExecutor, ToolRoute};
use pagis_core::{
    Agent, AgentStatus, AgentStore, ArtifactId, ArtifactStore, Block, CallId, CallStore, Channel,
    ChannelId, ChannelKind, ChannelStore, ConnectionId, Event, EventLog, KnownBlock,
    MemorySecretStore, MessageId, MessageStore, PhoneNumber, PhoneNumberId, PhoneNumberStore,
    PurchaseIntent, PurchaseIntentId, PurchaseIntentState, Run, RunId, RunState, RunStore,
    SecretStore, TriggerKind, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteArtifactStore, SqliteCallStore, SqliteChannelStore,
    SqliteConnectionStore, SqliteEventLog, SqliteMessageStore, SqlitePhoneNumberStore,
    SqliteRunStore, SqliteTrustListStore, SqliteWorkspaceStore,
};
use pagis_telephony::fake::{
    FakeCallTransport, FakeNumberCatalog, FakeStandingCallRule, TokioClock,
};
use pagis_telephony::hub::{DtmfPresence, MediaHub};
use pagis_telephony::session::{HANG_UP, SEND_DIGITS};
use pagis_telephony::{
    CALL_FAILED, CLASSIFY_PROMPT, CallBrief, CallDirection, CallTransport, DEFAULT_DURATION_CAP,
    EMERGENCY_RULE, EmergencyRefused, Endpoints, EndpointsDeps, FakeCallBridge, IVR_MODE_PROMPT,
    LiveCalls, NumberCatalogs, NumberDesk, NumberDeskDeps, PHONE_NOT_ASSIGNED, PhoneToolDeps,
    PhoneToolRuntime, RunTools, SipCredential, TAKE_A_MESSAGE, TELNYX_PROVIDER, TextTransports,
    TransportCapabilities, TrustTier, VoicemailPolicy,
};
use sqlx::SqlitePool;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+14155550199";

/// The Run holds two tools that need no approval.
struct TwoFreeTools;

#[async_trait]
impl RunTools for TwoFreeTools {
    async fn free_tools(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Vec<ToolDef>, String> {
        Ok(vec![tool("memory_read"), tool("memory_write")])
    }
}

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

struct World {
    runtime: PhoneToolRuntime,
    bridge: Arc<FakeCallBridge>,
    live: Arc<LiveCalls>,
    call_records: Arc<SqliteCallStore>,
    numbers: Arc<SqlitePhoneNumberStore>,
    agents: Arc<SqliteAgentStore>,
    artifacts: Arc<SqliteArtifactStore>,
    messages: Arc<SqliteMessageStore>,
    channel_id: ChannelId,
    blobs: Arc<object_store::memory::InMemory>,
    events: Arc<SqliteEventLog>,
    workspace_id: WorkspaceId,
    agent: Agent,
    run_id: RunId,
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
            home_exit_host_id: None,
        };
        workspaces.create(&workspace).await.unwrap();

        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let agent = Agent {
            id: pagis_core::AgentId::generate(),
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

        // The transcript Artifact points at the Run, so the Run exists.
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Dm,
            title: Some("general".to_string()),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        SqliteChannelStore::new(pool.clone())
            .create(&channel)
            .await
            .unwrap();
        let channel_id = channel.id.clone();
        let run = Run {
            id: RunId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            channel_id: Some(channel.id.clone()),
            root_message_id: None,
            trigger_kind: TriggerKind::Message,
            trigger_ref: Some(MessageId::generate().to_string()),
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
        SqliteRunStore::new(pool.clone())
            .create(&run)
            .await
            .unwrap();

        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::clone(&transport) as _,
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            secrets: Arc::clone(&secrets),
            bus: Arc::clone(&bus),
        }));
        let live = Arc::new(LiveCalls::default());
        let desk = Arc::new(NumberDesk::new(NumberDeskDeps {
            numbers: Arc::clone(&numbers) as _,
            connections,
            org_workspace_id: workspace.id.clone(),
            agents: Arc::clone(&agents) as _,
            catalogs: Arc::new(NumberCatalogs::single(
                TELNYX_PROVIDER,
                Arc::new(FakeNumberCatalog::default()),
            )),
            // Nothing in these tests texts, so no carrier here
            // carries a text.
            text_transports: Arc::new(TextTransports::new()),
            secrets,
            calls: Arc::clone(&live) as _,
            bus: Arc::clone(&bus),
            endpoints,
            standing_rule: Arc::new(FakeStandingCallRule::default()),
        }));
        let bridge = Arc::new(bridge);
        let messages = Arc::new(SqliteMessageStore::new(pool.clone()));
        let artifacts = Arc::new(SqliteArtifactStore::new(pool.clone()));
        let blobs = Arc::new(object_store::memory::InMemory::new());
        let call_records = Arc::new(SqliteCallStore::new(pool.clone()));
        let runtime = PhoneToolRuntime::new(PhoneToolDeps {
            desk,
            agents: Arc::clone(&agents) as _,
            runs: Arc::new(TwoFreeTools),
            run_records: Arc::new(SqliteRunStore::new(pool.clone())),
            messages: Arc::clone(&messages) as _,
            calls: Arc::clone(&call_records) as _,
            tiers: Arc::new(SqliteTrustListStore::new(pool.clone())),
            transport,
            bridge: Arc::clone(&bridge) as _,
            live: Arc::clone(&live),
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs) as _,
            bus,
        });
        Self {
            runtime,
            bridge,
            live,
            call_records,
            numbers,
            agents,
            artifacts,
            messages,
            channel_id,
            blobs,
            events,
            workspace_id: workspace.id,
            agent,
            run_id: run.id,
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

    fn call(&self, arguments: serde_json::Value) -> AuthorizedCall {
        AuthorizedCall {
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent.id.clone(),
            run_id: self.run_id.clone(),
            tool_name: pagis_broker::PHONE_CALL.to_string(),
            source_version: "test".to_string(),
            route: ToolRoute::Core {
                tool: CoreTool::PhoneCall,
            },
            arguments,
            selected_connection: None,
            grant_id: None,
            grant_revision: None,
            call_timeout: None,
            tool_call_id: None,
            host: None,
            approved_by_rule: false,
        }
    }

    /// One settled Call the Agent answered, written the way the
    /// bridge writes it (ADR-0020).
    async fn answered_call(&self, number: &PhoneNumber, said: &str) -> pagis_core::Call {
        let at = now_ms();
        let call = pagis_core::Call {
            id: pagis_core::CallId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent.id.clone(),
            run_id: self.run_id.clone(),
            phone_number_id: number.id.clone(),
            direction: CallDirection::Inbound,
            remote_e164: REMOTE.to_string(),
            agent_name: self.agent.name.clone(),
            own_e164: number.e164.clone(),
            purpose: "Take a message.".to_string(),
            tools: Vec::new(),
            tier: TrustTier::Unknown,
            state: pagis_core::CallState::Ended,
            outcome: Some(pagis_core::CallOutcome::Answered),
            ended_reason: Some("remote_hangup".to_string()),
            classification: None,
            message_left: false,
            transcript: vec![pagis_core::TranscriptLine::new(
                at,
                pagis_core::Speaker::Caller,
                said,
            )],
            recording_artifact_id: None,
            created_at: at,
            ringing_at: None,
            answered_at: Some(at),
            ended_at: Some(at),
            dismissed_at: None,
        };
        self.call_records.insert(&call).await.unwrap();
        call
    }

    /// A read of one Call's words, as the Agent that holds the line
    /// makes it.
    fn transcript_call(&self, call_id: &str) -> AuthorizedCall {
        AuthorizedCall {
            tool_name: pagis_broker::CALL_TRANSCRIPT.to_string(),
            route: ToolRoute::Core {
                tool: CoreTool::CallTranscript,
            },
            arguments: serde_json::json!({"call_id": call_id}),
            ..self.call(serde_json::json!({}))
        }
    }

    fn arguments() -> serde_json::Value {
        serde_json::json!({
            "to": REMOTE,
            "brief": "Ask if the table for four is still free at seven.",
            "success_criteria": "A yes or a no about the table."
        })
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
async fn the_daemon_writes_the_brief_and_the_agent_reads_the_result(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report(
            "Robin: Is the table free?\nRemote: Yes, at seven.",
        )),
    )
    .await;
    let number = world.held_line().await;

    let result = world.runtime.execute(world.call(World::arguments())).await;
    assert!(!result.is_error, "{}", result.content);

    // The brief: the Agent wrote the purpose and the criteria, and the
    // daemon wrote the rest.
    let placed = world.bridge.placed();
    assert_eq!(placed.len(), 1);
    let brief = &placed[0].brief;
    assert_eq!(brief.direction, CallDirection::Outbound);
    assert_eq!(brief.agent_id, world.agent.id);
    assert_eq!(brief.agent_name, "Robin");
    assert_eq!(brief.voice.as_deref(), Some("nova"));
    assert_eq!(brief.phone_number_id, number.id);
    assert_eq!(brief.own_e164, OWN);
    assert_eq!(brief.remote_e164, REMOTE);
    assert_eq!(brief.tier, TrustTier::Unknown);
    assert_eq!(
        brief.purpose,
        "Ask if the table for four is still free at seven."
    );
    assert_eq!(
        brief.success_criteria.as_deref(),
        Some("A yes or a no about the table.")
    );
    assert_eq!(brief.voicemail, VoicemailPolicy::HangUp);
    assert_eq!(brief.duration_cap, DEFAULT_DURATION_CAP);
    assert_eq!(brief.duration_cap, Duration::from_secs(600));
    assert_eq!(names(&brief.tools), vec!["memory_read", "memory_write"]);
    assert_eq!(brief.ivr_mode_prompt, IVR_MODE_PROMPT);
    assert_eq!(brief.classify_prompt, CLASSIFY_PROMPT);
    assert_eq!(brief.emergency_rule, EMERGENCY_RULE);

    // The result: the outcome, the ended reason, the classification,
    // the message flag, the duration, the transcript Artifact and the
    // transcript itself inside the untrusted envelope. The recording
    // is a pointer only.
    let value: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(value["outcome"], "answered");
    assert_eq!(value["ended_reason"], "hangup");
    assert_eq!(value["classification"], "human");
    assert_eq!(value["message_left"], false);
    assert_eq!(value["duration_s"], 42);
    assert_eq!(value["transcript"]["untrusted"], true);
    assert_eq!(value["transcript"]["source"], "remote_party");
    assert_eq!(value["transcript"]["trust"], "unknown");
    assert_eq!(value["tier"], "unknown");
    // The envelope names the tier at both ends, so the Run reads the
    // words as data with a stated standing (ADR-0021).
    assert_eq!(
        value["transcript"]["content"],
        "[BEGIN UNTRUSTED source=remote_party trust=unknown]\ncaller: Robin: Is the table \
         free?\nRemote: Yes, at seven.\n[END UNTRUSTED source=remote_party]"
    );
    assert!(value["recording_artifact_id"].is_null());
    let transcript_id = ArtifactId::from(
        value["transcript_artifact_id"]
            .as_str()
            .unwrap()
            .to_string(),
    );
    let artifact = world
        .artifacts
        .get(&world.workspace_id, &transcript_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(artifact.mime, "text/plain");
    assert_eq!(artifact.run_id.as_ref(), Some(&world.run_id));
    let bytes = object_store::ObjectStoreExt::get(
        world.blobs.as_ref(),
        &object_store::path::Path::from(artifact.storage_key),
    )
    .await
    .unwrap()
    .bytes()
    .await
    .unwrap();
    assert_eq!(
        String::from_utf8(bytes.to_vec()).unwrap(),
        "caller: Robin: Is the table free?\nRemote: Yes, at seven."
    );

    // The audit trail: placed, answered, ended, each with the number,
    // the Agent and the Run, and no credential.
    for event_type in ["call.placed", "call.answered", "call.ended"] {
        let events = world.events_of(event_type).await;
        assert_eq!(events.len(), 1, "{event_type}");
        let event = &events[0];
        assert_eq!(event.agent_id.as_ref(), Some(&world.agent.id));
        assert_eq!(event.run_id.as_ref(), Some(&world.run_id));
        assert_eq!(event.payload["to"], REMOTE);
        assert_eq!(event.payload["from"], OWN);
        assert_eq!(event.payload["call_id"], value["call_id"]);
        let text = event.payload.to_string();
        assert!(
            !text.contains("carrier-14155550123"),
            "{event_type}: {text}"
        );
    }
    let ended = &world.events_of("call.ended").await[0];
    assert_eq!(ended.payload["outcome"], "answered");
    assert_eq!(ended.payload["ended_reason"], "hangup");
    assert_eq!(
        ended.payload["transcript_artifact_id"],
        value["transcript_artifact_id"]
    );
    // The line is free again.
    assert!(world.live.session_tools(&number.id).is_none());
}

/// The strip (ADR-0022): the daemon mints one `call` block in the
/// Thread that sent the Agent to make the call, and it carries the Call
/// id only, because the Call record is the one source of truth.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_call_block_is_minted_in_the_triggering_thread(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("Robin: Hello.")),
    )
    .await;
    world.held_line().await;

    let result = world.runtime.execute(world.call(World::arguments())).await;
    assert!(!result.is_error, "{}", result.content);

    let timeline = world
        .messages
        .list_top_level(&world.workspace_id, &world.channel_id, None, 20)
        .await
        .unwrap();
    let call_ids: Vec<&str> = timeline
        .iter()
        .flat_map(|entry| entry.message.blocks.iter())
        .filter_map(|block| match block {
            Block::Known(KnownBlock::Call { call_id }) => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(call_ids.len(), 1, "one strip, in the triggering Thread");
    let placed = world.bridge.placed();
    assert_eq!(call_ids[0], placed[0].id.as_str());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_brief_naming_an_unheld_tool_gets_the_intersection(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    world.held_line().await;
    let mut arguments = World::arguments();
    arguments["tools"] = serde_json::json!(["memory_read", "host_shell", "no_such_tool"]);

    let result = world.runtime.execute(world.call(arguments)).await;
    assert!(!result.is_error, "{}", result.content);
    let placed = world.bridge.placed();
    assert_eq!(names(&placed[0].brief.tools), vec!["memory_read"]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_live_call_binds_the_session_tools_and_nothing_gated(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    let number = world.held_line().await;
    assert!(world.live.session_tools(&number.id).is_none());

    let result = world.runtime.execute(world.call(World::arguments())).await;
    assert!(!result.is_error, "{}", result.content);
    // The bridge saw the tools the live call bound: the session tools,
    // and no brief tool, because a stranger's words bind none.
    let placed = world.bridge.placed();
    assert_eq!(names(&placed[0].tools), vec![HANG_UP, SEND_DIGITS]);
    assert!(world.live.session_tools(&number.id).is_none());
}

#[tokio::test]
async fn a_session_tool_exists_only_while_the_call_is_live() {
    let live = Arc::new(LiveCalls::default());
    let number_id = PhoneNumberId::generate();
    assert!(live.session_tools(&number_id).is_none());
    let workspace_id = WorkspaceId::generate();
    let guard = live
        .begin(
            &workspace_id,
            &number_id,
            &CallId::generate(),
            vec![tool(HANG_UP)],
        )
        .unwrap();
    assert_eq!(
        names(&live.session_tools(&number_id).unwrap()),
        vec![HANG_UP]
    );
    // One active call per number (ADR-0020).
    let second = live.begin(&workspace_id, &number_id, &CallId::generate(), Vec::new());
    assert!(second.is_err());
    drop(guard);
    assert!(live.session_tools(&number_id).is_none());
}

/// The media hub of a live call is found only under the Workspace of
/// the call (ADR-0023). Another Workspace that names the Call id finds
/// no hub, so it can neither listen to the call nor hang it up.
#[tokio::test]
async fn a_live_hub_is_found_only_under_the_workspace_of_its_call() {
    let transport = FakeCallTransport::new(Arc::new(TokioClock));
    let mut opened = transport
        .open(&SipCredential::new("robin", "secret", "sip.telnyx.com"))
        .await
        .unwrap();
    let hub = MediaHub::start(opened.line.dial(OWN, REMOTE).await.unwrap());
    let _party = transport.dials().pop().expect("one dialed party");
    let live = Arc::new(LiveCalls::default());
    let owner = WorkspaceId::generate();
    let other = WorkspaceId::generate();
    let call_id = CallId::generate();
    let guard = live
        .begin(&owner, &PhoneNumberId::generate(), &call_id, Vec::new())
        .unwrap();
    live.attach_hub(&owner, &call_id, Arc::clone(&hub));

    assert!(live.hub(&other, &call_id).is_none());
    let own = live.hub(&owner, &call_id).expect("the hub of the call");
    assert!(Arc::ptr_eq(&own, &hub));
    assert!(!hub.is_ended());

    // The line frees when the call ends, and the hub goes with it.
    drop(guard);
    assert!(live.hub(&owner, &call_id).is_none());
}

/// A brief of an inbound call, for the session-tool tests.
fn a_brief() -> CallBrief {
    let agent = Agent {
        id: pagis_core::AgentId::generate(),
        workspace_id: WorkspaceId::generate(),
        name: "Robin".to_string(),
        job: "assistant".to_string(),
        description: String::new(),
        personality: "warm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    let number = PhoneNumber::new(
        PhoneNumberId::generate(),
        agent.workspace_id.clone(),
        ConnectionId::generate(),
        OWN.to_string(),
        "carrier".to_string(),
        Some(agent.id.clone()),
        now_ms(),
    );
    CallBrief::inbound(
        &agent,
        &number,
        REMOTE,
        TrustTier::Unknown,
        vec![tool("memory_read")],
    )
}

#[tokio::test]
async fn send_digits_is_absent_when_the_carrier_cannot_send_dtmf() {
    let brief = a_brief();
    let no_dtmf = TransportCapabilities {
        send_dtmf: false,
        answering_machine_detection: false,
        wideband_audio: false,
        public_ingress_required: false,
    };
    let tools = pagis_telephony::session::session_tools(&brief, no_dtmf, None);
    assert_eq!(names(&tools), vec![HANG_UP]);
}

#[tokio::test]
async fn send_digits_is_absent_when_the_negotiated_leg_has_no_telephone_event() {
    let brief = a_brief();
    let can_send = TransportCapabilities {
        send_dtmf: true,
        answering_machine_detection: false,
        wideband_audio: false,
        public_ingress_required: false,
    };
    // The transport can send digits, but this call's leg negotiated no
    // telephone-event payload type (ADR-0005).
    let absent = DtmfPresence {
        send_dtmf: false,
        receive_dtmf: false,
    };
    let tools = pagis_telephony::session::session_tools(&brief, can_send, Some(absent));
    assert_eq!(names(&tools), vec![HANG_UP]);

    let present = DtmfPresence {
        send_dtmf: true,
        receive_dtmf: true,
    };
    let tools = pagis_telephony::session::session_tools(&brief, can_send, Some(present));
    assert_eq!(names(&tools), vec![HANG_UP, SEND_DIGITS]);
}

#[tokio::test]
async fn the_answered_call_rebinds_the_tools_of_its_negotiated_leg() {
    let live = Arc::new(LiveCalls::default());
    let number_id = PhoneNumberId::generate();
    assert!(!live.rebind(&number_id, vec![tool(HANG_UP)]));
    let _guard = live
        .begin(
            &WorkspaceId::generate(),
            &number_id,
            &CallId::generate(),
            vec![tool(HANG_UP), tool(SEND_DIGITS)],
        )
        .unwrap();

    assert!(live.rebind(&number_id, vec![tool(HANG_UP)]));

    assert_eq!(
        names(&live.session_tools(&number_id).unwrap()),
        vec![HANG_UP]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_per_call_cap_never_lengthens_the_workspace_cap(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    world.held_line().await;
    let mut shorter = World::arguments();
    shorter["max_duration_s"] = serde_json::json!(30);
    shorter["voicemail"] = serde_json::json!("leave_message");
    world.runtime.execute(world.call(shorter)).await;
    let mut longer = World::arguments();
    longer["max_duration_s"] = serde_json::json!(9_999);
    world.runtime.execute(world.call(longer)).await;

    let placed = world.bridge.placed();
    assert_eq!(placed[0].brief.duration_cap, Duration::from_secs(30));
    assert_eq!(placed[0].brief.voicemail, VoicemailPolicy::LeaveMessage);
    assert_eq!(placed[1].brief.duration_cap, DEFAULT_DURATION_CAP);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_agent_without_a_number_cannot_call(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    let result = world.runtime.execute(world.call(World::arguments())).await;
    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some(PHONE_NOT_ASSIGNED));
    assert!(world.bridge.placed().is_empty());
    assert!(world.events_of("call.placed").await.is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_emergency_number_is_refused_before_anything_is_placed(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    world.held_line().await;
    let mut arguments = World::arguments();
    arguments["to"] = serde_json::json!("+1911");
    let result = world.runtime.execute(world.call(arguments)).await;
    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some(EmergencyRefused::CODE));
    assert!(world.bridge.placed().is_empty());
    assert!(world.events_of("call.placed").await.is_empty());
    assert_eq!(
        world
            .events_of("phone_number.emergency_refused")
            .await
            .len(),
        1
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_bridge_failure_ends_the_call_with_a_reason(pool: SqlitePool) {
    let world = World::build(pool, FakeCallBridge::failing("model_unavailable")).await;
    let number = world.held_line().await;
    let result = world.runtime.execute(world.call(World::arguments())).await;
    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some(CALL_FAILED));
    assert!(result.content.contains("model_unavailable"));
    assert_eq!(world.events_of("call.placed").await.len(), 1);
    assert!(world.events_of("call.answered").await.is_empty());
    let ended = world.events_of("call.ended").await;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].payload["outcome"], "failed");
    assert_eq!(ended[0].payload["ended_reason"], "model_unavailable");
    // The line is free again after a failure.
    assert!(world.live.session_tools(&number.id).is_none());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_inbound_call_answers_under_the_standing_brief(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    let number = world.held_line().await;

    // No standing brief: the Agent takes a message.
    let agent = world
        .agents
        .get(&world.workspace_id, &world.agent.id)
        .await
        .unwrap()
        .unwrap();
    let brief = CallBrief::inbound(
        &agent,
        &number,
        REMOTE,
        TrustTier::Unknown,
        vec![tool("memory_read")],
    );
    assert_eq!(brief.direction, CallDirection::Inbound);
    assert_eq!(brief.purpose, TAKE_A_MESSAGE);
    assert_eq!(brief.success_criteria, None);
    assert_eq!(brief.tier, TrustTier::Unknown);
    assert_eq!(brief.remote_e164, REMOTE);
    assert_eq!(brief.own_e164, OWN);

    // The standing brief is read at answer time, from the record.
    let mut agent = agent;
    agent.standing_brief = Some("Book appointments for the clinic.".to_string());
    world.agents.update(&agent).await.unwrap();
    let agent = world
        .agents
        .get(&world.workspace_id, &world.agent.id)
        .await
        .unwrap()
        .unwrap();
    let brief = CallBrief::inbound(&agent, &number, REMOTE, TrustTier::Unknown, Vec::new());
    assert_eq!(brief.purpose, "Book appointments for the clinic.");
    assert_eq!(brief.agent_name, "Robin");
    assert_eq!(brief.voice.as_deref(), Some("nova"));
    assert_eq!(brief.duration_cap, DEFAULT_DURATION_CAP);

    // A call runs on OpenAI, so a voice of another provider's speech
    // model is absent and the call takes the OpenAI default.
    let mut agent = agent;
    agent.voice = Some("Kore".to_string());
    let brief = CallBrief::inbound(&agent, &number, REMOTE, TrustTier::Unknown, Vec::new());
    assert_eq!(brief.voice, None);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_agent_reads_one_of_its_own_calls_inside_the_untrusted_envelope(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;
    let number = world.held_line().await;
    // The bridge settles a Call record for every call, inbound or
    // outbound (ADR-0020). This is one the Agent answered.
    let call = world
        .answered_call(&number, "the plan is still on, reach me on this number")
        .await;

    let read = world
        .runtime
        .execute(world.transcript_call(call.id.as_str()))
        .await;

    assert!(!read.is_error, "{}", read.content);
    let body = serde_json::from_str::<serde_json::Value>(&read.content).unwrap();
    assert_eq!(body["call_id"], call.id.as_str());
    assert_eq!(body["from"], REMOTE);
    assert_eq!(body["direction"], "inbound");
    assert_eq!(body["tier"], "unknown");
    let transcript = body["transcript"]["content"]
        .as_str()
        .expect("the transcript comes back as text");
    assert!(transcript.contains("the plan is still on"), "{transcript}");
    // The words of the other party are data, and they say so.
    assert!(transcript.contains("BEGIN UNTRUSTED"), "{transcript}");
    assert!(transcript.contains("trust=unknown"), "{transcript}");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_the_agent_does_not_hold_reads_as_no_call_at_all(pool: SqlitePool) {
    let world = World::build(
        pool,
        FakeCallBridge::reporting(FakeCallBridge::answered_report("")),
    )
    .await;

    let read = world
        .runtime
        .execute(world.transcript_call("call_somebody_elses"))
        .await;

    assert!(read.is_error);
    assert!(
        read.content.contains("call_somebody_elses"),
        "{}",
        read.content
    );
}
