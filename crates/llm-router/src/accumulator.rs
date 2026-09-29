//! Rebuild an assistant [`Message`] from stream events.

use std::collections::BTreeMap;

use crate::types::{ContentPart, FinishReason, Message, Role, StreamEvent, ToolCall, Usage};

/// Folds [`StreamEvent`]s back into the assistant message an agent loop
/// appends to the conversation before the next turn.
///
/// It absorbs the per-protocol quirks so consumers do not have to:
/// providers without `ReasoningEnd` (openai-chat) close their reasoning at
/// the next non-reasoning event or at `Finish`; a repeated `ToolCallStart`
/// for one call merges instead of duplicating; tool calls order by their
/// stream index whatever numbering the protocol used.
///
/// ```
/// use llm_router::{MessageAccumulator, StreamEvent};
///
/// let mut acc = MessageAccumulator::new();
/// for event in [
///     StreamEvent::TextDelta { text: "Hel".into() },
///     StreamEvent::TextDelta { text: "lo".into() },
/// ] {
///     acc.push(&event);
/// }
/// let streamed = acc.finish();
/// assert_eq!(streamed.message.text_content(), "Hello");
/// ```
#[derive(Debug, Default)]
pub struct MessageAccumulator {
    parts: Vec<ContentPart>,
    open: Open,
    tool_calls: BTreeMap<u32, ToolCall>,
    audio: Option<AudioBuf>,
    finish_reason: Option<FinishReason>,
    native_finish_reason: Option<String>,
    usage: Option<Usage>,
}

#[derive(Debug, Default)]
struct AudioBuf {
    id: Option<String>,
    /// One entry per `AudioDelta`, joined at `finish`.
    data: Vec<String>,
    transcript: String,
    expires_at: Option<u64>,
}

/// Join base64 chunks into one decodable base64 string. Chunks whose byte
/// length is not a multiple of 3 pad mid-stream, so plain string
/// concatenation would not decode; decode each chunk and re-encode the
/// whole. Falls back to concatenation when a chunk is not valid base64.
fn join_base64(chunks: Vec<String>) -> String {
    use base64::Engine;
    let engine = &base64::engine::general_purpose::STANDARD;
    let mut bytes = Vec::new();
    for chunk in &chunks {
        match engine.decode(chunk) {
            Ok(decoded) => bytes.extend_from_slice(&decoded),
            Err(_) => return chunks.concat(),
        }
    }
    engine.encode(bytes)
}

#[derive(Debug, Default)]
enum Open {
    #[default]
    None,
    Text(String),
    Reasoning(String),
}

/// The result of a consumed stream: the rebuilt assistant message and the
/// final metadata from its `Finish` event.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamedMessage {
    pub message: Message,
    /// `FinishReason::Other` when the stream ended without a `Finish` event.
    pub finish_reason: FinishReason,
    pub native_finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

impl MessageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta { text } => match &mut self.open {
                Open::Text(buf) => buf.push_str(text),
                _ => {
                    self.close_open();
                    self.open = Open::Text(text.clone());
                }
            },
            StreamEvent::ReasoningDelta { text } => match &mut self.open {
                Open::Reasoning(buf) => buf.push_str(text),
                _ => {
                    self.close_open();
                    self.open = Open::Reasoning(text.clone());
                }
            },
            StreamEvent::ReasoningEnd { signature } => {
                if let Open::Reasoning(text) = std::mem::take(&mut self.open) {
                    self.parts.push(ContentPart::Reasoning {
                        text,
                        signature: signature.clone(),
                    });
                }
            }
            StreamEvent::RedactedReasoning { data } => {
                self.close_open();
                self.parts
                    .push(ContentPart::RedactedReasoning { data: data.clone() });
            }
            StreamEvent::AudioDelta { data } => {
                self.audio.get_or_insert_default().data.push(data.clone());
            }
            StreamEvent::AudioTranscriptDelta { text } => {
                self.audio.get_or_insert_default().transcript.push_str(text);
            }
            StreamEvent::AudioEnd { id, expires_at } => {
                let audio = self.audio.get_or_insert_default();
                audio.id = id.clone();
                audio.expires_at = *expires_at;
            }
            StreamEvent::ToolCallStart { index, id, name } => {
                let call = self.tool_calls.entry(*index).or_default();
                if call.id.is_empty() {
                    call.id = id.clone();
                }
                if call.name.is_empty() {
                    call.name = name.clone();
                }
            }
            StreamEvent::ToolCallDelta { index, arguments } => {
                self.tool_calls
                    .entry(*index)
                    .or_default()
                    .arguments
                    .push_str(arguments);
            }
            StreamEvent::Finish {
                reason,
                native_reason,
                usage,
            } => {
                self.close_open();
                self.finish_reason = Some(*reason);
                self.native_finish_reason = native_reason.clone();
                self.usage = *usage;
            }
        }
    }

    pub fn finish(mut self) -> StreamedMessage {
        self.close_open();
        if let Some(audio) = self.audio.take() {
            self.parts.push(ContentPart::OutputAudio {
                id: audio.id,
                data: (!audio.data.is_empty()).then_some(join_base64(audio.data)),
                transcript: (!audio.transcript.is_empty()).then_some(audio.transcript),
                expires_at: audio.expires_at,
            });
        }
        // A call with no argument deltas (zero-argument tools) normalizes
        // to `{}`, so `arguments` always parses as JSON.
        let tool_calls = self
            .tool_calls
            .into_values()
            .map(|mut call| {
                if call.arguments.is_empty() {
                    call.arguments = "{}".to_owned();
                }
                call
            })
            .collect();
        StreamedMessage {
            message: Message {
                role: Role::Assistant,
                content: self.parts,
                tool_calls,
                tool_call_id: None,
            },
            finish_reason: self.finish_reason.unwrap_or(FinishReason::Other),
            native_finish_reason: self.native_finish_reason,
            usage: self.usage,
        }
    }

    fn close_open(&mut self) {
        match std::mem::take(&mut self.open) {
            Open::None => {}
            Open::Text(text) => self.parts.push(ContentPart::Text { text }),
            Open::Reasoning(text) => self.parts.push(ContentPart::Reasoning {
                text,
                signature: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[StreamEvent]) -> StreamedMessage {
        let mut acc = MessageAccumulator::new();
        for event in events {
            acc.push(event);
        }
        acc.finish()
    }

    #[test]
    fn closes_unterminated_reasoning_at_the_next_event() {
        // openai-chat shape: reasoning deltas, then text, no ReasoningEnd.
        let streamed = feed(&[
            StreamEvent::ReasoningDelta { text: "th".into() },
            StreamEvent::ReasoningDelta { text: "ink".into() },
            StreamEvent::TextDelta { text: "4".into() },
            StreamEvent::Finish {
                reason: FinishReason::Stop,
                native_reason: Some("stop".into()),
                usage: Some(Usage::default()),
            },
        ]);
        assert_eq!(
            streamed.message.content,
            vec![
                ContentPart::Reasoning {
                    text: "think".into(),
                    signature: None,
                },
                ContentPart::Text { text: "4".into() },
            ]
        );
        assert_eq!(streamed.finish_reason, FinishReason::Stop);
        assert!(streamed.usage.is_some());
    }

    #[test]
    fn keeps_signatures_and_redacted_blocks() {
        // anthropic shape: signed thinking, redacted block, then text.
        let streamed = feed(&[
            StreamEvent::ReasoningDelta { text: "hmm".into() },
            StreamEvent::ReasoningEnd {
                signature: Some("sig".into()),
            },
            StreamEvent::RedactedReasoning {
                data: "opaque".into(),
            },
            StreamEvent::TextDelta { text: "ok".into() },
        ]);
        assert_eq!(
            streamed.message.content,
            vec![
                ContentPart::Reasoning {
                    text: "hmm".into(),
                    signature: Some("sig".into()),
                },
                ContentPart::RedactedReasoning {
                    data: "opaque".into(),
                },
                ContentPart::Text { text: "ok".into() },
            ]
        );
        // No Finish event seen.
        assert_eq!(streamed.finish_reason, FinishReason::Other);
    }

    #[test]
    fn merges_repeated_tool_call_starts_and_orders_by_index() {
        let streamed = feed(&[
            // Anthropic-style block indexes: text at 0, tools at 1 and 2.
            StreamEvent::TextDelta {
                text: "on it".into(),
            },
            StreamEvent::ToolCallStart {
                index: 2,
                id: "b".into(),
                name: "second".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 2,
                arguments: "{}".into(),
            },
            StreamEvent::ToolCallStart {
                index: 1,
                id: "a".into(),
                name: "first".into(),
            },
            // A repeated start for the same call must not duplicate it.
            StreamEvent::ToolCallStart {
                index: 1,
                id: "a".into(),
                name: "first".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 1,
                arguments: "{\"x\"".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 1,
                arguments: ":1}".into(),
            },
        ]);
        assert_eq!(
            streamed.message.tool_calls,
            vec![
                ToolCall {
                    id: "a".into(),
                    name: "first".into(),
                    arguments: "{\"x\":1}".into(),
                },
                ToolCall {
                    id: "b".into(),
                    name: "second".into(),
                    arguments: "{}".into(),
                },
            ]
        );
        assert_eq!(streamed.message.text_content(), "on it");
    }
}
