//! One inbound call end to end (ADR-0020): the fake carrier rings
//! the line, the endpoint task answers, the bridge pumps the call with
//! the fake model server, and the Call record settles with an ended
//! reason.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{ToolCall, ToolResult};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, Call, CallDirection, CallId, CallOutcome, CallState,
    CallStore, Channel, ChannelId, ChannelKind, ChannelStore, ConnectionId, Event, EventBus,
    EventId, EventScope, EventStream, MemorySecretStore, NewEvent, PhoneNumber, PhoneNumberId, Run,
    RunId, RunState, RunStore, SecretStore, StoreError, TriggerKind, TrustTier, Workspace,
    WorkspaceId, WorkspaceStore, now_ms, render_transcript,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteCallStore, SqliteChannelStore, SqliteConnectionStore, SqliteEventLog,
    SqlitePhoneNumberStore, SqliteRunStore, SqliteWorkspaceStore,
};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberDirectory, TokioClock};
use pagis_telephony::model_fake::FakeModelSessions;
use pagis_telephony::{
    CallBridge as _, CallBrief, CallLog, CallTools, DEFAULT_DURATION_CAP, EMERGENCY_RULE,
    EndpointTask, Endpoints, EndpointsDeps, IVR_MODE_PROMPT, PlacedCall, RealtimeBridge,
    RealtimeBridgeDeps, RegistrationState, SipCredential, VoicemailPolicy,
};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+14155550199";

/// No tool of the brief is called in this test, so the broker answers
/// nothing.
struct NoTools;

#[async_trait]
impl CallTools for NoTools {
    async fn invoke(
        &self,
        _workspace: &WorkspaceId,
        _agent: &AgentId,
        _run: &RunId,
        _call: ToolCall,
    ) -> ToolResult {
        ToolResult::error("unavailable", "no tool runs in this test")
    }
}

/// A Trust List with no rows: every caller reaches Unknown.
struct NoLists;

#[async_trait]
impl pagis_core::TrustListStore for NoLists {
    async fn upsert(&self, _row: &pagis_core::TrustEntry) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Vec<pagis_core::TrustEntry>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _id: &pagis_core::TrustEntryId,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn candidate(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _agent_id: &AgentId,
        _subject: pagis_core::TrustSubject,
        _value: &str,
    ) -> Result<Option<pagis_core::TrustTier>, StoreError> {
        Ok(None)
    }
}

/// A bus that keeps nothing: the audit trail has its own tests.
struct SilentBus;

#[async_trait]
impl EventBus for SilentBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        Ok(Event {
            id: EventId::generate(),
            seq: 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// One line on the fake carrier with a stranger's call answered on it,
/// and the real bridge over the fake model server.
struct Desk {
    bridge: RealtimeBridge,
    calls: Arc<SqliteCallStore>,
    sessions: Arc<FakeModelSessions>,
    party: Arc<pagis_telephony::fake::RemoteParty>,
    hub: pagis_telephony::IncomingHub,
    placed: PlacedCall,
    log: CallLog,
    _task: EndpointTask,
}

async fn desk(pool: SqlitePool) -> Desk {
    let workspaces = SqliteWorkspaceStore::new(pool.clone());
    let person =
        pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
            .await
            .unwrap();
    let workspace = Workspace {
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

    let agents = SqliteAgentStore::new(pool.clone());
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace.id.clone(),
        name: "Robin".to_string(),
        job: "assistant".to_string(),
        description: String::new(),
        personality: "warm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: Some("marin".to_string()),
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
        title: Some("general".to_string()),
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteChannelStore::new(pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        channel_id: Some(channel.id),
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
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

    let number = PhoneNumber::new(
        PhoneNumberId::generate(),
        workspace.id.clone(),
        ConnectionId::generate(),
        OWN.to_string(),
        "carrier-14155550123".to_string(),
        Some(agent.id.clone()),
        now_ms(),
    );

    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let bus: Arc<dyn EventBus> = Arc::new(SilentBus);
    let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
        transport: Arc::clone(&transport) as _,
        numbers: Arc::clone(&numbers) as _,
        connections: Arc::new(SqliteConnectionStore::new(pool.clone())),
        org_workspace_id: workspace.id.clone(),
        secrets,
        bus: Arc::clone(&bus),
    }));
    let _events = SqliteEventLog::new(pool.clone());

    let calls = Arc::new(SqliteCallStore::new(pool.clone()));
    let sessions = Arc::new(FakeModelSessions::new());
    let bridge = RealtimeBridge::new(RealtimeBridgeDeps {
        endpoints,
        sessions: Arc::clone(&sessions) as _,
        tools: Arc::new(NoTools),
        calls: Arc::clone(&calls) as _,
        artifacts: Arc::new(pagis_storage_sqlite::SqliteArtifactStore::new(pool.clone())),
        blobs: Arc::new(object_store::memory::InMemory::new()),
        recordings: tempfile::tempdir().unwrap().keep(),
        live: Arc::new(pagis_telephony::LiveCalls::default()),
        tiers: Arc::new(NoLists),
        keypad: pagis_telephony::Keypad {
            code: Arc::new(pagis_telephony::keypad::NoCode),
            failures: Arc::new(pagis_storage_sqlite::SqliteKeypadFailureStore::new(
                pool.clone(),
            )),
            clock: Arc::new(pagis_core::SystemClock),
        },
        live_tiers: Arc::new(pagis_telephony::LiveTiers::default()),
        cancel: CancellationToken::new(),
    });

    // The line comes up and a stranger calls it.
    let (answered, mut incoming) = tokio::sync::mpsc::channel(1);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![number.clone()])),
        answered,
    );
    task.watch()
        .wait_for(|state| *state == RegistrationState::Registered)
        .await
        .expect("the line registers");
    let party = transport.ring(OWN, REMOTE);
    let hub = incoming.recv().await.expect("the task answers the call");

    let placed = PlacedCall {
        id: CallId::generate(),
        workspace_id: workspace.id.clone(),
        run_id: run.id.clone(),
        brief: CallBrief {
            direction: CallDirection::Inbound,
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            voice: agent.voice.clone(),
            phone_number_id: number.id.clone(),
            own_e164: OWN.to_string(),
            remote_e164: REMOTE.to_string(),
            tier: TrustTier::Unknown,
            purpose: pagis_telephony::TAKE_A_MESSAGE.to_string(),
            success_criteria: None,
            voicemail: VoicemailPolicy::HangUp,
            duration_cap: DEFAULT_DURATION_CAP,
            tools: Vec::new(),
            ivr_mode_prompt: IVR_MODE_PROMPT,
            classify_prompt: pagis_telephony::CLASSIFY_PROMPT,
            emergency_rule: EMERGENCY_RULE,
        },
        tools: Vec::new(),
    };
    let log = CallLog::new(Arc::clone(&bus), &placed);
    Desk {
        bridge,
        calls,
        sessions,
        party,
        hub,
        placed,
        log,
        _task: task,
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_inbound_call_settles_its_call_record(pool: SqlitePool) {
    let Desk {
        bridge,
        calls,
        sessions,
        party,
        hub,
        placed,
        log,
        _task,
    } = desk(pool).await;
    let call_id = placed.id.clone();
    let workspace_id = placed.workspace_id.clone();

    let party_hangs_up = {
        let sessions = Arc::clone(&sessions);
        let party = Arc::clone(&party);
        tokio::spawn(async move {
            let peer = loop {
                if let Some(peer) = sessions.peers().first().cloned() {
                    break peer;
                }
                tokio::task::yield_now().await;
            };
            peer.wait_for("session.update").await;
            peer.caller_transcript("this is Sam, please call me back");
            peer.agent_transcript("I will pass the message on.");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            party.hangup();
        })
    };

    let report = bridge
        .answer(hub, placed, &log)
        .await
        .expect("the call runs");
    party_hangs_up.await.unwrap();

    assert_eq!(report.outcome, CallOutcome::Answered);
    assert_eq!(report.ended_reason, "remote_hangup");

    let settled: Call = calls
        .get(&workspace_id, &call_id)
        .await
        .unwrap()
        .expect("a Call record");
    assert_eq!(settled.state, CallState::Ended);
    assert_eq!(settled.direction, CallDirection::Inbound);
    assert_eq!(settled.remote_e164, REMOTE);
    assert_eq!(settled.tier, TrustTier::Unknown);
    assert_eq!(settled.outcome, Some(CallOutcome::Answered));
    assert_eq!(settled.ended_reason.as_deref(), Some("remote_hangup"));
    assert!(settled.answered_at.is_some());
    assert!(settled.ended_at.is_some());
    assert!(render_transcript(&settled.transcript).contains("caller: this is Sam"));
}

/// The Call record exists from the moment the call is answered, before
/// the tier settles and the model session opens: the strip in the
/// Thread reads it at once, and a call whose session never opens still
/// settles as failed with the reason.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_inbound_call_whose_session_does_not_open_settles_as_failed(pool: SqlitePool) {
    let Desk {
        bridge,
        calls,
        sessions,
        hub,
        placed,
        log,
        _task,
        ..
    } = desk(pool).await;
    let call_id = placed.id.clone();
    let workspace_id = placed.workspace_id.clone();
    sessions.fail_next(1);

    let error = bridge
        .answer(hub, placed, &log)
        .await
        .expect_err("no session, no call");
    assert!(error.0.contains("unavailable"), "{error}");

    let settled: Call = calls
        .get(&workspace_id, &call_id)
        .await
        .unwrap()
        .expect("a Call record");
    assert_eq!(settled.state, CallState::Ended);
    assert_eq!(settled.outcome, Some(CallOutcome::Failed));
    assert_eq!(
        settled.ended_reason.as_deref(),
        Some(pagis_telephony::MODEL_UNAVAILABLE)
    );
    assert!(settled.answered_at.is_some());
    assert!(settled.ended_at.is_some());
}
