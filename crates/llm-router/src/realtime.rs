//! Realtime voice sockets: authenticated endpoint routing.
//!
//! The router resolves the model alias, opens the upstream WebSocket with
//! the provider's credentials, and hands the socket to the caller. The
//! caller pumps frames in the protocol named by [`RealtimeProtocol`].
//! The router does not translate events. The caller owns the adapter
//! that maps those frames to its product-level session contract.

use std::pin::Pin;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use reqwest::header::HeaderMap;

use crate::config::{ProtocolKind, ProviderConfig};
use crate::error::Error;

#[cfg(not(target_arch = "wasm32"))]
mod tungstenite;
#[cfg(not(target_arch = "wasm32"))]
pub use tungstenite::TungsteniteTransport;

/// An application frame. The transport handles WebSocket control frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeMessage {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

impl RealtimeMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn binary(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Binary(bytes.into())
    }
}

/// A transport failure. The connection adds the provider name.
pub type RealtimeError = Box<dyn std::error::Error + Send + Sync>;

/// The stream and sink contract of a realtime socket.
pub trait RealtimeIo:
    Stream<Item = Result<RealtimeMessage, RealtimeError>> + Sink<RealtimeMessage, Error = RealtimeError>
{
}

impl<T> RealtimeIo for T where
    T: Stream<Item = Result<RealtimeMessage, RealtimeError>>
        + Sink<RealtimeMessage, Error = RealtimeError>
{
}

/// A socket that a caller can split into its sink and stream.
#[cfg(not(target_arch = "wasm32"))]
pub type RealtimeSocket = Pin<Box<dyn RealtimeIo + Send>>;
/// A socket that stays on the host's thread.
#[cfg(target_arch = "wasm32")]
pub type RealtimeSocket = Pin<Box<dyn RealtimeIo>>;

/// The pending connection of a transport.
#[cfg(not(target_arch = "wasm32"))]
pub type RealtimeConnectFuture<'a> =
    futures::future::BoxFuture<'a, Result<RealtimeSocket, RealtimeError>>;
/// The pending connection on the host's thread.
#[cfg(target_arch = "wasm32")]
pub type RealtimeConnectFuture<'a> =
    futures::future::LocalBoxFuture<'a, Result<RealtimeSocket, RealtimeError>>;

/// Opens an outbound WebSocket with the supplied headers and deadline.
/// Credentials stay in headers. A transport must not put them in the URL.
#[cfg(not(target_arch = "wasm32"))]
pub trait RealtimeTransport: Send + Sync {
    fn connect<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
        timeout: Duration,
    ) -> RealtimeConnectFuture<'a>;
}

/// Opens an outbound WebSocket with the supplied headers and deadline.
/// The host supplies this transport; a browser WebSocket cannot send
/// an Authorization header. Credentials must not go in the URL.
#[cfg(target_arch = "wasm32")]
pub trait RealtimeTransport {
    fn connect<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
        timeout: Duration,
    ) -> RealtimeConnectFuture<'a>;
}

/// A connected upstream realtime socket.
///
/// The socket is a [`futures::Stream`] of incoming frames and a
/// [`futures::Sink`] of outgoing frames ([`RealtimeMessage`]); use
/// [`futures::StreamExt::split`] to pump both directions concurrently.
/// Dropping it closes the connection; call [`RealtimeConnection::close`]
/// for a clean shutdown.
pub struct RealtimeConnection {
    pub provider: String,
    pub model: String,
    pub protocol: RealtimeProtocol,
    pub socket: RealtimeSocket,
}

impl std::fmt::Debug for RealtimeConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeConnection")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}

impl RealtimeConnection {
    /// Send one frame upstream.
    pub async fn send(&mut self, message: RealtimeMessage) -> Result<(), Error> {
        self.socket
            .send(message)
            .await
            .map_err(|e| realtime_error(&self.provider, e))
    }

    /// Receive the next frame. `None` when the upstream closed.
    pub async fn next(&mut self) -> Option<Result<RealtimeMessage, Error>> {
        self.socket
            .next()
            .await
            .map(|r| r.map_err(|e| realtime_error(&self.provider, e)))
    }

    /// Close the connection cleanly.
    pub async fn close(mut self) -> Result<(), Error> {
        self.socket
            .close()
            .await
            .map_err(|e| realtime_error(&self.provider, e))
    }
}

/// What a realtime socket is opened for. OpenAI keys the socket by
/// query: a conversation names its model, a transcription-only session
/// names its intent and picks the transcription model and the audio
/// format in `session.update`. Deepgram takes the model and the audio
/// format in the query of `/listen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeIntent {
    Conversation,
    /// Speech to text of PCM16 mono audio at `sample_rate` Hz.
    Transcription {
        sample_rate: u32,
    },
}

/// The event contract carried by one conversation socket.
///
/// The router opens the right endpoint. The caller owns the adapter
/// because it owns the conversation semantics above the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeProtocol {
    OpenAiRealtime,
    OpenAiLive,
    /// Deepgram's streaming speech to text: binary PCM frames in, JSON
    /// `Results` out, and `CloseStream` to finish.
    DeepgramListen,
    /// ElevenLabs Scribe's realtime speech to text: base64 audio chunks
    /// in, partial and committed transcripts out, and a chunk with
    /// `commit` to finish.
    ElevenLabsScribe,
}

/// Open the provider's realtime WebSocket for `model`, authenticated with
/// the provider's key and headers.
pub(crate) async fn connect(
    provider_key: &str,
    provider: &ProviderConfig,
    model: &str,
    intent: RealtimeIntent,
    timeout: Duration,
    transport: &dyn RealtimeTransport,
) -> Result<RealtimeConnection, Error> {
    let (url, protocol) = realtime_url(
        provider_key,
        provider.protocol,
        &provider.base_url,
        model,
        intent,
    )?;
    let mut headers = HeaderMap::new();
    for (name, value) in &provider.headers {
        let name: reqwest::header::HeaderName = name.parse().map_err(|_| {
            Error::InvalidConfig(format!(
                "provider `{provider_key}` header `{name}` is not a valid header name"
            ))
        })?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
            Error::InvalidConfig(format!(
                "provider `{provider_key}` header `{name}` has a value not valid in a header"
            ))
        })?;
        headers.insert(name, value);
    }
    if !provider.api_key.is_empty() {
        let (name, value) = match protocol {
            RealtimeProtocol::ElevenLabsScribe => (
                reqwest::header::HeaderName::from_static("xi-api-key"),
                provider.api_key.clone(),
            ),
            RealtimeProtocol::DeepgramListen => (
                reqwest::header::AUTHORIZATION,
                format!("Token {}", provider.api_key),
            ),
            RealtimeProtocol::OpenAiRealtime | RealtimeProtocol::OpenAiLive => (
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", provider.api_key),
            ),
        };
        let value = crate::protocol::sensitive_header(provider_key, &value)?;
        headers.insert(name, value);
    }

    let socket = transport
        .connect(url.as_str(), &headers, timeout)
        .await
        .map_err(|e| realtime_error(provider_key, e))?;
    Ok(RealtimeConnection {
        provider: provider_key.to_owned(),
        model: model.to_owned(),
        protocol,
        socket,
    })
}

/// Derive the realtime WebSocket URL from the provider's HTTP base URL:
/// `https://host/v1` becomes `wss://host/v1/realtime?model=...` for a
/// conversation and `wss://host/v1/realtime?intent=transcription` for a
/// transcription-only session. On Deepgram a transcription session is
/// `wss://host/v1/listen` with the model and the audio format.
fn realtime_url(
    provider_key: &str,
    kind: ProtocolKind,
    base_url: &str,
    model: &str,
    intent: RealtimeIntent,
) -> Result<(reqwest::Url, RealtimeProtocol), Error> {
    let mut url = reqwest::Url::parse(base_url).map_err(|e| {
        Error::InvalidConfig(format!(
            "provider `{provider_key}` base_url `{base_url}`: {e}"
        ))
    })?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => {
            return Err(Error::InvalidConfig(format!(
                "provider `{provider_key}` base_url scheme `{other}` has no WebSocket form"
            )));
        }
    };
    url.set_scheme(scheme).map_err(|_| {
        Error::InvalidConfig(format!(
            "provider `{provider_key}` base_url cannot switch to `{scheme}`"
        ))
    })?;
    let protocol = match (kind, intent) {
        (ProtocolKind::Deepgram, RealtimeIntent::Transcription { sample_rate }) => {
            let path = format!("{}/listen", url.path().trim_end_matches('/'));
            url.set_path(&path);
            url.query_pairs_mut()
                .append_pair("model", model)
                .append_pair("encoding", "linear16")
                .append_pair("sample_rate", &sample_rate.to_string())
                .append_pair("channels", "1")
                .append_pair("interim_results", "true")
                .append_pair("punctuate", "true")
                .append_pair("smart_format", "true");
            RealtimeProtocol::DeepgramListen
        }
        // The live model of a Scribe generation is its `_realtime` model,
        // so the alias names the generation for both the held clip and
        // the live session.
        (ProtocolKind::ElevenLabs, RealtimeIntent::Transcription { sample_rate }) => {
            let path = format!(
                "{}/speech-to-text/realtime",
                url.path().trim_end_matches('/')
            );
            url.set_path(&path);
            let live_model = if model.ends_with("_realtime") {
                model.to_owned()
            } else {
                format!("{model}_realtime")
            };
            url.query_pairs_mut()
                .append_pair("model_id", &live_model)
                .append_pair("audio_format", &format!("pcm_{sample_rate}"))
                .append_pair("commit_strategy", "manual");
            RealtimeProtocol::ElevenLabsScribe
        }
        (ProtocolKind::Deepgram | ProtocolKind::ElevenLabs, RealtimeIntent::Conversation) => {
            return Err(Error::Unsupported {
                provider: provider_key.to_owned(),
                feature: "realtime conversation",
            });
        }
        (_, RealtimeIntent::Conversation) => {
            if model.starts_with("gpt-live-") {
                let path = format!("{}/live/sessions", url.path().trim_end_matches('/'));
                url.set_path(&path);
                RealtimeProtocol::OpenAiLive
            } else {
                let path = format!("{}/realtime", url.path().trim_end_matches('/'));
                url.set_path(&path);
                url.query_pairs_mut().append_pair("model", model);
                RealtimeProtocol::OpenAiRealtime
            }
        }
        (_, RealtimeIntent::Transcription { .. }) => {
            let path = format!("{}/realtime", url.path().trim_end_matches('/'));
            url.set_path(&path);
            url.query_pairs_mut().append_pair("intent", "transcription");
            RealtimeProtocol::OpenAiRealtime
        }
    };
    Ok((url, protocol))
}

fn realtime_error(provider: &str, error: impl std::fmt::Display) -> Error {
    Error::Realtime {
        provider: provider.to_owned(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ProtocolKind, RealtimeIntent, RealtimeProtocol, realtime_url};

    #[test]
    fn derives_the_websocket_url_from_the_base_url() {
        let (url, protocol) = realtime_url(
            "p",
            ProtocolKind::OpenAiChat,
            "https://api.openai.com/v1",
            "gpt-realtime-2.1",
            RealtimeIntent::Conversation,
        )
        .unwrap();
        assert_eq!(protocol, RealtimeProtocol::OpenAiRealtime);
        assert_eq!(
            url.as_str(),
            "wss://api.openai.com/v1/realtime?model=gpt-realtime-2.1"
        );
        let (url, protocol) = realtime_url(
            "p",
            ProtocolKind::OpenAiChat,
            "http://localhost:8000/v1",
            "unmute-model",
            RealtimeIntent::Conversation,
        )
        .unwrap();
        assert_eq!(protocol, RealtimeProtocol::OpenAiRealtime);
        assert_eq!(
            url.as_str(),
            "ws://localhost:8000/v1/realtime?model=unmute-model"
        );
    }

    #[test]
    fn a_transcription_session_names_its_intent_and_not_a_model() {
        let (url, protocol) = realtime_url(
            "p",
            ProtocolKind::OpenAiChat,
            "https://api.openai.com/v1",
            "gpt-4o-transcribe",
            RealtimeIntent::Transcription {
                sample_rate: 24_000,
            },
        )
        .unwrap();
        assert_eq!(protocol, RealtimeProtocol::OpenAiRealtime);
        assert_eq!(
            url.as_str(),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
    }
}
