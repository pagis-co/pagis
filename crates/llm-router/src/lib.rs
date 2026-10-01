//! A lightweight, high-performance LLM router.
//!
//! One neutral interface over many providers. Providers are data over a small
//! set of protocol codecs; the router resolves a model alias to an ordered
//! candidate list and falls back on retryable errors. See `docs/DESIGN.md`.
//!
//! ```no_run
//! use llm_router::{Candidate, ChatRequest, Message, ProviderConfig, Router, RouterConfig};
//!
//! # async fn example() -> Result<(), llm_router::Error> {
//! let config = RouterConfig::new()
//!     .provider("openai", ProviderConfig::openai(std::env::var("OPENAI_API_KEY").unwrap()))
//!     .provider("anthropic", ProviderConfig::anthropic(std::env::var("ANTHROPIC_API_KEY").unwrap()))
//!     .model("default", [
//!         Candidate::new("anthropic", "claude-sonnet-4-5"),
//!         Candidate::new("openai", "gpt-5"),
//!     ]);
//! let router = Router::new(config)?;
//! let response = router
//!     .chat(&ChatRequest::new("default", vec![Message::user("Hello!")]))
//!     .await?;
//! println!("{}", response.message.text_content());
//! # Ok(())
//! # }
//! ```

mod accumulator;
mod config;
mod error;
pub mod protocol;
mod realtime;
mod registry;
mod router;
mod stream_util;
mod types;
mod voices;

pub use accumulator::{MessageAccumulator, StreamedMessage};
pub use config::{
    Candidate, CompatConfig, MaxTokensField, ProtocolKind, ProviderConfig, RetryConfig,
    RouterConfig, TimeoutConfig,
};
pub use error::{Error, ErrorKind};
pub use realtime::{
    RealtimeConnection, RealtimeIntent, RealtimeMessage, RealtimeProtocol, RealtimeSocket,
};
pub use registry::{
    DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_OUTPUT_TOKENS, ListedModel, ListedVoice, ModelInfo,
    ModelMetadata, ModelPrices, model_info, model_metadata,
};
pub use router::{AttemptInfo, ChatStream, Router};
pub use types::{
    AudioFormat, AudioOut, CachePolicy, ChatRequest, ChatResponse, ContentPart, EmbeddingsRequest,
    EmbeddingsResponse, FinishReason, GeneratedImage, ImageData, ImageInput, ImageRequest,
    ImageResponse, JsonSchemaFormat, Message, Modality, ReasoningConfig, ReasoningEffort, Role,
    SizeSpec, SpeechRequest, SpeechResponse, StreamEvent, Tool, ToolCall, ToolChoice,
    TranscriptSegment, TranscriptWord, TranscriptionRequest, TranscriptionResponse, Usage,
    VideoJob, VideoRequest, VideoStatus,
};
pub use voices::OPENAI_VOICES;
