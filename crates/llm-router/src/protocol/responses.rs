//! The `openai-responses` codec: the OpenAI Responses API.
//!
//! Chat over `POST /responses`, stateless: `store: false`, and each request
//! resends the full transcript as input items. This protocol carries the
//! features Chat Completions does not — provider-defined tools such as
//! computer use (`Tool::provider_defined("computer", ...)` on current
//! models, `computer_use_preview` on older ones) and encrypted reasoning
//! replay.
//!
//! Mappings into the neutral types:
//! - A `computer_call` output item becomes a [`ToolCall`] named `computer`
//!   whose `arguments` is the whole item as JSON — read the `action` or
//!   `actions` field from it, and pass the assistant message back
//!   unmodified so the item replays verbatim. An item with neither field
//!   asks for the screen: it gets `actions: [{"type": "screenshot"}]`.
//! - The tool result for a computer call sends its first image part as a
//!   `computer_call_output` screenshot. A function result sends text as a
//!   `function_call_output`, or an input-content array when it has images.
//! - A `reasoning` item's summary becomes a [`ContentPart::Reasoning`]; the
//!   full item (needed for stateless replay when it carries
//!   `encrypted_content`) becomes a [`ContentPart::RedactedReasoning`] and
//!   replays verbatim. Request `encrypted_content` with
//!   `extra["include"] = ["reasoning.encrypted_content"]`.
//!
//! Not on this protocol: audio, stop sequences (both report as errors),
//! embeddings and the media modalities (use an openai-chat provider on the
//! same base URL).

use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::openai::{classify, extract_error_fields, get, post};
use crate::protocol::{ByteStream, EventStream, ModelPage, Protocol, model_list};
use crate::types::{
    ChatRequest, ChatResponse, ContentPart, FinishReason, Message, Modality, Role, StreamEvent,
    ToolCall, ToolChoice, Usage,
};

pub struct OpenAiResponses;

impl Protocol for OpenAiResponses {
    /// The Responses API shares its model list with Chat Completions.
    fn build_list_models_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        _after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Ok(get(http, provider, &provider.compat.model_list_path))
    }

    fn parse_list_models(&self, provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
        model_list::parse_openai(provider_key, body)
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
        if req.audio.is_some() || req.modalities.contains(&Modality::Audio) {
            return Err(Error::Unsupported {
                provider: provider_key.to_owned(),
                feature: "audio",
            });
        }
        if !req.stop.is_empty() {
            return Err(Error::InvalidConfig(format!(
                "provider `{provider_key}`: the openai-responses protocol has no stop sequences"
            )));
        }

        let mut body = json!({
            "model": model,
            "input": encode_input(provider_key, &req.messages)?,
            // Stateless: the router resends the transcript; nothing is
            // stored server-side.
            "store": false,
            // Reasoning models reject a replayed function/computer call
            // without its reasoning item; encrypted content makes those
            // items replayable without server-side state. Harmless on
            // non-reasoning models.
            "include": ["reasoning.encrypted_content"],
        });
        let obj = body.as_object_mut().expect("body is an object");
        let instructions = system_text(&req.messages);
        if !instructions.is_empty() {
            obj.insert("instructions".into(), json!(instructions));
        }
        if let Some(t) = req.temperature {
            obj.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            obj.insert("top_p".into(), json!(p));
        }
        if let Some(m) = req.max_tokens {
            obj.insert("max_output_tokens".into(), json!(m));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| match &t.kind {
                    // Provider-defined tools pass through: the type plus
                    // config (`computer`, `computer_use_preview`, ...).
                    Some(kind) => {
                        let mut tool = json!({"type": kind});
                        let tool_obj = tool.as_object_mut().expect("tool is an object");
                        for (key, value) in &t.config {
                            tool_obj.insert(key.clone(), value.clone());
                        }
                        tool
                    }
                    // Function tools are flat on this API.
                    None => json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }),
                })
                .collect();
            obj.insert("tools".into(), json!(tools));
            // The deprecated preview computer tool requires truncation;
            // the GA `computer` tool does not.
            if req
                .tools
                .iter()
                .any(|t| t.kind.as_deref() == Some("computer_use_preview"))
            {
                obj.insert("truncation".into(), json!("auto"));
            }
        }
        if let Some(choice) = &req.tool_choice {
            let value = match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Required => json!("required"),
                // Forcing a provider-defined tool uses its hosted type;
                // a function tool uses the function shape.
                ToolChoice::Tool(name) => {
                    let kind = req
                        .tools
                        .iter()
                        .find(|t| &t.name == name)
                        .and_then(|t| t.kind.as_deref());
                    match kind {
                        Some(kind) => json!({"type": kind}),
                        None => json!({"type": "function", "name": name}),
                    }
                }
            };
            obj.insert("tool_choice".into(), value);
        }
        if let Some(reasoning) = &req.reasoning {
            obj.insert(
                "reasoning".into(),
                json!({"effort": crate::protocol::openai::effort_name(reasoning)}),
            );
        }
        if let Some(format) = &req.output_schema {
            let mut schema = json!({
                "type": "json_schema",
                "name": format.name,
                "schema": format.schema,
                "strict": true,
            });
            if let Some(description) = &format.description {
                schema["description"] = json!(description);
            }
            obj.insert("text".into(), json!({"format": schema}));
        }
        if stream {
            obj.insert("stream".into(), json!(true));
        }
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }
        Ok(post(http, provider, "/responses").json(&body))
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
                message: format!("failed to decode response: {e}"),
            })?;
        if let Some(error) = &wire.error {
            return Err(Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("response failed: {}", error.message),
            });
        }

        let mut content = Vec::new();
        let mut tool_calls = Vec::new();
        let mut refused = false;
        for item in &wire.output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    for part in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => {
                                let text = str_field(part, "text");
                                if !text.is_empty() {
                                    content.push(ContentPart::Text { text });
                                }
                            }
                            Some("refusal") => {
                                refused = true;
                                let text = str_field(part, "refusal");
                                if !text.is_empty() {
                                    content.push(ContentPart::Text { text });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some("reasoning") => {
                    let summary: String = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|s| str_field(s, "text"))
                        .collect();
                    if !summary.is_empty() {
                        content.push(ContentPart::Reasoning {
                            text: summary,
                            signature: None,
                        });
                    }
                    // Keep the whole item so stateless replay works when it
                    // carries encrypted_content.
                    if item.get("encrypted_content").is_some_and(|v| !v.is_null()) {
                        content.push(ContentPart::RedactedReasoning {
                            data: item.to_string(),
                        });
                    }
                }
                Some("function_call") => tool_calls.push(ToolCall {
                    id: str_field(item, "call_id"),
                    name: str_field(item, "name"),
                    arguments: str_field(item, "arguments"),
                }),
                Some("computer_call") => tool_calls.push(computer_tool_call(item)),
                _ => {}
            }
        }

        let native = wire
            .incomplete_details
            .as_ref()
            .map(|d| d.reason.clone())
            .or(wire.status.clone());
        let finish_reason = if refused {
            FinishReason::ContentFilter
        } else {
            match wire.status.as_deref() {
                Some("completed") if !tool_calls.is_empty() => FinishReason::ToolCalls,
                Some("completed") => FinishReason::Stop,
                Some("incomplete") if native.as_deref() == Some("max_output_tokens") => {
                    FinishReason::Length
                }
                _ => FinishReason::Other,
            }
        };

        Ok(ChatResponse {
            provider: provider_key.to_owned(),
            model: wire.model.unwrap_or_else(|| model.to_owned()),
            message: Message {
                role: Role::Assistant,
                content,
                tool_calls,
                tool_call_id: None,
            },
            finish_reason,
            native_finish_reason: native,
            usage: wire.usage.map(Usage::from).unwrap_or_default(),
            cost_usd: None,
        })
    }

    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        let (message, code) = extract_error_fields(raw.as_ref());
        let message = message.unwrap_or_else(|| {
            String::from_utf8_lossy(&body[..body.len().min(2048)])
                .trim()
                .to_owned()
        });
        let kind = classify(status, code.as_deref(), &message);
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
            let mut saw_tool_call = false;

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
                if event.data.trim() == "[DONE]" {
                    break;
                }
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
                let index = data
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                match data.get("type").and_then(Value::as_str) {
                    // Refusal text streams like regular text; the final
                    // finish reason still reports the refusal.
                    Some("response.output_text.delta") | Some("response.refusal.delta") => {
                        let text = str_field(&data, "delta");
                        if !text.is_empty() {
                            yield Ok(StreamEvent::TextDelta { text });
                        }
                    }
                    Some("response.reasoning_summary_text.delta") => {
                        let text = str_field(&data, "delta");
                        if !text.is_empty() {
                            yield Ok(StreamEvent::ReasoningDelta { text });
                        }
                    }
                    Some("response.output_item.added") => {
                        let Some(item) = data.get("item") else { continue };
                        match item.get("type").and_then(Value::as_str) {
                            Some("function_call") => {
                                saw_tool_call = true;
                                yield Ok(StreamEvent::ToolCallStart {
                                    index,
                                    id: str_field(item, "call_id"),
                                    name: str_field(item, "name"),
                                });
                            }
                            Some("computer_call") => {
                                saw_tool_call = true;
                                yield Ok(StreamEvent::ToolCallStart {
                                    index,
                                    id: str_field(item, "call_id"),
                                    name: "computer".to_owned(),
                                });
                            }
                            _ => {}
                        }
                    }
                    Some("response.function_call_arguments.delta") => {
                        let arguments = str_field(&data, "delta");
                        if !arguments.is_empty() {
                            yield Ok(StreamEvent::ToolCallDelta { index, arguments });
                        }
                    }
                    Some("response.output_item.done") => {
                        let Some(item) = data.get("item") else { continue };
                        match item.get("type").and_then(Value::as_str) {
                            // A computer call arrives whole: forward the
                            // full item as this call's arguments.
                            Some("computer_call") => {
                                yield Ok(StreamEvent::ToolCallDelta {
                                    index,
                                    arguments: computer_call_item(item).to_string(),
                                });
                            }
                            Some("reasoning")
                                if item
                                    .get("encrypted_content")
                                    .is_some_and(|v| !v.is_null()) =>
                            {
                                yield Ok(StreamEvent::RedactedReasoning {
                                    data: item.to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                    Some("response.completed") | Some("response.incomplete") => {
                        let response = data.get("response").cloned().unwrap_or(Value::Null);
                        let usage = response
                            .get("usage")
                            .and_then(|u| {
                                serde_json::from_value::<WireUsage>(u.clone()).ok()
                            })
                            .map(Usage::from);
                        let native = response
                            .pointer("/incomplete_details/reason")
                            .or_else(|| response.get("status"))
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        let reason = match native.as_deref() {
                            Some("completed") if saw_tool_call => FinishReason::ToolCalls,
                            Some("completed") => FinishReason::Stop,
                            Some("max_output_tokens") => FinishReason::Length,
                            _ => FinishReason::Other,
                        };
                        yield Ok(StreamEvent::Finish {
                            reason,
                            native_reason: native,
                            usage,
                        });
                        return;
                    }
                    Some("response.failed") | Some("error") => {
                        let message = data
                            .pointer("/response/error/message")
                            .or_else(|| data.pointer("/error/message"))
                            .or_else(|| data.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("provider sent an error event")
                            .to_owned();
                        let code = data
                            .pointer("/response/error/code")
                            .or_else(|| data.pointer("/error/code"))
                            .or_else(|| data.get("code"))
                            .and_then(Value::as_str);
                        // An error event carries the code of the HTTP error
                        // it stands for, so it follows the same retry rules.
                        let status = match code {
                            Some("server_error") => Some((500, ErrorKind::Server)),
                            Some("rate_limit_exceeded") => Some((429, ErrorKind::RateLimit)),
                            _ => None,
                        };
                        yield Err(match status {
                            Some((status, kind)) => Error::Provider {
                                provider: provider.clone(),
                                status,
                                kind,
                                message,
                                raw: Some(data.clone()),
                            },
                            None => Error::Stream { provider: provider.clone(), message },
                        });
                        return;
                    }
                    _ => {}
                }
            }

            yield Err(Error::Stream {
                provider: provider.clone(),
                message: "stream ended without response.completed".to_owned(),
            });
        }
        .boxed()
    }
}

/// True when a tool call's arguments carry a `computer_call` wire item —
/// the classification for replay and result encoding, independent of the
/// tool's name.
fn is_computer_call_item(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .map(|t| t == "computer_call")
        })
        .unwrap_or(false)
}

/// A `computer_call` becomes a tool call named `computer` whose arguments
/// carry the whole item, so it replays verbatim and the executor reads the
/// action(s) from it — [`ToolCall::computer_action`] unwraps it.
fn computer_tool_call(item: &Value) -> ToolCall {
    ToolCall {
        id: str_field(item, "call_id"),
        name: "computer".to_owned(),
        arguments: computer_call_item(item).to_string(),
    }
}

/// The `computer_call` item as the caller runs and replays it. A model
/// can send a call with neither `action` nor `actions`: that call asks
/// for the screen, and the API refuses to replay it without an action.
/// Such a call gets `actions: [{"type": "screenshot"}]`, the action the
/// caller runs for it.
fn computer_call_item(item: &Value) -> Value {
    let mut item = item.clone();
    if let Some(fields) = item.as_object_mut()
        && !fields.contains_key("action")
        && !fields.contains_key("actions")
    {
        fields.insert("actions".to_owned(), json!([{"type": "screenshot"}]));
    }
    item
}

fn system_text(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(Message::text_content)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Encode non-system messages as Responses input items.
fn encode_input(provider_key: &str, messages: &[Message]) -> Result<Vec<Value>, Error> {
    // Computer calls — classified by their arguments carrying a
    // `computer_call` wire item, not by name, so a function tool that
    // happens to be named `computer` stays a function call. Their results
    // encode as computer_call_output; the map keeps each call's
    // `pending_safety_checks` for the acknowledgment on that output.
    let computer_calls: std::collections::HashMap<&str, Value> = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.tool_calls)
        .filter(|c| is_computer_call_item(&c.arguments))
        .map(|c| {
            let checks = serde_json::from_str::<Value>(&c.arguments)
                .ok()
                .and_then(|item| item.get("pending_safety_checks").cloned())
                .unwrap_or(Value::Null);
            (c.id.as_str(), checks)
        })
        .collect();

    let mut items = Vec::new();
    for message in messages {
        match message.role {
            Role::System => {}
            Role::User => {
                let parts = encode_user_parts(provider_key, &message.content)?;
                items.push(json!({"role": "user", "content": parts}));
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                for part in &message.content {
                    match part {
                        ContentPart::Text { text } => {
                            parts.push(json!({"type": "output_text", "text": text}));
                        }
                        // A stored reasoning item replays verbatim;
                        // summaries and foreign reasoning drop out.
                        ContentPart::RedactedReasoning { data } => {
                            if let Ok(item) = serde_json::from_str::<Value>(data)
                                && item.get("type").and_then(Value::as_str) == Some("reasoning")
                            {
                                items.push(item);
                            }
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    items.push(json!({"role": "assistant", "content": parts}));
                }
                for call in &message.tool_calls {
                    // A computer call replays as its verbatim wire item.
                    if is_computer_call_item(&call.arguments) {
                        let item = serde_json::from_str::<Value>(&call.arguments)
                            .expect("is_computer_call_item parsed it");
                        items.push(item);
                        continue;
                    }
                    items.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments,
                    }));
                }
            }
            Role::Tool => {
                let call_id = message.tool_call_id.clone().unwrap_or_default();
                if let Some(checks) = computer_calls.get(call_id.as_str()) {
                    // A computer call answers with a screenshot.
                    let url = message
                        .content
                        .iter()
                        .find_map(|p| match p {
                            ContentPart::ImageUrl { url } => Some(url.clone()),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            Error::InvalidConfig(format!(
                                "provider `{provider_key}`: the result for computer call \
                                 `{call_id}` needs an image part (the screenshot)"
                            ))
                        })?;
                    let mut output = json!({
                        "type": "computer_call_output",
                        "call_id": call_id,
                        "output": {"type": "computer_screenshot", "image_url": url},
                    });
                    // Replying to a computer call is the caller's decision
                    // to proceed: it acknowledges the call's pending safety
                    // checks (the API refuses to continue otherwise).
                    // Surface the checks to the user first — they sit in
                    // the call's arguments.
                    if checks.as_array().is_some_and(|c| !c.is_empty()) {
                        output["acknowledged_safety_checks"] = checks.clone();
                    }
                    items.push(output);
                } else {
                    let has_image = message
                        .content
                        .iter()
                        .any(|part| matches!(part, ContentPart::ImageUrl { .. }));
                    let output = if has_image {
                        Value::Array(encode_user_parts(provider_key, &message.content)?)
                    } else {
                        Value::String(message.text_content())
                    };
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output,
                    }));
                }
            }
        }
    }
    Ok(items)
}

fn encode_user_parts(provider_key: &str, parts: &[ContentPart]) -> Result<Vec<Value>, Error> {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(Ok(json!({"type": "input_text", "text": text}))),
            ContentPart::ImageUrl { url } => {
                Some(Ok(json!({"type": "input_image", "image_url": url})))
            }
            ContentPart::InputAudio { .. } | ContentPart::OutputAudio { .. } => {
                Some(Err(Error::Unsupported {
                    provider: provider_key.to_owned(),
                    feature: "audio",
                }))
            }
            ContentPart::Reasoning { .. } | ContentPart::RedactedReasoning { .. } => None,
        })
        .collect()
}

fn str_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[derive(Deserialize)]
struct WireResponse {
    model: Option<String>,
    status: Option<String>,
    incomplete_details: Option<WireIncomplete>,
    error: Option<WireError>,
    #[serde(default)]
    output: Vec<Value>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireIncomplete {
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    input_tokens_details: Option<WireInputDetails>,
    output_tokens_details: Option<WireOutputDetails>,
}

#[derive(Deserialize)]
struct WireInputDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Deserialize)]
struct WireOutputDetails {
    #[serde(default)]
    reasoning_tokens: u64,
}

impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Usage {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cache_read_input_tokens: w.input_tokens_details.map(|d| d.cached_tokens).unwrap_or(0),
            cache_write_input_tokens: 0,
            reasoning_tokens: w
                .output_tokens_details
                .map(|d| d.reasoning_tokens)
                .unwrap_or(0),
        }
    }
}
