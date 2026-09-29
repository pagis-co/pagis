//! Full-daemon WebSocket tests: first-frame auth, the domain
//! event firehose, `last_seq` resume, and the `resync` fallback. The
//! tenant tests at the end hold two sockets of two people and prove
//! that neither reads the other's Workspace.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_server::RingConfig;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions, TwoTenants};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(daemon: &TestDaemon) -> Socket {
    connect_as(daemon, daemon.cookie()).await
}

/// A socket of one person's Session.
async fn connect_as(daemon: &TestDaemon, cookie: &str) -> Socket {
    let (socket, _) = connect_async(daemon.ws_request_as(&daemon.ws_url(), cookie))
        .await
        .expect("ws connect");
    socket
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

/// Authenticate and consume the `ready` ack. Returns the socket.
async fn authed_socket(daemon: &TestDaemon, last_seq: Option<&str>) -> (Socket, serde_json::Value) {
    authed_socket_as(daemon, daemon.cookie(), last_seq).await
}

/// The same, for a named person's Session.
async fn authed_socket_as(
    daemon: &TestDaemon,
    cookie: &str,
    last_seq: Option<&str>,
) -> (Socket, serde_json::Value) {
    let mut socket = connect_as(daemon, cookie).await;
    let mut auth = serde_json::json!({ "type": "auth" });
    if let Some(last_seq) = last_seq {
        auth["last_seq"] = serde_json::json!(last_seq);
    }
    send_json(&mut socket, auth).await;
    let first = next_frame(&mut socket).await;
    (socket, first)
}

/// Send a message over REST; returns its message id.
async fn rest_send(daemon: &TestDaemon, channel_id: &str, pending_id: &str, text: &str) -> String {
    rest_send_as(daemon, daemon.cookie(), channel_id, pending_id, text).await
}

async fn rest_send_as(
    daemon: &TestDaemon,
    cookie: &str,
    channel_id: &str,
    pending_id: &str,
    text: &str,
) -> String {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

async fn rest_create_channel(daemon: &TestDaemon) -> String {
    rest_create_channel_as(daemon, daemon.cookie()).await
}

async fn rest_create_channel_as(daemon: &TestDaemon, cookie: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "title": "general" }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_socket_without_a_session_is_refused_at_the_upgrade() {
    let daemon = TestDaemon::start().await;

    let error = connect_async(daemon.ws_url())
        .await
        .expect_err("the daemon upgrades no socket without a session");

    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 401);
        }
        other => panic!("unexpected handshake failure: {other}"),
    }
}

/// A first frame that is not an `auth` frame is refused, whoever sends
/// it: the frame carries the resume point, and the socket cannot go live
/// without one.
#[tokio::test]
async fn a_first_frame_that_is_not_auth_is_rejected_with_an_error_frame() {
    let daemon = TestDaemon::start().await;
    let mut socket = connect(&daemon).await;

    send_json(&mut socket, serde_json::json!({ "type": "hello" })).await;

    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["payload"]["code"], "unauthorized");
}

#[tokio::test]
async fn the_firehose_delivers_domain_events_with_the_envelope() {
    let daemon = TestDaemon::start().await;
    let (mut socket, ready) = authed_socket(&daemon, None).await;
    assert_eq!(ready["type"], "ready");

    let channel_id = rest_create_channel(&daemon).await;
    let created = next_frame(&mut socket).await;
    assert_eq!(created["type"], "channel.created");
    assert_eq!(created["payload"]["channel_id"], channel_id);
    assert!(created["seq"].is_string());
    assert_eq!(created["seq"], created["payload"]["id"]);

    rest_send(&daemon, &channel_id, "p1", "hello").await;
    let completed = next_frame(&mut socket).await;
    assert_eq!(completed["type"], "message.completed");
    assert_eq!(completed["payload"]["event_type"], "message.completed");
    assert_eq!(completed["payload"]["channel_id"], channel_id);
    assert_eq!(completed["payload"]["payload"]["author_kind"], "user");
}

#[tokio::test]
async fn ping_answers_pong() {
    let daemon = TestDaemon::start().await;
    let (mut socket, ready) = authed_socket(&daemon, None).await;
    assert_eq!(ready["type"], "ready");

    send_json(&mut socket, serde_json::json!({ "type": "ping" })).await;
    assert_eq!(next_frame(&mut socket).await["type"], "pong");
}

#[tokio::test]
async fn resume_within_the_buffer_replays_exactly_the_gap() {
    let daemon = TestDaemon::start().await;
    let channel_id = rest_create_channel(&daemon).await;

    let (mut socket, ready) = authed_socket(&daemon, None).await;
    assert_eq!(ready["type"], "ready");
    rest_send(&daemon, &channel_id, "p1", "m1").await;
    let seen = next_frame(&mut socket).await;
    assert_eq!(seen["type"], "message.completed");
    let last_seq = seen["seq"].as_str().unwrap().to_string();
    drop(socket);

    let m2 = rest_send(&daemon, &channel_id, "p2", "m2").await;
    let m3 = rest_send(&daemon, &channel_id, "p3", "m3").await;

    let (mut socket, ready) = authed_socket(&daemon, Some(&last_seq)).await;
    assert_eq!(ready["type"], "ready");
    let first = next_frame(&mut socket).await;
    let second = next_frame(&mut socket).await;
    assert_eq!(first["replay"], true);
    assert_eq!(second["replay"], true);
    // Exactly the gap, in order: m2 then m3, nothing replayed before.
    assert_eq!(first["type"], "message.completed");
    assert_eq!(first["payload"]["payload"]["message_id"], m2);
    assert_eq!(second["payload"]["payload"]["message_id"], m3);
    // The stream continues live past the gap.
    let m4 = rest_send(&daemon, &channel_id, "p4", "m4").await;
    let next = next_frame(&mut socket).await;
    assert_eq!(next["replay"], false);
    assert_eq!(next["payload"]["payload"]["message_id"], m4);
}

#[tokio::test]
async fn a_gap_beyond_the_buffer_sends_resync() {
    let daemon = TestDaemon::start_with_ring(RingConfig {
        capacity: 2,
        min_age: Duration::ZERO,
    })
    .await;
    let channel_id = rest_create_channel(&daemon).await;

    let (mut socket, ready) = authed_socket(&daemon, None).await;
    assert_eq!(ready["type"], "ready");
    rest_send(&daemon, &channel_id, "p1", "m1").await;
    let seen = next_frame(&mut socket).await;
    let last_seq = seen["seq"].as_str().unwrap().to_string();
    drop(socket);

    // Push the seen event out of the two-entry ring.
    rest_send(&daemon, &channel_id, "p2", "m2").await;
    rest_send(&daemon, &channel_id, "p3", "m3").await;
    rest_send(&daemon, &channel_id, "p4", "m4").await;

    let (mut socket, ready) = authed_socket(&daemon, Some(&last_seq)).await;
    assert_eq!(ready["type"], "ready");
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "resync");
}

/// No frame arrives within the window. A socket that must stay silent
/// is proved by waiting, so the window is long enough that a frame the
/// daemon did send would have arrived.
async fn stays_silent(socket: &mut Socket) {
    match tokio::time::timeout(Duration::from_millis(750), socket.next()).await {
        Err(_) => {}
        Ok(Some(Ok(Message::Text(text)))) => {
            panic!("the socket received a frame it has no right to: {text}")
        }
        Ok(other) => panic!("unexpected socket state: {other:?}"),
    }
}

/// Two people hold a socket each, and neither firehose carries the
/// other's Workspace: not live, and not on `last_seq` replay.
#[tokio::test]
async fn neither_socket_receives_an_event_of_the_other_workspace() {
    let world = TwoTenants::start().await;
    let daemon = &world.daemon;

    let (mut a_socket, ready) = authed_socket_as(daemon, &world.a.cookie, None).await;
    assert_eq!(ready["type"], "ready");
    let (mut b_socket, ready) = authed_socket_as(daemon, &world.b.cookie, None).await;
    assert_eq!(ready["type"], "ready");

    // Person A writes. A's socket sees it; B's socket sees nothing.
    let a_channel = rest_create_channel_as(daemon, &world.a.cookie).await;
    rest_send_as(daemon, &world.a.cookie, &a_channel, "p1", "A's words").await;
    let created = next_frame(&mut a_socket).await;
    assert_eq!(created["type"], "channel.created");
    assert_eq!(next_frame(&mut a_socket).await["type"], "message.completed");
    stays_silent(&mut b_socket).await;

    // Person B writes. B's socket sees its own event, and that event is
    // B's resume point.
    let b_channel = rest_create_channel_as(daemon, &world.b.cookie).await;
    let b_created = next_frame(&mut b_socket).await;
    assert_eq!(b_created["type"], "channel.created");
    assert_eq!(b_created["payload"]["channel_id"], b_channel);
    let b_last_seq = b_created["seq"].as_str().expect("an event id").to_string();
    drop(b_socket);

    // While B is away, A writes again. B resumes from its own event and
    // the replay carries none of A's.
    rest_send_as(daemon, &world.a.cookie, &a_channel, "p2", "more of A's").await;
    let (mut b_socket, ready) = authed_socket_as(daemon, &world.b.cookie, Some(&b_last_seq)).await;
    assert_eq!(ready["type"], "ready");
    stays_silent(&mut b_socket).await;

    // The resumed socket is live for B's own events, so the silence is
    // the filter and not a dead stream.
    rest_send_as(daemon, &world.b.cookie, &b_channel, "p3", "B's words").await;
    let b_message = next_frame(&mut b_socket).await;
    assert_eq!(b_message["type"], "message.completed");
    assert_eq!(b_message["payload"]["channel_id"], b_channel);
}

/// A `subscribe` frame that names another person's Channel is refused,
/// and the in-flight text of that Channel is not sent.
#[tokio::test]
async fn a_subscribe_to_another_persons_channel_is_refused_and_sends_no_in_flight_text() {
    let brain = Arc::new(ScriptedBrain::default());
    // The run streams its draft and then holds, so the text is in
    // flight while person B asks for it.
    brain.push(Script::hang(&["A's draft nobody else may read"]));
    let world = TwoTenants::on(
        TestDaemon::start_with(TestDaemonOptions {
            brain: Arc::clone(&brain) as _,
            ..TestDaemonOptions::default()
        })
        .await,
    )
    .await;
    let daemon = &world.daemon;
    let a_channel = daemon.dm_channel_id.clone();

    // Person A subscribes and drives the run, so the hub really holds
    // the accumulated draft.
    let (mut a_socket, ready) = authed_socket_as(daemon, &world.a.cookie, None).await;
    assert_eq!(ready["type"], "ready");
    send_json(
        &mut a_socket,
        serde_json::json!({ "type": "subscribe", "channel_id": a_channel }),
    )
    .await;
    rest_send_as(daemon, &world.a.cookie, &a_channel, "p1", "draft it").await;
    loop {
        let frame = next_frame(&mut a_socket).await;
        if frame["type"] == "message.delta" {
            assert_eq!(frame["payload"]["text"], "A's draft nobody else may read");
            break;
        }
    }

    // Person B asks for the same Channel by id.
    let (mut b_socket, ready) = authed_socket_as(daemon, &world.b.cookie, None).await;
    assert_eq!(ready["type"], "ready");
    send_json(
        &mut b_socket,
        serde_json::json!({ "type": "subscribe", "channel_id": a_channel }),
    )
    .await;

    let refusal = next_frame(&mut b_socket).await;
    assert_eq!(refusal["type"], "error");
    assert_eq!(refusal["payload"]["code"], "not_found");
    // No snapshot, no delta and no progress frame follows the refusal.
    stays_silent(&mut b_socket).await;
}
