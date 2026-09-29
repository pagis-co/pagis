//! Tests for the `openai-chat` codec against a mock server.

use crate::common;

use common::single_provider_router;
use futures::StreamExt;
use llm_router::{
    Candidate, ChatRequest, CompatConfig, ContentPart, Error, ErrorKind, FinishReason,
    JsonSchemaFormat, MaxTokensField, Message, ProtocolKind, ProviderConfig, ReasoningConfig,
    ReasoningEffort, Router, RouterConfig, StreamEvent, Tool, ToolCall, ToolChoice,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request_with_everything() -> ChatRequest {
    let mut req = ChatRequest::new(
        "m",
        vec![
            Message::system("Be brief."),
            Message {
                content: vec![
                    ContentPart::Text {
                        text: "What is in this image?".into(),
                    },
                    ContentPart::ImageUrl {
                        url: "https://example.com/cat.png".into(),
                    },
                ],
                ..Message::user("")
            },
        ],
    );
    req.temperature = Some(0.5);
    req.max_tokens = Some(100);
    req.tools = vec![Tool::function(
        "get_weather",
        "Get the weather",
        json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )];
    req.tool_choice = Some(ToolChoice::Auto);
    req
}

#[tokio::test]
async fn chat_translates_request_and_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "model": "concrete-model-2025",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "A cat."},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 8,
                "prompt_tokens_details": {"cached_tokens": 12},
                "completion_tokens_details": {"reasoning_tokens": 3}
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let response = router.chat(&request_with_everything()).await.unwrap();

    assert_eq!(response.provider, "p");
    assert_eq!(response.model, "concrete-model-2025");
    assert_eq!(response.message.text_content(), "A cat.");
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.native_finish_reason.as_deref(), Some("stop"));
    assert_eq!(response.usage.input_tokens, 20);
    assert_eq!(response.usage.output_tokens, 8);
    assert_eq!(response.usage.cache_read_input_tokens, 12);
    assert_eq!(response.usage.reasoning_tokens, 3);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "concrete-model");
    assert_eq!(sent["temperature"], 0.5);
    assert_eq!(sent["max_tokens"], 100);
    assert_eq!(sent["tool_choice"], "auto");
    assert_eq!(sent["messages"][0]["role"], "system");
    assert_eq!(sent["messages"][0]["content"], "Be brief.");
    assert_eq!(sent["messages"][1]["content"][0]["type"], "text");
    assert_eq!(
        sent["messages"][1]["content"][1]["image_url"]["url"],
        "https://example.com/cat.png"
    );
    assert_eq!(sent["tools"][0]["function"]["name"], "get_weather");
    assert!(sent.get("stream").is_none());
}

#[tokio::test]
async fn chat_round_trips_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());

    // Decode: a tool-call response normalizes to `tool_calls` on the message.
    let req = ChatRequest::new("m", vec![Message::user("Weather in Paris?")]);
    let response = router.chat(&req).await.unwrap();
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(
        response.message.tool_calls,
        vec![ToolCall {
            id: "call_1".into(),
            name: "get_weather".into(),
            arguments: "{\"city\":\"Paris\"}".into(),
        }]
    );

    // Encode: assistant tool calls and the tool result round-trip.
    let req = ChatRequest::new(
        "m",
        vec![
            Message::user("Weather in Paris?"),
            response.message.clone(),
            Message::tool("call_1", "Sunny, 21C"),
        ],
    );
    router.chat(&req).await.unwrap();
    let sent: Value = server.received_requests().await.unwrap()[1]
        .body_json()
        .unwrap();
    assert_eq!(sent["messages"][1]["role"], "assistant");
    assert_eq!(sent["messages"][1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        sent["messages"][1]["tool_calls"][0]["function"]["arguments"],
        "{\"city\":\"Paris\"}"
    );
    assert_eq!(sent["messages"][2]["role"], "tool");
    assert_eq!(sent["messages"][2]["tool_call_id"], "call_1");
    assert_eq!(sent["messages"][2]["content"], "Sunny, 21C");
}

#[tokio::test]
async fn chat_stream_emits_normalized_events() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let stream = router.chat_stream(&req).await.unwrap();
    assert_eq!(stream.provider, "p");
    assert_eq!(stream.model, "concrete-model");

    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;
    let usage = match events.last() {
        Some(StreamEvent::Finish {
            reason: FinishReason::ToolCalls,
            native_reason,
            usage: Some(usage),
        }) => {
            assert_eq!(native_reason.as_deref(), Some("tool_calls"));
            *usage
        }
        other => panic!("unexpected final event: {other:?}"),
    };
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(
        events[..4],
        [
            StreamEvent::TextDelta { text: "Hel".into() },
            StreamEvent::TextDelta { text: "lo".into() },
            StreamEvent::ToolCallStart {
                index: 0,
                id: "call_1".into(),
                name: "get_weather".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                arguments: "{\"city\":\"Paris\"}".into(),
            },
        ]
    );
    assert_eq!(events.len(), 5);

    // The stream request asks for usage in the final chunk.
    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn provider_errors_normalize_to_kinds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"message": "Rate limit reached", "type": "requests", "code": "rate_limit_exceeded"}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let error = router.chat(&req).await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    let Error::Provider {
        provider,
        status,
        kind,
        message,
        raw,
    } = *last
    else {
        panic!("expected Provider error");
    };
    assert_eq!(provider, "p");
    assert_eq!(status, 429);
    assert_eq!(kind, ErrorKind::RateLimit);
    assert_eq!(message, "Rate limit reached");
    assert!(raw.is_some());
}

#[tokio::test]
async fn reasoning_maps_to_effort_and_decodes_reasoning_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "4",
                    "reasoning_content": "2 + 2 is basic arithmetic."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("2+2?")]);
    req.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::High),
        max_tokens: None,
    });
    let response = router.chat(&req).await.unwrap();

    assert_eq!(
        response.message.content[0],
        ContentPart::Reasoning {
            text: "2 + 2 is basic arithmetic.".into(),
            signature: None,
        }
    );
    assert_eq!(response.message.text_content(), "4");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["reasoning_effort"], "high");
}

#[tokio::test]
async fn stream_emits_reasoning_deltas() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking...\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"4\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("2+2?")]);
    let stream = router.chat_stream(&req).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events[0],
        StreamEvent::ReasoningDelta {
            text: "thinking...".into()
        }
    );
    assert_eq!(events[1], StreamEvent::TextDelta { text: "4".into() });
}

#[tokio::test]
async fn compat_flags_control_injected_params() {
    let server = MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    // A strict server (Mistral-style): no stream_options, no reasoning_effort;
    // an OpenAI-style output cap field.
    let mut provider = ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k");
    provider.compat = CompatConfig {
        stream_options: false,
        max_tokens_field: MaxTokensField::MaxCompletionTokens,
        reasoning_effort: false,
        ..CompatConfig::default()
    };
    let config = RouterConfig::new()
        .provider("p", provider)
        .model("m", [Candidate::new("p", "concrete-model")]);
    let router = Router::new(config).unwrap();

    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.max_tokens = Some(50);
    req.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::Low),
        max_tokens: None,
    });
    router.chat_stream(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["stream"], true);
    assert!(sent.get("stream_options").is_none());
    assert!(sent.get("reasoning_effort").is_none());
    assert!(sent.get("max_tokens").is_none());
    assert_eq!(sent["max_completion_tokens"], 50);
}

#[tokio::test]
async fn decodes_reasoning_from_the_reasoning_field_variant() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {"role": "assistant", "content": "4", "reasoning": "arithmetic"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("2+2?")]);
    let response = router.chat(&req).await.unwrap();
    assert_eq!(
        response.message.content[0],
        ContentPart::Reasoning {
            text: "arithmetic".into(),
            signature: None,
        }
    );
}

#[tokio::test]
async fn synthesizes_tool_call_indexes_for_servers_that_repeat_index_zero() {
    // Ollama-style: two parallel tool calls, both streamed with index 0.
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"f1\",\"arguments\":\"{\\\"x\\\":1}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"b\",\"function\":{\"name\":\"f2\",\"arguments\":\"{\\\"y\\\":2}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let stream = router.chat_stream(&req).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events[..4],
        [
            StreamEvent::ToolCallStart {
                index: 0,
                id: "a".into(),
                name: "f1".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                arguments: "{\"x\":1}".into(),
            },
            StreamEvent::ToolCallStart {
                index: 1,
                id: "b".into(),
                name: "f2".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 1,
                arguments: "{\"y\":2}".into(),
            },
        ]
    );
}

#[tokio::test]
async fn extra_params_and_provider_headers_pass_through() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-custom", "yes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {"role": "assistant", "content": "{}"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut provider = ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k");
    provider
        .headers
        .insert("x-custom".to_owned(), "yes".to_owned());
    let config = RouterConfig::new()
        .provider("p", provider)
        .model("m", [Candidate::new("p", "concrete-model")]);
    let router = Router::new(config).unwrap();

    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.extra
        .insert("response_format".to_owned(), json!({"type": "json_object"}));
    req.extra.insert("seed".to_owned(), json!(42));
    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["response_format"]["type"], "json_object");
    assert_eq!(sent["seed"], 42);
}

#[tokio::test]
async fn chat_completions_requests_strict_structured_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {"role": "assistant", "content": "{\"claims\":[]}"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("Extract claims")]);
    req.output_schema = Some(JsonSchemaFormat {
        name: "knowledge_proposal".into(),
        description: None,
        schema: json!({
            "type": "object",
            "properties": {"claims": {"type": "array"}},
            "required": ["claims"],
            "additionalProperties": false
        }),
    });

    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(
        sent["response_format"],
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "knowledge_proposal",
                "schema": {
                    "type": "object",
                    "properties": {"claims": {"type": "array"}},
                    "required": ["claims"],
                    "additionalProperties": false
                },
                "strict": true
            }
        })
    );
}
