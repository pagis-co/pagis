//! A Pagis client that acts as the Home Exit of its Host (ADR-0029).
//!
//! It opens the exit socket of a registered Host over a real WebSocket,
//! with the Session cookie, and serves the streams of the daemon with
//! [`FakeHomeExit`], which speaks the protocol of the Client App: yamux,
//! the preamble and the status line. A test reaches no site on the
//! internet, so the fake maps each destination to a target of the test.
//! The Client App's own exit code has its own tests and an interop test
//! with the daemon.

use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use pagis_computer::fake::FakeHomeExit;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::TestDaemon;

/// The open exit socket of one Host. Dropping it closes the socket,
/// which makes the Host absent as a Home Exit.
pub struct ExitClient {
    pump: tokio::task::JoinHandle<()>,
    serve: tokio::task::JoinHandle<()>,
    /// The code of the daemon's Close frame, once one arrived.
    close: Arc<Mutex<Option<u16>>>,
}

/// How the client relays the bytes between its fake Home Exit and the
/// socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relay {
    /// The bytes as they are.
    Plain,
    /// Each frame of the fake goes out whole, and a yamux Ping follows
    /// it, which the daemon owes an answer to. The answers are taken out
    /// on the way in. A peer that measures its round trip pings the same
    /// way, every ten seconds for the yamux crate; this one pings at
    /// every frame, so the yamux of the daemon often holds an answer to
    /// send while the bytes of a bulk transfer fill the socket.
    Pinging,
}

/// The opaque values of the pings of [`Relay::Pinging`]. The yamux crate
/// counts its own pings up from zero, so its answers never reach these.
const PING_BASE: u32 = 0x8000_0000;

/// The length of the header of a yamux frame, and the types and flags
/// that the relay reads.
const HEADER: usize = 12;
const DATA: u8 = 0;
const PING: u8 = 2;
const SYN: u16 = 0x1;
const ACK: u16 = 0x2;

impl ExitClient {
    /// Open the exit socket of `host_id` with `cookie`, and serve its
    /// streams with `exit`. The answer is the HTTP status of a refused
    /// handshake.
    pub async fn connect(
        daemon: &TestDaemon,
        cookie: &str,
        host_id: &str,
        exit: Arc<FakeHomeExit>,
    ) -> Result<Self, u16> {
        Self::connect_with(daemon, cookie, host_id, exit, Relay::Plain).await
    }

    /// The same, with a yamux Ping after each frame of the fake (see
    /// [`Relay::Pinging`]).
    pub async fn connect_pinging(
        daemon: &TestDaemon,
        cookie: &str,
        host_id: &str,
        exit: Arc<FakeHomeExit>,
    ) -> Result<Self, u16> {
        Self::connect_with(daemon, cookie, host_id, exit, Relay::Pinging).await
    }

    async fn connect_with(
        daemon: &TestDaemon,
        cookie: &str,
        host_id: &str,
        exit: Arc<FakeHomeExit>,
        relay: Relay,
    ) -> Result<Self, u16> {
        let url = format!("ws://{}/api/v1/hosts/{host_id}/exit", daemon.addr);
        let socket = match connect_async(daemon.ws_request_as(&url, cookie)).await {
            Ok((socket, _)) => socket,
            Err(Error::Http(response)) => return Err(response.status().as_u16()),
            Err(error) => panic!("the exit socket did not open: {error}"),
        };
        let (pipe, client_app_end) = tokio::io::duplex(64 * 1024);
        let serve = tokio::spawn(exit.serve(client_app_end.compat()));
        let close = Arc::new(Mutex::new(None));
        let closed = Arc::clone(&close);
        // The two directions run apart, so a full pipe one way never
        // stops the bytes of the other way.
        let pump = tokio::spawn(async move {
            let (mut sink, mut stream) = socket.split();
            let (mut pipe_reader, mut pipe_writer) = tokio::io::split(pipe);
            let inbound = async {
                let mut pending = Vec::new();
                while let Some(frame) = stream.next().await {
                    let bytes = match frame {
                        Ok(Message::Binary(bytes)) => bytes,
                        Ok(Message::Close(frame)) => {
                            *closed.lock().expect("the close code") =
                                Some(frame.map_or(1005, |frame| frame.code.into()));
                            return;
                        }
                        Ok(_) => continue,
                        Err(_) => return,
                    };
                    let bytes = match relay {
                        Relay::Plain => bytes.to_vec(),
                        Relay::Pinging => {
                            pending.extend_from_slice(&bytes);
                            whole_frames(&mut pending)
                                .into_iter()
                                .filter(|frame| !answers_a_relay_ping(frame))
                                .flatten()
                                .collect()
                        }
                    };
                    if pipe_writer.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
            };
            let outbound = async {
                let mut buffer = vec![0; 64 * 1024];
                let mut pending = Vec::new();
                let mut pings = PING_BASE;
                loop {
                    let count = match pipe_reader.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(count) => count,
                    };
                    let messages = match relay {
                        Relay::Plain => vec![buffer[..count].to_vec()],
                        Relay::Pinging => {
                            pending.extend_from_slice(&buffer[..count]);
                            let mut messages = Vec::new();
                            for frame in whole_frames(&mut pending) {
                                messages.push(frame);
                                messages.push(ping(pings));
                                pings = pings.wrapping_add(1).max(PING_BASE);
                            }
                            messages
                        }
                    };
                    for message in messages {
                        if sink.send(Message::Binary(message.into())).await.is_err() {
                            return;
                        }
                    }
                }
            };
            tokio::select! {
                () = inbound => {}
                () = outbound => {}
            }
        });
        Ok(Self { pump, serve, close })
    }

    /// Wait for the socket to end, and answer the code of the daemon's
    /// Close frame, or `None` when it ended with none. The test fails
    /// when the socket stays open for five seconds.
    pub async fn closed(&mut self) -> Option<u16> {
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.pump)
            .await
            .expect("the exit socket is still open after five seconds")
            .expect("the pump ends");
        *self.close.lock().expect("the close code")
    }

    /// Close the socket, as a Client App that goes away does.
    pub fn disconnect(self) {
        drop(self);
    }
}

impl Drop for ExitClient {
    fn drop(&mut self) {
        self.pump.abort();
        self.serve.abort();
    }
}

/// Take each whole yamux frame out of the front of `bytes`. A Data frame
/// carries its body; every other frame is its header alone.
fn whole_frames(bytes: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    while bytes.len() >= HEADER {
        let length = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let size = HEADER + if bytes[1] == DATA { length } else { 0 };
        if bytes.len() < size {
            break;
        }
        frames.push(bytes.drain(..size).collect());
    }
    frames
}

/// A Ping of the relay, with its opaque value.
fn ping(opaque: u32) -> Vec<u8> {
    let mut frame = vec![0, PING];
    frame.extend_from_slice(&SYN.to_be_bytes());
    frame.extend_from_slice(&0u32.to_be_bytes());
    frame.extend_from_slice(&opaque.to_be_bytes());
    frame
}

/// Whether `frame` is the daemon's answer to a Ping of the relay.
fn answers_a_relay_ping(frame: &[u8]) -> bool {
    let flags = u16::from_be_bytes([frame[2], frame[3]]);
    let opaque = u32::from_be_bytes([frame[8], frame[9], frame[10], frame[11]]);
    frame[1] == PING && flags & ACK != 0 && opaque >= PING_BASE
}
