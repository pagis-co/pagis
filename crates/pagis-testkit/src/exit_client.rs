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
        let url = format!("ws://{}/api/v1/hosts/{host_id}/exit", daemon.addr);
        let mut socket = match connect_async(daemon.ws_request_as(&url, cookie)).await {
            Ok((socket, _)) => socket,
            Err(Error::Http(response)) => return Err(response.status().as_u16()),
            Err(error) => panic!("the exit socket did not open: {error}"),
        };
        let (mut pipe, client_app_end) = tokio::io::duplex(64 * 1024);
        let serve = tokio::spawn(exit.serve(client_app_end.compat()));
        let close = Arc::new(Mutex::new(None));
        let closed = Arc::clone(&close);
        let pump = tokio::spawn(async move {
            let mut buffer = vec![0; 64 * 1024];
            loop {
                tokio::select! {
                    frame = socket.next() => match frame {
                        Some(Ok(Message::Binary(bytes))) => {
                            if pipe.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            *closed.lock().expect("the close code") =
                                Some(frame.map_or(1005, |frame| frame.code.into()));
                            return;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => return,
                    },
                    read = pipe.read(&mut buffer) => match read {
                        Ok(0) | Err(_) => return,
                        Ok(count) => {
                            let frame = Message::Binary(buffer[..count].to_vec().into());
                            if socket.send(frame).await.is_err() {
                                return;
                            }
                        }
                    },
                }
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
