//! The axum WebSocket adapter for the frame protocols.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage, WebSocket};
use futures::{Sink, Stream};

use crate::{CloseFrame, Message, MessageSocket};

/// Carry protocol frames over an upgraded axum WebSocket.
pub(crate) struct AxumSocket(pub(crate) WebSocket);

impl Stream for AxumSocket {
    type Item = Result<Message, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|frame| frame.map(|frame| frame.map(from_axum)))
    }
}

impl Sink<Message> for AxumSocket {
    type Error = axum::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, frame: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.0).start_send(into_axum(frame))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

impl MessageSocket for AxumSocket {
    type SocketError = axum::Error;
}

fn from_axum(frame: AxumMessage) -> Message {
    match frame {
        AxumMessage::Text(text) => Message::Text(text.to_string()),
        AxumMessage::Binary(bytes) => Message::Binary(bytes),
        AxumMessage::Ping(bytes) => Message::Ping(bytes),
        AxumMessage::Pong(bytes) => Message::Pong(bytes),
        AxumMessage::Close(frame) => Message::Close(frame.map(|frame| CloseFrame {
            code: frame.code,
            reason: frame.reason.to_string(),
        })),
    }
}

fn into_axum(frame: Message) -> AxumMessage {
    match frame {
        Message::Text(text) => AxumMessage::Text(text.into()),
        Message::Binary(bytes) => AxumMessage::Binary(bytes),
        Message::Ping(bytes) => AxumMessage::Ping(bytes),
        Message::Pong(bytes) => AxumMessage::Pong(bytes),
        Message::Close(frame) => AxumMessage::Close(frame.map(|frame| AxumCloseFrame {
            code: frame.code,
            reason: frame.reason.into(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage};

    use crate::{CloseFrame, Message};

    fn frames() -> Vec<(Message, AxumMessage)> {
        let mut frames = vec![];
        for text in ["", "a frame\0 with 日本語 🦀"] {
            frames.push((Message::Text(text.into()), AxumMessage::Text(text.into())));
        }
        for bytes in [vec![], vec![0, 1, 127, 128, 255]] {
            frames.extend([
                (
                    Message::Binary(bytes.clone().into()),
                    AxumMessage::Binary(bytes.clone().into()),
                ),
                (
                    Message::Ping(bytes.clone().into()),
                    AxumMessage::Ping(bytes.clone().into()),
                ),
                (
                    Message::Pong(bytes.clone().into()),
                    AxumMessage::Pong(bytes.into()),
                ),
            ]);
        }
        frames.push((Message::Close(None), AxumMessage::Close(None)));
        for (code, reason) in [(1000, ""), (1008, "the session ended"), (4001, "日本語 🦀")] {
            frames.push((
                Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })),
                AxumMessage::Close(Some(AxumCloseFrame {
                    code,
                    reason: reason.into(),
                })),
            ));
        }
        frames
    }

    #[test]
    fn each_axum_frame_keeps_its_kind_and_payload() {
        for (expected, frame) in frames() {
            assert_eq!(super::from_axum(frame), expected);
        }
    }

    #[test]
    fn each_protocol_frame_keeps_its_kind_and_payload() {
        for (frame, expected) in frames() {
            assert_eq!(super::into_axum(frame), expected);
        }
    }
}
