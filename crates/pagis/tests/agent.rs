//! Full-daemon agent loop tests: a DM message triggers Pixie,
//! deltas stream over the subscribed WebSocket with mid-stream
//! catch-up, cancel stops generation, and a restart fails the
//! unfinished run.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
}

async fn send_json(socket: &mut Socket, frame: serde_json::Value) {
    socket
        .send(Message::text(frame.to_string()))
        .await
        .expect("ws send");
}

/// The next text frame as JSON, within a timeout.
async fn next_frame(socket: &mut Socket) -> serde_json::Value {
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

/// The next frame of one type, skipping others.
async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// Authenticate, consume `ready`, and subscribe to the DM channel.
async fn subscribed_socket(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    send_json(&mut socket, serde_json::json!({ "type": "auth" })).await;
    let ready = next_frame(&mut socket).await;
    assert_eq!(ready["type"], "ready");
    send_json(
        &mut socket,
        serde_json::json!({ "type": "subscribe", "channel_id": daemon.dm_channel_id }),
    )
    .await;
    socket
}

async fn rest_send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

async fn timeline(daemon: &TestDaemon) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_dm_message_streams_the_reply_over_the_subscribed_socket() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Hel", "lo!"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi Pixie").await;

    // The two deltas, in order, with the seq the client dedups on.
    let first = next_frame_of(&mut socket, "message.delta").await;
    assert_eq!(first["payload"]["text"], "Hel");
    assert_eq!(first["payload"]["seq"], 1);
    assert_eq!(first["payload"]["catch_up"], false);
    assert_eq!(first["payload"]["channel_id"], daemon.dm_channel_id);
    assert!(first["seq"].is_null(), "deltas are ephemeral: no seq");
    let second = next_frame_of(&mut socket, "message.delta").await;
    assert_eq!(second["payload"]["text"], "lo!");
    assert_eq!(second["payload"]["seq"], 2);
    assert_eq!(
        second["payload"]["message_id"],
        first["payload"]["message_id"]
    );

    // The durable completion event follows on the firehose.
    let completed = next_frame_of(&mut socket, "message.completed").await;
    assert_eq!(
        completed["payload"]["payload"]["author_kind"], "agent",
        "the agent's completion"
    );
    assert_eq!(
        completed["payload"]["payload"]["message_id"],
        first["payload"]["message_id"]
    );
    assert_eq!(completed["payload"]["run_id"], first["payload"]["run_id"]);

    // The timeline holds the settled reply, the run's progress row,
    // the trigger, and the greeting, newest first.
    let items = timeline(&daemon).await["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0]["author_kind"], "agent");
    assert_eq!(items[0]["status"], "complete");
    assert_eq!(items[0]["text_content"], "Hello!");
    assert_eq!(items[0]["run_id"], first["payload"]["run_id"]);
    assert_eq!(items[1]["blocks"][0]["type"], "progress");
    assert_eq!(items[2]["author_kind"], "user");
    assert_eq!(items[3]["author_kind"], "agent");
    assert!(items[3]["run_id"].is_null());
}

#[tokio::test]
async fn an_unsubscribed_socket_receives_no_deltas() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Quiet"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;

    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    send_json(&mut socket, serde_json::json!({ "type": "auth" })).await;
    assert_eq!(next_frame(&mut socket).await["type"], "ready");

    rest_send(&daemon, "p1", "hi").await;

    // The firehose still delivers durable events, but no delta frame
    // arrives before the agent's completion.
    loop {
        let frame = next_frame(&mut socket).await;
        assert_ne!(frame["type"], "message.delta");
        if frame["type"] == "message.completed"
            && frame["payload"]["payload"]["author_kind"] == "agent"
        {
            break;
        }
    }
}

#[tokio::test]
async fn a_mid_stream_subscribe_catches_up_with_one_delta() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Hello ", "world"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut early = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi").await;

    // Wait until both deltas streamed to the early subscriber.
    let mut last = next_frame_of(&mut early, "message.delta").await;
    if last["payload"]["seq"] != 2 {
        last = next_frame_of(&mut early, "message.delta").await;
    }
    assert_eq!(last["payload"]["seq"], 2);

    // A fresh subscriber reads the accumulated partial as one frame.
    let mut late = subscribed_socket(&daemon).await;
    let catch_up = next_frame_of(&mut late, "message.delta").await;
    assert_eq!(catch_up["payload"]["catch_up"], true);
    assert_eq!(catch_up["payload"]["text"], "Hello world");
    assert_eq!(catch_up["payload"]["seq"], 2);
    assert_eq!(
        catch_up["payload"]["message_id"],
        last["payload"]["message_id"]
    );
}

#[tokio::test]
async fn cancel_stops_the_stream_and_persists_the_partial_as_failed() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Partial answer"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi").await;
    let delta = next_frame_of(&mut socket, "message.delta").await;
    let run_id = delta["payload"]["run_id"].as_str().unwrap().to_string();

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/api/v1/runs/{run_id}/cancel", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["outcome"], "canceling");

    // The partial persists as failed and the run ends canceled.
    let failed = next_frame_of(&mut socket, "message.failed").await;
    assert_eq!(failed["payload"]["run_id"], run_id);
    let state_changed = next_frame_of(&mut socket, "run.state_changed").await;
    assert_eq!(state_changed["payload"]["payload"]["to"], "canceled");

    let items = timeline(&daemon).await["items"].as_array().unwrap().clone();
    assert_eq!(items[0]["author_kind"], "agent");
    assert_eq!(items[0]["status"], "failed");
    assert_eq!(items[0]["text_content"], "Partial answer");

    // Canceling again reports the terminal state; unknown runs are 404.
    let repeat: serde_json::Value = client
        .post(format!("{}/api/v1/runs/{run_id}/cancel", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repeat["outcome"], "already_ended");
    assert_eq!(repeat["state"], "canceled");

    // The work record reports who stopped the run and after how long.
    let steps = run_steps(&daemon, &run_id).await;
    assert_eq!(steps["state"], "canceled");
    assert_eq!(steps["stopped"]["by"], "user");
    assert!(steps["stopped"]["after_ms"].as_i64().unwrap() >= 0);
    assert!(steps["failure"].is_null());
    let unknown = client
        .post(format!(
            "{}/api/v1/runs/01UNKNOWNRUN/cancel",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

async fn run_steps(daemon: &TestDaemon, run_id: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/runs/{run_id}/steps", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET steps");
    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("steps JSON");
    assert_eq!(status, 200, "steps failed: {body}");
    body
}

#[tokio::test]
async fn the_steps_of_a_completed_run_fold_its_tool_calls() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "memory_read",
        serde_json::json!({"path": "shared/MEMORY.md"}),
    ));
    brain.push(Script::reply(&["Nothing saved."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "Do I have any travel coming up?").await;
    let run_id = loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break frame["payload"]["run_id"].as_str().unwrap().to_string();
        }
    };

    let steps = run_steps(&daemon, &run_id).await;
    assert_eq!(steps["state"], "completed");
    assert!(steps["worked_ms"].as_i64().unwrap() >= 0);
    assert!(steps["stopped"].is_null());
    assert!(steps["failure"].is_null());
    let items = steps["steps"].as_array().unwrap();
    assert_eq!(items.len(), 1, "steps: {items:?}");
    assert_eq!(items[0]["index"], 1);
    assert_eq!(items[0]["label"], "Read from memory");
    assert_eq!(items[0]["kind"], "memory");
    assert!(items[0]["ended_at"].as_i64().is_some());
    assert!(items[0]["duration_ms"].as_i64().unwrap() >= 0);
    assert_eq!(items[0]["live"], false);
    assert!(items[0]["screenshot_id"].is_null());

    let unknown = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/runs/01UNKNOWNRUN/steps",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test]
async fn a_restart_fails_the_unfinished_run_and_its_partial() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Half a rep"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi").await;
    let delta = next_frame_of(&mut socket, "message.delta").await;
    let run_id = delta["payload"]["run_id"].as_str().unwrap().to_string();
    drop(socket);

    let daemon = daemon.restart(TestDaemonOptions::default()).await;

    use pagis_core::{RunId, RunState, RunStore};
    let run = pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &RunId::from(run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, RunState::Failed);
    assert_eq!(run.error.as_deref(), Some(pagis_agent::RESTART_ERROR));

    let items = timeline(&daemon).await["items"].as_array().unwrap().clone();
    assert_eq!(items[0]["author_kind"], "agent");
    assert_eq!(items[0]["status"], "failed");
}
