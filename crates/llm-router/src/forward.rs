//! The forward of a provider's own request: the router sends a request
//! that a client wrote in the provider's format with the provider key, makes
//! one attempt, and meters the usage of the answer.

use std::task::Poll;

use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName};
use tokio::sync::{mpsc, oneshot};

use crate::config::ProtocolKind;
use crate::error::Error;
use crate::protocol::{ByteStream, Meter, codec, unsupported_forward};
use crate::router::{MAX_BODY_BYTES, Router};
use crate::types::Usage;

/// The chunks that the pass-through holds for the meter before it waits.
const METER_CHANNEL_CAPACITY: usize = 64;

/// A request that a client wrote in a provider's own format.
#[derive(Debug, Clone)]
pub struct ForwardRequest {
    /// The API that the body is written in.
    pub wire: ProtocolKind,
    /// The path relative to the provider's `base_url`, such as `/messages`.
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// The provider's answer to a forwarded request.
pub struct Forwarded {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// The provider's bytes, unchanged. Drop it to close the connection.
    pub body: ByteStream,
    /// Resolves when the body ends, or when the caller drops it first.
    pub metered: oneshot::Receiver<Metered>,
}

/// The model and the usage that the answer to a forwarded request gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Metered {
    pub model: Option<String>,
    pub usage: Option<Usage>,
    /// `false` when the caller dropped the body or the transport failed
    /// before the body ended. `usage` then holds what the answer gave up to
    /// that point.
    pub complete: bool,
}

/// One item the pass-through hands to the meter.
enum Piece {
    Chunk(Bytes),
    /// The body ended.
    End,
}

impl Router {
    /// Forward a request that a client wrote in a provider's own format, with
    /// the provider key.
    ///
    /// The router sends `POST {base_url}{path}` with the caller's headers,
    /// then each provider `headers` entry whose name the caller did not set,
    /// then the provider key as the provider's codec sends it. The caller's
    /// own credential (`authorization`, `x-api-key`) never reaches the
    /// provider. The body goes unchanged.
    ///
    /// One attempt, with no fallback, no retry and no stream timeouts: the
    /// client of the forward owns its retries. Every HTTP status answers
    /// `Ok`; only a transport failure is an `Err`.
    ///
    /// The wire chooses the meter, and the provider's protocol chooses the
    /// credential. An `AnthropicMessages` wire forwards to a provider of
    /// that protocol. An `OpenAiResponses` or `OpenAiChat` wire forwards to
    /// a provider of either OpenAI protocol. Another pair is unsupported.
    pub async fn forward(
        &self,
        provider_key: &str,
        req: ForwardRequest,
    ) -> Result<Forwarded, Error> {
        let (provider, protocol) = self.provider(provider_key)?;
        let meter = codec(req.wire)
            .forward_meter()
            .ok_or_else(|| unsupported_forward(provider_key))?;
        // Without a leading slash, the path could extend the host of the
        // base URL and send the key to another server.
        if !req.path.starts_with('/') {
            return Err(Error::InvalidConfig(format!(
                "forward path `{}` must start with `/`",
                req.path
            )));
        }

        let mut headers = req.headers;
        headers.remove(AUTHORIZATION);
        headers.remove("x-api-key");
        // The meter reads the body, so the provider sends it uncompressed.
        headers.remove(ACCEPT_ENCODING);
        for (name, value) in &provider.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                Error::InvalidConfig(format!(
                    "provider `{provider_key}` header name `{name}` is not valid"
                ))
            })?;
            if !headers.contains_key(&name) {
                let value = value.parse().map_err(|_| {
                    Error::InvalidConfig(format!(
                        "provider `{provider_key}` header `{name}` has a value not valid in a header"
                    ))
                })?;
                headers.insert(name, value);
            }
        }
        protocol.forward(provider_key, provider, req.wire, &mut headers)?;

        let response = self
            .http
            .post(format!("{}{}", provider.base_url, req.path))
            .headers(headers)
            .body(req.body)
            .send()
            .await
            .map_err(|e| self.transport(provider_key, e))?;
        let status = response.status();
        let headers = response.headers().clone();
        let event_stream = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"));

        let (pieces, received) = mpsc::channel(METER_CHANNEL_CAPACITY);
        let (done, metered) = oneshot::channel();
        tokio::spawn(run_meter(meter, event_stream, received, done));

        let mut upstream = response.bytes_stream();
        let body = async_stream::stream! {
            while let Some(item) = upstream.next().await {
                match item {
                    Ok(chunk) => {
                        // The meter drains the channel to the end, so a send
                        // fails only when the meter task is gone. The bytes
                        // still pass to the caller.
                        let _ = pieces.send(Piece::Chunk(chunk.clone())).await;
                        yield Ok(chunk);
                    }
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                }
            }
            let _ = pieces.send(Piece::End).await;
        }
        .boxed();

        Ok(Forwarded {
            status,
            headers,
            body,
            metered,
        })
    }
}

/// Read the answer from the pass-through and resolve `done` when the body
/// ends or the pass-through goes away.
async fn run_meter(
    mut meter: Box<dyn Meter>,
    event_stream: bool,
    mut pieces: mpsc::Receiver<Piece>,
    done: oneshot::Sender<Metered>,
) {
    let complete = if event_stream {
        meter_events(meter.as_mut(), &mut pieces).await
    } else {
        meter_body(meter.as_mut(), &mut pieces).await
    };
    let (model, usage) = meter.finish();
    // The caller may not wait for the meter.
    let _ = done.send(Metered {
        model,
        usage,
        complete,
    });
}

/// Feed each server-sent event to the meter. Returns whether the body
/// ended.
async fn meter_events(meter: &mut dyn Meter, pieces: &mut mpsc::Receiver<Piece>) -> bool {
    let mut complete = false;
    {
        let chunks = futures::stream::poll_fn(|cx| match pieces.poll_recv(cx) {
            Poll::Ready(Some(Piece::Chunk(chunk))) => {
                Poll::Ready(Some(Ok::<_, std::convert::Infallible>(chunk)))
            }
            Poll::Ready(Some(Piece::End)) => {
                complete = true;
                Poll::Ready(None)
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        });
        let mut events = chunks.eventsource();
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => meter.event(&event.event, &event.data),
                // A body that is not a valid event stream meters no more.
                Err(_) => break,
            }
        }
    }
    // Drain what the parser left, so the pass-through never waits on a
    // full channel.
    while !complete {
        match pieces.recv().await {
            Some(Piece::Chunk(_)) => {}
            Some(Piece::End) => complete = true,
            None => break,
        }
    }
    complete
}

/// Feed the whole body to the meter, up to [`MAX_BODY_BYTES`]. A larger
/// body meters nothing. Returns whether the body ended.
async fn meter_body(meter: &mut dyn Meter, pieces: &mut mpsc::Receiver<Piece>) -> bool {
    let mut body = Vec::new();
    let mut over_cap = false;
    while let Some(piece) = pieces.recv().await {
        match piece {
            Piece::Chunk(chunk) if !over_cap => {
                if body.len() + chunk.len() > MAX_BODY_BYTES {
                    over_cap = true;
                    body = Vec::new();
                } else {
                    body.extend_from_slice(&chunk);
                }
            }
            Piece::Chunk(_) => {}
            Piece::End => {
                if !over_cap {
                    meter.body(&body);
                }
                return true;
            }
        }
    }
    false
}
