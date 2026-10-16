//! Inbound Calls and texts reach the Workspace's Chief of Staff. A live Call can move to another Agent of the Workspace.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberCatalog, PartyState, TokioClock};
use pagis_telephony::{CallDirection, FakeCallBridge};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+16505550100";

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame arrives")
            .expect("the socket is open")
            .expect("a text frame");
        let Message::Text(text) = frame else { continue };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        if value["type"] == frame_type {
            return value;
        }
    }
}

struct Desk {
    daemon: TestDaemon,
    transport: Arc<FakeCallTransport>,
    bridge: Arc<FakeCallBridge>,
}

impl Desk {
    /// A daemon whose default Agent holds a registered line.
    async fn start() -> Self {
        Self::start_with(TestDaemonOptions::default()).await
    }

    /// A daemon on `options` whose default Agent holds a registered line.
    async fn start_with(options: TestDaemonOptions) -> Self {
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let bridge = Arc::new(FakeCallBridge::reporting(FakeCallBridge::answered_report(
            "I would like to book a table.",
        )));
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            call_transport: Arc::clone(&transport) as _,
            call_bridge: if options.model_sessions.is_some() {
                None
            } else {
                Some(Arc::clone(&bridge) as _)
            },
            number_catalog: Arc::new(FakeNumberCatalog::offering(&[OWN])),
            ..options
        })
        .await;
        let desk = Self {
            daemon,
            transport,
            bridge,
        };
        desk.daemon
            .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
            .await;
        let (status, body) = desk
            .daemon
            .set_up_provider(
                "telnyx",
                "sip",
                serde_json::json!({
                    "username": "robin",
                    "password": "sip-secret",
                    "domain": "sip.telnyx.com",
                }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = desk
            .post(
                "/api/v1/workspace/number",
                serde_json::json!({ "e164": OWN }),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !desk.transport.is_registered("robin") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the line registers");
        desk
    }

    async fn firehose(&self) -> Socket {
        let (mut socket, _) = connect_async(self.daemon.ws_request(&self.daemon.ws_url()))
            .await
            .expect("ws connect");
        socket
            .send(Message::text(
                serde_json::json!({ "type": "auth" }).to_string(),
            ))
            .await
            .expect("ws send");
        next_frame_of(&mut socket, "ready").await;
        socket
    }

    async fn get(&self, path: &str) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .get(format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .post(format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    async fn put(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .put(format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }
}

#[tokio::test]
async fn a_call_to_the_desk_line_runs_in_the_agents_thread_with_the_user() {
    let desk = Desk::start().await;
    let (status, body) = desk
        .put(
            &format!("/api/v1/agents/{}", desk.daemon.agent_id),
            serde_json::json!({
                "name": "Robin",
                "job": "assistant",
                "personality": "warm",
                "standing_brief": "Book appointments for the clinic.",
            }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let mut socket = desk.firehose().await;

    let party = desk.transport.ring(OWN, REMOTE);

    // The call is placed as inbound, under the Agent's Run, and ends.
    let placed = next_frame_of(&mut socket, "call.placed").await;
    assert_eq!(placed["payload"]["payload"]["direction"], "inbound");
    assert_eq!(placed["payload"]["payload"]["from"], OWN);
    assert_eq!(placed["payload"]["payload"]["to"], REMOTE);
    let call_id = placed["payload"]["payload"]["call_id"]
        .as_str()
        .unwrap()
        .to_string();
    let run_id = placed["payload"]["run_id"].as_str().unwrap().to_string();
    let ended = next_frame_of(&mut socket, "call.ended").await;
    assert_eq!(
        ended["payload"]["payload"]["outcome"], "answered",
        "{ended}"
    );
    assert_eq!(ended["payload"]["payload"]["call_id"], call_id);
    assert!(ended["payload"]["payload"]["transcript_file_id"].is_string());

    // The bridge answered under the standing brief.
    let answered = desk.bridge.answered();
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0].brief.direction, CallDirection::Inbound);
    assert_eq!(
        answered[0].brief.purpose,
        "Book appointments for the clinic."
    );
    assert_eq!(answered[0].brief.remote_e164, REMOTE);
    assert_eq!(answered[0].run_id.as_str(), run_id);

    // The Run belongs to the Agent, in its Thread with the user, and
    // completed with the call.
    let settled = next_frame_of(&mut socket, "run.state_changed").await;
    assert_eq!(settled["payload"]["run_id"], run_id);
    assert_eq!(settled["payload"]["payload"]["to"], "completed");
    let (status, runs) = desk.get("/api/v1/runs").await;
    assert_eq!(status, 200, "{runs}");
    let run = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["id"] == run_id)
        .expect("the Run is listed");
    assert_eq!(run["agent_id"], desk.daemon.agent_id);
    assert_eq!(run["channel_id"], desk.daemon.dm_channel_id);
    assert_eq!(run["trigger_kind"], "event");
    assert_eq!(run["trigger_ref"], call_id);
    assert_eq!(run["state"], "completed");

    // The `call` block landed in the Thread with the user.
    let (status, page) = desk
        .get(&format!(
            "/api/v1/channels/{}/messages",
            desk.daemon.dm_channel_id
        ))
        .await;
    assert_eq!(status, 200, "{page}");
    let strip = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["blocks"][0]["type"] == "call")
        .unwrap_or_else(|| panic!("the call block is in the Thread: {page}"));
    assert_eq!(strip["blocks"][0]["call_id"], call_id);
    assert_eq!(strip["run_id"], run_id);
    assert_eq!(strip["author"]["kind"], "system");

    // The caller heard the end, and the line is free again.
    assert!(matches!(party.state(), PartyState::Ended(_)));
}

#[tokio::test]
async fn a_desk_line_carries_a_standing_call_rule() {
    let desk = Desk::start().await;
    let mut socket = desk.firehose().await;
    desk.transport.ring(OWN, REMOTE);
    next_frame_of(&mut socket, "call.ended").await;

    let (status, page) = desk
        .get(&format!(
            "/api/v1/event-subscriptions?agent_id={}",
            desk.daemon.agent_id
        ))
        .await;

    assert_eq!(status, 200, "{page}");
    let rules = page["items"].as_array().unwrap();
    let rule = rules
        .iter()
        .find(|rule| rule["event_kind"] == "call.ended")
        .unwrap_or_else(|| panic!("the line carries a standing call rule: {page}"));
    assert_eq!(rule["state"], "active");
    assert_eq!(rule["revision"], 1);
    assert_eq!(rule["created_by"]["kind"], "system");
    assert_eq!(rule["channel_id"], desk.daemon.dm_channel_id);
    // Every caller wakes the Agent: the tier gates what the words are
    // worth, never whether it wakes (ADR-0021).
    assert_eq!(rule["filter"], serde_json::json!({}));
    assert!(
        rule["instruction"]
            .as_str()
            .unwrap()
            .contains("call_transcript"),
        "the rule tells the Agent to read the call with the tool: {rule}"
    );
}

#[tokio::test]
async fn a_call_the_agent_answered_wakes_it_afterwards() {
    let desk = Desk::start().await;
    let mut socket = desk.firehose().await;

    desk.transport.ring(OWN, REMOTE);

    let placed = next_frame_of(&mut socket, "call.placed").await;
    let call_id = placed["payload"]["payload"]["call_id"]
        .as_str()
        .unwrap()
        .to_string();
    next_frame_of(&mut socket, "call.ended").await;

    // ADR-0021: the Agent takes the message, and the Run that reads it
    // afterwards reads data.
    let wakeup = next_frame_of(&mut socket, "wakeup.created").await;
    assert_eq!(wakeup["payload"]["agent_id"], desk.daemon.agent_id);
    assert_eq!(wakeup["payload"]["channel_id"], desk.daemon.dm_channel_id);
    assert_eq!(
        wakeup["payload"]["payload"]["source_kind"],
        "event_subscription"
    );

    // The event the Wake-up carries names the call and the caller, and
    // never what was said.
    let (status, events) = desk
        .get(&format!(
            "/api/v1/event-subscriptions/{}/events",
            wakeup["payload"]["payload"]["rule_id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(status, 200, "{events}");
    let event = &events["items"][0];
    assert_eq!(event["metadata"]["call_id"], call_id);
    assert_eq!(event["metadata"]["from"], REMOTE);
    assert_eq!(event["metadata"]["trust_tier"], "unknown");
    assert!(
        !events.to_string().contains("book a table"),
        "the event carries the envelope alone: {events}"
    );
}

/// The run list names each Run by its trigger: the first line of a
/// message, the name of a Schedule and the caller of an inbound Call
/// (ADR-0002).
#[tokio::test]
async fn the_run_list_names_each_run_by_its_trigger() {
    let brain = Arc::new(ScriptedBrain::default());
    for reply in ["Booked.", "Your plan is ready.", "I took the message."] {
        brain.push(Script::reply(&[reply]));
    }
    let desk = Desk::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = desk.firehose().await;

    let (status, body) = desk
        .post(
            &format!("/api/v1/channels/{}/messages", desk.daemon.dm_channel_id),
            serde_json::json!({
                "pending_id": "trip",
                "text": "Book the Austin trip\nwith a late checkout",
            }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    // The Schedule comes due on the wall clock, as the socket waits on
    // the clock of the daemon.
    let due = chrono::DateTime::from_timestamp_millis(pagis_core::now_ms() + 3_000)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (status, body) = desk
        .post(
            "/api/v1/schedules",
            serde_json::json!({
                "agent_id": desk.daemon.agent_id,
                "name": "Morning plan",
                "instruction": "Prepare today's plan",
                "channel_id": desk.daemon.dm_channel_id,
                "root_message_id": null,
                "local_time": due.trim_end_matches('Z'),
                "timezone": "UTC",
            }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    desk.transport.ring(OWN, REMOTE);
    let placed = next_frame_of(&mut socket, "call.placed").await;
    let call_run = placed["payload"]["run_id"].as_str().unwrap().to_string();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let runs = loop {
        let (status, page) = desk.get("/api/v1/runs").await;
        assert_eq!(status, 200, "{page}");
        let runs = page["items"].as_array().unwrap().clone();
        if runs.iter().any(|run| run["trigger_kind"] == "schedule") {
            break runs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Schedule starts a Run: {page}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let title_of = |found: &dyn Fn(&serde_json::Value) -> bool| {
        runs.iter()
            .find(|run| found(run))
            .map(|run| run["title"].clone())
            .unwrap_or_else(|| panic!("the Run is listed: {runs:?}"))
    };
    assert_eq!(
        title_of(&|run| run["trigger_kind"] == "message"),
        "Book the Austin trip"
    );
    assert_eq!(
        title_of(&|run| run["trigger_kind"] == "schedule"),
        "Morning plan"
    );
    assert_eq!(
        title_of(&|run| run["id"] == call_run),
        format!("Call from {REMOTE}")
    );
}

#[tokio::test]
async fn a_workspace_has_one_active_number_and_release_frees_it() {
    let desk = Desk::start().await;
    let (status, body) = desk
        .post("/api/v1/workspace/number", serde_json::json!({"e164": OWN}))
        .await;
    assert_eq!(status, 409, "{body}");
    let response = reqwest::Client::new()
        .delete(format!("{}/api/v1/workspace/number", desk.daemon.base_url))
        .header("cookie", desk.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 204);
    let (status, body) = desk.get("/api/v1/workspace/number").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["number"].is_null(), "{body}");
}

#[tokio::test]
async fn the_next_call_follows_the_chief_of_staff_designation() {
    let desk = Desk::start().await;
    let mut socket = desk.firehose().await;
    desk.transport.ring(OWN, REMOTE);
    next_frame_of(&mut socket, "call.ended").await;
    let (status, agent) = desk
        .post(
            "/api/v1/agents",
            serde_json::json!({"name":"Wren", "job":"support", "personality":"calm"}),
        )
        .await;
    assert_eq!(status, 201, "{agent}");
    let (status, workspace) = desk
        .put(
            "/api/v1/workspace/chief-of-staff",
            serde_json::json!({"agent_id":agent["id"]}),
        )
        .await;
    assert_eq!(status, 200, "{workspace}");
    desk.transport.ring(OWN, REMOTE);
    let placed = next_frame_of(&mut socket, "call.placed").await;
    assert_eq!(placed["payload"]["agent_id"], agent["id"]);
    next_frame_of(&mut socket, "call.ended").await;
    assert_eq!(
        desk.bridge.answered()[1].brief.agent_id.as_str(),
        agent["id"].as_str().unwrap()
    );
}

#[tokio::test]
async fn an_inbound_text_wakes_the_chief_with_an_untrusted_envelope_and_tier() {
    let text = Arc::new(pagis_telephony::fake::FakeTextTransport::default());
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["I took the message."]));
    let desk = Desk::start_with(TestDaemonOptions {
        text_transport: text.clone(),
        brain: brain.clone(),
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = desk.firehose().await;
    text.push_inbound(pagis_telephony::text::InboundText {
        carrier_id: "message-1".into(),
        from_e164: REMOTE.into(),
        to_e164: OWN.into(),
        body: "Please book a table.".into(),
        received_at: pagis_core::now_ms(),
        media_urls: vec![],
    });
    let wakeup = next_frame_of(&mut socket, "wakeup.created").await;
    assert_eq!(wakeup["payload"]["agent_id"], desk.daemon.agent_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let requests = brain.requests();
            if requests.iter().any(|request| {
                format!("{}\n{:?}", request.system, request.messages)
                    .contains("Please book a table.")
            }) {
                let text = requests
                    .iter()
                    .map(|request| format!("{}\n{:?}", request.system, request.messages))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    text.contains("[BEGIN UNTRUSTED source=text_message trust=unknown]"),
                    "{text}"
                );
                assert!(text.contains("unknown"), "{text}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Chief of Staff reads the Text Message");
}

async fn model_peer(
    sessions: &pagis_telephony::model_fake::FakeModelSessions,
    index: usize,
) -> Arc<pagis_telephony::model_fake::ModelPeer> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(peer) = sessions.peers().get(index) {
                return peer.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Call opens a model session")
}

async fn confirmed_call() -> (
    Desk,
    Arc<pagis_telephony::model_fake::FakeModelSessions>,
    Socket,
    Arc<pagis_telephony::fake::RemoteParty>,
    String,
) {
    let sessions = Arc::new(pagis_telephony::model_fake::FakeModelSessions::new());
    let desk = Desk::start_with(TestDaemonOptions {
        model_sessions: Some(sessions.clone()),
        keys: pagis_testkit::test_provider_keys(vec![("OPENAI_API_KEY", "sk-test")]),
        ..TestDaemonOptions::default()
    })
    .await;
    let (status, body) = desk
        .post(
            "/api/v1/settings/trust-list",
            serde_json::json!({"value":REMOTE,"tier":"owner"}),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let (status, body) = desk
        .put(
            "/api/v1/settings/keypad-code",
            serde_json::json!({"code":"123456"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let mut socket = desk.firehose().await;
    let party = desk.transport.ring(OWN, REMOTE);
    let placed = next_frame_of(&mut socket, "call.placed").await;
    let call_id = placed["payload"]["payload"]["call_id"]
        .as_str()
        .unwrap()
        .to_string();
    tokio::time::sleep(Duration::from_millis(200)).await;
    for digit in "123456#".chars() {
        party.press(digit);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let peer = model_peer(&sessions, 0).await;
    let config = peer.wait_for("session.update").await;
    assert!(
        config["session"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "hand_off_call"),
        "{config}"
    );
    (desk, sessions, socket, party, call_id)
}

#[tokio::test]
async fn a_hand_off_keeps_the_remote_party_and_moves_the_call_and_run() {
    let (desk, sessions, mut socket, party, call_id) = confirmed_call().await;
    let (status, agent) = desk
        .post(
            "/api/v1/agents",
            serde_json::json!({"name":"Wren", "job":"clinic", "personality":"calm"}),
        )
        .await;
    assert_eq!(status, 201, "{agent}");
    let target = agent["id"].as_str().unwrap();
    let (status, body) = desk.put(&format!("/api/v1/agents/{target}"), serde_json::json!({"name":"Wren", "job":"clinic", "personality":"calm", "standing_brief":"Arrange clinic appointments.", "voice":"nova"})).await;
    assert_eq!(status, 200, "{body}");
    model_peer(&sessions, 0).await.function_call(
        "handoff",
        "hand_off_call",
        serde_json::json!({"agent":target}),
    );
    let peer = model_peer(&sessions, 1).await;
    let config = peer.wait_for("session.update").await;
    assert_eq!(config["session"]["audio"]["output"]["voice"], "nova");
    let instructions = config["session"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Wren") && instructions.contains("Arrange clinic appointments."),
        "{config}"
    );
    assert!(!matches!(party.state(), PartyState::Ended(_)));
    let (status, call) = desk.get(&format!("/api/v1/calls/{call_id}")).await;
    assert_eq!(status, 200, "{call}");
    assert_eq!(call["agent_id"], target, "{call}");
    let (status, runs) = desk.get("/api/v1/runs").await;
    assert_eq!(status, 200, "{runs}");
    let run = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["id"] == call["run_id"])
        .unwrap_or_else(|| panic!("the Call Run is listed: {runs}"));
    assert_eq!(run["agent_id"], target, "{run}");
    party.hangup();
    next_frame_of(&mut socket, "call.ended").await;
}

#[tokio::test]
async fn a_hand_off_refuses_another_workspace_and_an_archived_agent() {
    let (desk, sessions, _, party, call_id) = confirmed_call().await;
    let client = reqwest::Client::new();
    let response = client.post(format!("{}/api/v1/administration/people", desk.daemon.administration_base_url))
        .header("cookie", desk.daemon.cookie())
        .json(&serde_json::json!({"email":"other@example.com", "name":"Other", "password":"a-long-test-password"}))
        .send().await.unwrap();
    assert_eq!(response.status().as_u16(), 201);
    let person: serde_json::Value = response.json().await.unwrap();
    let person_id = pagis_core::UserId::from(person["person"]["id"].as_str().unwrap().to_string());
    let cookie = desk.daemon.cookie_for(&person_id).await;
    let agents: serde_json::Value = client
        .get(format!("{}/api/v1/agents", desk.daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other = &agents["items"][0]["id"];
    let peer = model_peer(&sessions, 0).await;
    peer.function_call(
        "foreign",
        "hand_off_call",
        serde_json::json!({"agent":other}),
    );
    let refused = peer.wait_for("conversation.item.create").await;
    assert!(
        refused
            .to_string()
            .contains("active Agent of this Workspace"),
        "{refused}"
    );
    let (status, agent) = desk
        .post(
            "/api/v1/agents",
            serde_json::json!({"name":"Wren", "job":"clinic", "personality":"calm"}),
        )
        .await;
    assert_eq!(status, 201, "{agent}");
    let target = agent["id"].as_str().unwrap();
    let (status, body) = desk
        .post(
            &format!("/api/v1/agents/{target}/archive"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    peer.function_call(
        "archived",
        "hand_off_call",
        serde_json::json!({"agent":target}),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while peer.sent_of("conversation.item.create").len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sessions.peers().len(), 1);
    let (_, call) = desk.get(&format!("/api/v1/calls/{call_id}")).await;
    assert_eq!(call["agent_id"], desk.daemon.agent_id);
    party.hangup();
}

#[tokio::test]
async fn a_workspace_with_no_chief_refuses_the_call_before_answer() {
    let desk = Desk::start().await;
    let (_, agents) = desk.get("/api/v1/agents").await;
    for agent in agents["items"].as_array().unwrap() {
        let (status, body) = desk
            .post(
                &format!("/api/v1/agents/{}/archive", agent["id"].as_str().unwrap()),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    let party = desk.transport.ring(OWN, REMOTE);
    tokio::time::timeout(Duration::from_secs(5), async {
        while matches!(party.state(), PartyState::Calling | PartyState::Ringing) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(party.state(), PartyState::Refused(_)));
    assert!(desk.bridge.answered().is_empty());
}

async fn callback(age: i64, archived: bool) {
    use pagis_core::{CallStore, PhoneNumberStore};
    let desk = Desk::start().await;
    let (status, agent) = desk
        .post(
            "/api/v1/agents",
            serde_json::json!({"name":"Wren","job":"Calls","personality":"calm"}),
        )
        .await;
    assert_eq!(status, 201, "{agent}");
    let target = agent["id"].as_str().unwrap();
    let pool = desk.daemon.pool();
    let numbers = pagis_storage_sqlite::SqlitePhoneNumberStore::new(pool.clone());
    let number = numbers
        .list(&desk.daemon.workspace_id)
        .await
        .unwrap()
        .remove(0);
    // An inbound Call supplies a stored Run for the outbound record.
    let mut socket = desk.firehose().await;
    desk.transport.ring(OWN, REMOTE);
    let ended = next_frame_of(&mut socket, "call.ended").await;
    let run_id = ended["payload"]["run_id"].as_str().unwrap();
    let mut outbound = pagis_testkit::fixture::missed_call(
        &desk.daemon.workspace_id,
        &pagis_core::AgentId::from(target.to_string()),
        &pagis_core::RunId::from(run_id.to_string()),
    );
    outbound.direction = CallDirection::Outbound;
    outbound.phone_number_id = number.id;
    outbound.remote_e164 = REMOTE.into();
    outbound.created_at = pagis_core::now_ms() - age;
    pagis_storage_sqlite::SqliteCallStore::new(pool.clone())
        .insert(&outbound)
        .await
        .unwrap();
    if archived {
        let (status, body) = desk
            .post(
                &format!("/api/v1/agents/{target}/archive"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    desk.transport.ring(OWN, REMOTE);
    let placed = next_frame_of(&mut socket, "call.placed").await;
    let expected = if archived || age > 7 * 24 * 60 * 60 * 1000 {
        desk.daemon.agent_id.as_str()
    } else {
        target
    };
    assert_eq!(placed["payload"]["agent_id"], expected);
    next_frame_of(&mut socket, "call.ended").await;
    assert_eq!(
        desk.bridge.answered().last().unwrap().brief.tier,
        pagis_core::TrustTier::Unknown
    );
}
#[tokio::test]
async fn callback_inside_window_reaches_calling_agent() {
    callback(1000, false).await;
}
#[tokio::test]
async fn callback_after_window_reaches_chief_of_staff() {
    callback(8 * 24 * 60 * 60 * 1000, false).await;
}
#[tokio::test]
async fn callback_after_agent_archive_reaches_chief_of_staff() {
    callback(1000, true).await;
}

#[tokio::test]
async fn grants_reject_workspace_number_of_another_workspace() {
    let pair = pagis_testkit::tenancy::TwoTenants::start().await;
    let client = reqwest::Client::new();
    let agent: serde_json::Value = client
        .post(format!("{}/api/v1/agents", pair.daemon.base_url))
        .header("cookie", &pair.b.cookie)
        .json(&serde_json::json!({"name":"Wren", "job":"Calls", "personality":"calm"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let number = pagis_core::PhoneNumberStore::list(
        &pagis_storage_sqlite::SqlitePhoneNumberStore::new(pair.daemon.pool().clone()),
        &pair.a.workspace_id,
    )
    .await
    .unwrap()
    .into_iter()
    .find(|number| number.status == pagis_core::PhoneNumberStatus::Active)
    .unwrap();
    let response = client
        .post(format!(
            "{}/api/v1/grants/workspace-number",
            pair.daemon.base_url
        ))
        .header("cookie", &pair.b.cookie)
        .json(&serde_json::json!({"agent_id":agent["id"],"number_id":number.id}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn grants_create_list_and_revoke_workspace_number_access() {
    let desk = Desk::start().await;
    let (_, page) = desk.get("/api/v1/workspace/number").await;
    let (status, grant) = desk
        .post(
            "/api/v1/grants/workspace-number",
            serde_json::json!({
                "agent_id":desk.daemon.agent_id,"number_id":page["number"]["id"],
            }),
        )
        .await;
    assert_eq!(status, 201, "{grant}");
    assert_eq!(grant["resource_kind"], "workspace_number");
    assert_eq!(grant["resource_id"], page["number"]["id"]);
    let (_, grants) = desk.get("/api/v1/grants").await;
    assert!(
        grants["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == grant["id"])
    );
    let (status, _) = desk
        .put(
            &format!("/api/v1/grants/{}/rules", grant["id"].as_str().unwrap()),
            serde_json::json!({"allow":["all"]}),
        )
        .await;
    assert_eq!(status, 422);
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/grants/{}",
            desk.daemon.base_url,
            grant["id"].as_str().unwrap()
        ))
        .header("cookie", desk.daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 204);
    let (_, grants) = desk.get("/api/v1/grants").await;
    assert!(
        !grants["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == grant["id"])
    );
}

#[tokio::test]
async fn granted_phone_call_dials_workspace_number_and_records_agent_and_run() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "phone_call",
        serde_json::json!({"to":REMOTE,"purpose":"Book a table"}),
    ));
    brain.push(Script::reply(&["Done."]));
    let sessions = Arc::new(pagis_telephony::model_fake::FakeModelSessions::new());
    let desk = Desk::start_with(TestDaemonOptions {
        brain,
        model_sessions: Some(sessions.clone()),
        keys: pagis_testkit::test_provider_keys(vec![("OPENAI_API_KEY", "sk-test")]),
        ..TestDaemonOptions::default()
    })
    .await;
    let (_, page) = desk.get("/api/v1/workspace/number").await;
    let (status, grant) = desk
        .post(
            "/api/v1/grants/workspace-number",
            serde_json::json!({"agent_id":desk.daemon.agent_id,"number_id":page["number"]["id"]}),
        )
        .await;
    assert_eq!(status, 201, "{grant}");
    let mut socket = desk.firehose().await;
    let (status, message) = desk
        .post(
            &format!("/api/v1/channels/{}/messages", desk.daemon.dm_channel_id),
            serde_json::json!({"pending_id":"call-table","text":"Book a table"}),
        )
        .await;
    assert_eq!(status, 201, "{message}");
    let requested = next_frame_of(&mut socket, "request.created").await;
    let request = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap();
    let (status, body) = desk
        .post(
            &format!("/api/v1/requests/{request}/decision"),
            serde_json::json!({"decision":"approved"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let placed = next_frame_of(&mut socket, "call.placed").await;
    let call_id = placed["payload"]["payload"]["call_id"].as_str().unwrap();
    let run_id = placed["payload"]["run_id"].as_str().unwrap();
    let party = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(party) = desk.transport.dials().first() {
                break party.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the carrier receives the Call");
    assert!(desk.transport.calls().iter().any(|call| matches!(call, pagis_telephony::fake::LineCall::Dial { from, to } if from == OWN && to == REMOTE)));
    party.answer();
    let peer = model_peer(&sessions, 0).await;
    peer.wait_for("session.update").await;
    party.speak(pagis_telephony::audio::Frame::silence(
        pagis_telephony::audio::Codec::Pcmu,
    ));
    peer.wait_for("input_audio_buffer.append").await;
    peer.function_call(
        "classify",
        "report_answer",
        serde_json::json!({"category":"human"}),
    );
    party.hangup();
    next_frame_of(&mut socket, "call.ended").await;
    let (status, call) = desk.get(&format!("/api/v1/calls/{call_id}")).await;
    assert_eq!(status, 200, "{call}");
    assert_eq!(call["agent_id"], desk.daemon.agent_id);
    assert_eq!(call["run_id"], run_id);
    assert_eq!(call["direction"], "outbound");
    assert_eq!(call["own_e164"], OWN);
    assert_eq!(call["remote_e164"], REMOTE);
}
