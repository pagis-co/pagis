//! Tests for alias resolution and the ordered fallback loop.

use std::time::Duration;

use llm_router::{
    Candidate, ChatRequest, Error, ErrorKind, Message, ProtocolKind, ProviderConfig, RetryConfig,
    Router, RouterConfig, TimeoutConfig, Tool,
};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn openrouter_uses_its_responses_endpoint() {
    let provider = ProviderConfig::openrouter("sk-openrouter");

    assert_eq!(provider.protocol, ProtocolKind::OpenAiResponses);
    assert_eq!(provider.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(provider.api_key, "sk-openrouter");
}

/// One attempt per candidate and millisecond backoff, so fallback tests stay
/// deterministic and fast.
fn no_retry() -> RetryConfig {
    RetryConfig {
        max_attempts: 1,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
    }
}

fn ok_response(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "concrete-model",
        "choices": [{
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    }))
}

fn two_provider_config(primary: &MockServer, secondary: &MockServer) -> RouterConfig {
    RouterConfig::new()
        .provider(
            "primary",
            ProviderConfig::new(ProtocolKind::OpenAiChat, primary.uri(), "k1"),
        )
        .provider(
            "secondary",
            ProviderConfig::new(ProtocolKind::OpenAiChat, secondary.uri(), "k2"),
        )
        .model(
            "m",
            [
                Candidate::new("primary", "model-a"),
                Candidate::new("secondary", "model-b"),
            ],
        )
        .retry(no_retry())
}

#[tokio::test]
async fn falls_back_on_retryable_error() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {"message": "internal error"}
        })))
        .expect(1)
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .respond_with(ok_response("from secondary"))
        .expect(1)
        .mount(&secondary)
        .await;

    let router = Router::new(two_provider_config(&primary, &secondary)).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.provider, "secondary");
    assert_eq!(response.message.text_content(), "from secondary");
}

#[tokio::test]
async fn a_400_fails_at_once() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"message": "bad request", "type": "invalid_request_error"}
        })))
        .expect(1)
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .respond_with(ok_response("never"))
        .expect(0)
        .mount(&secondary)
        .await;

    let config = two_provider_config(&primary, &secondary).retry(RetryConfig {
        max_attempts: 3,
        initial_backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
    });
    let router = Router::new(config).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let error = router.chat(&req).await.unwrap_err();

    let Error::Provider { kind, .. } = error else {
        panic!("expected Provider error, got: {error:?}");
    };
    assert_eq!(kind, ErrorKind::InvalidRequest);
}

#[tokio::test]
async fn exhausts_all_candidates() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    for server in [&primary, &secondary] {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({
                "error": {"message": "overloaded"}
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    let router = Router::new(two_provider_config(&primary, &secondary)).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let error = router.chat(&req).await.unwrap_err();

    let Error::Exhausted { model, last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert_eq!(model, "m");
    let Error::Provider { provider, kind, .. } = *last else {
        panic!("expected Provider error");
    };
    assert_eq!(provider, "secondary");
    assert_eq!(kind, ErrorKind::Overloaded);
}

#[tokio::test]
async fn exhausted_error_displays_the_last_provider_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"message": "You have no credits remaining"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = RouterConfig::new()
        .provider(
            "openai",
            ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "key"),
        )
        .model("default", [Candidate::new("openai", "model")])
        .retry(no_retry());
    let router = Router::new(config).unwrap();
    let request = ChatRequest::new("default", vec![Message::user("hello")]);

    let error = router.chat(&request).await.unwrap_err();

    assert_eq!(
        error.to_string(),
        "all candidates for model `default` failed: provider `openai` returned status 429 \
         (RateLimit): You have no credits remaining"
    );
}

#[tokio::test]
async fn resolves_direct_provider_model_ids() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_response("direct"))
        .mount(&server)
        .await;

    let config = RouterConfig::new().provider(
        "openai",
        ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k"),
    );
    let router = Router::new(config).unwrap();
    let req = ChatRequest::new("openai/gpt-test", vec![Message::user("hi")]);
    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.provider, "openai");
    let sent: serde_json::Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "gpt-test");
}

#[tokio::test]
async fn two_rate_limits_then_success_use_one_candidate_and_report_two_retries() {
    use std::sync::{Arc, Mutex};

    let server = MockServer::start().await;
    // The first two requests hit a rate limit; the third succeeds.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"message": "slow down"}
        })))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ok_response("third try"))
        .expect(1)
        .mount(&server)
        .await;

    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "model-a")])
        .retry(RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        });
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&attempts);
    let router = Router::new(config).unwrap().on_attempt(move |attempt| {
        seen.lock()
            .expect("attempt lock")
            .push(attempt.error.is_some());
    });
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.message.text_content(), "third try");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(*attempts.lock().expect("attempt lock"), [true, true, false]);
}

#[test]
fn default_retry_budget_makes_three_attempts_over_about_thirty_seconds() {
    let retry = RetryConfig::default();

    assert_eq!(retry.max_attempts, 3);
    assert_eq!(retry.initial_backoff, Duration::from_secs(10));
    assert_eq!(retry.max_backoff, Duration::from_secs(20));
}

#[tokio::test]
async fn stream_falls_back_when_first_event_times_out() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sse, "text/event-stream")
                .set_delay(Duration::from_secs(5)),
        )
        .expect(1)
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .expect(1)
        .mount(&secondary)
        .await;

    let config = two_provider_config(&primary, &secondary).timeouts(TimeoutConfig {
        first_event: Duration::from_millis(200),
        ..TimeoutConfig::default()
    });
    let router = Router::new(config).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let stream = router.chat_stream(&req).await.unwrap();

    assert_eq!(stream.provider, "secondary");
}

#[tokio::test]
async fn rejects_unknown_model_alias() {
    let router = Router::new(RouterConfig::new()).unwrap();
    let req = ChatRequest::new("nope", vec![Message::user("hi")]);
    let error = router.chat(&req).await.unwrap_err();
    assert!(matches!(error, Error::UnknownModel(m) if m == "nope"));
}

#[test]
fn rejects_plain_http_to_non_local_hosts() {
    let config = RouterConfig::new().provider(
        "p",
        ProviderConfig::new(ProtocolKind::OpenAiChat, "http://api.openai.com/v1", "k"),
    );
    let Err(error) = Router::new(config) else {
        panic!("expected InvalidConfig");
    };
    assert!(matches!(error, Error::InvalidConfig(_)));

    // Loopback hosts stay allowed for local servers.
    for base in [
        "http://localhost:11434/v1",
        "http://127.0.0.1:8080/v1",
        "http://[::1]:8080/v1",
    ] {
        let config = RouterConfig::new().provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiChat, base, "ollama"),
        );
        assert!(Router::new(config).is_ok(), "expected {base} to be allowed");
    }
}

#[test]
fn api_keys_never_appear_in_debug_or_serialized_config() {
    let config = RouterConfig::new().provider(
        "p",
        ProviderConfig::new(
            ProtocolKind::OpenAiChat,
            "https://api.openai.com/v1",
            "sk-secret",
        ),
    );
    assert!(!format!("{config:?}").contains("sk-secret"));
    assert!(
        !serde_json::to_string(&config)
            .unwrap()
            .contains("sk-secret")
    );
}

#[tokio::test]
async fn attempt_hook_sees_absorbed_failures() {
    use std::sync::{Arc, Mutex};

    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {"message": "boom"}
        })))
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .respond_with(ok_response("ok"))
        .mount(&secondary)
        .await;

    let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let router = Router::new(two_provider_config(&primary, &secondary))
        .unwrap()
        .on_attempt(move |info| {
            sink.lock()
                .unwrap()
                .push((info.provider.to_owned(), info.error.is_some()));
        });
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    router.chat(&req).await.unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("primary".to_owned(), true),
            ("secondary".to_owned(), false)
        ]
    );
}

#[tokio::test]
async fn a_stream_that_errors_before_its_first_event_falls_back() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    // The primary opens the stream, then sends garbage as its first event.
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw("data: not-json\n\n", "text/event-stream"),
        )
        .expect(1)
        .mount(&primary)
        .await;
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .expect(1)
        .mount(&secondary)
        .await;

    let router = Router::new(two_provider_config(&primary, &secondary)).unwrap();
    let stream = router
        .chat_stream(&ChatRequest::new("m", vec![Message::user("hi")]))
        .await
        .unwrap();
    // Nothing was delivered by the primary, so the router fell back.
    assert_eq!(stream.provider, "secondary");
}

/// OpenAI's Responses stream sends its reasoning item before any text or
/// tool call, and can fail after it with a `server_error`. Nothing a
/// caller acts on arrived, so the router retries the same candidate, and
/// the caller never sees the failed attempt's reasoning.
#[tokio::test]
async fn a_stream_that_fails_before_its_first_output_retries() {
    use futures::StreamExt as _;
    let server = MockServer::start().await;
    let failed = concat!(
        "data: {\"type\":\"response.created\",\"response\":{}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_failed\",\"encrypted_content\":\"abc\"}}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"An error occurred while processing your request.\"}}}\n\n",
    );
    let ok = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(failed, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(ok, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiResponses, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "model-a")])
        .retry(RetryConfig {
            max_attempts: 2,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        });

    let stream = Router::new(config)
        .unwrap()
        .chat_stream(&ChatRequest::new("m", vec![Message::user("hi")]))
        .await
        .unwrap();

    assert_eq!(stream.retries, 1);
    let events: Vec<_> = stream.events.collect().await;
    let debug = format!("{events:?}");
    assert!(events.iter().all(Result::is_ok), "{debug}");
    assert!(debug.contains("hi"), "{debug}");
    assert!(!debug.contains("rs_failed"), "{debug}");
}

/// OpenAI's Responses stream can fail with a `server_error` after it
/// opens a computer call and before the call is complete. A caller acts
/// on a tool call only when the stream finishes, so nothing it acts on
/// arrived, and the router retries the same candidate.
#[tokio::test]
async fn a_stream_that_fails_inside_a_tool_call_retries() {
    use futures::StreamExt as _;
    let server = MockServer::start().await;
    let failed = concat!(
        "data: {\"type\":\"response.created\",\"response\":{}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_failed\",\"encrypted_content\":\"abc\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"computer_call\",\"call_id\":\"call_failed\"}}\n\n",
        "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"An error occurred while processing your request.\"}\n\n",
    );
    let ok = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"call_id\":\"call_ok\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"call_id\":\"call_ok\",\"actions\":[{\"type\":\"screenshot\"}]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(failed, "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(ok, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiResponses, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "model-a")])
        .retry(RetryConfig {
            max_attempts: 2,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        });

    let stream = Router::new(config)
        .unwrap()
        .chat_stream(&ChatRequest::new("m", vec![Message::user("hi")]))
        .await
        .unwrap();

    assert_eq!(stream.retries, 1);
    let events: Vec<_> = stream.events.collect().await;
    let debug = format!("{events:?}");
    assert!(events.iter().all(Result::is_ok), "{debug}");
    assert!(debug.contains("call_ok"), "{debug}");
    assert!(!debug.contains("call_failed"), "{debug}");
    assert!(!debug.contains("rs_failed"), "{debug}");
}

/// Text streams to the caller as it arrives, so a stream that fails
/// after its first text does not retry: the caller already showed it.
#[tokio::test]
async fn a_stream_that_fails_after_its_first_text_does_not_retry() {
    use futures::StreamExt as _;
    let server = MockServer::start().await;
    let failed = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"An error occurred while processing your request.\"}\n\n",
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(failed, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiResponses, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "model-a")])
        .retry(RetryConfig {
            max_attempts: 2,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        });

    let stream = Router::new(config)
        .unwrap()
        .chat_stream(&ChatRequest::new("m", vec![Message::user("hi")]))
        .await
        .unwrap();

    assert_eq!(stream.retries, 0);
    let events: Vec<_> = stream.events.collect().await;
    assert!(matches!(events.first(), Some(Ok(_))), "{events:?}");
    assert!(matches!(events.last(), Some(Err(_))), "{events:?}");
}

/// An error event about the request itself is not retried: the same
/// request fails the same way.
#[tokio::test]
async fn a_stream_that_refuses_the_request_does_not_retry() {
    let server = MockServer::start().await;
    let refused = concat!(
        "data: {\"type\":\"response.created\",\"response\":{}}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_prompt\",\"message\":\"Invalid prompt.\"}}}\n\n",
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(refused, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiResponses, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "model-a")])
        .retry(RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
        });

    let result = Router::new(config)
        .unwrap()
        .chat_stream(&ChatRequest::new("m", vec![Message::user("hi")]))
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn a_fallback_candidate_gets_only_its_own_provider_tools() {
    let primary = MockServer::start().await;
    let secondary = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "type": "error",
            "error": {"type": "api_error", "message": "overloaded"}
        })))
        .expect(1)
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .respond_with(ok_response("from secondary"))
        .expect(1)
        .mount(&secondary)
        .await;

    let config = RouterConfig::new()
        .provider(
            "primary",
            ProviderConfig::new(ProtocolKind::AnthropicMessages, primary.uri(), "k1"),
        )
        .provider(
            "secondary",
            ProviderConfig::new(ProtocolKind::OpenAiChat, secondary.uri(), "k2"),
        )
        .model(
            "m",
            [
                Candidate::new("primary", "model-a"),
                Candidate::new("secondary", "model-b"),
            ],
        )
        .retry(no_retry());
    let router = Router::new(config).unwrap();
    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.tools = vec![
        Tool::provider_defined("computer_20251124", "computer", serde_json::Map::new())
            .for_candidate("primary", "model-a"),
        Tool::function("get_weather", "Get the weather", json!({"type": "object"})),
    ];

    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.provider, "secondary");
    let primary_sent: serde_json::Value = primary.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(primary_sent["tools"][0]["type"], "computer_20251124");
    let secondary_sent: serde_json::Value = secondary.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    let tools = secondary_sent["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "{secondary_sent}");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
}
