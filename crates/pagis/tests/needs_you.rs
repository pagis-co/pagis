//! The Needs-You Queue (ADR-0022, ADR-0030): the daemon derives it on
//! each read, and the Person dismisses a failed Run or a missed Call, and
//! the record keeps the time, so the item stays out of the queue after a
//! reload.
//!
//! The daemon also publishes `needs_you.added` when an item enters the
//! queue and `needs_you.removed` when it leaves, and the event socket
//! carries them to each client of the Person.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use pagis::keypad::VaultCodeCheck;
use pagis_core::{
    AgentId, CallStore, ChannelId, MemorySecretStore, RequestStore, RunId, RunState, RunStore,
    SecretStore, SystemClock, TrustTier, WorkspaceId,
};
use pagis_storage_sqlite::{SqliteCallStore, SqliteRequestStore, SqliteRunStore};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberCatalog, TokioClock};
use pagis_telephony::{
    BridgeError, CallBridge, CallLog, CallReport, FakeCallBridge, IncomingHub, Keypad, PlacedCall,
    TierGate,
};
use pagis_testkit::fixture::{missed_call, pending_request, queued_run};
use pagis_testkit::{
    HostAnswer, HostClient, Script, ScriptedBrain, Socket, TestDaemon, TestDaemonOptions,
    TwoTenants,
};
use tokio_tungstenite::tungstenite::Message;

async fn post(daemon: &TestDaemon, path: &str) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .status()
}

async fn first_item(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    let page = reqwest::Client::new()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    page["items"][0].clone()
}

#[tokio::test]
async fn a_dismissed_failure_and_missed_call_keep_the_time_on_their_records() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let mut run = queued_run(
        &daemon.workspace_id,
        &agent_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    run.state = RunState::Failed;
    SqliteRunStore::new(daemon.pool().clone())
        .create(&run)
        .await
        .expect("write the failed Run");
    let call = missed_call(&daemon.workspace_id, &agent_id, &run.id);
    SqliteCallStore::new(daemon.pool().clone())
        .insert(&call)
        .await
        .expect("write the missed Call");

    assert_eq!(
        first_item(&daemon, "/api/v1/runs?state=failed").await["dismissed_at"],
        serde_json::Value::Null
    );
    assert_eq!(
        post(
            &daemon,
            &format!("/api/v1/runs/{}/dismiss", run.id.as_str())
        )
        .await,
        204
    );
    assert_eq!(
        post(
            &daemon,
            &format!("/api/v1/calls/{}/dismiss", call.id.as_str())
        )
        .await,
        204
    );

    assert!(first_item(&daemon, "/api/v1/runs?state=failed").await["dismissed_at"].is_i64());
    assert!(first_item(&daemon, "/api/v1/calls?direction=inbound").await["dismissed_at"].is_i64());
}

#[tokio::test]
async fn a_dismissal_of_no_record_is_not_found() {
    let daemon = TestDaemon::start().await;

    assert_eq!(post(&daemon, "/api/v1/runs/no-such-run/dismiss").await, 404);
    assert_eq!(
        post(&daemon, "/api/v1/calls/no-such-call/dismiss").await,
        404
    );
}

/// The Needs-You Queue that `cookie` reads.
async fn needs_you(base_url: &str, cookie: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("{base_url}/api/v1/needs-you"))
        .header("cookie", cookie)
        .send()
        .await
        .expect("read the Needs-You Queue");
    assert_eq!(response.status(), 200);
    response.json().await.expect("the queue")
}

/// Each item of a queue as `<kind> <id>`.
fn rows(queue: &serde_json::Value) -> Vec<String> {
    queue["items"]
        .as_array()
        .expect("the items")
        .iter()
        .map(|item| format!("{} {}", item["kind"], item["id"]).replace('"', ""))
        .collect()
}

#[tokio::test]
async fn the_queue_holds_a_pending_request_and_a_failed_run() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    let runs = SqliteRunStore::new(daemon.pool().clone());
    let parked = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
    runs.create(&parked).await.expect("write the parked Run");
    let request = pending_request(&daemon.workspace_id, &agent_id, &parked.id, "echo hi");
    SqliteRequestStore::new(daemon.pool().clone())
        .create(&request)
        .await
        .expect("write the pending Request");
    let mut failed = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
    failed.state = RunState::Failed;
    runs.create(&failed).await.expect("write the failed Run");

    let queue = needs_you(&daemon.base_url, daemon.cookie()).await;

    assert_eq!(queue["count"], 2);
    assert_eq!(
        rows(&queue),
        [
            format!("approval request:{}", request.id),
            format!("failed run:{}", failed.id),
        ]
    );
    assert_eq!(
        queue["items"][0]["url"],
        format!("/c/{}", daemon.dm_channel_id)
    );
    assert_eq!(queue["items"][1]["url"], format!("/runs/{}", failed.id));
}

/// The reader reads the failed Runs of today page by page, so a day
/// with more failures than one page holds them all.
#[tokio::test]
async fn the_queue_holds_every_failure_of_a_busy_day() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    let runs = SqliteRunStore::new(daemon.pool().clone());
    for _ in 0..101 {
        let mut failed = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
        failed.state = RunState::Failed;
        runs.create(&failed).await.expect("write a failed Run");
    }

    let queue = needs_you(&daemon.base_url, daemon.cookie()).await;

    assert_eq!(queue["count"], 101);
}

#[tokio::test]
async fn person_b_reads_nothing_of_person_as_queue() {
    let world = TwoTenants::start().await;
    let a_request = format!("approval request:{}", world.a_id("request_id"));

    let a_queue = needs_you(&world.daemon.base_url, &world.a.cookie).await;
    let b_queue = needs_you(&world.daemon.base_url, &world.b.cookie).await;

    assert!(rows(&a_queue).contains(&a_request), "A reads {a_queue}");
    assert_eq!(b_queue["items"], serde_json::json!([]));
    assert_eq!(b_queue["count"], 0);
}

/// The next `needs_you.*` frame of the event socket. Every other frame
/// is passed over.
async fn next_needs_you(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"]
            .as_str()
            .is_some_and(|frame_type| frame_type.starts_with("needs_you."))
        {
            return frame;
        }
    }
}

/// The next `needs_you.*` frame about the item `item_id`. A frame about
/// another item is passed over.
async fn next_needs_you_of(socket: &mut Socket, item_id: &str) -> serde_json::Value {
    loop {
        let frame = next_needs_you(socket).await;
        if needs_you_item_id(&frame) == item_id {
            return frame;
        }
    }
}

/// The id of the item that a `needs_you.*` frame names.
fn needs_you_item_id(frame: &serde_json::Value) -> String {
    let payload = &frame["payload"]["payload"];
    payload["item"]["id"]
        .as_str()
        .or_else(|| payload["item_id"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The next text frame of the event socket, as JSON.
async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("the frame is JSON");
        }
    }
}

/// Send one message from the Person into the DM, which starts a Run.
async fn send_to_the_agent(daemon: &TestDaemon, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "p-1", "text": text }))
        .send()
        .await
        .expect("send the message");
    assert_eq!(response.status(), 201);
}

/// Clear the failed keypad codes of the Workspace of `cookie`, as
/// Settings does.
async fn clear_keypad_failures(base_url: &str, cookie: &str) {
    let response = reqwest::Client::new()
        .delete(format!("{base_url}/api/v1/settings/keypad-code/failures"))
        .header("cookie", cookie)
        .send()
        .await
        .expect("clear the failed keypad codes");
    assert_eq!(response.status(), 204);
}

/// Write a failed Run of today in one Workspace, with no event, as a
/// record that the queue reads.
async fn plant_failed_run(daemon: &TestDaemon, workspace_id: &WorkspaceId) -> RunId {
    let mut run = queued_run(
        workspace_id,
        &AgentId::from(daemon.agent_id.clone()),
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    run.state = RunState::Failed;
    daemon
        .stores()
        .runs
        .create(&run)
        .await
        .expect("write the failed Run");
    run.id
}

#[tokio::test]
async fn a_request_enters_the_queue_and_its_decision_takes_it_out() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "echo hi" }),
    ));
    brain.push(Script::reply(&["It printed."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let _host = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::Echo).await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;

    send_to_the_agent(&daemon, "run echo for me").await;

    let added = next_needs_you(&mut socket).await;
    assert_eq!(added["type"], "needs_you.added", "{added}");
    let payload = &added["payload"]["payload"];
    assert_eq!(payload["count"], 1, "{added}");
    assert_eq!(payload["item"]["kind"], "approval", "{added}");
    let request_id = payload["item"]["request_id"]
        .as_str()
        .expect("the approval names its Request")
        .to_string();
    assert_eq!(payload["item"]["id"], format!("request:{request_id}"));

    let decided = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "decision": "approved" }))
        .send()
        .await
        .expect("decide the Request");
    assert_eq!(decided.status(), 200);

    let removed = next_needs_you(&mut socket).await;
    assert_eq!(removed["type"], "needs_you.removed", "{removed}");
    assert_eq!(
        removed["payload"]["payload"],
        serde_json::json!({ "item_id": format!("request:{request_id}"), "count": 0 })
    );
}

#[tokio::test]
async fn a_failed_run_enters_the_queue_and_its_dismissal_takes_it_out() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::fail_after(&[], "the model is down"));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;

    send_to_the_agent(&daemon, "do the work").await;

    let added = next_needs_you(&mut socket).await;
    assert_eq!(added["type"], "needs_you.added", "{added}");
    let payload = &added["payload"]["payload"];
    assert_eq!(payload["item"]["kind"], "failed", "{added}");
    assert_eq!(payload["count"], 1, "{added}");
    let run_id = payload["item"]["run_id"]
        .as_str()
        .expect("the failure names its Run")
        .to_string();

    assert_eq!(
        post(&daemon, &format!("/api/v1/runs/{run_id}/dismiss")).await,
        204
    );

    let removed = next_needs_you(&mut socket).await;
    assert_eq!(removed["type"], "needs_you.removed", "{removed}");
    assert_eq!(
        removed["payload"]["payload"],
        serde_json::json!({ "item_id": format!("run:{run_id}"), "count": 0 })
    );
}

/// A bridge that answers each inbound Call. During the Call a caller
/// types a wrong Keypad Code six times, each time on a gate of its own,
/// and the sixth wrong code starts a delay (ADR-0021).
struct WrongCodeBridge {
    keypad: OnceLock<Keypad>,
    inner: FakeCallBridge,
}

#[async_trait]
impl CallBridge for WrongCodeBridge {
    async fn place(&self, call: PlacedCall, log: &CallLog) -> Result<CallReport, BridgeError> {
        self.inner.place(call, log).await
    }

    async fn answer(
        &self,
        incoming: IncomingHub,
        call: PlacedCall,
        log: &CallLog,
    ) -> Result<CallReport, BridgeError> {
        let keypad = self.keypad.get().expect("the test gives the keypad");
        for _ in 0..6 {
            let gate = TierGate::inbound(
                TrustTier::Owner,
                call.workspace_id.clone(),
                keypad.clone(),
                log.clone(),
            );
            for digit in "975319#".chars() {
                assert_eq!(gate.late_digit(digit).await, None, "a wrong code");
            }
        }
        self.inner.answer(incoming, call, log).await
    }
}

const OWN: &str = "+14155550123";
const REMOTE: &str = "+16505550100";

#[tokio::test]
async fn a_keypad_delay_enters_the_queue_and_the_settings_clear_takes_it_out() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let bridge = Arc::new(WrongCodeBridge {
        keypad: OnceLock::new(),
        inner: FakeCallBridge::reporting(FakeCallBridge::answered_report("Hello.")),
    });
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        secrets: Arc::clone(&secrets),
        call_transport: Arc::clone(&transport) as _,
        call_bridge: Some(Arc::clone(&bridge) as _),
        number_catalog: Arc::new(FakeNumberCatalog::offering(&[OWN])),
        ..TestDaemonOptions::default()
    })
    .await;
    assert!(
        bridge
            .keypad
            .set(Keypad {
                code: Arc::new(VaultCodeCheck::new(Arc::clone(&secrets))),
                failures: daemon.stores().keypad_failures.clone(),
                clock: Arc::new(SystemClock),
            })
            .is_ok()
    );
    let client = reqwest::Client::new();
    let set = client
        .put(format!("{}/api/v1/settings/keypad-code", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "code": "246813" }))
        .send()
        .await
        .expect("set the Keypad Code");
    assert_eq!(set.status(), 200);
    daemon
        .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
        .await;
    let (status, body) = daemon
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
    let number = client
        .post(format!("{}/api/v1/settings/phone-numbers", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "e164": OWN, "agent_id": daemon.agent_id }))
        .send()
        .await
        .expect("buy the number");
    assert_eq!(number.status(), 201);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !transport.is_registered("robin") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the line registers");
    let mut socket = daemon.event_socket(daemon.cookie()).await;

    transport.ring(OWN, REMOTE);

    // The answered Call can wake the Agent afterwards, so the socket
    // can also carry an item of another kind.
    let added = next_needs_you_of(&mut socket, "keypad").await;
    assert_eq!(added["type"], "needs_you.added", "{added}");
    let item = &added["payload"]["payload"]["item"];
    assert_eq!(item["kind"], "keypad", "{added}");
    assert_eq!(item["failed_attempts"], 6, "{added}");

    clear_keypad_failures(&daemon.base_url, daemon.cookie()).await;

    let removed = next_needs_you_of(&mut socket, "keypad").await;
    assert_eq!(removed["type"], "needs_you.removed", "{removed}");
}

#[tokio::test]
async fn an_event_that_does_not_change_the_queue_publishes_nothing() {
    let daemon = TestDaemon::start().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    // The Run is written with no event, so the next event of the
    // Workspace finds it.
    let run_id = plant_failed_run(&daemon, &daemon.workspace_id).await;
    clear_keypad_failures(&daemon.base_url, daemon.cookie()).await;
    let added = next_needs_you(&mut socket).await;
    assert_eq!(added["type"], "needs_you.added", "{added}");
    assert_eq!(needs_you_item_id(&added), format!("run:{run_id}"));

    // The count is clear already, so the queue stays the same.
    clear_keypad_failures(&daemon.base_url, daemon.cookie()).await;
    assert_eq!(
        post(&daemon, &format!("/api/v1/runs/{run_id}/dismiss")).await,
        204
    );

    let next = next_needs_you(&mut socket).await;
    assert_eq!(next["type"], "needs_you.removed", "{next}");
    assert_eq!(needs_you_item_id(&next), format!("run:{run_id}"));
}

#[tokio::test]
async fn a_restart_publishes_nothing_for_an_item_that_was_there_before() {
    let daemon = TestDaemon::start().await;
    let run_id = plant_failed_run(&daemon, &daemon.workspace_id).await;
    let daemon = daemon.restart(TestDaemonOptions::default()).await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;

    clear_keypad_failures(&daemon.base_url, daemon.cookie()).await;
    assert_eq!(
        post(&daemon, &format!("/api/v1/runs/{run_id}/dismiss")).await,
        204
    );

    let first = next_needs_you(&mut socket).await;
    assert_eq!(first["type"], "needs_you.removed", "{first}");
    assert_eq!(
        first["payload"]["payload"],
        serde_json::json!({ "item_id": format!("run:{run_id}"), "count": 0 })
    );
}

#[tokio::test]
async fn an_event_in_one_workspace_publishes_nothing_to_the_other() {
    let world = TwoTenants::start().await;
    let daemon = &world.daemon;
    let mut a_socket = daemon.event_socket(&world.a.cookie).await;
    let mut b_socket = daemon.event_socket(&world.b.cookie).await;

    let a_run = plant_failed_run(daemon, &world.a.workspace_id).await;
    clear_keypad_failures(&daemon.base_url, &world.a.cookie).await;
    let a_added = next_needs_you_of(&mut a_socket, &format!("run:{a_run}")).await;
    assert_eq!(a_added["type"], "needs_you.added", "{a_added}");

    let b_run = plant_failed_run(daemon, &world.b.workspace_id).await;
    clear_keypad_failures(&daemon.base_url, &world.b.cookie).await;
    let b_first = next_needs_you(&mut b_socket).await;
    assert_eq!(b_first["type"], "needs_you.added", "{b_first}");
    assert_eq!(needs_you_item_id(&b_first), format!("run:{b_run}"));
    assert_eq!(b_first["payload"]["payload"]["count"], 1, "{b_first}");
}
