//! The five frame protocols run without a listener.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_core::HostId;
use pagis_testkit::{
    ExitClient, HostAnswer, HostClient, SessionClient, TestDaemon, TestDaemonOptions,
};
use pagis_voice::fake::{FakeVoice, VoiceCall};
use tokio_tungstenite::tungstenite::Message;

async fn text(socket: &mut pagis_testkit::Socket) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(&frame.into_text().unwrap()).unwrap()
}

#[tokio::test]
async fn events_use_the_message_pair_and_the_router() {
    let daemon = TestDaemon::start_in_memory().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    let response = daemon
        .client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"title": "general"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let channel: serde_json::Value = response.json().await.unwrap();
    let event = text(&mut socket).await;
    assert_eq!(event["type"], "channel.created");
    assert_eq!(event["payload"]["channel_id"], channel["id"]);
    socket
        .send(Message::text(r#"{"type":"ping"}"#))
        .await
        .unwrap();
    assert_eq!(text(&mut socket).await["type"], "pong");
}

#[tokio::test]
async fn a_close_frame_ends_the_event_protocol() {
    let daemon = TestDaemon::start_in_memory().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    socket.send(Message::Close(None)).await.unwrap();
    let ended = tokio::time::timeout(Duration::from_millis(200), socket.next()).await;
    assert!(
        matches!(ended, Ok(None)),
        "the protocol stays open after Close"
    );
}

#[tokio::test]
async fn dictation_carries_audio_and_commits_the_draft() {
    let voice = Arc::new(FakeVoice::hearing("the draft"));
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        voice: voice.clone(),
        ..TestDaemonOptions::default()
    })
    .await;
    let path = format!("/api/v1/channels/{}/dictate", daemon.dm_channel_id);
    let mut socket = daemon.open_socket(&path, daemon.cookie()).await.unwrap();
    assert_eq!(
        text(&mut socket).await,
        serde_json::json!({"type": "ready", "live": false})
    );
    socket
        .send(Message::Binary(vec![1, 0, 2, 0].into()))
        .await
        .unwrap();
    socket
        .send(Message::text(r#"{"type":"commit"}"#))
        .await
        .unwrap();
    assert_eq!(
        text(&mut socket).await,
        serde_json::json!({"type": "transcript.final", "text": "the draft"})
    );
    assert!(
        voice
            .calls()
            .iter()
            .any(|call| matches!(call, VoiceCall::Transcribe { .. }))
    );
}

#[tokio::test]
async fn listen_live_reports_a_call_that_is_not_live() {
    let daemon = TestDaemon::start_in_memory().await;
    let mut socket = daemon
        .open_socket("/api/v1/calls/absent/listen", daemon.cookie())
        .await
        .unwrap();
    let frame = text(&mut socket).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "not_found");
}

#[tokio::test]
async fn a_session_socket_carries_process_bytes() {
    use futures::{AsyncReadExt, AsyncWriteExt};
    let daemon = TestDaemon::start_in_memory().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let _sessions = SessionClient::connect(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        pagis_broker::fake::FakeClientApp::opening("/work"),
    )
    .await
    .unwrap();
    let host_id = HostId::from(host.host_id().to_string());
    wait_until(|| daemon.host_sessions.is_open(&host_id)).await;
    let request = pagis_broker::OpenRequest {
        session_id: pagis_core::CodingSessionId::generate(),
        command: "cat".into(),
        args: vec![],
        cwd: "/work".into(),
        env: Default::default(),
        worktree: None,
    };
    let mut opened = daemon
        .host_sessions
        .open(&daemon.workspace_id, &host_id, &request)
        .await
        .unwrap();
    opened.stream.write_all(b"frame bytes").await.unwrap();
    let mut bytes = [0; 11];
    tokio::time::timeout(Duration::from_secs(5), opened.stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"frame bytes");
}

#[tokio::test]
async fn an_exit_socket_ends_with_its_session() {
    let daemon = TestDaemon::start_in_memory().await;
    let cookie = daemon.cookie_for(&daemon.user_id).await;
    let host = HostClient::connect_as(
        &daemon,
        &cookie,
        "Air",
        "macos",
        &["exit"],
        HostAnswer::ok(),
    )
    .await;
    let exit = pagis_computer::fake::FakeHomeExit::new(|_, _| {
        pagis_computer::fake::FakeExitAnswer::Refuse("refused".into())
    });
    let mut client = ExitClient::connect(&daemon, &cookie, host.host_id(), exit)
        .await
        .unwrap();
    let host_id = HostId::from(host.host_id().to_string());
    wait_until(|| daemon.home_exits.is_open(&host_id)).await;
    daemon.sign_out(&cookie).await;
    assert_eq!(client.closed().await, Some(1008));
    wait_until(|| !daemon.home_exits.is_open(&host_id)).await;
}

async fn wait_until(ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
