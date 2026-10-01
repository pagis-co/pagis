//! Listen-Live (ADR-0020): the daemon carries a live call to the
//! browser over `WS /api/v1/calls/{id}/listen`, as a mono mix of both
//! directions in G.711, one message per 20 ms frame. A listener who
//! joins hears the call from that moment, a listener that never reads
//! does not slow the call, and `POST /calls/{id}/control` is reserved.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use pagis_core::CallId;
use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::fake::{FakeCallTransport, RemoteParty, TokioClock};
use pagis_telephony::hub::MediaHub;
use pagis_telephony::{CallTransport, SipCredential};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The Call record of a live call, in the seeded person's Workspace.
async fn seed_call_record(daemon: &TestDaemon, call_id: &CallId) {
    use pagis_core::now_ms;
    use pagis_core::{
        Call, CallDirection, CallState, CallStore, PhoneNumberId, RunStore, TrustTier,
    };
    let run = pagis_testkit::fixture::queued_run(
        &daemon.workspace_id,
        &pagis_core::AgentId::from(daemon.agent_id.clone()),
        &pagis_core::ChannelId::from(daemon.dm_channel_id.clone()),
    );
    pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone())
        .create(&run)
        .await
        .expect("write the Run of the live call");
    pagis_storage_sqlite::SqliteCallStore::new(daemon.pool().clone())
        .insert(&Call {
            id: call_id.clone(),
            workspace_id: daemon.workspace_id.clone(),
            agent_id: pagis_core::AgentId::from(daemon.agent_id.clone()),
            run_id: run.id.clone(),
            phone_number_id: PhoneNumberId::generate(),
            direction: CallDirection::Outbound,
            remote_e164: "+14155550124".to_string(),
            agent_name: "Pixie".to_string(),
            own_e164: "+14155550123".to_string(),
            purpose: "listen live".to_string(),
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
        })
        .await
        .expect("write the Call record of the live call");
}

/// A daemon with one call live on a fake carrier.
struct Desk {
    daemon: TestDaemon,
    call_id: CallId,
    party: Arc<RemoteParty>,
    _hub: Arc<MediaHub>,
}

impl Desk {
    async fn start() -> Self {
        let daemon = TestDaemon::start().await;
        let transport = FakeCallTransport::new(Arc::new(TokioClock));
        let credential = SipCredential::new("user", "secret", "sip.example.test");
        let mut opened = transport.open(&credential).await.expect("open the line");
        let leg = opened
            .line
            .dial("+14155550123", "+14155550124")
            .await
            .expect("dial the far side");
        let party = transport.dials().pop().expect("one dialed party");
        let hub = MediaHub::start(leg);
        let call_id = CallId::generate();
        // A live call always has its record: the bridge writes one when
        // the call starts (ADR-0020). The hub goes in under the seeded
        // person's Workspace, as the bridge puts it there.
        seed_call_record(&daemon, &call_id).await;
        daemon
            .live_calls
            .attach_hub(&daemon.workspace_id, &call_id, Arc::clone(&hub));
        party.answer();
        Self {
            daemon,
            call_id,
            party,
            _hub: hub,
        }
    }

    fn listen_url(&self, call_id: &str) -> String {
        format!("ws://{}/api/v1/calls/{call_id}/listen", self.daemon.addr)
    }

    /// Open a listen socket and consume the first frame. The session
    /// cookie authenticates the upgrade, so the socket sends
    /// nothing first.
    async fn listen(&self, call_id: &str) -> (Socket, serde_json::Value) {
        self.listen_as(call_id, self.daemon.cookie()).await
    }

    /// The same, for the Session of `cookie`.
    async fn listen_as(&self, call_id: &str, cookie: &str) -> (Socket, serde_json::Value) {
        let url = self.listen_url(call_id);
        let (mut socket, _) = connect_async(self.daemon.ws_request_as(&url, cookie))
            .await
            .expect("ws connect");
        let first = next_text(&mut socket).await;
        (socket, first)
    }

    /// The Remote Party speaks one loud frame every 20 ms.
    fn speak(&self) -> tokio::task::JoinHandle<()> {
        let party = Arc::clone(&self.party);
        tokio::spawn(async move {
            let frame = Frame::new(
                Codec::Pcmu,
                vec![audio_codec_algorithms::encode_ulaw(8000); FRAME_BYTES],
            );
            loop {
                party.speak(frame.clone());
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    }
}

async fn next_text(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("frame is JSON");
        }
    }
}

async fn next_binary(socket: &mut Socket) -> Vec<u8> {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Binary(bytes) = frame {
            return bytes.to_vec();
        }
    }
}

#[tokio::test]
async fn a_listener_hears_the_call_as_one_frame_every_twenty_milliseconds() {
    let desk = Desk::start().await;
    let speech = desk.speak();

    let (mut socket, ready) = desk.listen(desk.call_id.as_str()).await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["codec"], "PCMU");

    let started = Instant::now();
    let mut frames = Vec::new();
    for _ in 0..25 {
        frames.push(next_binary(&mut socket).await);
    }
    let elapsed = started.elapsed();

    assert!(
        frames.iter().all(|frame| frame.len() == FRAME_BYTES),
        "a message is one 20 ms G.711 frame"
    );
    assert!(
        elapsed >= Duration::from_millis(350),
        "25 frames arrived in {elapsed:?}: faster than the pacer"
    );
    // The mix carries the Remote Party: the frames are not silence.
    let silence = Codec::Pcmu.silence_byte();
    assert!(
        frames
            .iter()
            .any(|frame| frame.iter().any(|byte| *byte != silence)),
        "the listener heard silence only"
    );
    speech.abort();
}

#[tokio::test]
async fn a_call_that_is_not_live_is_not_listenable() {
    let desk = Desk::start().await;

    let (_socket, frame) = desk.listen(CallId::generate().as_str()).await;

    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "not_found");
}

#[tokio::test]
async fn a_listen_socket_without_a_session_is_refused() {
    let desk = Desk::start().await;

    let error = connect_async(desk.listen_url(desk.call_id.as_str()))
        .await
        .expect_err("the daemon upgrades no socket without a session");

    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 401);
        }
        other => panic!("unexpected handshake failure: {other}"),
    }
}

/// Hang up (ADR-0022): the user ends a live call from the call
/// inspector, and the daemon hangs up its own leg. There is no such
/// control on a call that is not live.
#[tokio::test]
async fn the_user_hangs_up_a_live_call() {
    let desk = Desk::start().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!(
            "{}/api/v1/calls/{}/hangup",
            desk.daemon.base_url,
            desk.call_id.as_str()
        ))
        .header("cookie", desk.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);

    let missing = client
        .post(format!(
            "{}/api/v1/calls/no-such-call/hangup",
            desk.daemon.base_url
        ))
        .header("cookie", desk.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

/// The Call record is the one source of truth the call surfaces read.
/// A call with no record answers 404, so the strip says so
/// rather than showing a call that does not exist.
#[tokio::test]
async fn a_call_with_no_record_is_not_found() {
    let desk = Desk::start().await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/calls/{}",
            desk.daemon.base_url,
            CallId::generate().as_str()
        ))
        .header("cookie", desk.daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn control_is_reserved() {
    let desk = Desk::start().await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/calls/{}/control",
            desk.daemon.base_url,
            desk.call_id.as_str()
        ))
        .header("cookie", desk.daemon.cookie())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 501);
}

/// A call does not survive a restart (ADR-0020): a Call record a
/// stopped daemon left live settles at the next boot with
/// `daemon_restart`, so the call surfaces never show a call that is
/// not running.
#[tokio::test]
async fn a_call_left_live_by_a_stopped_daemon_settles_at_boot() {
    use pagis_core::{
        Call, CallDirection, CallState, CallStore, PhoneNumberId, Run, RunId, RunState, RunStore,
        TriggerKind, TrustTier, WorkspaceStore, now_ms,
    };
    use pagis_storage_sqlite::{SqliteCallStore, SqliteRunStore, SqliteWorkspaceStore};

    let daemon = TestDaemon::start_with(TestDaemonOptions {
        call_bridge: None,
        ..TestDaemonOptions::default()
    })
    .await;
    let pool = daemon.pool().clone();
    let workspace = SqliteWorkspaceStore::new(pool.clone())
        .list()
        .await
        .expect("workspaces")
        .remove(0);
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: pagis_core::AgentId::from(daemon.agent_id.clone()),
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
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .expect("the run");
    let call_id = CallId::generate();
    let live = Call {
        id: call_id.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: run.agent_id.clone(),
        run_id: run.id.clone(),
        phone_number_id: PhoneNumberId::generate(),
        direction: CallDirection::Inbound,
        remote_e164: "+14155550199".to_string(),
        agent_name: "Pixie".to_string(),
        own_e164: "+14155550123".to_string(),
        purpose: String::new(),
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
    SqliteCallStore::new(pool.clone())
        .insert(&live)
        .await
        .expect("the live call");

    let daemon = daemon
        .restart(TestDaemonOptions {
            call_bridge: None,
            ..TestDaemonOptions::default()
        })
        .await;

    let call = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/calls/{}",
            daemon.base_url,
            call_id.as_str()
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(call["state"], "ended");
    assert_eq!(call["ended_reason"], "daemon_restart");
    assert_eq!(call["outcome"], "answered");
}

/// The calls list: `GET /api/v1/calls` gives one page of Call
/// records, newest first, narrowed by direction and paged with
/// `before`. Home reads it to find the calls that were missed today.
#[tokio::test]
async fn the_calls_list_filters_by_direction_and_pages() {
    use pagis_core::{
        Call, CallDirection, CallOutcome, CallState, CallStore, PhoneNumberId, Run, RunId,
        RunState, RunStore, TriggerKind, TrustTier, WorkspaceStore, now_ms,
    };
    use pagis_storage_sqlite::{SqliteCallStore, SqliteRunStore, SqliteWorkspaceStore};

    let daemon = TestDaemon::start().await;
    let pool = daemon.pool().clone();
    let workspace = SqliteWorkspaceStore::new(pool.clone())
        .list()
        .await
        .expect("workspaces")
        .remove(0);
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: agent_id.clone(),
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Event,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Completed,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        started_at: Some(now_ms()),
        ended_at: Some(now_ms()),
        created_at: now_ms(),
    };
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .expect("the run");

    let calls = SqliteCallStore::new(pool.clone());
    let mut ids = Vec::new();
    for (direction, remote) in [
        (CallDirection::Inbound, "+14155550001"),
        (CallDirection::Outbound, "+14155550002"),
        (CallDirection::Inbound, "+14155550003"),
    ] {
        let id = pagis_core::CallId::generate();
        ids.push(id.clone());
        calls
            .insert(&Call {
                id,
                workspace_id: workspace.id.clone(),
                agent_id: agent_id.clone(),
                run_id: run.id.clone(),
                phone_number_id: PhoneNumberId::generate(),
                direction,
                remote_e164: remote.to_string(),
                agent_name: "Pixie".to_string(),
                own_e164: "+14155550123".to_string(),
                purpose: String::new(),
                tools: Vec::new(),
                tier: TrustTier::Unknown,
                state: CallState::Ended,
                outcome: Some(CallOutcome::NoAnswer),
                ended_reason: Some("no_answer".to_string()),
                classification: None,
                message_left: false,
                transcript: Vec::new(),
                recording_artifact_id: None,
                created_at: now_ms(),
                ringing_at: None,
                answered_at: None,
                ended_at: Some(now_ms()),
                dismissed_at: None,
            })
            .await
            .expect("the call");
    }

    let client = reqwest::Client::new();
    let page = |query: String| {
        let client = client.clone();
        let url = format!("{}/api/v1/calls?{query}", daemon.base_url);
        let cookie = daemon.cookie().to_string();
        async move {
            let response = client
                .get(url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            response.json::<serde_json::Value>().await.unwrap()
        }
    };

    // Newest first, and the outbound call is left out.
    let inbound = page("direction=inbound".to_string()).await;
    let listed: Vec<&str> = inbound["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["remote_e164"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec!["+14155550003", "+14155550001"]);

    // One page at a time, and `before` is exclusive.
    let first = page("direction=inbound&limit=1".to_string()).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], ids[2].as_str());
    let next = page(format!(
        "direction=inbound&limit=1&before={}",
        ids[2].as_str()
    ))
    .await;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["items"][0]["id"], ids[0].as_str());

    // An unknown state is a validation error, not an empty page.
    let bad = client
        .get(format!("{}/api/v1/calls?state=nonsense", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
}

/// A listener hears a call no longer than their Session lives. A
/// sign-out closes the listen socket with 1008 while the call goes on.
#[tokio::test]
async fn a_sign_out_closes_the_listen_socket_with_1008() {
    let desk = Desk::start().await;
    let speech = desk.speak();
    let signing_out = desk.daemon.cookie_for(&desk.daemon.user_id).await;
    let (mut socket, ready) = desk.listen_as(desk.call_id.as_str(), &signing_out).await;
    assert_eq!(ready["type"], "ready");
    next_binary(&mut socket).await;

    desk.daemon.sign_out(&signing_out).await;

    let closed = pagis_testkit::read_until_closed(&mut socket).await;
    assert_eq!(closed.code, 1008);
    assert!(closed.frames.is_empty(), "{:?}", closed.frames);
    speech.abort();
}
