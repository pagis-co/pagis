//! Protocol codecs.
//!
//! A codec translates neutral types to and from one wire protocol. All codec
//! methods are synchronous; the router owns the HTTP client and the async
//! request flow.

mod anthropic;
mod deepgram;
mod elevenlabs;
mod model_list;
mod openai;
mod responses;
mod veo;

pub use anthropic::AnthropicMessages;
pub use deepgram::Deepgram;
pub use elevenlabs::ElevenLabs;
pub use openai::OpenAiChat;
pub use responses::OpenAiResponses;
pub use veo::Veo;

use bytes::Bytes;
use futures::stream::BoxStream;

use crate::config::{ProtocolKind, ProviderConfig};
use crate::error::Error;
use crate::registry::{ListedModel, ListedVoice};
use crate::types::{
    ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse, ImageRequest, ImageResponse,
    SpeechRequest, StreamEvent, TranscriptionRequest, TranscriptionResponse, VideoJob,
    VideoRequest,
};

/// The raw response body stream handed to a codec for decoding.
pub type ByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;

/// A stream of decoded events.
pub type EventStream = BoxStream<'static, Result<StreamEvent, Error>>;

/// One page of a provider's model list.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPage {
    pub models: Vec<ListedModel>,
    /// The cursor of the next page, or `None` on the last page.
    pub next: Option<String>,
}

/// One page of the voices a provider lists apart from its models.
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePage {
    pub voices: Vec<ListedVoice>,
    /// The cursor of the next page, or `None` on the last page.
    pub next: Option<String>,
}

pub trait Protocol: Send + Sync {
    /// Build the HTTP request for `req` against `provider`, with the alias
    /// already resolved to the concrete `model` id. Media-only protocols
    /// keep the default, which reports chat as unsupported.
    fn build_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &ChatRequest,
        _stream: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(unsupported_chat(provider_key))
    }

    /// Decode a successful non-streaming response body.
    fn parse_response(
        &self,
        provider_key: &str,
        _model: &str,
        _body: &[u8],
    ) -> Result<ChatResponse, Error> {
        Err(unsupported_chat(provider_key))
    }

    /// Map an HTTP error status and body to a normalized [`Error`].
    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error;

    /// Decode a successful streaming response body into events.
    fn stream_events(&self, provider_key: &str, _bytes: ByteStream) -> EventStream {
        let error = unsupported_chat(provider_key);
        Box::pin(futures::stream::once(async move { Err(error) }))
    }

    /// Build an embeddings request. Protocols without embeddings keep the
    /// default, which reports the feature as unsupported.
    fn build_embeddings_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &EmbeddingsRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "embeddings",
        })
    }

    fn parse_embeddings_response(
        &self,
        provider_key: &str,
        _model: &str,
        _body: &[u8],
    ) -> Result<EmbeddingsResponse, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "embeddings",
        })
    }

    /// Build an image-generation request. Protocols without image generation
    /// keep the default, which reports the feature as unsupported.
    fn build_image_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &ImageRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "image generation",
        })
    }

    fn parse_image_response(
        &self,
        provider_key: &str,
        _model: &str,
        _body: &[u8],
    ) -> Result<ImageResponse, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "image generation",
        })
    }

    /// Build a text-to-speech request. The response is raw audio bytes; the
    /// router reads them and the `Content-Type` header directly.
    fn build_speech_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &SpeechRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "speech synthesis",
        })
    }

    /// Build a speech-to-text request.
    fn build_transcription_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &TranscriptionRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "transcription",
        })
    }

    fn parse_transcription_response(
        &self,
        provider_key: &str,
        _model: &str,
        _body: &[u8],
    ) -> Result<TranscriptionResponse, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "transcription",
        })
    }

    /// Build the request for one page of the models the key can use.
    /// `after` is the cursor of the previous page. Protocols without a
    /// list endpoint keep the default, which reports the model list as
    /// unsupported.
    fn build_list_models_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(unsupported_model_list(provider_key))
    }

    /// Decode one page of the model list.
    fn parse_list_models(&self, provider_key: &str, _body: &[u8]) -> Result<ModelPage, Error> {
        Err(unsupported_model_list(provider_key))
    }

    /// Whether the provider lists its voices apart from its models: the
    /// voices of an account, which every speech model takes. The model
    /// list then reads them and gives them to each speech model.
    fn lists_voices_apart(&self) -> bool {
        false
    }

    /// Build the request for one page of the voices listed apart.
    /// `after` is the cursor of the previous page.
    fn build_list_voices_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "voice list",
        })
    }

    /// Decode one page of the voices listed apart.
    fn parse_list_voices(&self, provider_key: &str, _body: &[u8]) -> Result<VoicePage, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "voice list",
        })
    }

    /// Build a request that submits a video-generation job.
    fn build_video_create_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _model: &str,
        _req: &VideoRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "video generation",
        })
    }

    /// Decode a job object — from the create response or a status poll.
    /// `VideoJob::id` holds the provider's native id here; the router
    /// rewrites it to the router-scoped form.
    fn parse_video_job(
        &self,
        provider_key: &str,
        _model: &str,
        _body: &[u8],
    ) -> Result<VideoJob, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "video generation",
        })
    }

    /// Build a request that polls the job with the given native id.
    fn build_video_status_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _native_id: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "video generation",
        })
    }

    /// Build a request that fetches the artifact of a completed job. `job`
    /// carries the fresh status, for providers that deliver a signed URL in
    /// it (Veo); others fetch by native id.
    fn build_video_content_request(
        &self,
        _http: &reqwest::Client,
        provider_key: &str,
        _provider: &ProviderConfig,
        _native_id: &str,
        _job: &VideoJob,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::Unsupported {
            provider: provider_key.to_owned(),
            feature: "video generation",
        })
    }
}

fn unsupported_model_list(provider_key: &str) -> Error {
    Error::Unsupported {
        provider: provider_key.to_owned(),
        feature: "model list",
    }
}

fn unsupported_chat(provider_key: &str) -> Error {
    Error::Unsupported {
        provider: provider_key.to_owned(),
        feature: "chat",
    }
}

/// A header value carrying a credential: parsed, and marked sensitive so
/// it stays redacted in header Debug output.
pub(crate) fn sensitive_header(
    provider_key: &str,
    value: &str,
) -> Result<reqwest::header::HeaderValue, Error> {
    let mut header = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
        Error::InvalidConfig(format!(
            "provider `{provider_key}` api_key contains characters not valid in a header"
        ))
    })?;
    header.set_sensitive(true);
    Ok(header)
}

/// Cap provider-supplied error text, so a hostile response body cannot
/// flood error messages and logs.
pub(crate) fn cap_error_text(mut message: String) -> String {
    const MAX_ERROR_TEXT: usize = 2048;
    if message.len() > MAX_ERROR_TEXT {
        let mut end = MAX_ERROR_TEXT;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    message
}

/// The codec instance for a protocol kind.
pub fn codec(kind: ProtocolKind) -> &'static dyn Protocol {
    match kind {
        ProtocolKind::OpenAiChat => &OpenAiChat,
        ProtocolKind::OpenAiResponses => &OpenAiResponses,
        ProtocolKind::AnthropicMessages => &AnthropicMessages,
        ProtocolKind::ElevenLabs => &ElevenLabs,
        ProtocolKind::Deepgram => &Deepgram,
        ProtocolKind::Veo => &Veo,
    }
}
