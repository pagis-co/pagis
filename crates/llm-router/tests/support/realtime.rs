//! Frame channels for transport and adapter tests. No port opens.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::{Sink, Stream, StreamExt};
use llm_router::{
    RealtimeConnectFuture, RealtimeError, RealtimeMessage, RealtimeSocket, RealtimeTransport,
};
use reqwest::header::HeaderMap;

struct Socket {
    incoming: UnboundedReceiver<RealtimeMessage>,
    outgoing: UnboundedSender<RealtimeMessage>,
}

impl Stream for Socket {
    type Item = Result<RealtimeMessage, RealtimeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_next_unpin(cx).map(|frame| frame.map(Ok))
    }
}

impl Sink<RealtimeMessage> for Socket {
    type Error = RealtimeError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, frame: RealtimeMessage) -> Result<(), Self::Error> {
        self.outgoing.unbounded_send(frame).map_err(Into::into)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.outgoing.close_channel();
        Poll::Ready(Ok(()))
    }
}

pub struct MemoryTransport {
    socket: Mutex<Option<RealtimeSocket>>,
    pub requests: Mutex<Vec<(String, HeaderMap, Duration)>>,
}

impl MemoryTransport {
    pub fn pair() -> (Arc<Self>, RealtimeSocket) {
        let (to_host, host_incoming) = unbounded();
        let (to_peer, peer_incoming) = unbounded();
        let socket = Box::pin(Socket {
            incoming: host_incoming,
            outgoing: to_peer,
        });
        let peer = Box::pin(Socket {
            incoming: peer_incoming,
            outgoing: to_host,
        });
        (
            Arc::new(Self {
                socket: Mutex::new(Some(socket)),
                requests: Mutex::new(Vec::new()),
            }),
            peer,
        )
    }
}

impl RealtimeTransport for MemoryTransport {
    fn connect<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
        timeout: Duration,
    ) -> RealtimeConnectFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap()
                .push((url.to_string(), headers.clone(), timeout));
            self.socket
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| "no test socket remains".into())
        })
    }
}
