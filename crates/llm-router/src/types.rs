//! Neutral request, response, and stream types.
//!
//! The fields map 1:1 to OpenAI chat semantics, the industry substrate.
//! Protocol codecs translate these types to and from each provider's wire
//! format.

use serde::{Deserialize, Serialize};

use crate::config::Candidate;
use serde_json::Value;

/// A chat request against a model alias or a `provider/model` pair.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    /// A model alias from the router config, or a direct `provider/model` id.
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Ask the model to reason before it answers. Codecs map this to the
    /// provider's control (effort level or thinking budget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// Prompt-cache hints. Codecs apply this only where the provider needs
    /// explicit hints (Anthropic); providers with implicit caching ignore it.
    #[serde(default, skip_serializing_if = "CachePolicy::is_none")]
    pub cache: CachePolicy,
    /// Output modalities the model should produce. Empty means the provider
    /// default (text). Set `[Text, Audio]` with [`ChatRequest::audio`] for
    /// spoken responses on audio-capable chat models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<Modality>,
    /// Audio output settings, required when `modalities` includes `Audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioOut>,
    /// A JSON Schema that constrains the model's final text output. Provider
    /// codecs translate this neutral contract to their native request shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonSchemaFormat>,
    /// Provider-specific params the neutral fields do not cover
    /// (`response_format`, `seed`, `parallel_tool_calls`, `top_k`, ...).
    /// Codecs merge them into the wire body last, so they also override
    /// encoded fields.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// One strict JSON result contract for a model turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonSchemaFormat {
    /// Provider-visible schema name. Use letters, digits, underscores, and dashes.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: Value,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            ..Self::default()
        }
    }
}

/// How much to reason. `max_tokens` (a thinking-token budget) wins where the
/// provider takes a budget; `effort` wins where it takes a level. When only
/// the other form is set, the codec converts it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

/// Prompt-cache hint placement.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    /// No hints.
    #[default]
    None,
    /// Place hints at the stable prefix boundaries: the system prompt, the
    /// last tool definition, and the last message. This is the right default
    /// for agent loops, where each turn extends the cached prefix.
    Auto,
}

impl CachePolicy {
    pub fn is_none(&self) -> bool {
        *self == CachePolicy::None
    }
}

/// An output modality of a chat response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Audio,
}

impl Modality {
    pub fn wire_name(self) -> &'static str {
        match self {
            Modality::Text => "text",
            Modality::Audio => "audio",
        }
    }
}

/// Audio output settings for chat: which voice, which container format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioOut {
    pub voice: String,
    pub format: AudioFormat,
}

/// An audio container or encoding format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
    Opus,
    Aac,
    /// Raw 16-bit PCM.
    Pcm16,
}

impl AudioFormat {
    pub fn wire_name(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Flac => "flac",
            AudioFormat::Opus => "opus",
            AudioFormat::Aac => "aac",
            AudioFormat::Pcm16 => "pcm16",
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Mp3 => "audio/mpeg",
            AudioFormat::Flac => "audio/flac",
            AudioFormat::Opus => "audio/ogg",
            AudioFormat::Aac => "audio/aac",
            AudioFormat::Pcm16 => "audio/pcm",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ContentPart>,
    /// Tool calls made by the assistant. Only valid on `Role::Assistant`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The id of the tool call this message answers. Only valid on `Role::Tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentPart::Text { text: text.into() }],
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::text(Role::System, text)
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::text(Role::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(Role::Assistant, text)
    }

    /// A tool result for the tool call with id `tool_call_id`.
    pub fn tool(tool_call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentPart::Text { text: text.into() }],
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// The concatenated text of all text parts.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    /// An image by https URL or `data:` URI.
    ImageUrl {
        url: String,
    },
    /// Reasoning the model produced before its answer. Pass assistant
    /// messages back with these parts unmodified — providers that sign
    /// reasoning (Anthropic) reject altered or missing blocks in tool loops.
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Provider-encrypted reasoning. Opaque; pass it back unmodified.
    RedactedReasoning {
        data: String,
    },
    /// Audio input, base64-encoded. Only `Wav` and `Mp3` are accepted as
    /// chat input by current providers.
    InputAudio {
        /// Base64-encoded audio bytes.
        data: String,
        format: AudioFormat,
    },
    /// Audio the assistant produced. Pass assistant messages back with this
    /// part unmodified — providers replay it by `id` in multi-turn chats
    /// until `expires_at`.
    OutputAudio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Base64-encoded audio bytes. Absent when the provider replays by id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript: Option<String>,
        /// Unix seconds after which the provider forgets the audio id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
}

/// A tool definition: a function with a JSON Schema, or a provider-defined
/// tool named by its wire type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the arguments object. Unused on provider-defined
    /// tools.
    #[serde(default)]
    pub parameters: Value,
    /// The provider's tool type for a provider-defined tool, passed through
    /// as data: `computer_20251124`, `bash_20250124`, `text_editor_20250728`
    /// (Anthropic; set the matching `anthropic-beta` header via
    /// `ProviderConfig::headers`), or `computer` / `computer_use_preview`
    /// (openai-responses). `None` means a plain function tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Config fields of a provider-defined tool, merged into its wire
    /// object (`display_width_px`, `display_height_px`, ...).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub config: serde_json::Map<String, Value>,
    /// The one candidate this tool belongs to, or `None` for a tool every
    /// candidate takes. A provider-defined tool is one provider's wire
    /// type, so the router sends it only to the candidate named here and
    /// leaves it out when the request falls back to another candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Candidate>,
}

impl Tool {
    /// Send this tool only to the candidate `provider`/`model`.
    pub fn for_candidate(mut self, provider: impl Into<String>, model: impl Into<String>) -> Self {
        self.candidate = Some(Candidate::new(provider, model));
        self
    }

    /// Whether the candidate `provider`/`model` takes this tool.
    pub fn applies_to(&self, provider: &str, model: &str) -> bool {
        self.candidate
            .as_ref()
            .is_none_or(|own| own.provider == provider && own.model == model)
    }

    /// A function tool with a JSON Schema for its arguments.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            ..Self::default()
        }
    }

    /// A provider-defined tool such as Anthropic computer use
    /// (`computer_20251124`) or the openai-responses computer tool.
    ///
    /// On openai-responses, provider-defined computer calls come back named
    /// `computer` whatever `name` says because the wire item has no name.
    /// Read the action with [`ToolCall::computer_action`]. A plain function
    /// tool can also use the name `computer`; it remains a function call.
    pub fn provider_defined(
        kind: impl Into<String>,
        name: impl Into<String>,
        config: serde_json::Map<String, Value>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: Some(kind.into()),
            config,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    /// Force a call to the named tool.
    Tool(String),
}

/// A tool call made by the assistant. `arguments` is the raw JSON string.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    /// The action payload of a computer-use call, with any wire wrapper
    /// removed. On anthropic-messages the arguments already are the action
    /// object; on openai-responses they are the whole `computer_call` item,
    /// and this returns its `actions` (or `action`) field. The action
    /// vocabulary stays the provider's own.
    ///
    /// Returns `Value::Null` when the arguments do not parse.
    pub fn computer_action(&self) -> Value {
        let parsed: Value = serde_json::from_str(&self.arguments).unwrap_or(Value::Null);
        match parsed.get("type").and_then(Value::as_str) {
            Some("computer_call") => parsed
                .get("actions")
                .or_else(|| parsed.get("action"))
                .cloned()
                .unwrap_or(Value::Null),
            _ => parsed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatResponse {
    /// The provider key that served the request.
    pub provider: String,
    /// The concrete model id that served the request.
    pub model: String,
    pub message: Message,
    pub finish_reason: FinishReason,
    /// The provider's raw finish reason, preserved for debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_finish_reason: Option<String>,
    pub usage: Usage,
    /// The USD cost of this response, when the model is in the metadata
    /// registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Other,
}

/// Normalized token usage with non-overlapping buckets.
///
/// Invariants: `cache_read_input_tokens` and `cache_write_input_tokens` are
/// parts of `input_tokens`; `reasoning_tokens` is a part of `output_tokens`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingsRequest {
    /// A model alias or a direct `provider/model` id.
    pub model: String,
    pub input: Vec<String>,
}

impl EmbeddingsRequest {
    pub fn new(model: impl Into<String>, input: impl IntoIterator<Item = String>) -> Self {
        Self {
            model: model.into(),
            input: input.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingsResponse {
    pub provider: String,
    pub model: String,
    /// One vector per input, in input order.
    pub embeddings: Vec<Vec<f32>>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageRequest {
    /// A model alias or a direct `provider/model` id.
    pub model: String,
    pub prompt: String,
    /// Number of images. Provider default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Output size. Provider default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<SizeSpec>,
    /// Quality tier such as `low`, `medium`, `high` (OpenAI) or a provider
    /// tier name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Output image format such as `png`, `jpeg`, `webp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// Background handling: `transparent`, `opaque`, or `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Reference images to edit or draw from. When present, the openai-chat
    /// codec routes to `/images/edits` (multipart) instead of
    /// `/images/generations`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_images: Vec<ImageInput>,
    /// An alpha mask telling the model which areas of the first input image
    /// to replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<ImageInput>,
    /// Provider-specific params, merged into the wire body last.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl ImageRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            ..Self::default()
        }
    }
}

/// An output size, in whichever convention the provider takes: exact pixels
/// (OpenAI, Together) or an aspect ratio plus resolution tier (Gemini,
/// Imagen, Veo).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SizeSpec {
    Pixels {
        width: u32,
        height: u32,
    },
    Aspect {
        /// e.g. `16:9`.
        ratio: String,
        /// Resolution tier such as `720p` or `1K`, when the provider takes
        /// one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<String>,
    },
}

impl SizeSpec {
    pub fn pixels(width: u32, height: u32) -> Self {
        SizeSpec::Pixels { width, height }
    }

    /// The `WIDTHxHEIGHT` string pixel-convention providers take, when this
    /// size is in pixels.
    pub fn pixel_string(&self) -> Option<String> {
        match self {
            SizeSpec::Pixels { width, height } => Some(format!("{width}x{height}")),
            SizeSpec::Aspect { .. } => None,
        }
    }
}

/// An input image, by URL or as base64 bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageInput {
    B64 {
        /// Base64-encoded image bytes.
        data: String,
        /// e.g. `image/png`.
        media_type: String,
    },
    Url {
        url: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageResponse {
    pub provider: String,
    pub model: String,
    pub images: Vec<GeneratedImage>,
    /// The media type of the images, when the provider reports the format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Token usage, on providers that bill image generation in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneratedImage {
    pub image: ImageData,
    /// The prompt the provider rewrote and actually used, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageData {
    /// Base64-encoded image bytes.
    B64 { data: String },
    /// A provider-hosted, usually short-lived, URL.
    Url { url: String },
}

/// A video-generation request. Video generation is an async job: submit with
/// [`Router::create_video`](crate::Router::create_video), poll with
/// [`Router::video_status`](crate::Router::video_status), fetch the artifact
/// with [`Router::video_content`](crate::Router::video_content).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VideoRequest {
    /// A model alias or a direct `provider/model` id.
    pub model: String,
    pub prompt: String,
    /// Clip length in seconds. Provider default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
    /// Output size: pixels (Sora) or aspect ratio + resolution tier (Veo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<SizeSpec>,
    /// A first-frame reference image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_image: Option<ImageInput>,
    /// Provider-specific params, merged into the wire body last.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl VideoRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            ..Self::default()
        }
    }
}

/// The five normalized states of a generation job.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Canceled,
}

/// A video-generation job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoJob {
    /// A router-scoped job id. Opaque: it encodes the provider and the native
    /// job id, so `video_status` and `video_content` route without extra
    /// state. Store it as-is.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub status: VideoStatus,
    /// Completion percentage, on providers that report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    /// Unix seconds after which the provider deletes the artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// The artifact URL, on providers that deliver by URL (Veo). Fetch it
    /// through [`Router::video_content`](crate::Router::video_content) —
    /// the URL can require provider auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_url: Option<String>,
    /// The failure message when `status` is `Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A text-to-speech request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpeechRequest {
    /// A model alias or a direct `provider/model` id.
    pub model: String,
    /// The text to speak.
    pub input: String,
    /// The voice name (OpenAI) or voice id (ElevenLabs).
    pub voice: String,
    /// Output format. Provider default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormat>,
    /// Speed multiplier. Providers without the control ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// Delivery instructions (tone, accent). Providers without the control
    /// ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Provider-specific params, merged into the wire body last.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl SpeechRequest {
    pub fn new(
        model: impl Into<String>,
        input: impl Into<String>,
        voice: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            input: input.into(),
            voice: voice.into(),
            ..Self::default()
        }
    }
}

/// Synthesized speech: raw audio bytes and their media type.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechResponse {
    pub provider: String,
    pub model: String,
    pub audio: bytes::Bytes,
    /// The `Content-Type` the provider reported, e.g. `audio/mpeg`.
    pub media_type: String,
}

/// A speech-to-text request. The audio goes as raw bytes; `media_type`
/// names its format (e.g. `audio/wav`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptionRequest {
    /// A model alias or a direct `provider/model` id.
    pub model: String,
    pub audio: bytes::Bytes,
    pub media_type: String,
    /// ISO 639-1 hint, e.g. `en`.
    pub language: Option<String>,
    /// Style or vocabulary hint for providers that take one.
    pub prompt: Option<String>,
    /// Ask for segment and word timestamps where the provider offers them.
    /// On OpenAI this is a whisper-1 feature — the gpt-4o transcribe models
    /// reject the `verbose_json` format it needs. Deepgram returns word
    /// timings either way.
    pub timestamps: bool,
    /// Provider-specific params. String and number values become extra form
    /// fields (OpenAI) or query params (Deepgram).
    pub extra: serde_json::Map<String, Value>,
}

impl TranscriptionRequest {
    pub fn new(
        model: impl Into<String>,
        audio: impl Into<bytes::Bytes>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            audio: audio.into(),
            media_type: media_type.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TranscriptionResponse {
    pub provider: String,
    pub model: String,
    pub text: String,
    /// Timed segments, when requested and offered by the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptSegment>,
    /// Timed words, when requested and offered by the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptWord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TranscriptSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TranscriptWord {
    pub start_s: f64,
    pub end_s: f64,
    pub word: String,
}

/// One event in a chat stream. A flat, tagged union that agent loops can
/// consume directly.
///
/// `index` correlates the events of one tool call within a stream. Values are
/// protocol-specific; treat them as opaque keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    /// A reasoning block closed. Carries the provider's signature when the
    /// provider signs reasoning; keep it to rebuild the assistant message.
    ///
    /// Only providers with delimited reasoning blocks (Anthropic) emit this.
    /// openai-chat providers emit bare `ReasoningDelta`s; treat the first
    /// non-reasoning event or `Finish` as the end of reasoning there —
    /// [`MessageAccumulator`](crate::MessageAccumulator) handles both.
    ReasoningEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Provider-encrypted reasoning, arriving whole. Opaque; keep it in the
    /// rebuilt assistant message so tool loops can replay it.
    RedactedReasoning {
        data: String,
    },
    /// A chunk of assistant audio, base64-encoded. Distinct from `TextDelta`
    /// so a UI can route audio to a player and text to a view.
    AudioDelta {
        data: String,
    },
    /// A chunk of the transcript of the assistant audio.
    AudioTranscriptDelta {
        text: String,
    },
    /// The assistant audio closed. Carries the provider's audio id for
    /// multi-turn replay. Emitted before `Finish` when the response had audio.
    AudioEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: u32,
        /// A fragment of the arguments JSON string.
        arguments: String,
    },
    /// The final event of a successful stream.
    Finish {
        reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
}
