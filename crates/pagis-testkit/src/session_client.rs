//! A Pagis client that starts the Coding Sessions of its Host
//! (ADR-0033).
//!
//! It opens the session socket of a registered Host over a real
//! WebSocket, with the Session cookie, and serves the streams of the
//! daemon with [`FakeClientApp`], which speaks the protocol of the Client
//! App: yamux, the open request and the answer line. It starts no
//! process: after `ok` it writes back each byte that it reads. The Client
//! App's own session code has its own tests and an interop test with the
//! daemon.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_broker::fake::FakeClientApp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::TestDaemon;

/// The open session socket of one Host. Dropping it closes the socket,
/// which ends every Coding Session on it.
pub struct SessionClient {
    pump: tokio::task::JoinHandle<()>,
    serve: tokio::task::JoinHandle<()>,
    /// The code of the daemon's Close frame, once one arrived.
    close: Arc<Mutex<Option<u16>>>,
}

impl SessionClient {
    /// Open the session socket of `host_id` with `cookie`, and serve its
    /// streams with `client_app`. The answer is the HTTP status of a
    /// refused handshake.
    pub async fn connect(
        daemon: &TestDaemon,
        cookie: &str,
        host_id: &str,
        client_app: Arc<FakeClientApp>,
    ) -> Result<Self, u16> {
        let url = format!("ws://{}/api/v1/hosts/{host_id}/sessions", daemon.addr);
        let socket = match connect_async(daemon.ws_request_as(&url, cookie)).await {
            Ok((socket, _)) => socket,
            Err(Error::Http(response)) => return Err(response.status().as_u16()),
            Err(error) => panic!("the session socket did not open: {error}"),
        };
        let (pipe, client_app_end) = tokio::io::duplex(64 * 1024);
        let serve = tokio::spawn(client_app.serve(client_app_end.compat()));
        let close = Arc::new(Mutex::new(None));
        let closed = Arc::clone(&close);
        // The two directions run apart, so a full pipe one way never
        // stops the bytes of the other way.
        let pump = tokio::spawn(async move {
            let (mut sink, mut stream) = socket.split();
            let (mut pipe_reader, mut pipe_writer) = tokio::io::split(pipe);
            let inbound = async {
                while let Some(frame) = stream.next().await {
                    match frame {
                        Ok(Message::Binary(bytes)) => {
                            if pipe_writer.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                        Ok(Message::Close(frame)) => {
                            *closed.lock().expect("the close code") =
                                Some(frame.map_or(1005, |frame| frame.code.into()));
                            return;
                        }
                        Ok(_) => {}
                        Err(_) => return,
                    }
                }
            };
            let outbound = async {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    let count = match pipe_reader.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(count) => count,
                    };
                    let frame = Message::Binary(buffer[..count].to_vec().into());
                    if sink.send(frame).await.is_err() {
                        return;
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
        tokio::time::timeout(Duration::from_secs(5), &mut self.pump)
            .await
            .expect("the session socket is still open after five seconds")
            .expect("the pump ends");
        *self.close.lock().expect("the close code")
    }

    /// Close the socket, as a Client App that goes away does.
    pub fn disconnect(self) {
        drop(self);
    }
}

impl Drop for SessionClient {
    fn drop(&mut self) {
        self.pump.abort();
        self.serve.abort();
    }
}
