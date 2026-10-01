//! Recording and the settled Call record (ADR-0020).
//!
//! The first three tests read the mux step alone: it takes the two raw
//! G.711 legs from the disk and answers one 8 kHz stereo WAV, the
//! Remote Party on the left and the Agent on the right. It reads only
//! files, so it settles a call the daemon was killed in the middle of.
//!
//! The next three run a call to each of the three ended reasons the
//! bridge owns and read what the call left: a playable recording and a
//! settled Call record.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use object_store::ObjectStoreExt as _;
use pagis_broker::{ToolCall, ToolResult};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, ArtifactStore, Call, CallDirection, CallId, CallState,
    CallStore, Channel, ChannelId, ChannelKind, ChannelStore, ConnectionId, Event, EventBus,
    EventId, EventScope, EventStream, MemorySecretStore, NewEvent, PhoneNumber, PhoneNumberId, Run,
    RunId, RunState, RunStore, SecretStore, Speaker, StoreError, TriggerKind, TrustTier, Workspace,
    WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteArtifactStore, SqliteCallStore, SqliteChannelStore,
    SqliteConnectionStore, SqlitePhoneNumberStore, SqliteRunStore, SqliteWorkspaceStore,
};
use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberDirectory, RemoteParty, TokioClock};
use pagis_telephony::hub::{Direction, Recorded, RecordingSink};
use pagis_telephony::model_fake::{FakeModelSessions, ModelPeer};
use pagis_telephony::recording::{self, CallRecorder, RECORDING_MIME};
use pagis_telephony::{
    CallBridge as _, CallBrief, CallLog, CallTools, DAEMON_RESTART, DEFAULT_DURATION_CAP,
    EMERGENCY_RULE, EndpointTask, Endpoints, EndpointsDeps, IVR_MODE_PROMPT, LiveCalls,
    MODEL_UNAVAILABLE, PlacedCall, RealtimeBridge, RealtimeBridgeDeps, RegistrationState,
    SipCredential, VoicemailPolicy,
};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+14155550199";

// -- the mux step -------------------------------------------------------

/// The samples of one channel of a WAV, 0 for left and 1 for right.
fn channel(wav: &[u8], index: usize) -> Vec<i16> {
    let mut reader = hound::WavReader::new(Cursor::new(wav)).expect("a readable WAV");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 8000);
    assert_eq!(spec.bits_per_sample, 16);
    reader
        .samples::<i16>()
        .map(|sample| sample.expect("a sample"))
        .skip(index)
        .step_by(2)
        .collect()
}

/// A frame whose every sample is this one code.
fn tone(codec: Codec, byte: u8) -> Frame {
    Frame::new(codec, vec![byte; FRAME_BYTES])
}

#[tokio::test]
async fn the_mux_puts_the_remote_party_left_and_the_agent_right() {
    let dir = tempfile::tempdir().unwrap();
    let call_id = CallId::generate();
    let recorder = CallRecorder::create(dir.path(), &call_id, Codec::Pcmu)
        .await
        .expect("the legs open");
    let loud = audio_codec_algorithms::encode_ulaw(8000);
    let quiet = audio_codec_algorithms::encode_ulaw(1000);
    recorder
        .write(Recorded {
            direction: Direction::Uplink,
            frame: tone(Codec::Pcmu, loud),
        })
        .await;
    recorder
        .write(Recorded {
            direction: Direction::Downlink,
            frame: tone(Codec::Pcmu, quiet),
        })
        .await;

    let wav = recording::mux(dir.path(), &call_id)
        .await
        .expect("the mux reads the legs")
        .expect("the call recorded audio");
    let left = channel(&wav, 0);
    let right = channel(&wav, 1);
    assert_eq!(left.len(), FRAME_BYTES);
    assert_eq!(right.len(), FRAME_BYTES);
    // The Remote Party is the loud one, so the left channel is louder.
    assert!(left[0].abs() > right[0].abs());
}

#[tokio::test]
async fn the_mux_pads_the_leg_a_killed_daemon_cut_short() {
    let dir = tempfile::tempdir().unwrap();
    let call_id = CallId::generate();
    let recorder = CallRecorder::create(dir.path(), &call_id, Codec::Pcmu)
        .await
        .expect("the legs open");
    for _ in 0..3 {
        recorder
            .write(Recorded {
                direction: Direction::Downlink,
                frame: Frame::silence(Codec::Pcmu),
            })
            .await;
    }
    recorder
        .write(Recorded {
            direction: Direction::Uplink,
            frame: Frame::silence(Codec::Pcmu),
        })
        .await;
    // Nothing closes the recorder: this is the state a killed daemon
    // leaves on the disk.
    drop(recorder);

    let wav = recording::mux(dir.path(), &call_id)
        .await
        .expect("the mux reads the legs")
        .expect("the call recorded audio");
    assert_eq!(channel(&wav, 0).len(), 3 * FRAME_BYTES);
    assert_eq!(channel(&wav, 1).len(), 3 * FRAME_BYTES);
}

#[tokio::test]
async fn a_call_that_recorded_nothing_has_no_recording() {
    let dir = tempfile::tempdir().unwrap();
    let muxed = recording::mux(dir.path(), &CallId::generate())
        .await
        .expect("the mux answers");
    assert!(muxed.is_none());
}

// -- one call to each ended reason --------------------------------------

/// No tool of the brief is called in these tests.
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
        _workspace_id: &WorkspaceId,
    ) -> Result<Vec<pagis_core::TrustEntry>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::TrustEntryId,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn candidate(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _subject: pagis_core::TrustSubject,
        _value: &str,
    ) -> Result<Option<TrustTier>, StoreError> {
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

/// One workspace with one Agent on one line, and the bridge that runs
/// its calls.
struct World {
    bridge: RealtimeBridge,
    transport: Arc<FakeCallTransport>,
    sessions: Arc<FakeModelSessions>,
    calls: Arc<SqliteCallStore>,
    artifacts: Arc<SqliteArtifactStore>,
    blobs: Arc<object_store::memory::InMemory>,
    bus: Arc<dyn EventBus>,
    cancel: CancellationToken,
    workspace_id: WorkspaceId,
    agent: Agent,
    run_id: RunId,
    number_id: PhoneNumberId,
}

impl World {
    async fn build(pool: SqlitePool) -> Self {
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
        };
        SqliteWorkspaceStore::new(pool.clone())
            .create(&workspace)
            .await
            .unwrap();

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
        SqliteAgentStore::new(pool.clone())
            .create(&agent)
            .await
            .unwrap();

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
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let bus: Arc<dyn EventBus> = Arc::new(SilentBus);
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::clone(&transport) as _,
            numbers: Arc::new(SqlitePhoneNumberStore::new(pool.clone())),
            connections: Arc::new(SqliteConnectionStore::new(pool.clone())),
            org_workspace_id: workspace.id.clone(),
            secrets,
            bus: Arc::clone(&bus),
        }));
        let calls = Arc::new(SqliteCallStore::new(pool.clone()));
        let artifacts = Arc::new(SqliteArtifactStore::new(pool.clone()));
        let blobs = Arc::new(object_store::memory::InMemory::new());
        let sessions = Arc::new(FakeModelSessions::new());
        let cancel = CancellationToken::new();
        let bridge = RealtimeBridge::new(RealtimeBridgeDeps {
            endpoints,
            sessions: Arc::clone(&sessions) as _,
            tools: Arc::new(NoTools),
            calls: Arc::clone(&calls) as _,
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs) as _,
            recordings: tempfile::tempdir().unwrap().keep(),
            live: Arc::new(LiveCalls::default()),
            tiers: Arc::new(NoLists),
            keypad: pagis_telephony::Keypad {
                code: Arc::new(pagis_telephony::keypad::NoCode),
                failures: Arc::new(pagis_storage_sqlite::SqliteKeypadFailureStore::new(
                    pool.clone(),
                )),
                clock: Arc::new(pagis_core::SystemClock),
            },
            live_tiers: Arc::new(pagis_telephony::LiveTiers::default()),
            cancel: cancel.clone(),
        });
        Self {
            bridge,
            transport,
            sessions,
            calls,
            artifacts,
            blobs,
            bus,
            cancel,
            workspace_id: workspace.id,
            agent,
            run_id: run.id,
            number_id: number.id,
        }
    }

    fn placed(&self, duration_cap: Duration) -> PlacedCall {
        PlacedCall {
            id: CallId::generate(),
            workspace_id: self.workspace_id.clone(),
            run_id: self.run_id.clone(),
            brief: CallBrief {
                direction: CallDirection::Inbound,
                agent_id: self.agent.id.clone(),
                agent_name: self.agent.name.clone(),
                voice: self.agent.voice.clone(),
                phone_number_id: self.number_id.clone(),
                own_e164: OWN.to_string(),
                remote_e164: REMOTE.to_string(),
                tier: TrustTier::Unknown,
                purpose: pagis_telephony::TAKE_A_MESSAGE.to_string(),
                success_criteria: None,
                voicemail: VoicemailPolicy::HangUp,
                duration_cap,
                tools: Vec::new(),
                ivr_mode_prompt: IVR_MODE_PROMPT,
                classify_prompt: pagis_telephony::CLASSIFY_PROMPT,
                emergency_rule: EMERGENCY_RULE,
            },
            tools: Vec::new(),
        }
    }

    /// Ring the line, answer the call, and pump it. The test scripts
    /// the Remote Party and the model peer while the call runs.
    async fn call<F, Fut>(&self, duration_cap: Duration, script: F) -> Call
    where
        F: FnOnce(Arc<RemoteParty>, Arc<ModelPeer>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let (answered, mut incoming) = tokio::sync::mpsc::channel(1);
        let task = EndpointTask::spawn(
            Arc::clone(&self.transport) as _,
            Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
            Arc::new(FakeNumberDirectory::with(vec![FakeNumberDirectory::held(
                OWN,
            )])),
            answered,
        );
        task.watch()
            .wait_for(|state| *state == RegistrationState::Registered)
            .await
            .expect("the line registers");
        let party = self.transport.ring(OWN, REMOTE);
        let hub = incoming.recv().await.expect("the task answers the call");

        let placed = self.placed(duration_cap);
        let call_id = placed.id.clone();
        let log = CallLog::new(Arc::clone(&self.bus), &placed);
        let sessions = Arc::clone(&self.sessions);
        let scripted = tokio::spawn(async move {
            let peer = loop {
                if let Some(peer) = sessions.peers().first().cloned() {
                    break peer;
                }
                tokio::task::yield_now().await;
            };
            peer.wait_for("session.update").await;
            script(party, peer).await;
        });
        self.bridge
            .answer(hub, placed, &log)
            .await
            .expect("the call runs");
        // A script that talks until the call ends does not return.
        scripted.abort();
        self.calls
            .get(&self.workspace_id, &call_id)
            .await
            .unwrap()
            .expect("a Call record")
    }

    /// The bytes of the recording one settled record points at.
    async fn recording_of(&self, call: &Call) -> Vec<u8> {
        let id = call
            .recording_artifact_id
            .clone()
            .expect("the record points at a recording");
        let artifact = self
            .artifacts
            .get(&self.workspace_id, &id)
            .await
            .unwrap()
            .expect("the Artifact row");
        assert_eq!(artifact.mime, RECORDING_MIME);
        let path = object_store::path::Path::from(artifact.storage_key);
        self.blobs
            .get(&path)
            .await
            .expect("the blob")
            .bytes()
            .await
            .expect("the bytes")
            .to_vec()
    }
}

/// What every settled record must say, whatever ended the call.
fn assert_settled(call: &Call, ended_reason: &str) {
    assert_eq!(call.state, CallState::Ended);
    assert_eq!(call.ended_reason.as_deref(), Some(ended_reason));
    assert_eq!(call.direction, CallDirection::Inbound);
    assert_eq!(call.remote_e164, REMOTE);
    assert_eq!(call.tier, TrustTier::Unknown);
    assert!(call.outcome.is_some());
    assert!(call.answered_at.is_some());
    assert!(call.ended_at.is_some());
}

/// A recording both channels of which are playable and the same length.
fn assert_playable(wav: &[u8]) {
    let left = channel(wav, 0);
    let right = channel(wav, 1);
    assert!(!left.is_empty(), "the recording has no audio");
    assert_eq!(left.len(), right.len());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_the_model_left_settles_with_a_recording(pool: SqlitePool) {
    let world = World::build(pool).await;
    let sessions = Arc::clone(&world.sessions);
    let settled = world
        .call(DEFAULT_DURATION_CAP, move |party, peer| async move {
            party.speak(Frame::silence(Codec::Pcmu));
            peer.caller_transcript("is anybody there?");
            // The socket goes and the reconnect finds no model.
            sessions.fail_next(1);
            peer.drop_socket();
        })
        .await;

    assert_settled(&settled, MODEL_UNAVAILABLE);
    assert_playable(&world.recording_of(&settled).await);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_the_daemon_stopped_settles_with_a_recording(pool: SqlitePool) {
    let world = World::build(pool).await;
    let cancel = world.cancel.clone();
    let settled = world
        .call(DEFAULT_DURATION_CAP, move |party, peer| async move {
            party.speak(Frame::silence(Codec::Pcmu));
            peer.caller_transcript("one moment");
            cancel.cancel();
        })
        .await;

    assert_settled(&settled, DAEMON_RESTART);
    assert_playable(&world.recording_of(&settled).await);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_that_ran_out_of_time_settles_with_a_recording(pool: SqlitePool) {
    let world = World::build(pool).await;
    let settled = world
        .call(Duration::from_millis(300), |party, peer| async move {
            peer.caller_transcript("this will take a while");
            // The party keeps talking until the cap ends the call.
            loop {
                party.speak(Frame::silence(Codec::Pcmu));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;

    assert_settled(&settled, pagis_telephony::DURATION_CAP);
    assert_playable(&world.recording_of(&settled).await);
    assert!(
        settled
            .transcript
            .iter()
            .any(|line| line.speaker == Speaker::Caller),
    );
}

/// A call the daemon never settled, because it was killed. The next
/// start reads it from the disk and settles it (ADR-0020).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_killed_daemon_settles_its_calls_at_the_next_start(pool: SqlitePool) {
    let world = World::build(pool).await;
    let placed = world.placed(DEFAULT_DURATION_CAP);
    let interrupted = Call {
        id: placed.id.clone(),
        workspace_id: placed.workspace_id.clone(),
        agent_id: world.agent.id.clone(),
        run_id: world.run_id.clone(),
        phone_number_id: world.number_id.clone(),
        direction: CallDirection::Inbound,
        remote_e164: REMOTE.to_string(),
        agent_name: world.agent.name.clone(),
        own_e164: OWN.to_string(),
        purpose: "take a message".to_string(),
        tools: Vec::new(),
        tier: TrustTier::Unknown,
        state: CallState::Live,
        outcome: None,
        ended_reason: None,
        classification: None,
        message_left: false,
        transcript: Vec::new(),
        recording_artifact_id: None,
        created_at: now_ms(),
        ringing_at: None,
        answered_at: Some(now_ms()),
        ended_at: None,
        dismissed_at: None,
    };
    world.calls.insert(&interrupted).await.unwrap();

    assert_eq!(world.bridge.recover().await.unwrap(), 1);
    let settled = world
        .calls
        .get(&world.workspace_id, &placed.id)
        .await
        .unwrap()
        .expect("a Call record");
    assert_settled(&settled, DAEMON_RESTART);
    // The call left no audio on the disk, so it has no recording; the
    // record still settles, because every Call ends with a reason.
    assert!(settled.recording_artifact_id.is_none());
    assert!(world.calls.list_unsettled().await.unwrap().is_empty());
}
