//! Full-daemon thread tests: thread fetch, reply rollups in the
//! timeline, and a thread message triggering a thread-bound run whose
//! reply posts in the thread.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn create_channel(daemon: &TestDaemon) -> String {
    let response = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "title": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let channel: serde_json::Value = response.json().await.unwrap();
    channel["id"].as_str().unwrap().to_string()
}

async fn send(daemon: &TestDaemon, channel_id: &str, body: serde_json::Value) -> serde_json::Value {
    let response = client()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn get(daemon: &TestDaemon, path: &str) -> reqwest::Response {
    client()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn timeline(daemon: &TestDaemon, channel_id: &str) -> Vec<serde_json::Value> {
    let page: serde_json::Value = get(daemon, &format!("/api/v1/channels/{channel_id}/messages"))
        .await
        .json()
        .await
        .unwrap();
    page["items"].as_array().unwrap().clone()
}

async fn send_json(socket: &mut Socket, frame: serde_json::Value) {
    socket
        .send(Message::text(frame.to_string()))
        .await
        .expect("ws send");
}

/// The next frame of one type, skipping others, within a timeout.
async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("frame is JSON");
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
    next_frame_of(&mut socket, "ready").await;
    send_json(
        &mut socket,
        serde_json::json!({ "type": "subscribe", "channel_id": daemon.dm_channel_id }),
    )
    .await;
    socket
}

/// The next agent `message.completed` event on the firehose.
async fn next_agent_completion(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = next_frame_of(socket, "message.completed").await;
        if frame["payload"]["payload"]["author_kind"] == "agent" {
            return frame;
        }
    }
}

#[tokio::test]
async fn thread_fetch_returns_root_and_replies() {
    let daemon = TestDaemon::start().await;
    let channel_id = create_channel(&daemon).await;

    let root = send(
        &daemon,
        &channel_id,
        serde_json::json!({ "pending_id": "root", "text": "root" }),
    )
    .await;
    let root_id = root["id"].as_str().unwrap();
    for i in 0..2 {
        send(
            &daemon,
            &channel_id,
            serde_json::json!({
                "pending_id": format!("r{i}"),
                "text": format!("reply {i}"),
                "parent_message_id": root_id,
            }),
        )
        .await;
    }

    let response = get(
        &daemon,
        &format!("/api/v1/channels/{channel_id}/threads/{root_id}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let thread: serde_json::Value = response.json().await.unwrap();
    assert_eq!(thread["root"]["id"], root_id);
    let replies = thread["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["text_content"], "reply 0");
    assert_eq!(replies[1]["text_content"], "reply 1");
    assert_eq!(replies[0]["parent_message_id"], root_id);
}

#[tokio::test]
async fn thread_fetch_rejects_a_reply_root_and_unknown_roots() {
    let daemon = TestDaemon::start().await;
    let channel_id = create_channel(&daemon).await;

    let root = send(
        &daemon,
        &channel_id,
        serde_json::json!({ "pending_id": "root", "text": "root" }),
    )
    .await;
    let root_id = root["id"].as_str().unwrap();
    let reply = send(
        &daemon,
        &channel_id,
        serde_json::json!({
            "pending_id": "r0",
            "text": "reply",
            "parent_message_id": root_id,
        }),
    )
    .await;
    let reply_id = reply["id"].as_str().unwrap();

    // A reply does not root a thread: threads are one level deep.
    let nested = get(
        &daemon,
        &format!("/api/v1/channels/{channel_id}/threads/{reply_id}"),
    )
    .await;
    assert_eq!(nested.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = nested.json().await.unwrap();
    assert_eq!(body["error"]["code"], "validation");

    let unknown = get(
        &daemon,
        &format!("/api/v1/channels/{channel_id}/threads/01UNKNOWNMESSAGE"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // A root from another channel is not this channel's thread.
    let other_channel = create_channel(&daemon).await;
    let foreign = get(
        &daemon,
        &format!("/api/v1/channels/{other_channel}/threads/{root_id}"),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reply_rollups_update_in_the_timeline() {
    let daemon = TestDaemon::start().await;
    let channel_id = create_channel(&daemon).await;

    let root = send(
        &daemon,
        &channel_id,
        serde_json::json!({ "pending_id": "root", "text": "root" }),
    )
    .await;
    let root_id = root["id"].as_str().unwrap();

    let items = timeline(&daemon, &channel_id).await;
    assert_eq!(items[0]["reply_count"], 0);
    assert_eq!(items[0]["last_reply_at"], serde_json::Value::Null);
    assert_eq!(items[0]["reply_authors"], serde_json::json!([]));

    let mut last = serde_json::Value::Null;
    for i in 0..2 {
        let reply = send(
            &daemon,
            &channel_id,
            serde_json::json!({
                "pending_id": format!("r{i}"),
                "text": format!("reply {i}"),
                "parent_message_id": root_id,
            }),
        )
        .await;
        last = reply["created_at"].clone();
    }

    // The timeline stays one top-level item, now with the rollup.
    let items = timeline(&daemon, &channel_id).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], root_id);
    assert_eq!(items[0]["reply_count"], 2);
    assert_eq!(items[0]["last_reply_at"], last);
    // Both replies are the user's, so the user is the one author.
    assert_eq!(
        items[0]["reply_authors"],
        serde_json::json!([{ "author_kind": "user", "author_agent_id": null }])
    );
}

#[tokio::test]
async fn a_reply_completion_event_carries_its_parent() {
    // The live rollup path: the client invalidates the timeline on
    // `message.completed`, so the event must identify thread replies.
    let daemon = TestDaemon::start().await;
    let channel_id = create_channel(&daemon).await;
    let mut socket = subscribed_socket(&daemon).await;

    let root = send(
        &daemon,
        &channel_id,
        serde_json::json!({ "pending_id": "root", "text": "root" }),
    )
    .await;
    let root_id = root["id"].as_str().unwrap();
    next_frame_of(&mut socket, "message.completed").await;

    send(
        &daemon,
        &channel_id,
        serde_json::json!({
            "pending_id": "r0",
            "text": "reply",
            "parent_message_id": root_id,
        }),
    )
    .await;
    let event = next_frame_of(&mut socket, "message.completed").await;
    assert_eq!(event["payload"]["channel_id"], channel_id);
    assert_eq!(event["payload"]["payload"]["parent_message_id"], root_id);
}

#[tokio::test]
async fn a_thread_message_triggers_a_thread_bound_run() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Top-level answer"]));
    brain.push(Script::reply(&["Thread answer"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let channel_id = daemon.dm_channel_id.clone();
    let mut socket = subscribed_socket(&daemon).await;

    // The top-level message triggers a top-level run.
    send(
        &daemon,
        &channel_id,
        serde_json::json!({ "pending_id": "p1", "text": "hi Pixie" }),
    )
    .await;
    next_agent_completion(&mut socket).await;
    let items = timeline(&daemon, &channel_id).await;
    let agent_reply_id = items[0]["id"].as_str().unwrap().to_string();
    assert_eq!(items[0]["author_kind"], "agent");

    // A reply in the thread rooted at the agent's message triggers a
    // run bound to that thread.
    send(
        &daemon,
        &channel_id,
        serde_json::json!({
            "pending_id": "p2",
            "text": "tell me more",
            "parent_message_id": agent_reply_id,
        }),
    )
    .await;

    // The streaming delta already carries the thread.
    let delta = next_frame_of(&mut socket, "message.delta").await;
    assert_eq!(delta["payload"]["parent_message_id"], agent_reply_id);
    let run_id = delta["payload"]["run_id"].as_str().unwrap().to_string();
    let completion = next_agent_completion(&mut socket).await;
    assert_eq!(
        completion["payload"]["payload"]["parent_message_id"],
        agent_reply_id
    );

    // Reply publication precedes terminal progress settlement.
    loop {
        let ended = next_frame_of(&mut socket, "run.state_changed").await;
        if ended["payload"]["run_id"] == run_id && ended["payload"]["payload"]["to"] == "completed"
        {
            break;
        }
    }

    // The run row binds to the thread root.
    use pagis_core::{RunId, RunStore};
    let run = pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &RunId::from(run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        run.root_message_id.as_ref().map(|m| m.to_string()),
        Some(agent_reply_id.clone())
    );

    // The agent's reply posts in the thread, not the timeline.
    let items = timeline(&daemon, &channel_id).await;
    assert_eq!(
        items.len(),
        4,
        "the greeting, user root, top-level progress row, and agent reply stay top-level"
    );
    assert_eq!(items[1]["blocks"][0]["type"], "progress");
    assert_eq!(items[2]["reply_count"].as_i64(), Some(0));
    assert_eq!(items[0]["id"], agent_reply_id);
    assert_eq!(items[0]["reply_count"], 2);

    let thread: serde_json::Value = get(
        &daemon,
        &format!("/api/v1/channels/{channel_id}/threads/{agent_reply_id}"),
    )
    .await
    .json()
    .await
    .unwrap();
    // The thread shows the run's progress row beside the
    // exchange, in the order they were posted. The root's rollup
    // counts the exchange only, so it stays at two.
    let replies = thread["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["text_content"], "tell me more");
    assert_eq!(replies[1]["blocks"][0]["type"], "progress");
    assert_eq!(replies[1]["blocks"][0]["text"], "Done");
    assert_eq!(replies[2]["author_kind"], "agent");
    assert_eq!(replies[2]["text_content"], "Thread answer");
    assert_eq!(replies[2]["parent_message_id"], agent_reply_id);
}
