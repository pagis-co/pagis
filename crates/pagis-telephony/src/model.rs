//! The provider-neutral live voice seam (ADR-0020).
//!
//! The call loop sends typed commands and receives typed events. A
//! provider adapter owns its wire protocol. Telephone audio is G.711
//! mu-law on both sides of this seam.

use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use pagis_broker::ToolDef;
use serde_json::{Value, json};

use crate::audio::FRAME_DURATION;

/// The audio format of both directions. G.711 is 8 kHz by definition.
pub const AUDIO_FORMAT: &str = "audio/pcmu";

/// The model session did not open, or it stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the model session failed: {0}")]
pub struct ModelError(pub String);

/// The configuration the call loop can replace while a call runs.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    pub instructions: String,
    pub tools: Vec<ToolDef>,
    pub voice: Option<String>,
}

impl SessionConfig {
    pub fn new(
        instructions: impl Into<String>,
        tools: Vec<ToolDef>,
        voice: Option<String>,
    ) -> Self {
        Self {
            instructions: instructions.into(),
            tools,
            voice,
        }
    }
}

/// What the call loop asks any live voice provider to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientCommand {
    Configure(SessionConfig),
    AppendAudio(Bytes),
    Truncate {
        item_id: String,
        packets_played: u64,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
    AddInstructions(String),
    ContinueResponse,
}

impl ClientCommand {
    /// Encode the provider-neutral command for OpenAI Realtime.
    pub(crate) fn realtime_event(&self) -> Value {
        match self {
            Self::Configure(config) => {
                let tools = function_tools(&config.tools);
                let mut output = json!({ "format": { "type": AUDIO_FORMAT } });
                if let (Some(voice), Some(output)) = (&config.voice, output.as_object_mut()) {
                    output.insert("voice".to_string(), json!(voice));
                }
                json!({
                    "type": "session.update",
                    "session": {
                        "type": "realtime",
                        "instructions": config.instructions,
                        "tools": tools,
                        "tool_choice": "auto",
                        "audio": {
                            "input": {
                                "format": { "type": AUDIO_FORMAT },
                                "turn_detection": { "type": "server_vad" },
                            },
                            "output": output,
                        },
                    },
                })
            }
            Self::AppendAudio(payload) => json!({
                "type": "input_audio_buffer.append",
                "audio": base64::engine::general_purpose::STANDARD.encode(payload),
            }),
            Self::Truncate {
                item_id,
                packets_played,
            } => json!({
                "type": "conversation.item.truncate",
                "item_id": item_id,
                "content_index": 0,
                "audio_end_ms": packets_played * FRAME_DURATION.as_millis() as u64,
            }),
            Self::FunctionCallOutput { call_id, output } => json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                },
            }),
            Self::AddInstructions(text) => json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "system",
                    "content": [{ "type": "input_text", "text": text }],
                },
            }),
            Self::ContinueResponse => json!({ "type": "response.create" }),
        }
    }
}

pub(crate) fn function_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect()
}

/// One provider session. `next` must be cancel safe because the call
/// loop uses it inside `select!`.
#[async_trait]
pub trait ModelSession: Send {
    async fn send(&mut self, command: ClientCommand) -> Result<(), ModelError>;
    async fn next(&mut self) -> Option<Result<ServerEvent, ModelError>>;
    async fn close(self: Box<Self>);
}

/// Where a model session comes from. The bridge opens one per call and
/// one more when it reconnects.
#[async_trait]
pub trait ModelSessions: Send + Sync {
    /// Open one live model session for a call of this Workspace. The
    /// model aliases and the keys a call resolves are the tenant's own,
    /// so the Workspace is an argument of the open.
    async fn open(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Box<dyn ModelSession>, ModelError>;
}

/// What any live voice provider reports to the call loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    SpeechStarted,
    AudioDelta {
        item_id: Option<String>,
        audio: Bytes,
    },
    AgentTranscript(String),
    CallerTranscript(String),
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Error(String),
    Other,
}

impl ServerEvent {
    /// Decode one OpenAI Realtime server event.
    pub(crate) fn parse_realtime(event: &Value) -> Self {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "input_audio_buffer.speech_started" => Self::SpeechStarted,
            "response.output_audio.delta" => decode_audio(event, "delta"),
            "response.output_audio_transcript.done" => text(event, "transcript")
                .map(Self::AgentTranscript)
                .unwrap_or(Self::Other),
            "conversation.item.input_audio_transcription.completed" => text(event, "transcript")
                .map(Self::CallerTranscript)
                .unwrap_or(Self::Other),
            "response.function_call_arguments.done" => function_call(event),
            "response.done" => usage(event.pointer("/response/usage")),
            "error" => Self::Error(
                event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the model reported an error")
                    .to_string(),
            ),
            _ => Self::Other,
        }
    }

    /// Decode one GPT-Live server event. Delegated Responses events
    /// are nested under `response.event`.
    pub(crate) fn parse_live(event: &Value) -> Self {
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "session.output_audio.delta" => decode_audio(event, "delta"),
            "session.input_transcript.delta" => text(event, "delta")
                .map(Self::CallerTranscript)
                .unwrap_or(Self::Other),
            "session.output_transcript.delta" => text(event, "delta")
                .map(Self::AgentTranscript)
                .unwrap_or(Self::Other),
            "response.event" => parse_live_response(event.get("event")),
            "error" => Self::Error(
                event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the model reported an error")
                    .to_string(),
            ),
            _ => Self::Other,
        }
    }
}

fn parse_live_response(event: Option<&Value>) -> ServerEvent {
    let Some(event) = event else {
        return ServerEvent::Other;
    };
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.output_item.done" => {
            let Some(item) = event.get("item") else {
                return ServerEvent::Other;
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return ServerEvent::Other;
            }
            function_call(item)
        }
        "response.completed" | "response.done" => usage(event.pointer("/response/usage")),
        _ => ServerEvent::Other,
    }
}

fn decode_audio(event: &Value, field: &str) -> ServerEvent {
    let Some(audio) = event.get(field).and_then(Value::as_str) else {
        return ServerEvent::Other;
    };
    match base64::engine::general_purpose::STANDARD.decode(audio) {
        Ok(audio) => ServerEvent::AudioDelta {
            item_id: text(event, "item_id"),
            audio: Bytes::from(audio),
        },
        Err(error) => ServerEvent::Error(format!("the audio is not base64: {error}")),
    }
}

fn function_call(event: &Value) -> ServerEvent {
    let (Some(call_id), Some(name)) = (text(event, "call_id"), text(event, "name")) else {
        return ServerEvent::Other;
    };
    ServerEvent::FunctionCall {
        call_id,
        name,
        arguments: text(event, "arguments").unwrap_or_else(|| "{}".to_string()),
    }
}

fn usage(usage: Option<&Value>) -> ServerEvent {
    let Some(usage) = usage else {
        return ServerEvent::Other;
    };
    ServerEvent::Usage {
        input_tokens: tokens(usage, "input_tokens"),
        output_tokens: tokens(usage, "output_tokens"),
    }
}

fn text(event: &Value, field: &str) -> Option<String> {
    event.get(field).and_then(Value::as_str).map(str::to_string)
}

fn tokens(usage: &Value, field: &str) -> u64 {
    usage.get(field).and_then(Value::as_u64).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_unwraps_a_delegated_function_call() {
        let event = json!({
            "type": "response.event",
            "event": {
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "hang_up",
                    "arguments": "{\"reason\":\"done\"}",
                },
            },
        });

        assert_eq!(
            ServerEvent::parse_live(&event),
            ServerEvent::FunctionCall {
                call_id: "call-1".to_string(),
                name: "hang_up".to_string(),
                arguments: "{\"reason\":\"done\"}".to_string(),
            }
        );
    }

    #[test]
    fn live_decodes_pcmu_audio() {
        let event = json!({
            "type": "session.output_audio.delta",
            "delta": base64::engine::general_purpose::STANDARD.encode([1, 2, 3]),
        });

        assert_eq!(
            ServerEvent::parse_live(&event),
            ServerEvent::AudioDelta {
                item_id: None,
                audio: Bytes::from_static(&[1, 2, 3]),
            }
        );
    }

    #[test]
    fn realtime_configure_is_a_session_update_with_g711_and_server_vad() {
        let event = ClientCommand::Configure(SessionConfig::new("Be brief.", Vec::new(), None))
            .realtime_event();

        assert_eq!(event["type"], "session.update");
        assert_eq!(
            event["session"]["audio"]["input"]["format"]["type"],
            AUDIO_FORMAT
        );
        assert_eq!(
            event["session"]["audio"]["input"]["turn_detection"]["type"],
            "server_vad"
        );
    }
}
