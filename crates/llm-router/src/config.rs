//! Router configuration: providers and model aliases.
//!
//! A provider is data over a protocol codec: which protocol, which base URL,
//! which key. A model alias names an ordered candidate list; the router tries
//! candidates in order.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::registry::ModelInfo;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouterConfig {
    pub providers: HashMap<String, ProviderConfig>,
    /// Model alias -> ordered fallback candidates.
    pub models: HashMap<String, Vec<Candidate>>,
    /// Metadata overrides and additions, keyed by concrete model id. Entries
    /// here win over the built-in table.
    #[serde(default)]
    pub model_info: HashMap<String, ModelInfo>,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub timeouts: TimeoutConfig,
}

impl RouterConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn provider(mut self, key: impl Into<String>, config: ProviderConfig) -> Self {
        self.providers.insert(key.into(), config);
        self
    }

    pub fn model(
        mut self,
        alias: impl Into<String>,
        candidates: impl IntoIterator<Item = Candidate>,
    ) -> Self {
        self.models
            .insert(alias.into(), candidates.into_iter().collect());
        self
    }

    pub fn model_info(mut self, model: impl Into<String>, info: ModelInfo) -> Self {
        self.model_info.insert(model.into(), info);
        self
    }

    pub fn retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub fn timeouts(mut self, timeouts: TimeoutConfig) -> Self {
        self.timeouts = timeouts;
        self
    }
}

/// Same-candidate retry policy. Only retryable errors retry; the backoff
/// doubles per attempt up to `max_backoff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Attempts per candidate, including the first one. At least 1.
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_secs(10),
            max_backoff: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeoutConfig {
    pub connect: Duration,
    /// Total time for one non-streaming request.
    pub request: Duration,
    /// Time from sending a streaming request until its first event arrives.
    /// When it fires, the router falls back to the next candidate.
    pub first_event: Duration,
    /// Maximum quiet time between stream events. When it fires, the stream
    /// ends with an in-band error; the router does not re-route mid-stream.
    pub idle: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(120),
            first_event: Duration::from_secs(30),
            idle: Duration::from_secs(90),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub protocol: ProtocolKind,
    /// Base URL without a trailing slash, e.g. `https://api.openai.com/v1`.
    /// Must be `https`, unless the host is local (`localhost`, `127.0.0.0/8`,
    /// `::1`, `*.localhost`).
    pub base_url: String,
    /// The provider API key. Local servers that ignore auth still want a
    /// placeholder (Ollama documents any value, e.g. `"ollama"`); an empty
    /// string sends no auth header at all.
    ///
    /// The key never serializes and never prints in `Debug` output. Store
    /// keys outside serialized config and set this field after loading.
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// Wire-level deviations of this server from the protocol's default
    /// behavior.
    #[serde(default)]
    pub compat: CompatConfig,
    /// Extra headers sent with every request to this provider
    /// (`anthropic-beta`, OpenRouter attribution headers, proxy auth, ...).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("api_key", &"***")
            .field("compat", &self.compat)
            // Header values can carry credentials; print names only.
            .field("headers", &self.headers.keys())
            .finish()
    }
}

/// How an OpenAI-compatible server deviates from the common wire behavior.
/// The defaults fit most compatible servers (Ollama, llama.cpp, vLLM, Groq,
/// DeepSeek, Together); [`ProviderConfig::openai`] adjusts them for
/// api.openai.com. The openai-responses protocol reads
/// `model_list_path` alone, and the anthropic-messages protocol ignores
/// these.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatConfig {
    /// Send `stream_options: {"include_usage": true}` on streaming requests.
    /// Turn off for servers that reject unknown params with 422 (Mistral) —
    /// they return usage in the final chunk without it.
    pub stream_options: bool,
    /// The wire field for the output-token cap. api.openai.com requires
    /// `max_completion_tokens` on reasoning models; most compatible servers
    /// document only `max_tokens`.
    pub max_tokens_field: MaxTokensField,
    /// Send `reasoning_effort` when the request asks for reasoning. Turn off
    /// for servers that reject the param (Mistral, xAI grok-4); the request
    /// then runs with the server's default reasoning behavior.
    pub reasoning_effort: bool,
    /// The path of the model list under the base URL. OpenRouter's
    /// `/models` answers without a key, so [`ProviderConfig::openrouter`]
    /// reads `/models/user`, which takes the key and lists the models it
    /// can use.
    pub model_list_path: String,
}

impl Default for CompatConfig {
    fn default() -> Self {
        Self {
            stream_options: true,
            max_tokens_field: MaxTokensField::MaxTokens,
            reasoning_effort: true,
            model_list_path: "/models".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

impl MaxTokensField {
    pub fn wire_name(self) -> &'static str {
        match self {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        }
    }
}

impl ProviderConfig {
    pub fn new(
        protocol: ProtocolKind,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        Self {
            protocol,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            compat: CompatConfig::default(),
            headers: HashMap::new(),
        }
    }

    pub fn openai(api_key: impl Into<String>) -> Self {
        let mut config = Self::new(
            ProtocolKind::OpenAiChat,
            "https://api.openai.com/v1",
            api_key,
        );
        // api.openai.com rejects `max_tokens` on reasoning models.
        config.compat.max_tokens_field = MaxTokensField::MaxCompletionTokens;
        config
    }

    /// api.openai.com over the Responses API — required for OpenAI computer
    /// use models.
    pub fn openai_responses(api_key: impl Into<String>) -> Self {
        Self::new(
            ProtocolKind::OpenAiResponses,
            "https://api.openai.com/v1",
            api_key,
        )
    }

    /// OpenRouter over its Responses API. This protocol also gives
    /// compatible routed models access to provider-defined tools. The
    /// model list of the key names the speech and transcription models
    /// too, with the voices of each speech model; without the filter it
    /// names text models alone.
    pub fn openrouter(api_key: impl Into<String>) -> Self {
        let mut config = Self::new(
            ProtocolKind::OpenAiResponses,
            "https://openrouter.ai/api/v1",
            api_key,
        );
        config.compat.model_list_path =
            "/models/user?output_modalities=text,speech,transcription".to_owned();
        config
    }

    pub fn anthropic(api_key: impl Into<String>) -> Self {
        Self::new(
            ProtocolKind::AnthropicMessages,
            "https://api.anthropic.com/v1",
            api_key,
        )
    }

    pub fn elevenlabs(api_key: impl Into<String>) -> Self {
        Self::new(
            ProtocolKind::ElevenLabs,
            "https://api.elevenlabs.io/v1",
            api_key,
        )
    }

    pub fn deepgram(api_key: impl Into<String>) -> Self {
        Self::new(
            ProtocolKind::Deepgram,
            "https://api.deepgram.com/v1",
            api_key,
        )
    }

    pub fn veo(api_key: impl Into<String>) -> Self {
        Self::new(
            ProtocolKind::Veo,
            "https://generativelanguage.googleapis.com/v1beta",
            api_key,
        )
    }
}

/// The wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolKind {
    /// OpenAI Chat Completions and compatible APIs (Groq, DeepSeek, xAI,
    /// Mistral, Ollama, vLLM, ...).
    OpenAiChat,
    /// The OpenAI Responses API. Chat only; carries provider-defined tools
    /// (computer use) and encrypted reasoning replay.
    OpenAiResponses,
    /// The Anthropic Messages API.
    AnthropicMessages,
    /// The ElevenLabs API. Speech synthesis only.
    ElevenLabs,
    /// The Deepgram API. Transcription only.
    Deepgram,
    /// Google Veo video generation on the Gemini API. Video only.
    Veo,
}

/// One (provider, concrete model) pair in a fallback list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub provider: String,
    pub model: String,
}

impl Candidate {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}
