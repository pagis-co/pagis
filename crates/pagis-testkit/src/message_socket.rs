//! A bounded pair of message sockets. No listener or byte encoding is used.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::channel::mpsc;
use futures::{Sink, SinkExt, Stream, StreamExt};
use pagis_server::{CloseFrame, Message, MessageSocket};
use tokio_tungstenite::tungstenite::{self, Message as ClientMessage};

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

/// The client frame API shared by the TCP and in-memory test transports.
pub trait ClientTransport:
    Stream<Item = Result<ClientMessage, tungstenite::Error>>
    + Sink<ClientMessage, Error = tungstenite::Error>
    + Unpin
    + Send
{
}

impl<T> ClientTransport for T where
    T: Stream<Item = Result<ClientMessage, tungstenite::Error>>
        + Sink<ClientMessage, Error = tungstenite::Error>
        + Unpin
        + Send
{
}

pub type Socket = Pin<Box<dyn ClientTransport>>;

pub(crate) fn client_socket(socket: MemorySocket) -> Socket {
    Box::pin(
        socket
            .sink_map_err(channel_error)
            .with(|frame| futures::future::ready(server_frame(frame)))
            .map(|frame| frame.map(client_frame).map_err(channel_error)),
    )
}

fn channel_error(error: mpsc::SendError) -> tungstenite::Error {
    tungstenite::Error::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, error))
}

fn client_frame(frame: Message) -> ClientMessage {
    match frame {
        Message::Text(text) => ClientMessage::text(text.as_str()),
        Message::Binary(bytes) => ClientMessage::Binary(bytes),
        Message::Ping(bytes) => ClientMessage::Ping(bytes),
        Message::Pong(bytes) => ClientMessage::Pong(bytes),
        Message::Close(frame) => {
            ClientMessage::Close(frame.map(|frame| tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.as_str().into(),
            }))
        }
    }
}

fn server_frame(frame: ClientMessage) -> Result<Message, tungstenite::Error> {
    Ok(match frame {
        ClientMessage::Text(text) => Message::Text(text.as_str().into()),
        ClientMessage::Binary(bytes) => Message::Binary(bytes),
        ClientMessage::Ping(bytes) => Message::Ping(bytes),
        ClientMessage::Pong(bytes) => Message::Pong(bytes),
        ClientMessage::Close(frame) => Message::Close(frame.map(|frame| CloseFrame {
            code: frame.code.into(),
            reason: frame.reason.as_str().into(),
        })),
        ClientMessage::Frame(_) => {
            return Err(tungstenite::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a message socket takes messages, not encoded frames",
            )));
        }
    })
}

impl crate::TestDaemon {
    /// Open one of the five message protocols. A daemon with listeners
    /// tests the HTTP upgrade; a daemon without them serves frames directly.
    pub async fn open_socket(&self, path: &str, cookie: &str) -> Result<Socket, u16> {
        if self.server.is_some() {
            let url = format!("ws://{}{path}", self.addr);
            return match tokio_tungstenite::connect_async(self.ws_request_as(&url, cookie)).await {
                Ok((socket, _)) => Ok(Box::pin(socket)),
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    Err(response.status().as_u16())
                }
                Err(error) => panic!("the socket did not open: {error}"),
            };
        }
        use axum::extract::FromRequestParts;
        let (mut parts, _) = axum::http::Request::builder()
            .uri(path)
            .header("cookie", cookie)
            .body(())
            .unwrap()
            .into_parts();
        let tenant = pagis_server::resolve_session(&self.state, &parts.headers)
            .await
            .map_err(|error| error.status.as_u16())?
            .ok_or(401u16)?;
        parts.extensions.insert(tenant);
        let live = pagis_server::LiveTenant::from_request_parts(&mut parts, &self.state)
            .await
            .map_err(|error| error.status.as_u16())?;
        let segments: Vec<_> = path.trim_start_matches('/').split('/').collect();
        let (protocol, id) = match segments.as_slice() {
            ["api", "v1", "ws"] => ("events", ""),
            ["api", "v1", "channels", id, "dictate"] => ("dictation", *id),
            ["api", "v1", "calls", id, "listen"] => ("listen", *id),
            ["api", "v1", "hosts", id, "sessions"] => ("sessions", *id),
            ["api", "v1", "hosts", id, "exit"] => ("exit", *id),
            _ => return Err(404),
        };
        let host = match protocol {
            "sessions" => Some(
                pagis_server::session_socket::check_host(
                    &self.state,
                    &live.tenant,
                    &pagis_core::HostId::from(id.to_string()),
                )
                .await,
            ),
            "exit" => Some(
                pagis_server::native::exit_socket::check_host(
                    &self.state,
                    &live.tenant,
                    &pagis_core::HostId::from(id.to_string()),
                )
                .await,
            ),
            _ => None,
        }
        .transpose()
        .map_err(|error| error.status.as_u16())?;
        let id = id.to_string();
        let (server, client) = message_socket_pair();
        let state = Arc::clone(&self.state);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let served = async move {
                match protocol {
                    "events" => pagis_server::ws::serve_events(server, state, live).await,
                    "dictation" => {
                        pagis_server::voice::serve_dictation(server, state, live, id).await
                    }
                    "listen" => pagis_server::calls::serve_listen(server, state, live, id).await,
                    "sessions" => {
                        pagis_server::session_socket::serve_sessions(
                            server,
                            state,
                            live,
                            host.unwrap(),
                        )
                        .await;
                        Ok(())
                    }
                    "exit" => {
                        pagis_server::native::exit_socket::serve_exit(
                            server,
                            state,
                            live,
                            host.unwrap(),
                        )
                        .await;
                        Ok(())
                    }
                    _ => unreachable!(),
                }
            };
            tokio::select! {
                () = cancel.cancelled() => {}
                result = served => result.expect("the message protocol completes"),
            }
        });
        Ok(client_socket(client))
    }
}
