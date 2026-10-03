//! The line of a carrier Connection routes each inbound Call by the
//! number that was dialed (ADR-0020). Two People hold one Agent Phone
//! Number each on one carrier Connection, and the carrier sends every
//! `INVITE` of the account to one contact. The stored record of the
//! dialed number names the Workspace and the Agent; the socket that took
//! the `INVITE` names neither. A call that no Agent can take is refused
//! before it is answered, so it opens no Run, no Call record and no
//! bridge.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::{ToolCall, ToolDef, ToolResult};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, CallDirection, CallId, CallOutcome, CallStore,
    Connection, ConnectionId, ConnectionStore, MemorySecretStore, PhoneNumber, PhoneNumberStore,
    Run, RunId, RunState, RunStore, SecretStore, TriggerKind, TrustTier, User, Workspace,
    WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteArtifactStore, SqliteCallStore, SqliteConnectionStore, SqliteEventLog,
    SqliteMessageStore, SqlitePhoneNumberStore, SqliteRunStore, SqliteTrustListStore,
    SqliteWorkspaceStore,
};
use pagis_telephony::fake::{
    FakeCallTransport, FakeNumberCatalog, FakeStandingCallRule, LineCall, PartyState, RemoteParty,
    TokioClock,
};
use pagis_telephony::leg::EndedReason;
use pagis_telephony::model_fake::FakeModelSessions;
use pagis_telephony::{
    BridgeError, CallBridge, CallBrief, CallEvents, CallIngestError, CallLog, CallReport,
    CallTools, DEFAULT_DURATION_CAP, EMERGENCY_RULE, Endpoints, EndpointsDeps, FakeCallBridge,
    IVR_MODE_PROMPT, InboundCalls, InboundCallsDeps, InboundRuns, IncomingHub, LiveCalls,
    NumberCatalogs, NumberDesk, NumberDeskDeps, PlacedCall, RealtimeBridge, RealtimeBridgeDeps,
    Refusal, RegistrationFailure, RegistrationState, RunTools, SIP_DOMAIN_KEY, SIP_USERNAME_KEY,
    TELNYX_PROVIDER, TextTransports, TransportErrorCode, VoicemailPolicy, carrier_key_secret_name,
    dialed_e164, sip_password_secret_name,
};
use sqlx::SqlitePool;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const ALICE_LINE: &str = "+14155550123";
const BOB_LINE: &str = "+14155550124";
/// A number the installation holds and no Agent holds.
const SPARE_LINE: &str = "+14155550125";
/// A number the installation does not hold.
const NOT_HELD: &str = "+14155550126";
const CALLER: &str = "+16505550100";
/// The SIP username of the carrier's credential. Every line of the
/// account registers it, so the registrar cannot tell two lines apart.
const USERNAME: &str = "pagisline";
const ALIAS: &str = "carrier";

/// One Person's Workspace and the two Agents in it.
#[derive(Clone)]
struct Person {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    second_agent_id: AgentId,
}

/// Where one inbound Call opened its Run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenedRun {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
}

/// The daemon's half of an answered call: a Run in the Workspace the
/// runner names, kept so the test reads where each call ran.
struct RecordingRuns {
    runs: Arc<SqliteRunStore>,
    opened: Mutex<Vec<OpenedRun>>,
    closed: Mutex<Vec<RunId>>,
}

impl RecordingRuns {
    fn opened(&self) -> Vec<OpenedRun> {
        self.opened.lock().unwrap().clone()
    }

    fn closed(&self) -> usize {
        self.closed.lock().unwrap().len()
    }
}

#[async_trait]
impl InboundRuns for RecordingRuns {
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        call_id: &CallId,
    ) -> Result<Run, String> {
        let run = Run {
            id: RunId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            channel_id: None,
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
        self.opened.lock().unwrap().push(OpenedRun {
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
        });
        Ok(run)
    }

    async fn close(&self, workspace_id: &WorkspaceId, run_id: &RunId, error: Option<String>) {
        let mut run = self
            .runs
            .get(workspace_id, run_id)
            .await
            .unwrap()
            .expect("the Run was opened");
        run.state = if error.is_some() {
            RunState::Failed
        } else {
            RunState::Completed
        };
        run.error = error;
        run.ended_at = Some(now_ms());
        self.runs.update(&run).await.unwrap();
        self.closed.lock().unwrap().push(run_id.clone());
    }
}

/// The Run holds no tool, and no tool is called.
struct NoTools;

#[async_trait]
impl RunTools for NoTools {
    async fn free_tools(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Vec<ToolDef>, String> {
        Ok(Vec::new())
    }
}

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

/// The Trigger module is not under test here.
struct NoWakeups;

#[async_trait]
impl CallEvents for NoWakeups {
    async fn ingest(&self, _batch: pagis_core::IngestBatch) -> Result<(), CallIngestError> {
        Ok(())
    }
}

/// A bridge that stays on each call until the test lets it go, so a
/// second call arrives while the first one is live.
struct HoldingBridge {
    answered: Mutex<Vec<PlacedCall>>,
    release: watch::Sender<bool>,
}

impl HoldingBridge {
    fn new() -> Self {
        Self {
            answered: Mutex::new(Vec::new()),
            release: watch::channel(false).0,
        }
    }

    fn answered(&self) -> Vec<PlacedCall> {
        self.answered.lock().unwrap().clone()
    }
}

#[async_trait]
impl CallBridge for HoldingBridge {
    async fn place(&self, _call: PlacedCall, _log: &CallLog) -> Result<CallReport, BridgeError> {
        Err(BridgeError("this bridge places no call".to_string()))
    }

    async fn answer(
        &self,
        incoming: IncomingHub,
        call: PlacedCall,
        _log: &CallLog,
    ) -> Result<CallReport, BridgeError> {
        self.answered.lock().unwrap().push(call);
        let _ = self.release.subscribe().wait_for(|go| *go).await;
        incoming.hub.hangup().await;
        let _ = incoming
            .hub
            .watch_ended()
            .wait_for(|ended| ended.is_some())
            .await;
        Ok(FakeCallBridge::answered_report(""))
    }
}

/// What runs the calls the line hands over.
enum Bridge {
    /// Answers each call and hangs up at once.
    Scripted,
    /// Stays on each call until the test lets it go.
    Holding,
    /// The real bridge over the fake model server. It writes the Call
    /// record.
    Realtime,
}

struct World {
    desk: NumberDesk,
    numbers: Arc<SqlitePhoneNumberStore>,
    transport: Arc<FakeCallTransport>,
    scripted: Arc<FakeCallBridge>,
    holding: Arc<HoldingBridge>,
    realtime: Arc<RealtimeBridge>,
    sessions: Arc<FakeModelSessions>,
    runs: Arc<RecordingRuns>,
    run_store: Arc<SqliteRunStore>,
    calls: Arc<SqliteCallStore>,
    bus: Arc<dyn pagis_core::EventBus>,
    org: WorkspaceId,
    carrier: ConnectionId,
    alice: Person,
    bob: Person,
    _cancel: tokio_util::sync::DropGuard,
}

async fn workspace(workspaces: &SqliteWorkspaceStore, person: &User, name: &str) -> WorkspaceId {
    let workspace = Workspace {
        user_id: person.id.clone(),
        id: WorkspaceId::generate(),
        name: name.to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    workspaces.create(&workspace).await.unwrap();
    workspace.id
}

async fn agent(agents: &SqliteAgentStore, workspace_id: &WorkspaceId, name: &str) -> AgentId {
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
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
    agent.id
}

async fn person(
    workspaces: &SqliteWorkspaceStore,
    agents: &SqliteAgentStore,
    user: &User,
    name: &str,
) -> Person {
    let workspace_id = workspace(workspaces, user, name).await;
    Person {
        agent_id: agent(agents, &workspace_id, &format!("{name}'s Agent")).await,
        second_agent_id: agent(agents, &workspace_id, &format!("{name}'s second Agent")).await,
        workspace_id,
    }
}

/// Wait until `done` holds, and fail with `what` when it never does.
async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} never happened"));
}

impl World {
    /// One Org with a carrier Connection that holds a SIP credential,
    /// and two People, Alice and Bob, with a Workspace each.
    async fn build(pool: SqlitePool, bridge: Bridge) -> Self {
        let user = pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", now_ms())
            .await
            .unwrap();
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let org = workspace(&workspaces, &user, "Org").await;
        let alice = person(&workspaces, &agents, &user, "Alice").await;
        let bob = person(&workspaces, &agents, &user, "Bob").await;

        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let carrier = Connection {
            id: ConnectionId::generate(),
            workspace_id: org.clone(),
            provider: TELNYX_PROVIDER.to_string(),
            alias: ALIAS.to_string(),
            display_name: "Telnyx".to_string(),
            status: Connection::CONNECTED.to_string(),
            auth_mode: Connection::AUTH_MODE_BYO.to_string(),
            authorized_capabilities: Vec::new(),
            config: serde_json::json!({
                SIP_USERNAME_KEY: USERNAME,
                SIP_DOMAIN_KEY: "sip.telnyx.com",
            }),
            created_at: now_ms(),
        };
        connections.create(&carrier).await.unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        secrets
            .set(&carrier_key_secret_name(TELNYX_PROVIDER, ALIAS), "api-key")
            .unwrap();
        secrets
            .set(
                &sip_password_secret_name(TELNYX_PROVIDER, ALIAS),
                "sip-secret",
            )
            .unwrap();

        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::clone(&transport) as _,
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: org.clone(),
            secrets: Arc::clone(&secrets),
            bus: Arc::clone(&bus),
        }));
        let live = Arc::new(LiveCalls::default());
        let desk = NumberDesk::new(NumberDeskDeps {
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: org.clone(),
            agents: Arc::clone(&agents) as _,
            catalogs: Arc::new(NumberCatalogs::single(
                TELNYX_PROVIDER,
                Arc::new(FakeNumberCatalog::offering(&[
                    ALICE_LINE, BOB_LINE, SPARE_LINE,
                ])),
            )),
            text_transports: Arc::new(TextTransports::new()),
            secrets: Arc::clone(&secrets),
            calls: Arc::clone(&live) as _,
            bus: Arc::clone(&bus),
            endpoints: Arc::clone(&endpoints),
            standing_rule: Arc::new(FakeStandingCallRule::default()),
        });

        let cancel = CancellationToken::new();
        let calls = Arc::new(SqliteCallStore::new(pool.clone()));
        let artifacts = Arc::new(SqliteArtifactStore::new(pool.clone()));
        let blobs = Arc::new(object_store::memory::InMemory::new());
        let sessions = Arc::new(FakeModelSessions::new());
        let realtime = Arc::new(RealtimeBridge::new(RealtimeBridgeDeps {
            endpoints: Arc::clone(&endpoints),
            sessions: Arc::clone(&sessions) as _,
            tools: Arc::new(NoTools),
            calls: Arc::clone(&calls) as _,
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs) as _,
            recordings: tempfile::tempdir().unwrap().keep(),
            live: Arc::clone(&live),
            tiers: Arc::new(SqliteTrustListStore::new(pool.clone())),
            keypad: pagis_telephony::Keypad {
                code: Arc::new(pagis_telephony::keypad::NoCode),
                failures: Arc::new(pagis_storage_sqlite::SqliteKeypadFailureStore::new(
                    pool.clone(),
                )),
                clock: Arc::new(pagis_core::SystemClock),
            },
            live_tiers: Arc::new(pagis_telephony::LiveTiers::default()),
            cancel: cancel.clone(),
        }));
        let scripted = Arc::new(FakeCallBridge::reporting(FakeCallBridge::answered_report(
            "",
        )));
        let holding = Arc::new(HoldingBridge::new());
        let run_store = Arc::new(SqliteRunStore::new(pool.clone()));
        let runs = Arc::new(RecordingRuns {
            runs: Arc::clone(&run_store),
            opened: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
        });
        let inbound = Arc::new(InboundCalls::new(InboundCallsDeps {
            numbers: Arc::clone(&numbers) as _,
            agents: Arc::clone(&agents) as _,
            runs: Arc::clone(&runs) as _,
            run_tools: Arc::new(NoTools),
            run_records: Arc::clone(&run_store) as _,
            messages: Arc::new(SqliteMessageStore::new(pool.clone())),
            transport: Arc::clone(&transport) as _,
            bridge: match bridge {
                Bridge::Scripted => Arc::clone(&scripted) as _,
                Bridge::Holding => Arc::clone(&holding) as _,
                Bridge::Realtime => Arc::clone(&realtime) as _,
            },
            live,
            artifacts: artifacts as _,
            blobs,
            bus: Arc::clone(&bus),
            events: Arc::new(NoWakeups),
        }));
        inbound.spawn(
            endpoints
                .take_answered_calls()
                .expect("the answered calls are taken once"),
            cancel.clone(),
        );
        Self {
            desk,
            numbers,
            transport,
            scripted,
            holding,
            realtime,
            sessions,
            runs,
            run_store,
            calls,
            bus,
            org,
            carrier: carrier.id,
            alice,
            bob,
            _cancel: cancel.drop_guard(),
        }
    }

    /// Buy a number for a Person, and give it to the Agent when one is
    /// named.
    async fn buy(&self, person: &Person, e164: &str, agent_id: Option<&AgentId>) -> PhoneNumber {
        self.desk
            .buy(&person.workspace_id, e164, agent_id)
            .await
            .unwrap()
    }

    /// The daemon starts the carrier's line, which registers the
    /// carrier's SIP credential.
    async fn line_up(&self) {
        self.desk.start_carrier().await.unwrap();
        self.registered().await;
    }

    /// Where the carrier's line stands, as a number an Agent holds
    /// shows it.
    async fn line_state(&self, person: &Person, number: &PhoneNumber) -> RegistrationState {
        let number = self
            .numbers
            .get(&person.workspace_id, &number.id)
            .await
            .unwrap()
            .expect("the number is stored");
        self.desk
            .registration(&number)
            .expect("a number an Agent holds shows its line")
    }

    async fn registered(&self) {
        eventually("the line registers", || {
            self.transport.is_registered(USERNAME)
        })
        .await;
    }

    /// Ring the dialed number and wait until the call ran to its end.
    /// Answer where its Run was opened.
    async fn call(&self, dialed: &str) -> OpenedRun {
        let before = self.runs.closed();
        self.transport.ring(dialed, CALLER);
        eventually("the call runs to its end", || self.runs.closed() > before).await;
        self.runs.opened()[before].clone()
    }

    /// Ring the dialed number and answer where the caller stands once
    /// the line decided.
    async fn ring(&self, dialed: &str, from: &str) -> Arc<RemoteParty> {
        let party = self.transport.ring(dialed, from);
        let mut state = party.watch();
        tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|state| *state != PartyState::Calling),
        )
        .await
        .expect("the line answers or refuses the call")
        .unwrap();
        party
    }

    fn ran_in(&self, person: &Person) -> OpenedRun {
        OpenedRun {
            workspace_id: person.workspace_id.clone(),
            agent_id: person.agent_id.clone(),
        }
    }

    /// Nothing ran for any call: no Run, no Call record, no bridge.
    async fn nothing_ran(&self) {
        // A call that was answered after all reaches the runner within
        // this time.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(self.runs.opened(), Vec::new());
        assert!(self.scripted.answered().is_empty());
        assert!(self.holding.answered().is_empty());
        assert!(self.sessions.peers().is_empty());
        for workspace_id in [&self.org, &self.alice.workspace_id, &self.bob.workspace_id] {
            let records = self
                .calls
                .list(workspace_id, None, None, None, None, 10)
                .await
                .unwrap();
            assert!(records.is_empty(), "{records:?}");
        }
    }

    /// Every `INVITE` the line sent, as `(from, to)`.
    fn dials(&self) -> Vec<(String, String)> {
        self.transport
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                LineCall::Dial { from, to } => Some((from, to)),
                _ => None,
            })
            .collect()
    }

    fn count(&self, wanted: impl Fn(&LineCall) -> bool) -> usize {
        self.transport
            .calls()
            .iter()
            .filter(|call| wanted(call))
            .count()
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn each_call_runs_under_the_agent_that_holds_the_dialed_number(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;

    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));
    assert_eq!(world.call(BOB_LINE).await, world.ran_in(&world.bob));
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));

    // Each call ran under the brief of the dialed number's holder.
    let answered = world.scripted.answered();
    assert_eq!(answered.len(), 3);
    assert_eq!(answered[0].workspace_id, world.alice.workspace_id);
    assert_eq!(answered[0].brief.agent_id, world.alice.agent_id);
    assert_eq!(answered[0].brief.own_e164, ALICE_LINE);
    assert_eq!(answered[1].workspace_id, world.bob.workspace_id);
    assert_eq!(answered[1].brief.agent_id, world.bob.agent_id);
    assert_eq!(answered[1].brief.own_e164, BOB_LINE);
    // One line carries both numbers: one socket, one registration.
    assert_eq!(world.count(|call| matches!(call, LineCall::Open { .. })), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_follows_the_dialed_number_in_either_order_of_assignment(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    let alices = world.buy(&world.alice, ALICE_LINE, None).await;
    let bobs = world.buy(&world.bob, BOB_LINE, None).await;
    world.line_up().await;

    // Alice's number is assigned first.
    world
        .desk
        .assign(&world.alice.workspace_id, &alices.id, &world.alice.agent_id)
        .await
        .unwrap();
    world
        .desk
        .assign(&world.bob.workspace_id, &bobs.id, &world.bob.agent_id)
        .await
        .unwrap();
    world.registered().await;
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));
    assert_eq!(world.call(BOB_LINE).await, world.ran_in(&world.bob));

    // Bob's number is assigned first.
    world
        .desk
        .unassign(&world.alice.workspace_id, &alices.id)
        .await
        .unwrap();
    world
        .desk
        .unassign(&world.bob.workspace_id, &bobs.id)
        .await
        .unwrap();
    world
        .desk
        .assign(&world.bob.workspace_id, &bobs.id, &world.bob.agent_id)
        .await
        .unwrap();
    world
        .desk
        .assign(&world.alice.workspace_id, &alices.id, &world.alice.agent_id)
        .await
        .unwrap();
    world.registered().await;
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));
    assert_eq!(world.call(BOB_LINE).await, world.ran_in(&world.bob));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_to_a_number_the_installation_does_not_hold_is_refused_with_404(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world.line_up().await;

    let party = world.ring(NOT_HELD, CALLER).await;

    assert_eq!(party.state(), PartyState::Refused(Refusal::NotFound));
    assert_eq!(Refusal::NotFound.status(), 404);
    assert!(party.heard().is_empty(), "no media flowed");
    world.nothing_ran().await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_to_a_number_no_agent_holds_is_refused_with_480(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world.buy(&world.bob, SPARE_LINE, None).await;
    world.line_up().await;

    let party = world.ring(SPARE_LINE, CALLER).await;

    assert_eq!(party.state(), PartyState::Refused(Refusal::Unavailable));
    assert_eq!(Refusal::Unavailable.status(), 480);
    assert!(party.heard().is_empty(), "no media flowed");
    world.nothing_ran().await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_with_no_dialed_number_is_refused_before_it_is_answered(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world.line_up().await;

    // No number at all, and a national number that is not E.164.
    let silent = world.ring("", CALLER).await;
    let national = world.ring("4155550123", CALLER).await;

    assert_eq!(silent.state(), PartyState::Refused(Refusal::NotFound));
    assert_eq!(national.state(), PartyState::Refused(Refusal::NotFound));
    world.nothing_ran().await;
}

/// A credential connection can send the `INVITE` to the registered
/// Contact, so the Request-URI names the SIP username and `To` names the
/// dialed number. The call reaches the Agent that holds the `To` number.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_request_uri_that_names_the_sip_username_routes_by_the_to_number(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;
    let invite = rsipstack::sip::Request::try_from(format!(
        "INVITE sip:{USERNAME}@192.0.2.10:5061;transport=tls SIP/2.0\r\n\
         Via: SIP/2.0/TLS 192.0.2.1:5061;branch=z9hG4bK776asdhds\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:{CALLER}@sip.telnyx.com>;tag=1928301774\r\n\
         To: <sip:{BOB_LINE}@sip.telnyx.com>\r\n\
         Call-ID: a84b4c76e66710@sip.telnyx.com\r\n\
         CSeq: 314159 INVITE\r\n\
         Content-Length: 0\r\n\r\n"
    ))
    .expect("a SIP request");

    let dialed = dialed_e164(&invite, USERNAME).expect("the INVITE names a number");

    assert_eq!(dialed, BOB_LINE);
    assert_eq!(world.call(&dialed).await, world.ran_in(&world.bob));
}

/// A line that lost its registration opens a fresh socket and registers
/// again, and the calls still follow the dialed number.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_line_routes_by_the_dialed_number_after_it_reconnects(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    let alices = world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    // A short expiry, so the line refreshes every second.
    world.transport.grant_expiry(Some(Duration::from_secs(2)));
    world.line_up().await;

    world
        .transport
        .fail_with(Some(TransportErrorCode::Unreachable));
    tokio::time::timeout(Duration::from_secs(5), async {
        while world.line_state(&world.alice, &alices).await
            != RegistrationState::Failed(RegistrationFailure::Unreachable)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the refresh fails");
    world.transport.fail_with(None);
    tokio::time::timeout(Duration::from_secs(5), async {
        while world.line_state(&world.alice, &alices).await != RegistrationState::Registered {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the line registers again");
    assert_eq!(world.count(|call| matches!(call, LineCall::Open { .. })), 2);

    assert_eq!(world.call(BOB_LINE).await, world.ran_in(&world.bob));
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_moved_to_another_agent_reaches_the_new_holder(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    let alices = world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));

    world
        .desk
        .unassign(&world.alice.workspace_id, &alices.id)
        .await
        .unwrap();
    world
        .desk
        .assign(
            &world.alice.workspace_id,
            &alices.id,
            &world.alice.second_agent_id,
        )
        .await
        .unwrap();
    world.registered().await;

    assert_eq!(
        world.call(ALICE_LINE).await,
        OpenedRun {
            workspace_id: world.alice.workspace_id.clone(),
            agent_id: world.alice.second_agent_id.clone(),
        }
    );
    assert_eq!(world.call(BOB_LINE).await, world.ran_in(&world.bob));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn assigning_a_number_leaves_the_line_and_a_new_credential_restarts_it(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    let alices = world.buy(&world.alice, ALICE_LINE, None).await;
    world.line_up().await;
    let before = world.transport.calls();

    world
        .desk
        .assign(&world.alice.workspace_id, &alices.id, &world.alice.agent_id)
        .await
        .unwrap();
    world
        .desk
        .unassign(&world.alice.workspace_id, &alices.id)
        .await
        .unwrap();
    world
        .desk
        .assign(&world.alice.workspace_id, &alices.id, &world.alice.agent_id)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The line neither registered again nor dropped its registration.
    assert_eq!(world.transport.calls(), before);
    assert!(world.transport.is_registered(USERNAME));
    assert_eq!(
        world.line_state(&world.alice, &alices).await,
        RegistrationState::Registered
    );

    // A new SIP credential restarts the one line.
    world.desk.sip_credential_changed(&world.carrier).await;
    world.registered().await;
    let after: Vec<LineCall> = world.transport.calls()[before.len()..].to_vec();
    assert!(
        matches!(
            after.as_slice(),
            [
                LineCall::Unregister { .. },
                LineCall::Open { .. },
                LineCall::Register { .. },
            ]
        ),
        "{after:?}"
    );
    assert_eq!(world.call(ALICE_LINE).await, world.ran_in(&world.alice));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_second_call_to_a_number_on_a_call_is_busy_and_the_other_number_answers(
    pool: SqlitePool,
) {
    let world = World::build(pool, Bridge::Holding).await;
    world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;

    let first = world.ring(ALICE_LINE, CALLER).await;
    assert_eq!(first.state(), PartyState::Answered);
    eventually("the first call reaches the bridge", || {
        world.holding.answered().len() == 1
    })
    .await;

    let second = world.ring(ALICE_LINE, "+16505550101").await;
    assert_eq!(second.state(), PartyState::Refused(Refusal::Busy));

    let other = world.ring(BOB_LINE, "+16505550102").await;
    assert_eq!(other.state(), PartyState::Answered);
    eventually("the other call reaches the bridge", || {
        world.holding.answered().len() == 2
    })
    .await;
    let answered = world.holding.answered();
    assert_eq!(answered[0].workspace_id, world.alice.workspace_id);
    assert_eq!(answered[1].workspace_id, world.bob.workspace_id);
    assert_eq!(answered[1].brief.agent_id, world.bob.agent_id);

    world.holding.release.send_replace(true);
    eventually("both calls end", || world.runs.closed() == 2).await;
    assert_eq!(first.state(), PartyState::Ended(EndedReason::LocalHangup));
}

/// The inbound Call record names the dialed number beside the Remote
/// Party's number: the record lands in the Workspace of the number's
/// holder, and its own line is the number the caller dialed.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_call_record_of_an_inbound_call_names_the_dialed_number(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Realtime).await;
    let alices = world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    let bobs = world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;

    for (person, number) in [(&world.alice, &alices), (&world.bob, &bobs)] {
        let settled = world.runs.closed();
        let sessions = world.sessions.peers().len();
        let party = world.ring(&number.e164, CALLER).await;
        // The caller hangs up once the Agent's session is open.
        eventually("the model session opens", || {
            world.sessions.peers().len() > sessions
        })
        .await;
        party.hangup();
        eventually("the call settles", || world.runs.closed() > settled).await;

        let records = world
            .calls
            .list(&person.workspace_id, None, None, None, None, 10)
            .await
            .unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        let record = &records[0];
        assert_eq!(record.direction, CallDirection::Inbound);
        assert_eq!(record.own_e164, number.e164);
        assert_eq!(record.remote_e164, CALLER);
        assert_eq!(record.phone_number_id, number.id);
        assert_eq!(record.agent_id, person.agent_id);
    }
}

/// Both numbers place their calls on the one line of the carrier, and
/// each `INVITE` and each Call record names the number that placed it.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn each_number_on_one_line_places_its_own_outbound_call(pool: SqlitePool) {
    let world = World::build(pool, Bridge::Scripted).await;
    let alices = world
        .buy(&world.alice, ALICE_LINE, Some(&world.alice.agent_id))
        .await;
    let bobs = world
        .buy(&world.bob, BOB_LINE, Some(&world.bob.agent_id))
        .await;
    world.line_up().await;

    let mut placed_ids = Vec::new();
    for (person, number) in [(&world.alice, &alices), (&world.bob, &bobs)] {
        let placed = world.outbound(person, number).await;
        let dialed = world.dials().len();
        let transport = Arc::clone(&world.transport);
        // The callee is busy, so the call ends as soon as it rings.
        let callee = tokio::spawn(async move {
            loop {
                if let Some(party) = transport.dials().get(dialed) {
                    party.refuse(EndedReason::Busy);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let log = CallLog::new(Arc::clone(&world.bus), &placed);
        let report = world
            .realtime
            .place(placed.clone(), &log)
            .await
            .expect("the call is placed");
        callee.await.unwrap();
        assert_eq!(report.outcome, CallOutcome::Busy);
        placed_ids.push((person.workspace_id.clone(), placed.id));
    }

    assert_eq!(
        world.dials(),
        vec![
            (ALICE_LINE.to_string(), CALLER.to_string()),
            (BOB_LINE.to_string(), CALLER.to_string()),
        ]
    );
    for ((workspace_id, call_id), number) in placed_ids.iter().zip([&alices, &bobs]) {
        let record = world
            .calls
            .get(workspace_id, call_id)
            .await
            .unwrap()
            .expect("a Call record");
        assert_eq!(record.direction, CallDirection::Outbound);
        assert_eq!(record.own_e164, number.e164);
        assert_eq!(record.phone_number_id, number.id);
        assert_eq!(record.remote_e164, CALLER);
    }
    assert_eq!(world.count(|call| matches!(call, LineCall::Open { .. })), 1);
}

impl World {
    /// An outbound call one Person's Agent places from its number, with
    /// the Run it belongs to.
    async fn outbound(&self, person: &Person, number: &PhoneNumber) -> PlacedCall {
        let run = Run {
            id: RunId::generate(),
            workspace_id: person.workspace_id.clone(),
            agent_id: person.agent_id.clone(),
            channel_id: None,
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
        self.run_store.create(&run).await.unwrap();
        PlacedCall {
            id: CallId::generate(),
            workspace_id: person.workspace_id.clone(),
            run_id: run.id,
            brief: CallBrief {
                direction: CallDirection::Outbound,
                agent_id: person.agent_id.clone(),
                agent_name: "Robin".to_string(),
                voice: Some("marin".to_string()),
                phone_number_id: number.id.clone(),
                own_e164: number.e164.clone(),
                remote_e164: CALLER.to_string(),
                tier: TrustTier::Unknown,
                purpose: "book a table".to_string(),
                success_criteria: Some("a table is booked".to_string()),
                voicemail: VoicemailPolicy::HangUp,
                duration_cap: DEFAULT_DURATION_CAP,
                tools: Vec::new(),
                ivr_mode_prompt: IVR_MODE_PROMPT,
                classify_prompt: pagis_telephony::CLASSIFY_PROMPT,
                emergency_rule: EMERGENCY_RULE,
            },
            tools: Vec::new(),
        }
    }
}
