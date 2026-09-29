//! The WS resume ring buffer: recent event positions, bounded to
//! the last `capacity` events or `min_age`, whichever covers more. It
//! stores only (event id, log seq, created_at) — resume replay itself
//! reads the durable event log through the bus.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{Event, EventBus, EventScope, EventStream, NewEvent, StoreError, now_ms};

#[derive(Debug, Clone, Copy)]
pub struct RingConfig {
    pub capacity: usize,
    pub min_age: Duration,
}

impl Default for RingConfig {
    fn default() -> Self {
        RingConfig {
            capacity: 1000,
            min_age: Duration::from_secs(300),
        }
    }
}

struct Entry {
    id: String,
    seq: i64,
    created_at: i64,
}

pub struct EventRing {
    config: RingConfig,
    entries: Mutex<VecDeque<Entry>>,
}

impl EventRing {
    pub fn new(config: RingConfig) -> Self {
        EventRing {
            config,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push(&self, event: &Event) {
        let mut entries = self.entries.lock().expect("ring lock");
        entries.push_back(Entry {
            id: event.id.as_str().to_string(),
            seq: event.seq,
            created_at: event.created_at,
        });
        let min_age_ms = self.config.min_age.as_millis() as i64;
        let cutoff = now_ms() - min_age_ms;
        while entries.len() > self.config.capacity
            && entries.front().is_some_and(|e| e.created_at <= cutoff)
        {
            entries.pop_front();
        }
    }

    /// The log seq for an event id the buffer still covers, or `None`
    /// when the gap is no longer covered and the client must resync.
    pub fn position(&self, event_id: &str) -> Option<i64> {
        let entries = self.entries.lock().expect("ring lock");
        entries.iter().find(|e| e.id == event_id).map(|e| e.seq)
    }
}

/// An event bus that records every published event in the resume ring
/// before returning, so a client can always resume past its own writes.
pub struct RingedBus {
    inner: Arc<dyn EventBus>,
    ring: Arc<EventRing>,
}

impl RingedBus {
    pub fn new(inner: Arc<dyn EventBus>, ring: Arc<EventRing>) -> Self {
        RingedBus { inner, ring }
    }
}

#[async_trait]
impl EventBus for RingedBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let event = self.inner.publish(event).await?;
        self.ring.push(&event);
        Ok(event)
    }

    async fn subscribe(&self, scope: EventScope, after_seq: Option<i64>) -> EventStream {
        self.inner.subscribe(scope, after_seq).await
    }
}
