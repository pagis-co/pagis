//! A bounded pair of MessageSockets with no listener or frame encoding.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::channel::mpsc;
use futures::{Sink, Stream};

use crate::{Message, MessageSocket};

/// One end of an in-memory pair. Each direction holds at most 32 frames
/// plus the sender's reserved frame. A full direction applies backpressure.
pub struct MemorySocket {
    incoming: mpsc::Receiver<Message>,
    outgoing: mpsc::Sender<Message>,
}

/// Two message sockets that send each frame to the other end.
pub fn message_socket_pair() -> (MemorySocket, MemorySocket) {
    let (a_send, b_receive) = mpsc::channel(32);
    let (b_send, a_receive) = mpsc::channel(32);
    (
        MemorySocket {
            incoming: a_receive,
            outgoing: a_send,
        },
        MemorySocket {
            incoming: b_receive,
            outgoing: b_send,
        },
    )
}

impl Stream for MemorySocket {
    type Item = Result<Message, mpsc::SendError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming)
            .poll_next(cx)
            .map(|frame| frame.map(Ok))
    }
}

impl Sink<Message> for MemorySocket {
    type Error = mpsc::SendError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, frame: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.outgoing).start_send(frame)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_close(cx)
    }
}

impl MessageSocket for MemorySocket {
    type SocketError = mpsc::SendError;
}
