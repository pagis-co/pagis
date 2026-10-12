//! The frames of a protocol, separate from an HTTP upgrade.

use std::error::Error;
use std::future::Future;

use bytes::Bytes;
use futures::{Sink, SinkExt, Stream};

/// One frame of a message protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
    Close(Option<CloseFrame>),
}

/// The code and reason that end a message protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    pub code: u16,
    pub reason: String,
}

/// A bidirectional message transport. Text and binary frames keep their
/// boundaries. Close, Ping and Pong frames have the WebSocket meanings.
/// The host supplies the transport and its error type.
pub trait MessageSocket:
    Stream<Item = Result<Message, Self::SocketError>>
    + Sink<Message, Error = Self::SocketError>
    + Unpin
    + Send
{
    type SocketError: Error + Send + Sync + 'static;

    /// Flush outgoing frames and close the sending side.
    fn close(&mut self) -> impl Future<Output = Result<(), Self::SocketError>> + Send
    where
        Self: Sized,
    {
        SinkExt::close(self)
    }
}
