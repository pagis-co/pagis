//! A model server for tests, beside the seam as ADR-0020 asks.
//!
//! [`FakeModelSessions`] opens a [`ModelPeer`] the test scripts: it
//! reads every event the bridge sent, and it plays speech, transcripts,
//! function calls and errors back. It can also drop the socket, so the
//! reconnect path runs with no network.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{Notify, mpsc};

use crate::model::{ClientCommand, ModelError, ModelSession, ModelSessions, ServerEvent};

/// The model server a test writes the script for.
#[derive(Default)]
pub struct ModelPeer {
    sent: Mutex<Vec<Value>>,
    arrived: Notify,
    /// Into the bridge. `None` once the socket is dropped.
    feed: Mutex<Option<mpsc::UnboundedSender<Result<ServerEvent, ModelError>>>>,
}

impl ModelPeer {
    /// Every event the bridge sent, in order.
    pub fn sent(&self) -> Vec<Value> {
        self.sent.lock().unwrap().clone()
    }

    /// The events of one type the bridge sent, in order.
    pub fn sent_of(&self, event_type: &str) -> Vec<Value> {
        self.sent()
            .into_iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
            .collect()
    }

    /// Wait until the bridge has sent an event of this type, and answer
    /// with the first one. A test on a paused clock uses it in place of
    /// a sleep.
    pub async fn wait_for(&self, event_type: &str) -> Value {
        loop {
            let waiting = self.arrived.notified();
            if let Some(event) = self.sent_of(event_type).into_iter().next() {
                return event;
            }
            waiting.await;
        }
    }

    /// The provider reports that the Remote Party started to speak.
    pub fn speech_started(&self) {
        self.feed(ServerEvent::SpeechStarted);
    }

    /// One piece of the model's speech, in G.711 mu-law.
    pub fn speak(&self, item_id: &str, audio: &[u8]) {
        self.feed(ServerEvent::AudioDelta {
            item_id: Some(item_id.to_string()),
            audio: bytes::Bytes::copy_from_slice(audio),
        });
    }

    /// What the model said.
    pub fn agent_transcript(&self, transcript: &str) {
        self.feed(ServerEvent::AgentTranscript(transcript.to_string()));
    }

    /// What the Remote Party said.
    pub fn caller_transcript(&self, transcript: &str) {
        self.feed(ServerEvent::CallerTranscript(transcript.to_string()));
    }

    /// The model calls one tool.
    pub fn function_call(&self, call_id: &str, name: &str, arguments: Value) {
        self.feed(ServerEvent::FunctionCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
    }

    /// One response completed, with what it cost.
    pub fn response_done(&self, input_tokens: u64, output_tokens: u64) {
        self.feed(ServerEvent::Usage {
            input_tokens,
            output_tokens,
        });
    }

    pub fn error(&self, message: &str) {
        self.feed(ServerEvent::Error(message.to_string()));
    }

    /// The socket goes away without a word.
    pub fn drop_socket(&self) {
        self.feed.lock().unwrap().take();
    }

    /// True while the bridge holds this socket.
    pub fn is_open(&self) -> bool {
        self.feed.lock().unwrap().is_some()
    }

    fn feed(&self, event: ServerEvent) {
        let feed = self.feed.lock().unwrap().clone();
        if let Some(feed) = feed {
            let _ = feed.send(Ok(event));
        }
    }
}

/// A model server that opens a fresh [`ModelPeer`] per session. A test
/// reads the peers in the order the bridge opened them.
#[derive(Default)]
pub struct FakeModelSessions {
    peers: Mutex<Vec<Arc<ModelPeer>>>,
    /// How many of the next `open` calls fail.
    failures: Mutex<usize>,
}

impl FakeModelSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next `count` sessions do not open. An outbound call whose
    /// session does not open is never dialed.
    pub fn fail_next(&self, count: usize) {
        *self.failures.lock().unwrap() = count;
    }

    /// The sessions the bridge opened, in order.
    pub fn peers(&self) -> Vec<Arc<ModelPeer>> {
        self.peers.lock().unwrap().clone()
    }

    /// The session the bridge opened first. It panics when none was
    /// opened, because a test that reads it expects one.
    pub fn peer(&self) -> Arc<ModelPeer> {
        self.peers().first().cloned().expect("a session was opened")
    }
}

#[async_trait]
impl ModelSessions for FakeModelSessions {
    async fn open(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Box<dyn ModelSession>, ModelError> {
        {
            let mut failures = self.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(ModelError("the model is unavailable".to_string()));
            }
        }
        let (feed, incoming) = mpsc::unbounded_channel();
        let peer = Arc::new(ModelPeer {
            sent: Mutex::new(Vec::new()),
            arrived: Notify::new(),
            feed: Mutex::new(Some(feed)),
        });
        self.peers.lock().unwrap().push(Arc::clone(&peer));
        Ok(Box::new(FakeSession { peer, incoming }))
    }
}

struct FakeSession {
    peer: Arc<ModelPeer>,
    incoming: mpsc::UnboundedReceiver<Result<ServerEvent, ModelError>>,
}

#[async_trait]
impl ModelSession for FakeSession {
    async fn send(&mut self, command: ClientCommand) -> Result<(), ModelError> {
        if !self.peer.is_open() {
            return Err(ModelError("the socket is closed".to_string()));
        }
        self.peer
            .sent
            .lock()
            .unwrap()
            .push(command.realtime_event());
        self.peer.arrived.notify_one();
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<ServerEvent, ModelError>> {
        self.incoming.recv().await
    }

    async fn close(self: Box<Self>) {
        self.peer.drop_socket();
    }
}
