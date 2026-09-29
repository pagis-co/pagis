//! Tests for computer use: provider-defined tools on anthropic-messages and
//! the openai-responses protocol.

use crate::common;

use common::single_provider_router;
use futures::StreamExt;
use llm_router::{
    ChatRequest, ContentPart, Error, FinishReason, JsonSchemaFormat, Message, ProtocolKind, Role,
    StreamEvent, Tool, ToolCall,
};
use serde_json::{Map, Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn computer_tool(kind: &str) -> Tool {
    let mut config = Map::new();
    config.insert("display_width_px".into(), json!(1024));
    config.insert("display_height_px".into(), json!(768));
    Tool::provider_defined(kind, "computer", config)
}

#[tokio::test]
async fn typed_tools_pass_through_on_anthropic_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "computer",
                         "input": {"action": "screenshot"}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("Open the browser")]);
    req.tools = vec![
        computer_tool("computer_20251124"),
        Tool::provider_defined("bash_20250124", "bash", Map::new()),
        Tool::function("get_weather", "Get the weather", json!({"type": "object"})),
    ];
    let response = router.chat(&req).await.unwrap();
    assert_eq!(response.message.tool_calls[0].name, "computer");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(
        sent["tools"][0],
        json!({
            "type": "computer_20251124",
            "name": "computer",
            "display_width_px": 1024,
            "display_height_px": 768,
        })
    );
    assert_eq!(
        sent["tools"][1],
        json!({"type": "bash_20250124", "name": "bash"})
    );
    // Function tools keep their schema form alongside typed tools.
    assert_eq!(sent["tools"][2]["input_schema"], json!({"type": "object"}));
}

#[tokio::test]
async fn typed_tools_are_unsupported_on_chat_completions() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.tools = vec![computer_tool("computer")];
    let error = router.chat(&req).await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(
        matches!(*last, Error::Unsupported { feature, .. } if feature == "provider-defined tools")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn chat_round_trips_on_responses_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_1",
            "status": "completed",
            "model": "concrete-model",
            "output": [
                {"type": "reasoning", "id": "rs_1",
                 "summary": [{"type": "summary_text", "text": "thinking"}],
                 "encrypted_content": "opaque"},
                {"type": "message", "role": "assistant", "status": "completed",
                 "content": [{"type": "output_text", "text": "Sunny."}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                 "name": "get_weather", "arguments": "{\"city\":\"SF\"}"}
            ],
            "usage": {"input_tokens": 20, "output_tokens": 10,
                      "input_tokens_details": {"cached_tokens": 5},
                      "output_tokens_details": {"reasoning_tokens": 3}}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let mut req = ChatRequest::new(
        "m",
        vec![Message::system("Be brief."), Message::user("Weather?")],
    );
    req.max_tokens = Some(200);
    req.tools = vec![Tool::function(
        "get_weather",
        "Get the weather",
        json!({"type": "object"}),
    )];
    let response = router.chat(&req).await.unwrap();

    assert_eq!(response.message.text_content(), "Sunny.");
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(response.message.tool_calls[0].id, "call_1");
    assert_eq!(response.usage.cache_read_input_tokens, 5);
    assert_eq!(response.usage.reasoning_tokens, 3);
    // The reasoning item is kept whole for stateless replay.
    assert!(response.message.content.iter().any(|p| matches!(
        p,
        ContentPart::RedactedReasoning { data } if data.contains("rs_1")
    )));

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["instructions"], "Be brief.");
    assert_eq!(sent["store"], false);
    // Stateless reasoning replay needs encrypted content on every request.
    assert_eq!(sent["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(sent["max_output_tokens"], 200);
    assert_eq!(
        sent["input"][0],
        json!({"role": "user", "content": [{"type": "input_text", "text": "Weather?"}]})
    );
    // Function tools are flat on this API.
    assert_eq!(sent["tools"][0]["type"], "function");
    assert_eq!(sent["tools"][0]["name"], "get_weather");
}

#[tokio::test]
async fn responses_requests_strict_structured_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_structured",
            "status": "completed",
            "model": "concrete-model",
            "output": [{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": "{\"claims\":[]}"}]
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
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
        sent["text"]["format"],
        json!({
            "type": "json_schema",
            "name": "knowledge_proposal",
            "description": "Claims supported by the supplied sources",
            "schema": {
                "type": "object",
                "properties": {"claims": {"type": "array"}},
                "required": ["claims"],
                "additionalProperties": false
            },
            "strict": true
        })
    );
}

#[tokio::test]
async fn computer_use_round_trips_on_responses_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_2",
            "status": "completed",
            "output": [
                {"type": "computer_call", "id": "cu_2", "call_id": "call_2",
                 "actions": [{"type": "click", "button": "left", "x": 100, "y": 200}],
                 "status": "completed"}
            ],
            "usage": {"input_tokens": 30, "output_tokens": 8}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());

    // The transcript carries a prior computer call and its screenshot result.
    let prior_call_item = json!({
        "type": "computer_call", "id": "cu_1", "call_id": "call_1",
        "actions": [{"type": "screenshot"}],
        "pending_safety_checks": [{"id": "sc_1", "code": "malicious_instructions",
                                   "message": "check this"}],
        "status": "completed"
    });
    let mut req = ChatRequest::new(
        "m",
        vec![
            Message::user("Open the browser"),
            Message {
                role: Role::Assistant,
                content: vec![],
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "computer".into(),
                    arguments: prior_call_item.to_string(),
                }],
                tool_call_id: None,
            },
            Message {
                role: Role::Tool,
                content: vec![ContentPart::ImageUrl {
                    url: "data:image/png;base64,QUJD".into(),
                }],
                tool_calls: vec![],
                tool_call_id: Some("call_1".into()),
            },
        ],
    );
    req.tools = vec![computer_tool("computer")];
    let response = router.chat(&req).await.unwrap();

    // The new computer call arrives as a tool call carrying the whole item.
    let call = &response.message.tool_calls[0];
    assert_eq!(call.name, "computer");
    assert_eq!(call.id, "call_2");
    let item: Value = serde_json::from_str(&call.arguments).unwrap();
    assert_eq!(item["actions"][0]["type"], "click");
    assert_eq!(response.finish_reason, FinishReason::ToolCalls);

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    // The typed tool passed through with its config.
    assert_eq!(sent["tools"][0]["type"], "computer");
    assert_eq!(sent["tools"][0]["display_width_px"], 1024);
    // The prior computer_call item replayed verbatim.
    assert_eq!(sent["input"][1], prior_call_item);
    // Its result encoded as a computer screenshot, and replying
    // acknowledged the call's pending safety checks.
    assert_eq!(
        sent["input"][2],
        json!({
            "type": "computer_call_output",
            "call_id": "call_1",
            "output": {"type": "computer_screenshot",
                       "image_url": "data:image/png;base64,QUJD"},
            "acknowledged_safety_checks": [{"id": "sc_1",
                                            "code": "malicious_instructions",
                                            "message": "check this"}],
        })
    );
}

#[tokio::test]
async fn a_computer_call_with_no_action_replays_as_a_screenshot() {
    let server = MockServer::start().await;
    // The model asks for the screen with neither `action` nor `actions`.
    let empty_call_item = json!({
        "type": "computer_call", "id": "cu_1", "call_id": "call_1",
        "status": "completed"
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_1",
            "status": "completed",
            "output": [empty_call_item],
            "usage": {"input_tokens": 30, "output_tokens": 8}
        })))
        .expect(2)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("Look at the screen")]);
    req.tools = vec![computer_tool("computer")];
    let response = router.chat(&req).await.unwrap();

    // The call reads as the screenshot it asks for.
    let call = response.message.tool_calls[0].clone();
    assert_eq!(call.computer_action(), json!([{"type": "screenshot"}]));

    // The next request replays the call with that action, which the
    // provider accepts, and answers it with the screenshot.
    req.messages.push(response.message);
    req.messages.push(Message {
        role: Role::Tool,
        content: vec![ContentPart::ImageUrl {
            url: "data:image/png;base64,QUJD".into(),
        }],
        tool_calls: vec![],
        tool_call_id: Some("call_1".into()),
    });
    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[1]
        .body_json()
        .unwrap();
    assert_eq!(
        sent["input"][1],
        json!({
            "type": "computer_call", "id": "cu_1", "call_id": "call_1",
            "status": "completed",
            "actions": [{"type": "screenshot"}]
        })
    );
    assert_eq!(sent["input"][2]["type"], "computer_call_output");
}

#[tokio::test]
async fn a_streamed_computer_call_with_no_action_carries_a_screenshot() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"id\":\"cu_1\",\"call_id\":\"call_1\",\"status\":\"in_progress\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"id\":\"cu_1\",\"call_id\":\"call_1\",\"status\":\"completed\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":6}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let stream = router
        .chat_stream(&ChatRequest::new("m", vec![Message::user("Look")]))
        .await
        .unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    let arguments = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolCallDelta { arguments, .. } => Some(arguments.clone()),
            _ => None,
        })
        .expect("the computer call arrives as a tool call delta");
    let item: Value = serde_json::from_str(&arguments).unwrap();
    assert_eq!(item["actions"], json!([{"type": "screenshot"}]));
}

#[tokio::test]
async fn function_tool_results_can_carry_a_screenshot_on_responses_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "resp_portable",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": "Done."}]
            }],
            "usage": {"input_tokens": 30, "output_tokens": 8}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let mut req = ChatRequest::new(
        "m",
        vec![
            Message::user("Open the browser"),
            Message {
                role: Role::Assistant,
                content: vec![],
                tool_calls: vec![ToolCall {
                    id: "call_portable".into(),
                    name: "computer".into(),
                    arguments: r#"{"type":"click","x":100,"y":200}"#.into(),
                }],
                tool_call_id: None,
            },
            Message {
                role: Role::Tool,
                content: vec![
                    ContentPart::Text {
                        text: "Current screen after the action.".into(),
                    },
                    ContentPart::ImageUrl {
                        url: "data:image/png;base64,QUJD".into(),
                    },
                ],
                tool_calls: vec![],
                tool_call_id: Some("call_portable".into()),
            },
        ],
    );
    req.tools = vec![Tool::function(
        "computer",
        "Use the computer",
        json!({"type": "object"}),
    )];

    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(
        sent["input"][2],
        json!({
            "type": "function_call_output",
            "call_id": "call_portable",
            "output": [
                {"type": "input_text", "text": "Current screen after the action."},
                {"type": "input_image", "image_url": "data:image/png;base64,QUJD"}
            ]
        })
    );
}

#[tokio::test]
async fn preview_computer_tool_sets_truncation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed",
            "output": [{"type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": "ok"}]}]
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.tools = vec![computer_tool("computer_use_preview")];
    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    // The deprecated preview tool requires truncation; the GA tool does not.
    assert_eq!(sent["truncation"], "auto");
}

#[tokio::test]
async fn unsupported_features_skip_to_the_next_candidate_without_retries() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"},
                         "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;

    let config = llm_router::RouterConfig::new()
        .provider(
            "closed",
            llm_router::ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "test-key"),
        )
        .provider(
            "open",
            llm_router::ProviderConfig::new(
                ProtocolKind::OpenAiResponses,
                server.uri(),
                "test-key",
            ),
        )
        .model(
            "m",
            [
                llm_router::Candidate::new("closed", "chat-model"),
                llm_router::Candidate::new("open", "cua-model"),
            ],
        )
        .retry(llm_router::RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        });
    let attempts_on_closed = Arc::new(AtomicU32::new(0));
    let counter = attempts_on_closed.clone();
    let router = llm_router::Router::new(config)
        .unwrap()
        .on_attempt(move |info| {
            if info.provider == "closed" {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });

    // Typed tools are unsupported on chat completions: the first candidate
    // must be attempted exactly once — deterministic rejections do not
    // retry the same candidate.
    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.tools = vec![computer_tool("computer")];
    let error = router.chat(&req).await.unwrap_err();
    // The second candidate hits /chat-less /responses which this mock does
    // not serve; the call exhausts, which is fine — the assertion is about
    // attempt counts.
    let _ = error;
    assert_eq!(attempts_on_closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn responses_stream_yields_neutral_events() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_9\",\"name\":\"get_weather\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"city\\\":\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"\\\"SF\\\"}\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":1,\"delta\":\"On it.\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":6}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let stream = router
        .chat_stream(&ChatRequest::new("m", vec![Message::user("Weather?")]))
        .await
        .unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events,
        vec![
            StreamEvent::ToolCallStart {
                index: 0,
                id: "call_9".into(),
                name: "get_weather".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                arguments: "{\"city\":".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                arguments: "\"SF\"}".into(),
            },
            StreamEvent::TextDelta {
                text: "On it.".into()
            },
            StreamEvent::Finish {
                reason: FinishReason::ToolCalls,
                native_reason: Some("completed".into()),
                usage: Some(llm_router::Usage {
                    input_tokens: 12,
                    output_tokens: 6,
                    ..Default::default()
                }),
            },
        ]
    );
}

#[tokio::test]
async fn stop_sequences_are_rejected_on_responses_protocol() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let mut req = ChatRequest::new("m", vec![Message::user("hi")]);
    req.stop = vec!["END".into()];
    let error = router.chat(&req).await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}
