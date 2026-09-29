//! Tests for the `anthropic-messages` codec against a mock server.

use crate::common;

use common::single_provider_router;
use futures::StreamExt;
use llm_router::{
    CachePolicy, ChatRequest, ContentPart, Error, ErrorKind, FinishReason, JsonSchemaFormat,
    Message, ProtocolKind, ReasoningConfig, StreamEvent, Tool, ToolCall, ToolChoice,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_1",
        "model": "concrete-model-2025",
        "content": [{"type": "text", "text": "Hi."}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 25,
            "output_tokens": 7,
            "cache_read_input_tokens": 10,
            "cache_creation_input_tokens": 5
        }
    }))
}

#[tokio::test]
async fn chat_translates_request_and_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let mut req = ChatRequest::new(
        "m",
        vec![
            Message::system("Be brief."),
            Message::system("Answer in English."),
            Message {
                content: vec![
                    ContentPart::Text {
                        text: "What is in this image?".into(),
                    },
                    ContentPart::ImageUrl {
                        url: "data:image/png;base64,QUJD".into(),
                    },
                ],
                ..Message::user("")
            },
        ],
    );
    req.tools = vec![Tool::function(
        "get_weather",
        "Get the weather",
        json!({"type": "object"}),
    )];
    req.tool_choice = Some(ToolChoice::Required);

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.provider, "p");
    assert_eq!(response.model, "concrete-model-2025");
    assert_eq!(response.message.text_content(), "Hi.");
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.native_finish_reason.as_deref(), Some("end_turn"));
    // Normalized input includes the cache buckets Anthropic reports separately.
    assert_eq!(response.usage.input_tokens, 40);
    assert_eq!(response.usage.output_tokens, 7);
    assert_eq!(response.usage.cache_read_input_tokens, 10);
    assert_eq!(response.usage.cache_write_input_tokens, 5);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    // System messages hoist to the top-level field and never appear in messages.
    assert_eq!(sent["system"], "Be brief.\n\nAnswer in English.");
    assert_eq!(sent["messages"].as_array().unwrap().len(), 1);
    assert_eq!(sent["messages"][0]["role"], "user");
    assert_eq!(sent["messages"][0]["content"][0]["type"], "text");
    assert_eq!(
        sent["messages"][0]["content"][1]["source"]["type"],
        "base64"
    );
    assert_eq!(
        sent["messages"][0]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(sent["messages"][0]["content"][1]["source"]["data"], "QUJD");
    // The Messages API requires max_tokens; the codec defaults it.
    assert_eq!(sent["max_tokens"], 4096);
    assert_eq!(sent["tools"][0]["name"], "get_weather");
    assert!(sent["tools"][0].get("input_schema").is_some());
    assert_eq!(sent["tool_choice"]["type"], "any");
}

#[tokio::test]
async fn chat_requests_strict_structured_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("Extract claims")]);
    req.output_schema = Some(JsonSchemaFormat {
        name: "knowledge_proposal".into(),
        description: Some("Claims supported by the supplied sources".into()),
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
        sent["output_config"]["format"],
        json!({
            "type": "json_schema",
            "schema": {
                "type": "object",
                "properties": {"claims": {"type": "array"}},
                "required": ["claims"],
                "additionalProperties": false
            }
        })
    );
}

#[tokio::test]
async fn tool_calls_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "content": [
                {"type": "text", "text": "Let me check."},
                {"type": "tool_use", "id": "toolu_1", "name": "get_weather",
                 "input": {"city": "Paris"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 3}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());

    // Decode: tool_use blocks normalize to tool_calls.
    let req = ChatRequest::new("m", vec![Message::user("Weather in Paris?")]);
    let response = router.chat(&req).await.unwrap();
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(response.message.text_content(), "Let me check.");
    assert_eq!(
        response.message.tool_calls,
        vec![ToolCall {
            id: "toolu_1".into(),
            name: "get_weather".into(),
            arguments: "{\"city\":\"Paris\"}".into(),
        }]
    );

    // Encode: the assistant turn regains its tool_use block, and the tool
    // result becomes a tool_result block in a user message.
    let req = ChatRequest::new(
        "m",
        vec![
            Message::user("Weather in Paris?"),
            response.message.clone(),
            Message::tool("toolu_1", "Sunny, 21C"),
        ],
    );
    router.chat(&req).await.unwrap();
    let sent: Value = server.received_requests().await.unwrap()[1]
        .body_json()
        .unwrap();
    let messages = sent["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["text"], "Let me check.");
    assert_eq!(messages[1]["content"][1]["type"], "tool_use");
    assert_eq!(messages[1]["content"][1]["input"]["city"], "Paris");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "toolu_1");
    // Tool results carry content blocks, so tools can return images too.
    assert_eq!(messages[2]["content"][0]["content"][0]["type"], "text");
    assert_eq!(
        messages[2]["content"][0]["content"][0]["text"],
        "Sunny, 21C"
    );
}

#[tokio::test]
async fn chat_stream_emits_normalized_events() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5}}}\n\n",
        "event: ping\n",
        "data: {\"type\":\"ping\"}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let stream = router.chat_stream(&req).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events[..3],
        [
            StreamEvent::TextDelta { text: "Hi".into() },
            StreamEvent::ToolCallStart {
                index: 1,
                id: "toolu_1".into(),
                name: "get_weather".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 1,
                arguments: "{\"city\":\"Paris\"}".into(),
            },
        ]
    );
    match &events[3] {
        StreamEvent::Finish {
            reason: FinishReason::ToolCalls,
            native_reason,
            usage: Some(usage),
        } => {
            assert_eq!(native_reason.as_deref(), Some("tool_use"));
            assert_eq!(usage.input_tokens, 40);
            assert_eq!(usage.output_tokens, 7);
            assert_eq!(usage.cache_read_input_tokens, 10);
            assert_eq!(usage.cache_write_input_tokens, 5);
        }
        other => panic!("unexpected final event: {other:?}"),
    }
    assert_eq!(events.len(), 4);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["stream"], true);
}

#[tokio::test]
async fn provider_errors_normalize_to_kinds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(529).set_body_json(json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let error = router.chat(&req).await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    let Error::Provider { kind, message, .. } = *last else {
        panic!("expected Provider error");
    };
    assert_eq!(kind, ErrorKind::Overloaded);
    assert_eq!(message, "Overloaded");
}

#[tokio::test]
async fn reasoning_maps_to_thinking_and_round_trips_signatures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "content": [
                {"type": "thinking", "thinking": "Basic arithmetic.", "signature": "sig123"},
                {"type": "text", "text": "4"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("2+2?")]);
    req.temperature = Some(0.2);
    req.reasoning = Some(ReasoningConfig {
        effort: None,
        max_tokens: Some(2048),
    });
    let response = router.chat(&req).await.unwrap();

    assert_eq!(
        response.message.content[0],
        ContentPart::Reasoning {
            text: "Basic arithmetic.".into(),
            signature: Some("sig123".into()),
        }
    );

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["thinking"]["type"], "enabled");
    assert_eq!(sent["thinking"]["budget_tokens"], 2048);
    // max_tokens grows past the budget; sampling params drop with thinking on.
    assert_eq!(sent["max_tokens"], 2048 + 4096);
    assert!(sent.get("temperature").is_none());

    // The reasoning part encodes back as a signed thinking block.
    let mut follow_up = ChatRequest::new(
        "m",
        vec![
            Message::user("2+2?"),
            response.message.clone(),
            Message::user("and 3+3?"),
        ],
    );
    follow_up.reasoning = Some(ReasoningConfig::default());
    router.chat(&follow_up).await.unwrap();
    let sent: Value = server.received_requests().await.unwrap()[1]
        .body_json()
        .unwrap();
    assert_eq!(sent["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(
        sent["messages"][1]["content"][0]["thinking"],
        "Basic arithmetic."
    );
    assert_eq!(sent["messages"][1]["content"][0]["signature"], "sig123");
    // Effort-only reasoning converts to a default budget.
    assert_eq!(sent["thinking"]["budget_tokens"], 4096);
}

#[tokio::test]
async fn auto_cache_marks_system_last_tool_and_last_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok_response())
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::system("Be brief."), Message::user("hi")]);
    req.tools = vec![
        Tool::function("a", "", json!({"type": "object"})),
        Tool::function("b", "", json!({"type": "object"})),
    ];
    req.cache = CachePolicy::Auto;
    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    let ephemeral = json!({"type": "ephemeral"});
    assert_eq!(sent["system"][0]["cache_control"], ephemeral);
    assert!(sent["tools"][0].get("cache_control").is_none());
    assert_eq!(sent["tools"][1]["cache_control"], ephemeral);
    let last_message = sent["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        last_message["content"].as_array().unwrap().last().unwrap()["cache_control"],
        ephemeral
    );
}

#[tokio::test]
async fn stream_emits_reasoning_deltas_and_signature() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig123\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"4\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let req = ChatRequest::new("m", vec![Message::user("2+2?")]);
    let stream = router.chat_stream(&req).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events[..3],
        [
            StreamEvent::ReasoningDelta { text: "hmm".into() },
            StreamEvent::ReasoningEnd {
                signature: Some("sig123".into())
            },
            StreamEvent::TextDelta { text: "4".into() },
        ]
    );
    assert!(matches!(events[3], StreamEvent::Finish { .. }));
}

#[tokio::test]
async fn tool_results_can_carry_images_and_streams_forward_redacted_reasoning() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"opaque123\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());

    // A tool message with text and an image encodes both blocks.
    let mut tool_result = Message::tool("toolu_1", "screenshot taken");
    tool_result.content.push(ContentPart::ImageUrl {
        url: "data:image/png;base64,QUJD".into(),
    });
    let req = ChatRequest::new("m", vec![Message::user("take a screenshot"), tool_result]);
    let stream = router.chat_stream(&req).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;
    assert_eq!(
        events[0],
        StreamEvent::RedactedReasoning {
            data: "opaque123".into()
        }
    );

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    // The user text and the tool message merge into one wire user message;
    // the tool_result block is its second content block.
    let tool_result = &sent["messages"][0]["content"][1];
    assert_eq!(tool_result["type"], "tool_result");
    let blocks = &tool_result["content"];
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["source"]["type"], "base64");
}
