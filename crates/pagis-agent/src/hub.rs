//! Live streams: the in-memory accumulation buffer per streaming
//! message, fanned out as ephemeral `message.delta` frames. A
//! subscriber that arrives mid-stream reads the accumulated text as one
//! catch-up frame; `seq` orders catch-up against live deltas.

use std::collections::HashMap;
use std::sync::Mutex;

use pagis_core::{AgentId, ChannelId, MessageId, RunId};
use serde::Serialize;
use tokio::sync::broadcast;
use utoipa::ToSchema;

const BROADCAST_CAPACITY: usize = 1024;

/// One `message.delta` WS payload. `catch_up: true` carries the whole
/// accumulated text; a live frame carries one delta. A client applies a
/// frame only when `seq` follows the last one it folded.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeltaFrame {
    pub channel_id: String,
    pub message_id: String,
    pub run_id: String,
    /// The agent writing the message; the client names the row with it
    /// before the durable row arrives.
    pub agent_id: String,
    /// Set when the streaming message is a thread reply.
    pub parent_message_id: Option<String>,
    /// The count of deltas folded into this frame's text end position.
    pub seq: u64,
    pub text: String,
    pub catch_up: bool,
}

struct LiveStream {
    channel_id: ChannelId,
    run_id: RunId,
    agent_id: AgentId,
    parent_message_id: Option<MessageId>,
    seq: u64,
    text: String,
}

/// The delta hub: the agent loop writes, WebSocket sessions read.
pub struct StreamHub {
    live: Mutex<HashMap<MessageId, LiveStream>>,
    tx: broadcast::Sender<DeltaFrame>,
}

impl Default for StreamHub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            live: Mutex::new(HashMap::new()),
            tx,
        }
    }
}

impl StreamHub {
    /// Open the live stream for one streaming message.
    pub fn begin(
        &self,
        channel_id: ChannelId,
        message_id: MessageId,
        run_id: RunId,
        agent_id: AgentId,
        parent_message_id: Option<MessageId>,
    ) {
        let mut live = self.live.lock().expect("hub lock");
        live.insert(
            message_id,
            LiveStream {
                channel_id,
                run_id,
                agent_id,
                parent_message_id,
                seq: 0,
                text: String::new(),
            },
        );
    }

    /// Append one delta and broadcast it. The broadcast happens under
    /// the lock so a snapshot never races a frame out of order.
    pub fn push(&self, message_id: &MessageId, delta: &str) {
        let mut live = self.live.lock().expect("hub lock");
        let Some(stream) = live.get_mut(message_id) else {
            return;
        };
        stream.seq += 1;
        stream.text.push_str(delta);
        let _ = self.tx.send(DeltaFrame {
            channel_id: stream.channel_id.to_string(),
            message_id: message_id.to_string(),
            run_id: stream.run_id.to_string(),
            agent_id: stream.agent_id.to_string(),
            parent_message_id: stream.parent_message_id.as_ref().map(|m| m.to_string()),
            seq: stream.seq,
            text: delta.to_string(),
            catch_up: false,
        });
    }

    /// Close the live stream; the durable message row takes over.
    pub fn end(&self, message_id: &MessageId) {
        let mut live = self.live.lock().expect("hub lock");
        live.remove(message_id);
    }

    /// One catch-up frame per live stream in the channel: the whole
    /// accumulated text as one delta.
    pub fn snapshot(&self, channel_id: &str) -> Vec<DeltaFrame> {
        let live = self.live.lock().expect("hub lock");
        live.iter()
            .filter(|(_, stream)| stream.channel_id.as_str() == channel_id)
            .map(|(message_id, stream)| DeltaFrame {
                channel_id: stream.channel_id.to_string(),
                message_id: message_id.to_string(),
                run_id: stream.run_id.to_string(),
                agent_id: stream.agent_id.to_string(),
                parent_message_id: stream.parent_message_id.as_ref().map(|m| m.to_string()),
                seq: stream.seq,
                text: stream.text.clone(),
                catch_up: true,
            })
            .collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DeltaFrame> {
        self.tx.subscribe()
    }
}
