//! Realtime voice sockets: authenticated endpoint routing.
//!
//! The router resolves the model alias, opens the upstream WebSocket with
//! the provider's credentials, and hands the socket to the caller. The
//! caller pumps frames in the protocol named by [`RealtimeProtocol`].
//! The router does not translate events. The caller owns the adapter
//! that maps those frames to its product-level session contract.

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::config::ProviderConfig;
use crate::error::Error;

/// The frames of a realtime socket, re-exported so callers do not depend on
/// `tokio-tungstenite` directly. Realtime events are `Text` frames of
/// OpenAI Realtime JSON.
pub type RealtimeMessage = Message;

/// The socket type behind [`RealtimeConnection`], so a caller can split
/// it into a sink and a stream without naming `tokio-tungstenite`.
pub type RealtimeSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

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
            .close(None)
            .await
            .map_err(|e| realtime_error(&self.provider, e))
    }
}

/// What a realtime socket is opened for. OpenAI keys the socket by
/// query: a conversation names its model, a transcription-only session
/// names its intent and picks the transcription model in `session.update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeIntent {
    Conversation,
    Transcription,
}

/// The event contract carried by one conversation socket.
///
/// The router opens the right endpoint. The caller owns the adapter
/// because it owns the conversation semantics above the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeProtocol {
    OpenAiRealtime,
    OpenAiLive,
}

/// Open the provider's realtime WebSocket for `model`, authenticated with
/// the provider's key and headers.
pub(crate) async fn connect(
    provider_key: &str,
    provider: &ProviderConfig,
    model: &str,
    intent: RealtimeIntent,
    timeout: std::time::Duration,
) -> Result<RealtimeConnection, Error> {
    let (url, protocol) = realtime_url(provider_key, &provider.base_url, model, intent)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| realtime_error(provider_key, e))?;
    let headers = request.headers_mut();
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
        let value = crate::protocol::sensitive_header(
            provider_key,
            &format!("Bearer {}", provider.api_key),
        )?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }

    let (socket, _response) = tokio::time::timeout(timeout, connect_async(request))
        .await
        .map_err(|_| realtime_error(provider_key, "timed out opening the WebSocket"))?
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
/// transcription-only session.
fn realtime_url(
    provider_key: &str,
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
    let protocol = match intent {
        RealtimeIntent::Conversation => {
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
        RealtimeIntent::Transcription => {
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
    use super::{RealtimeIntent, RealtimeProtocol, realtime_url};

    #[test]
    fn derives_the_websocket_url_from_the_base_url() {
        let (url, protocol) = realtime_url(
            "p",
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
            "https://api.openai.com/v1",
            "gpt-4o-transcribe",
            RealtimeIntent::Transcription,
        )
        .unwrap();
        assert_eq!(protocol, RealtimeProtocol::OpenAiRealtime);
        assert_eq!(
            url.as_str(),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
    }
}
