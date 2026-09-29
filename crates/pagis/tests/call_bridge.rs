//! The daemon runs the realtime bridge: a call an Agent places
//! reaches the bridge that opens the model session and dials the
//! line, and not a stub. With no OpenAI key the call fails before it
//! is dialed, and the reason the Thread sees names the key.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_telephony::fake::FakeNumberCatalog;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

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

async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
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

async fn post(daemon: &TestDaemon, path: &str, body: serde_json::Value) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::json!({}));
    assert!(status.is_success(), "{path}: {status} {body}");
    body
}

#[tokio::test]
async fn a_call_reaches_the_realtime_bridge_and_fails_on_the_missing_key() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        // No bridge is injected, so the daemon builds its own.
        call_bridge: None,
        number_catalog: Arc::new(FakeNumberCatalog::offering(&["+14155550123"])),
        ..TestDaemonOptions::default()
    })
    .await;
    daemon
        .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
        .await;
    post(
        &daemon,
        "/api/v1/settings/phone-numbers",
        serde_json::json!({ "e164": "+14155550123", "agent_id": daemon.agent_id }),
    )
    .await;
    brain.push(Script::tool_call(
        &["calling"],
        pagis_broker::PHONE_CALL,
        serde_json::json!({ "to": "+16695550142", "brief": "say hello", "success_criteria": "hello was said" }),
    ));
    brain.push(Script::reply(&["the call failed"]));
    let mut socket = firehose(&daemon).await;

    post(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
        serde_json::json!({ "pending_id": "p1", "text": "call +1 669-555-0142" }),
    )
    .await;

    // The call waits for the user's card, as every outward tool does.
    let request = next_frame_of(&mut socket, "request.created").await;
    let request_id = request["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap();
    post(
        &daemon,
        &format!("/api/v1/requests/{request_id}/decision"),
        serde_json::json!({ "decision": "approved", "scope": "once" }),
    )
    .await;

    let ended = next_frame_of(&mut socket, "call.ended").await;
    let reason = ended["payload"]["payload"]["ended_reason"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(ended["payload"]["payload"]["outcome"], "failed", "{ended}");
    assert!(
        reason.contains("OpenAI"),
        "the real bridge names the missing key: {reason}"
    );
}
