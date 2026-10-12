//! Tests for realtime passthrough.

use std::time::Duration;

#[path = "support/realtime.rs"]
mod memory_transport;
use memory_transport::MemoryTransport;

use llm_router::{
    Candidate, Error, ProtocolKind, ProviderConfig, RealtimeMessage, RealtimeProtocol,
    RealtimeTransport, RetryConfig, Router, RouterConfig, TungsteniteTransport,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

/// A one-connection WebSocket server. Returns its base URL and a handle
/// that resolves with the request path and auth header it saw.
// The handshake callback's Result type comes from tokio-tungstenite.
#[allow(clippy::result_large_err)]
async fn ws_server() -> (String, tokio::task::JoinHandle<(String, String)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        // The first handshake has no answer. The transport deadline closes it.
        let (mut stalled, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stalled, &mut request)
            .await
            .unwrap();
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
        // Control frames stay in the transport. Application frames pass through.
        socket.send(Message::Ping(vec![1, 2].into())).await.unwrap();
        socket.send(Message::Pong(vec![3, 4].into())).await.unwrap();
        socket
            .send(Message::text("{\"type\":\"session.created\"}"))
            .await
            .unwrap();
        while let Some(Ok(frame)) = socket.next().await {
            match frame {
                Message::Text(_) | Message::Binary(_) => socket.send(frame).await.unwrap(),
                Message::Close(_) => {
                    socket.flush().await.unwrap();
                    break;
                }
                Message::Pong(_) => {}
                other => panic!("unexpected client frame: {other:?}"),
            }
        }
        (seen_path, seen_auth)
    });
    (format!("http://127.0.0.1:{}/v1", addr.port()), handle)
}

async fn memory_server() -> (
    std::sync::Arc<MemoryTransport>,
    tokio::task::JoinHandle<(String, String)>,
) {
    use futures::{SinkExt, StreamExt};
    let (transport, mut socket) = MemoryTransport::pair();
    let observed = transport.clone();
    let handle = tokio::spawn(async move {
        socket
            .send(RealtimeMessage::text("{\"type\":\"session.created\"}"))
            .await
            .unwrap();
        let frame = socket.next().await.unwrap().unwrap();
        socket.send(frame).await.unwrap();
        let requests = observed.requests.lock().unwrap();
        let (url, headers, _) = &requests[0];
        let url = reqwest::Url::parse(url).unwrap();
        let path = url.query().map_or_else(
            || url.path().to_string(),
            |query| format!("{}?{query}", url.path()),
        );
        (path, headers["authorization"].to_str().unwrap().to_string())
    });
    (transport, handle)
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
    let (transport, server) = memory_server().await;
    let base_url = "https://provider.test/v1";
    let router = router_for_model(base_url, ProtocolKind::OpenAiChat, "gpt-live-1")
        .with_realtime_transport(transport);

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
    let url = base_url.replace("http://", "ws://");
    let timeout_error = TungsteniteTransport
        .connect(
            &url,
            &reqwest::header::HeaderMap::new(),
            Duration::from_millis(25),
        )
        .await
        .err()
        .expect("the handshake times out");
    assert_eq!(timeout_error.to_string(), "timed out opening the WebSocket");
    let router = router_for(&base_url, ProtocolKind::OpenAiChat);

    let mut connection = router.realtime_connect("m").await.unwrap();
    assert_eq!(connection.provider, "p");
    assert_eq!(connection.model, "gpt-realtime-2.1");

    // The server's first event arrives as-is (passthrough, no translation).
    let first = connection.next().await.unwrap().unwrap();
    assert_eq!(
        first,
        RealtimeMessage::text("{\"type\":\"session.created\"}")
    );

    // Frames pump upstream and back.
    connection
        .send(RealtimeMessage::text(
            "{\"type\":\"input_audio_buffer.append\",\"audio\":\"QUJD\"}",
        ))
        .await
        .unwrap();
    let echoed = connection.next().await.unwrap().unwrap();
    assert_eq!(
        echoed,
        RealtimeMessage::text("{\"type\":\"input_audio_buffer.append\",\"audio\":\"QUJD\"}")
    );
    connection
        .send(RealtimeMessage::binary(vec![0, 255, 1]))
        .await
        .unwrap();
    assert_eq!(
        connection.next().await.unwrap().unwrap(),
        RealtimeMessage::binary(vec![0, 255, 1])
    );
    connection.send(RealtimeMessage::Close).await.unwrap();
    assert_eq!(
        connection.next().await.unwrap().unwrap(),
        RealtimeMessage::Close
    );
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
    let (transport, server) = memory_server().await;
    let base_url = "https://provider.test/v1";
    let router = router_for(base_url, ProtocolKind::OpenAiChat).with_realtime_transport(transport);

    let mut connection = router
        .realtime_transcription_connect("m", 24_000)
        .await
        .unwrap();
    assert_eq!(connection.model, "gpt-realtime-2.1");
    let first = connection.next().await.unwrap().unwrap();
    assert_eq!(
        first,
        RealtimeMessage::text("{\"type\":\"session.created\"}")
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

/// ElevenLabs streams speech to text on Scribe's realtime socket: the
/// live model of a Scribe generation is its `_realtime` model, the audio
/// format and a manual commit go in the query, and the key goes in
/// `xi-api-key`.
#[tokio::test]
async fn an_elevenlabs_transcription_socket_opens_scribe_realtime_with_a_manual_commit() {
    let (transport, _peer) = MemoryTransport::pair();
    let router = router_for_model(
        "https://provider.test/v1",
        ProtocolKind::ElevenLabs,
        "scribe_v2",
    )
    .with_realtime_transport(transport.clone());

    let connection = router
        .realtime_transcription_connect("m", 24_000)
        .await
        .unwrap();

    assert_eq!(connection.protocol, RealtimeProtocol::ElevenLabsScribe);
    let requests = transport.requests.lock().unwrap();
    let (url, headers, _) = &requests[0];
    let url = reqwest::Url::parse(url).unwrap();
    let path = format!("{}?{}", url.path(), url.query().unwrap());
    let key = headers["xi-api-key"].to_str().unwrap();
    assert_eq!(
        path,
        "/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_24000\
         &commit_strategy=manual"
    );
    assert_eq!(key, "test-key");
}

/// Deepgram streams speech to text on `/listen`: the model and the audio
/// format go in the query, and the key goes as a `Token`.
#[tokio::test]
async fn a_deepgram_transcription_socket_opens_listen_with_the_audio_format() {
    let (transport, server) = memory_server().await;
    let base_url = "https://provider.test/v1";
    let router = router_for_model(base_url, ProtocolKind::Deepgram, "nova-3")
        .with_realtime_transport(transport);

    let mut connection = router
        .realtime_transcription_connect("m", 24_000)
        .await
        .unwrap();
    assert_eq!(connection.protocol, RealtimeProtocol::DeepgramListen);
    connection.next().await.unwrap().unwrap();
    connection
        .send(RealtimeMessage::text("{\"type\":\"KeepAlive\"}"))
        .await
        .unwrap();
    connection.next().await.unwrap().unwrap();
    connection.close().await.unwrap();

    let (path, auth) = server.await.unwrap();
    assert_eq!(
        path,
        "/v1/listen?model=nova-3&encoding=linear16&sample_rate=24000&channels=1\
         &interim_results=true&punctuate=true&smart_format=true"
    );
    assert_eq!(auth, "Token test-key");
}
