//! Daemon-derived progress blocks end to end (ADR-0004): the
//! line rides ephemeral WS frames while the run works, a subscriber
//! that arrives mid-run reads the current state, and only the terminal
//! text persists in the run's `progress` block.

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

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// The next progress frame whose line is `text`, within the timeout.
async fn next_progress(socket: &mut Socket, text: &str) -> serde_json::Value {
    loop {
        let frame = next_frame_of(socket, "progress.state").await;
        assert!(
            frame["seq"].is_null(),
            "progress is ephemeral: it never carries an event id"
        );
        if frame["payload"]["text"] == text {
            return frame;
        }
    }
}

async fn subscribed_socket(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    send_json(&mut socket, serde_json::json!({ "type": "auth" })).await;
    assert_eq!(next_frame(&mut socket).await["type"], "ready");
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

async fn timeline(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let body: serde_json::Value = reqwest::Client::new()
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
        .unwrap();
    body["items"].as_array().unwrap().clone()
}

/// The one message whose blocks are the run's progress line.
fn progress_row(items: &[serde_json::Value]) -> serde_json::Value {
    items
        .iter()
        .find(|m| m["blocks"][0]["type"] == "progress")
        .expect("the run has a progress row")
        .clone()
}

/// The settled progress row. The terminal frame leaves the hub before
/// the row reaches the store, so the read follows it.
async fn settled_progress_row(daemon: &TestDaemon) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let row = progress_row(&timeline(daemon).await);
        if row["status"] == "complete" {
            return row;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the progress row did not settle: {row}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_run_streams_its_progress_and_keeps_only_the_terminal_line() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "shared/MEMORY.md",
            "content": "- nothing yet\n",
            "expected_memory_revision": "missing",
        }),
    ));
    brain.push(Script::reply(&["Filed."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "file this").await;

    // The daemon composes the line from the run state and the tool
    // call in flight, and every tick is ephemeral.
    let thinking = next_progress(&mut socket, "Thinking…").await;
    let run_id = thinking["payload"]["run_id"].as_str().unwrap().to_string();
    assert_eq!(thinking["payload"]["channel_id"], daemon.dm_channel_id);
    let working = next_progress(&mut socket, "Running `memory_write`…").await;
    assert_eq!(working["payload"]["run_id"], run_id.as_str());
    assert!(
        working["payload"]["seq"].as_u64().unwrap() > thinking["payload"]["seq"].as_u64().unwrap(),
        "seq orders the ticks"
    );
    let done = next_progress(&mut socket, "Done").await;
    assert_eq!(done["payload"]["run_id"], run_id.as_str());

    // The row settles with the terminal line and nothing else: the
    // finished run shows its last state, not a replay of the ticks.
    let row = settled_progress_row(&daemon).await;
    let items = timeline(&daemon).await;
    assert_eq!(row["id"], done["payload"]["message_id"]);
    assert_eq!(row["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(row["blocks"][0]["run_id"], run_id.as_str());
    assert_eq!(row["blocks"][0]["text"], "Done");
    assert_eq!(row["text_content"], "Done");
    assert_eq!(
        items
            .iter()
            .filter(|m| m["blocks"][0]["type"] == "progress")
            .count(),
        1,
        "one row per run, whatever the tick count"
    );

    // No tick became an `events` row: the run's event log holds its
    // state changes and tool calls only.
    let events: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/runs/{run_id}/events", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let types: Vec<&str> = events["events"]
        .as_array()
        .expect("the run transcript carries its events")
        .iter()
        .map(|e| e["event_type"].as_str().unwrap())
        .collect();
    assert!(
        !types.iter().any(|t| t.starts_with("progress")),
        "progress never reaches the event log: {types:?}"
    );
}

#[tokio::test]
async fn a_mid_run_subscribe_catches_up_with_the_current_line() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Working"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut early = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi").await;
    let live = next_progress(&mut early, "Thinking…").await;

    // A client that arrives mid-run reads the state, not the history.
    let mut late = subscribed_socket(&daemon).await;
    let catch_up = next_progress(&mut late, "Thinking…").await;
    assert_eq!(catch_up["payload"]["run_id"], live["payload"]["run_id"]);
    assert_eq!(
        catch_up["payload"]["message_id"],
        live["payload"]["message_id"]
    );
}

#[tokio::test]
async fn a_canceled_run_settles_its_progress_row() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Partial"]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let mut socket = subscribed_socket(&daemon).await;

    rest_send(&daemon, "p1", "hi").await;
    let live = next_progress(&mut socket, "Thinking…").await;
    let run_id = live["payload"]["run_id"].as_str().unwrap().to_string();

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/runs/{run_id}/cancel", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);

    let stopped = next_progress(&mut socket, "Stopped").await;
    assert_eq!(stopped["payload"]["run_id"], run_id.as_str());
    let row = settled_progress_row(&daemon).await;
    assert_eq!(row["blocks"][0]["text"], "Stopped");
}
