//! The `anthropic-messages` codec: the Anthropic Messages API.

use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::{ByteStream, EventStream, ModelPage, Protocol, model_list};
use crate::types::{
    CachePolicy, ChatRequest, ChatResponse, ContentPart, FinishReason, Message, ReasoningConfig,
    ReasoningEffort, Role, StreamEvent, ToolCall, ToolChoice, Usage,
};

/// The Messages API requires `max_tokens`; use this when the request sets none.
const DEFAULT_MAX_TOKENS: u32 = 4096;
const API_VERSION: &str = "2023-06-01";
/// The largest page the model list serves.
const MODEL_PAGE_LIMIT: &str = "1000";

pub struct AnthropicMessages;

impl Protocol for AnthropicMessages {
    fn build_list_models_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut query = vec![("limit", MODEL_PAGE_LIMIT)];
        if let Some(after) = after {
            query.push(("after_id", after));
        }
        let mut request = http
            .get(format!("{}/models", provider.base_url))
            .query(&query)
            .header("anthropic-version", API_VERSION);
        for (name, value) in &provider.headers {
            request = request.header(name, value);
        }
        if !provider.api_key.is_empty() {
            let key = crate::protocol::sensitive_header(provider_key, &provider.api_key)?;
            request = request.header("x-api-key", key);
        }
        Ok(request)
    }

    fn parse_list_models(&self, provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
        model_list::parse_anthropic(provider_key, body)
    }

    fn build_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // The Messages API has no audio: no input parts, no audio modality.
        let has_audio = req.audio.is_some()
            || req.modalities.contains(&crate::types::Modality::Audio)
            || req.messages.iter().any(|m| {
                m.content.iter().any(|p| {
                    matches!(
                        p,
                        ContentPart::InputAudio { .. } | ContentPart::OutputAudio { .. }
                    )
                })
            });
        if has_audio {
            return Err(Error::Unsupported {
                provider: provider_key.to_owned(),
                feature: "audio",
            });
        }

        let auto_cache = req.cache == CachePolicy::Auto;
        let thinking_budget = req.reasoning.as_ref().map(thinking_budget);
        // The API requires max_tokens greater than the thinking budget.
        let mut max_tokens = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        if let Some(budget) = thinking_budget {
            max_tokens = max_tokens.max(budget + DEFAULT_MAX_TOKENS);
        }

        let mut messages = encode_messages(&req.messages);
        if auto_cache {
            mark_last_block(&mut messages);
        }
        let mut body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
        });
        let obj = body.as_object_mut().expect("body is an object");
        let system = system_text(&req.messages);
        if !system.is_empty() {
            if auto_cache {
                obj.insert(
                    "system".into(),
                    json!([{
                        "type": "text",
                        "text": system,
                        "cache_control": {"type": "ephemeral"},
                    }]),
                );
            } else {
                obj.insert("system".into(), json!(system));
            }
        }
        if let Some(budget) = thinking_budget {
            obj.insert(
                "thinking".into(),
                json!({"type": "enabled", "budget_tokens": budget}),
            );
        } else {
            // The API rejects sampling params when thinking is on.
            if let Some(t) = req.temperature {
                obj.insert("temperature".into(), json!(t));
            }
            if let Some(p) = req.top_p {
                obj.insert("top_p".into(), json!(p));
            }
        }
        if !req.stop.is_empty() {
            obj.insert("stop_sequences".into(), json!(req.stop));
        }
        if !req.tools.is_empty() {
            let mut tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| match &t.kind {
                    // Provider-defined tools (computer use, bash, text
                    // editor) pass through: the type plus their config.
                    // Some need an `anthropic-beta` header — set it via
                    // `ProviderConfig::headers`.
                    Some(kind) => {
                        let mut tool = json!({"type": kind, "name": t.name});
                        let obj = tool.as_object_mut().expect("tool is an object");
                        for (key, value) in &t.config {
                            obj.insert(key.clone(), value.clone());
                        }
                        tool
                    }
                    None => json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    }),
                })
                .collect();
            if auto_cache && let Some(last) = tools.last_mut() {
                last["cache_control"] = json!({"type": "ephemeral"});
            }
            obj.insert("tools".into(), json!(tools));
        }
        if let Some(choice) = &req.tool_choice {
            let value = match choice {
                ToolChoice::Auto => json!({"type": "auto"}),
                ToolChoice::None => json!({"type": "none"}),
                ToolChoice::Required => json!({"type": "any"}),
                ToolChoice::Tool(name) => json!({"type": "tool", "name": name}),
            };
            obj.insert("tool_choice".into(), value);
        }
        if let Some(format) = &req.output_schema {
            obj.insert(
                "output_config".into(),
                json!({
                    "format": {
                        "type": "json_schema",
                        "schema": format.schema,
                    }
                }),
            );
        }
        if stream {
            obj.insert("stream".into(), json!(true));
        }
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }

        let mut request = http
            .post(format!("{}/messages", provider.base_url))
            .header("anthropic-version", API_VERSION)
            .json(&body);
        for (name, value) in &provider.headers {
            request = request.header(name, value);
        }
        if !provider.api_key.is_empty() {
            let key = crate::protocol::sensitive_header(provider_key, &provider.api_key)?;
            request = request.header("x-api-key", key);
        }
        Ok(request)
    }

    fn parse_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<ChatResponse, Error> {
        let wire: WireResponse =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode message: {e}"),
            })?;

        let mut content = Vec::new();
        let mut tool_calls = Vec::new();
        for block in wire.content {
            match block {
                WireBlock::Text { text } => content.push(ContentPart::Text { text }),
                WireBlock::Thinking {
                    thinking,
                    signature,
                } => content.push(ContentPart::Reasoning {
                    text: thinking,
                    signature,
                }),
                WireBlock::RedactedThinking { data } => {
                    content.push(ContentPart::RedactedReasoning { data })
                }
                WireBlock::ToolUse { id, name, input } => tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: input.to_string(),
                }),
                WireBlock::Other => {}
            }
        }

        Ok(ChatResponse {
            provider: provider_key.to_owned(),
            model: wire.model.unwrap_or_else(|| model.to_owned()),
            message: Message {
                role: Role::Assistant,
                content,
                tool_calls,
                tool_call_id: None,
            },
            finish_reason: map_stop_reason(wire.stop_reason.as_deref()),
            native_finish_reason: wire.stop_reason,
            usage: wire.usage.map(Usage::from).unwrap_or_default(),
            cost_usd: None,
        })
    }

    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        let error = raw.as_ref().and_then(|v| v.get("error"));
        let error_type = error
            .and_then(|e| e.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                String::from_utf8_lossy(&body[..body.len().min(2048)])
                    .trim()
                    .to_owned()
            });
        let kind = classify(status, error_type, &message);
        Error::Provider {
            provider: provider_key.to_owned(),
            status,
            kind,
            message: crate::protocol::cap_error_text(message),
            raw,
        }
    }

    fn stream_events(&self, provider_key: &str, bytes: ByteStream) -> EventStream {
        let provider = provider_key.to_owned();
        stream! {
            let mut events = bytes.eventsource();
            let mut usage = Usage::default();
            let mut finish: Option<(FinishReason, Option<String>)> = None;
            // Open thinking blocks: index -> accumulated signature.
            let mut thinking: std::collections::HashMap<u32, String> =
                std::collections::HashMap::new();

            while let Some(event) = events.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        yield Err(Error::Stream {
                            provider: provider.clone(),
                            message: e.to_string(),
                        });
                        return;
                    }
                };
                let data: Value = match serde_json::from_str(&event.data) {
                    Ok(data) => data,
                    Err(e) => {
                        yield Err(Error::Stream {
                            provider: provider.clone(),
                            message: format!("failed to decode stream event: {e}"),
                        });
                        return;
                    }
                };
                match event.event.as_str() {
                    "message_start" => {
                        if let Some(u) = data.pointer("/message/usage") {
                            merge_usage(&mut usage, u);
                        }
                    }
                    "content_block_start" => {
                        let index = index_of(&data);
                        let Some(block) = data.get("content_block") else { continue };
                        match block.get("type").and_then(Value::as_str) {
                            Some("tool_use") => {
                                yield Ok(StreamEvent::ToolCallStart {
                                    index,
                                    id: str_field(block, "id"),
                                    name: str_field(block, "name"),
                                });
                            }
                            Some("thinking") => {
                                thinking.insert(index, String::new());
                            }
                            // Redacted thinking arrives whole in the start
                            // event; forward it so tool loops can replay it.
                            Some("redacted_thinking") => {
                                yield Ok(StreamEvent::RedactedReasoning {
                                    data: str_field(block, "data"),
                                });
                            }
                            _ => {}
                        }
                    }
                    "content_block_delta" => {
                        let index = index_of(&data);
                        let Some(delta) = data.get("delta") else { continue };
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => {
                                let text = str_field(delta, "text");
                                if !text.is_empty() {
                                    yield Ok(StreamEvent::TextDelta { text });
                                }
                            }
                            Some("input_json_delta") => {
                                let arguments = str_field(delta, "partial_json");
                                if !arguments.is_empty() {
                                    yield Ok(StreamEvent::ToolCallDelta { index, arguments });
                                }
                            }
                            Some("thinking_delta") => {
                                let text = str_field(delta, "thinking");
                                if !text.is_empty() {
                                    yield Ok(StreamEvent::ReasoningDelta { text });
                                }
                            }
                            Some("signature_delta") => {
                                if let Some(signature) = thinking.get_mut(&index) {
                                    signature.push_str(&str_field(delta, "signature"));
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let index = index_of(&data);
                        if let Some(signature) = thinking.remove(&index) {
                            yield Ok(StreamEvent::ReasoningEnd {
                                signature: (!signature.is_empty()).then_some(signature),
                            });
                        }
                    }
                    "message_delta" => {
                        if let Some(u) = data.get("usage") {
                            merge_usage(&mut usage, u);
                        }
                        if let Some(reason) =
                            data.pointer("/delta/stop_reason").and_then(Value::as_str)
                        {
                            finish =
                                Some((map_stop_reason(Some(reason)), Some(reason.to_owned())));
                        }
                    }
                    "message_stop" => {
                        let (reason, native) =
                            finish.take().unwrap_or((FinishReason::Stop, None));
                        yield Ok(StreamEvent::Finish {
                            reason,
                            native_reason: native,
                            usage: Some(usage),
                        });
                        return;
                    }
                    "error" => {
                        let message = data
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("provider sent an error event")
                            .to_owned();
                        yield Err(Error::Stream { provider: provider.clone(), message });
                        return;
                    }
                    // "ping" and unknown events carry nothing the neutral
                    // stream needs.
                    _ => {}
                }
            }

            yield Err(Error::Stream {
                provider: provider.clone(),
                message: "stream ended without message_stop".to_owned(),
            });
        }
        .boxed()
    }
}

/// The thinking-token budget: an explicit budget wins; an effort level
/// converts to one.
fn thinking_budget(reasoning: &ReasoningConfig) -> u32 {
    reasoning
        .max_tokens
        .unwrap_or(match reasoning.effort.unwrap_or(ReasoningEffort::Medium) {
            ReasoningEffort::Low => 1024,
            ReasoningEffort::Medium => 4096,
            ReasoningEffort::High => 16384,
        })
}

/// Attach a cache hint to the last content block of the last message, so the
/// cached prefix extends each turn. Thinking blocks cannot carry
/// `cache_control`; skip past them.
fn mark_last_block(messages: &mut [Value]) {
    if let Some(blocks) = messages
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
        && let Some(last) = blocks.iter_mut().rev().find(|block| {
            !matches!(
                block.get("type").and_then(Value::as_str),
                Some("thinking") | Some("redacted_thinking")
            )
        })
    {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
}

fn system_text(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(Message::text_content)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Encode non-system messages. Tool results become `tool_result` blocks in a
/// user message; consecutive same-role messages merge into one wire message.
fn encode_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<(&str, Vec<Value>)> = Vec::new();
    for message in messages {
        let (role, blocks) = match message.role {
            Role::System => continue,
            Role::User => ("user", encode_content(&message.content)),
            Role::Assistant => {
                let mut blocks = encode_content(&message.content);
                for call in &message.tool_calls {
                    let input: Value =
                        serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": input,
                    }));
                }
                ("assistant", blocks)
            }
            // Tool results carry full content blocks, so tools can return
            // images (screenshots) alongside text.
            Role::Tool => (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id,
                    "content": encode_content(&message.content),
                })],
            ),
        };
        match out.last_mut() {
            Some((last_role, last_blocks)) if *last_role == role => {
                last_blocks.extend(blocks);
            }
            _ => out.push((role, blocks)),
        }
    }
    out.into_iter()
        .map(|(role, blocks)| json!({"role": role, "content": blocks}))
        .collect()
}

fn encode_content(parts: &[ContentPart]) -> Vec<Value> {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(json!({"type": "text", "text": text})),
            ContentPart::ImageUrl { url } => Some(json!({
                "type": "image",
                "source": image_source(url),
            })),
            ContentPart::Reasoning { text, signature } => Some(json!({
                "type": "thinking",
                "thinking": text,
                "signature": signature.clone().unwrap_or_default(),
            })),
            ContentPart::RedactedReasoning { data } => Some(json!({
                "type": "redacted_thinking",
                "data": data,
            })),
            // Audio never reaches encoding; build_request rejects it first.
            ContentPart::InputAudio { .. } | ContentPart::OutputAudio { .. } => None,
        })
        .collect()
}

/// A `data:` URI becomes a base64 source; anything else a URL source.
fn image_source(url: &str) -> Value {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((media_type, data)) = rest.split_once(";base64,")
    {
        return json!({"type": "base64", "media_type": media_type, "data": data});
    }
    json!({"type": "url", "url": url})
}

fn map_stop_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        Some("refusal") => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

fn classify(status: u16, error_type: &str, message: &str) -> ErrorKind {
    match error_type {
        "authentication_error" | "permission_error" => ErrorKind::Authentication,
        "rate_limit_error" => ErrorKind::RateLimit,
        "overloaded_error" => ErrorKind::Overloaded,
        "api_error" => ErrorKind::Server,
        "invalid_request_error" => {
            if message.to_lowercase().contains("too long") {
                ErrorKind::ContextLength
            } else {
                ErrorKind::InvalidRequest
            }
        }
        _ => match status {
            401 | 403 => ErrorKind::Authentication,
            429 => ErrorKind::RateLimit,
            400..=499 => ErrorKind::InvalidRequest,
            503 | 529 => ErrorKind::Overloaded,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::Unknown,
        },
    }
}

fn index_of(data: &Value) -> u32 {
    data.get("index").and_then(Value::as_u64).unwrap_or(0) as u32
}

fn str_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Fold a wire usage object into the accumulated usage. Anthropic reports
/// `input_tokens` without the cache buckets; the normalized `input_tokens`
/// includes them.
fn merge_usage(usage: &mut Usage, wire: &Value) {
    let get = |field: &str| wire.get(field).and_then(Value::as_u64);
    if let Some(v) = get("cache_read_input_tokens") {
        usage.cache_read_input_tokens = v;
    }
    if let Some(v) = get("cache_creation_input_tokens") {
        usage.cache_write_input_tokens = v;
    }
    if let Some(v) = get("input_tokens") {
        usage.input_tokens = v
            .saturating_add(usage.cache_read_input_tokens)
            .saturating_add(usage.cache_write_input_tokens);
    }
    if let Some(v) = get("output_tokens") {
        usage.output_tokens = v;
    }
}

#[derive(Deserialize)]
struct WireResponse {
    model: Option<String>,
    content: Vec<WireBlock>,
    stop_reason: Option<String>,
    usage: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}

impl From<Map<String, Value>> for Usage {
    fn from(map: Map<String, Value>) -> Self {
        let mut usage = Usage::default();
        merge_usage(&mut usage, &Value::Object(map));
        usage
    }
}
