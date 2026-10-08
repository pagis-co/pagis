//! The Client App's end of a session socket, in this process, for tests.
//!
//! [`FakeClientApp`] speaks the protocol of the Client App: it accepts
//! the yamux streams of the daemon, reads each open request, answers one
//! line as the test says, and after an answer that opened the session it
//! writes back each byte that it reads, as `cat` does. It starts no
//! process. The Client App's own session code has its own tests and an
//! interop test with the daemon.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::{AsyncReadExt, AsyncWriteExt};

use crate::host_sessions::{
    OpenAnswer, OpenFailure, OpenRequest, REQUEST_LIMIT, read_line, yamux_config,
};

/// What the fake answers each open request with.
#[derive(Debug, Clone)]
pub enum FakeAnswer {
    /// `ok` in this directory, then the bytes back.
    Open { cwd: String },
    /// A refusal with this code and message.
    Refuse { code: OpenFailure, message: String },
    /// These bytes, then nothing while the stream stays open.
    Raw(Vec<u8>),
    /// No answer, while the stream stays open.
    Silent,
}

/// One fake Client App that answers every stream the same way.
pub struct FakeClientApp {
    answer: FakeAnswer,
    requests: Mutex<Vec<OpenRequest>>,
    streams: AtomicUsize,
}

impl FakeClientApp {
    pub fn answering(answer: FakeAnswer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            requests: Mutex::new(Vec::new()),
            streams: AtomicUsize::new(0),
        })
    }

    /// A Client App that opens every session in `cwd`.
    pub fn opening(cwd: &str) -> Arc<Self> {
        Self::answering(FakeAnswer::Open {
            cwd: cwd.to_string(),
        })
    }

    /// The open requests that it read, in order.
    pub fn requests(&self) -> Vec<OpenRequest> {
        self.requests.lock().expect("fake requests").clone()
    }

    /// How many streams the daemon opened to it.
    pub fn streams(&self) -> usize {
        self.streams.load(Ordering::SeqCst)
    }

    /// Serve the Client App's end of one session socket until it ends.
    pub async fn serve<T>(self: Arc<Self>, socket: T)
    where
        T: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let mut connection = yamux::Connection::new(socket, yamux_config(), yamux::Mode::Server);
        while let Some(Ok(stream)) =
            futures::future::poll_fn(|cx| connection.poll_next_inbound(cx)).await
        {
            self.streams.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(Arc::clone(&self).carry(stream));
        }
    }

    /// One stream: the request, the answer, then the bytes.
    async fn carry(self: Arc<Self>, mut stream: yamux::Stream) {
        let Ok(line) = read_line(&mut stream, REQUEST_LIMIT).await else {
            return;
        };
        let Ok(request) = serde_json::from_str::<OpenRequest>(&line) else {
            return;
        };
        self.requests.lock().expect("fake requests").push(request);
        let answer = match &self.answer {
            FakeAnswer::Open { cwd } => OpenAnswer::Opened { cwd: cwd.clone() }.line(),
            FakeAnswer::Refuse { code, message } => OpenAnswer::Refused {
                error: *code,
                message: message.clone(),
            }
            .line(),
            FakeAnswer::Raw(bytes) => {
                let _ = stream.write_all(bytes).await;
                let _ = stream.flush().await;
                return hold(stream).await;
            }
            FakeAnswer::Silent => return hold(stream).await,
        };
        if stream.write_all(answer.as_bytes()).await.is_err() || stream.flush().await.is_err() {
            return;
        }
        if let FakeAnswer::Open { .. } = self.answer {
            let (mut reader, mut writer) = stream.split();
            let _ = futures::io::copy(&mut reader, &mut writer).await;
            let _ = writer.close().await;
        } else {
            let _ = stream.close().await;
        }
    }
}

/// Keep a stream open, unanswered, until the runtime ends.
async fn hold(stream: yamux::Stream) {
    std::future::pending::<()>().await;
    drop(stream);
}
