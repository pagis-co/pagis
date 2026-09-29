//! Tests for realtime passthrough.

use std::time::Duration;

use llm_router::{
    Candidate, Error, ProtocolKind, ProviderConfig, RealtimeMessage, RetryConfig, Router,
    RouterConfig,
};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

/// A one-connection WebSocket server. Returns its base URL and a handle
/// that resolves with the request path and auth header it saw.
// The handshake callback's Result type comes from tokio-tungstenite.
#[allow(clippy::result_large_err)]
async fn ws_server() -> (String, tokio::task::JoinHandle<(String, String)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut seen_path = String::new();
        let mut seen_auth = String::new();
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp| {
            seen_path = req.uri().to_string();
            seen_auth = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            Ok::<Response, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>(resp)
        })
        .await
        .unwrap();
        use futures::{SinkExt, StreamExt};
        // Speak like a realtime server: announce the session, then echo.
        socket
            .send(RealtimeMessage::text("{\"type\":\"session.created\"}"))
            .await
            .unwrap();
        if let Some(Ok(frame)) = socket.next().await {
            socket.send(frame).await.unwrap();
        }
        (seen_path, seen_auth)
    });
    (format!("http://127.0.0.1:{}/v1", addr.port()), handle)
}

fn router_for(base_url: &str, kind: ProtocolKind) -> Router {
    router_for_model(base_url, kind, "gpt-realtime-2.1")
}

fn router_for_model(base_url: &str, kind: ProtocolKind, model: &str) -> Router {
    let config = RouterConfig::new()
        .provider("p", ProviderConfig::new(kind, base_url, "test-key"))
        .model("m", [Candidate::new("p", model)])
        .retry(RetryConfig {
            max_attempts: 1,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        });
    Router::new(config).unwrap()
}

#[tokio::test]
async fn gpt_live_uses_the_live_session_endpoint() {
    let (base_url, server) = ws_server().await;
    let router = router_for_model(&base_url, ProtocolKind::OpenAiChat, "gpt-live-1");

    let mut connection = router.realtime_connect("m").await.unwrap();
    assert_eq!(connection.model, "gpt-live-1");
    connection.next().await.unwrap().unwrap();
    connection
        .send(RealtimeMessage::text("{\"type\":\"session.start\"}"))
        .await
        .unwrap();
    connection.next().await.unwrap().unwrap();
    connection.close().await.unwrap();

    let (path, auth) = server.await.unwrap();
    assert_eq!(path, "/v1/live/sessions");
    assert_eq!(auth, "Bearer test-key");
}

#[tokio::test]
async fn realtime_connects_with_auth_and_pumps_frames() {
    let (base_url, server) = ws_server().await;
    let router = router_for(&base_url, ProtocolKind::OpenAiChat);

    let mut connection = router.realtime_connect("m").await.unwrap();
    assert_eq!(connection.provider, "p");
    assert_eq!(connection.model, "gpt-realtime-2.1");

    // The server's first event arrives as-is (passthrough, no translation).
    let first = connection.next().await.unwrap().unwrap();
    assert_eq!(
        first.into_text().unwrap().as_str(),
        "{\"type\":\"session.created\"}"
    );

    // Frames pump upstream and back.
    connection
        .send(RealtimeMessage::text(
            "{\"type\":\"input_audio_buffer.append\",\"audio\":\"QUJD\"}",
        ))
        .await
        .unwrap();
    let echoed = connection.next().await.unwrap().unwrap();
    assert!(echoed.into_text().unwrap().contains("input_audio_buffer"));
    connection.close().await.unwrap();

    let (path, auth) = server.await.unwrap();
    assert_eq!(path, "/v1/realtime?model=gpt-realtime-2.1");
    assert_eq!(auth, "Bearer test-key");
}

#[tokio::test]
async fn realtime_is_unsupported_off_the_openai_protocol() {
    let router = router_for("http://127.0.0.1:9/v1", ProtocolKind::AnthropicMessages);
    let error = router.realtime_connect("m").await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(matches!(*last, Error::Unsupported { feature, .. } if feature == "realtime"));
}

#[tokio::test]
async fn realtime_connect_failure_is_retryable() {
    // Nothing listens on this port; the connect error must classify as
    // retryable so an alias can fall back to another candidate.
    let router = router_for("http://127.0.0.1:9/v1", ProtocolKind::OpenAiChat);
    let error = router.realtime_connect("m").await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(last.is_retryable());
}

#[tokio::test]
async fn a_transcription_session_opens_with_the_transcription_intent() {
    let (base_url, server) = ws_server().await;
    let router = router_for(&base_url, ProtocolKind::OpenAiChat);

    let mut connection = router.realtime_transcription_connect("m").await.unwrap();
    assert_eq!(connection.model, "gpt-realtime-2.1");
    let first = connection.next().await.unwrap().unwrap();
    assert_eq!(
        first.into_text().unwrap().as_str(),
        "{\"type\":\"session.created\"}"
    );
    connection
        .send(RealtimeMessage::text("{\"type\":\"session.update\"}"))
        .await
        .unwrap();
    connection.next().await.unwrap().unwrap();
    connection.close().await.unwrap();

    let (path, auth) = server.await.unwrap();
    assert_eq!(path, "/v1/realtime?intent=transcription");
    assert_eq!(auth, "Bearer test-key");
}
