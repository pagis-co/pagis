//! Inbound calls reach the bridge (ADR-0022): a call that rings
//! the Agent's desk line is answered, runs under the Agent that holds
//! the number in a Run the daemon opens in its Thread with the user,
//! and settles there as a `call` block. The bridge is the scripted
//! fake, so no model session opens.

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
            call_bridge: Some(Arc::clone(&bridge) as _),
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
                "/api/v1/settings/phone-numbers",
                serde_json::json!({ "e164": OWN, "agent_id": desk.daemon.agent_id }),
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
    assert!(ended["payload"]["payload"]["transcript_artifact_id"].is_string());

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
    assert_eq!(strip["author_kind"], "system");

    // The caller heard the end, and the line is free again.
    assert!(matches!(party.state(), PartyState::Ended(_)));
}

#[tokio::test]
async fn a_desk_line_carries_a_standing_call_rule() {
    let desk = Desk::start().await;

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
    assert_eq!(rule["creator"], "user");
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
