//! Tests for the forward of a provider's own request.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use llm_router::protocol::ByteStream;
use llm_router::{
    Error, ForwardRequest, Metered, ProtocolKind, ProviderConfig, RetryConfig, Router,
    RouterConfig, Usage,
};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::single_provider_router;

/// A Messages request as Claude Code writes it: fields the neutral request
/// does not carry, a `cache_control` marker in place, and spacing that a
/// decode and an encode would not keep.
const CLAUDE_CODE_BODY: &str = concat!(
    r#"{"model":"claude-sonnet-4-5", "max_tokens":32000,"#,
    r#""messages":[{"role":"user","content":[{"type":"text","text":"hi","#,
    r#""cache_control":{"type":"ephemeral"}}]}],"#,
    r#""context_management":{"edits":[{"type":"clear_tool_uses_20250919"}]},"#,
    r#""output_config":{"effort":"high"},"stream":true}"#,
);

const MESSAGE_START: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",",
    "\"model\":\"claude-sonnet-4-5-20250929\",\"usage\":{\"input_tokens\":25,",
    "\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n",
);

fn stream_body() -> String {
    [
        MESSAGE_START,
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]
    .concat()
}

fn messages_request(path: &str, body: &str) -> ForwardRequest {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("context-management-2025-06-27,interleaved-thinking-2025-05-14"),
    );
    // The token the client used to reach the caller.
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer session-token"),
    );
    ForwardRequest {
        wire: ProtocolKind::AnthropicMessages,
        path: path.to_owned(),
        headers,
        body: Bytes::copy_from_slice(body.as_bytes()),
    }
}

async fn read_all(mut body: ByteStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    out
}

fn router_with(provider: ProviderConfig, max_attempts: u32) -> Router {
    let config = RouterConfig::new()
        .provider("p", provider)
        .retry(RetryConfig {
            max_attempts,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        });
    Router::new(config).unwrap()
}

#[tokio::test]
async fn forward_sends_the_callers_request_with_the_provider_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1",
            "model": "claude-sonnet-4-5-20250929",
            "content": [{"type": "text", "text": "Hi."}],
            "usage": {
                "input_tokens": 25,
                "output_tokens": 7,
                "cache_read_input_tokens": 10,
                "cache_creation_input_tokens": 5
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut provider =
        ProviderConfig::new(ProtocolKind::AnthropicMessages, server.uri(), "test-key");
    provider.headers = HashMap::from([
        ("anthropic-beta".to_owned(), "provider-beta".to_owned()),
        ("x-provider".to_owned(), "one".to_owned()),
    ]);
    let router = router_with(provider, 1);

    let forwarded = router
        .forward("p", messages_request("/messages", CLAUDE_CODE_BODY))
        .await
        .unwrap();

    let sent = &server.received_requests().await.unwrap()[0];
    assert_eq!(sent.body, CLAUDE_CODE_BODY.as_bytes());
    let header = |name: &str| {
        sent.headers
            .get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(header("anthropic-version"), ["2023-06-01"]);
    // The caller's header wins over the provider entry of the same name.
    assert_eq!(
        header("anthropic-beta"),
        ["context-management-2025-06-27,interleaved-thinking-2025-05-14"]
    );
    assert_eq!(header("x-provider"), ["one"]);
    assert_eq!(header("x-api-key"), ["test-key"]);
    assert!(header("authorization").is_empty());

    assert_eq!(forwarded.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&read_all(forwarded.body).await).unwrap();
    assert_eq!(body["content"][0]["text"], "Hi.");
    assert_eq!(
        forwarded.metered.await.unwrap(),
        Metered {
            model: Some("claude-sonnet-4-5-20250929".to_owned()),
            usage: Some(Usage {
                input_tokens: 40,
                output_tokens: 7,
                cache_read_input_tokens: 10,
                cache_write_input_tokens: 5,
                reasoning_tokens: 0,
            }),
            complete: true,
        }
    );
}

#[tokio::test]
async fn forward_relays_a_stream_unchanged_and_meters_its_events() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(stream_body(), "text/event-stream"))
        .mount(&server)
        .await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());

    let forwarded = router
        .forward("p", messages_request("/messages", CLAUDE_CODE_BODY))
        .await
        .unwrap();

    assert_eq!(read_all(forwarded.body).await, stream_body().as_bytes());
    assert_eq!(
        forwarded.metered.await.unwrap(),
        Metered {
            model: Some("claude-sonnet-4-5-20250929".to_owned()),
            usage: Some(Usage {
                input_tokens: 40,
                output_tokens: 7,
                cache_read_input_tokens: 10,
                cache_write_input_tokens: 5,
                reasoning_tokens: 0,
            }),
            complete: true,
        }
    );
}

#[tokio::test]
async fn forward_meters_no_usage_for_a_token_count() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages/count_tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 12})))
        .mount(&server)
        .await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());

    let forwarded = router
        .forward(
            "p",
            messages_request("/messages/count_tokens", CLAUDE_CODE_BODY),
        )
        .await
        .unwrap();

    assert_eq!(read_all(forwarded.body).await, br#"{"input_tokens":12}"#);
    assert_eq!(
        forwarded.metered.await.unwrap(),
        Metered {
            model: None,
            usage: None,
            complete: true,
        }
    );
}

#[tokio::test]
async fn forward_passes_an_error_unchanged_after_one_attempt() {
    let error = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Slow down"}}"#;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .insert_header("x-should-retry", "true")
                .set_body_raw(error, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The candidate loop would retry a rate limit three times.
    let router = router_with(
        ProviderConfig::new(ProtocolKind::AnthropicMessages, server.uri(), "test-key"),
        3,
    );

    let forwarded = router
        .forward("p", messages_request("/messages", CLAUDE_CODE_BODY))
        .await
        .unwrap();

    assert_eq!(forwarded.status, 429);
    assert_eq!(forwarded.headers["retry-after"], "7");
    assert_eq!(forwarded.headers["x-should-retry"], "true");
    assert_eq!(read_all(forwarded.body).await, error.as_bytes());
    assert_eq!(forwarded.metered.await.unwrap().usage, None);
}

/// A provider that sends the head of an event stream and `message_start`,
/// then holds the connection open.
async fn stalled_stream_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0_u8; 4096];
        // Read the head and the body of the request before the answer.
        while !request.ends_with(CLAUDE_CODE_BODY.as_bytes()) {
            let n = socket.read(&mut buf).await.unwrap();
            assert_ne!(n, 0, "the client closed before its request ended");
            request.extend_from_slice(&buf[..n]);
        }
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                    transfer-encoding: chunked\r\n\r\n";
        let chunk = format!("{:x}\r\n{MESSAGE_START}\r\n", MESSAGE_START.len());
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(chunk.as_bytes()).await.unwrap();
        // Hold the stream open until the client goes away.
        while socket.read(&mut buf).await.is_ok_and(|n| n > 0) {}
    });
    format!("http://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn forward_meters_the_usage_so_far_when_the_caller_drops_the_body() {
    let base_url = stalled_stream_server().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &base_url);

    let mut forwarded = router
        .forward("p", messages_request("/messages", CLAUDE_CODE_BODY))
        .await
        .unwrap();
    let mut seen = Vec::new();
    while !seen.ends_with(MESSAGE_START.as_bytes()) {
        let chunk = forwarded.body.next().await.unwrap().unwrap();
        seen.extend_from_slice(&chunk);
    }
    drop(forwarded.body);

    let metered = tokio::time::timeout(Duration::from_secs(5), forwarded.metered)
        .await
        .expect("the meter resolves when the caller drops the body")
        .unwrap();
    assert_eq!(
        metered,
        Metered {
            model: Some("claude-sonnet-4-5-20250929".to_owned()),
            usage: Some(Usage {
                input_tokens: 40,
                output_tokens: 1,
                cache_read_input_tokens: 10,
                cache_write_input_tokens: 5,
                reasoning_tokens: 0,
            }),
            complete: false,
        }
    );
}

#[tokio::test]
async fn forward_of_another_wire_is_unsupported() {
    let server = MockServer::start().await;
    let anthropic = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let openai = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());

    for (router, wire) in [
        (&anthropic, ProtocolKind::OpenAiChat),
        (&openai, ProtocolKind::OpenAiChat),
    ] {
        let request = ForwardRequest {
            wire,
            ..messages_request("/chat/completions", "{}")
        };
        let error = router.forward("p", request).await.err().unwrap();
        assert!(
            matches!(
                error,
                Error::Unsupported {
                    feature: "forward",
                    ..
                }
            ),
            "{error:?}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn forward_needs_a_path_under_the_base_url() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());

    let error = router
        .forward("p", messages_request("messages", "{}"))
        .await
        .err()
        .unwrap();

    assert!(matches!(error, Error::InvalidConfig(_)), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}
