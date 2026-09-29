//! Event bus implementation: events-table append first, then a
//! `tokio::sync::broadcast` seq wakeup. Subscribers read the table for
//! delivery, so a lagged in-memory channel never loses events.

use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use pagis_core::{Event, EventBus, EventLog, EventScope, EventStream, NewEvent, StoreError};
use tokio::sync::broadcast;

const DEFAULT_WAKEUP_CAPACITY: usize = 256;
const CATCH_UP_BATCH: u32 = 256;

pub struct AuditEventBus {
    log: Arc<dyn EventLog>,
    wakeup: broadcast::Sender<i64>,
}

impl AuditEventBus {
    pub fn new(log: Arc<dyn EventLog>) -> Self {
        Self::with_capacity(log, DEFAULT_WAKEUP_CAPACITY)
    }

    pub fn with_capacity(log: Arc<dyn EventLog>, wakeup_capacity: usize) -> Self {
        let (wakeup, _) = broadcast::channel(wakeup_capacity);
        Self { log, wakeup }
    }
}

#[async_trait]
impl EventBus for AuditEventBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let event = self.log.append(event).await?;
        // No receivers is fine: the table is the source of truth.
        let _ = self.wakeup.send(event.seq);
        Ok(event)
    }

    async fn subscribe(&self, scope: EventScope, after_seq: Option<i64>) -> EventStream {
        // Register for wakeups before reading the cursor so no event
        // published in between is missed.
        let mut wakeups = self.wakeup.subscribe();
        let log = Arc::clone(&self.log);
        let mut cursor = match after_seq {
            Some(seq) => seq,
            None => match log.latest_seq().await {
                Ok(seq) => seq,
                Err(e) => {
                    tracing::error!(error = %e, "event bus subscribe failed to read cursor");
                    return Box::pin(futures::stream::empty());
                }
            },
        };
        Box::pin(stream! {
            loop {
                loop {
                    match log
                        .list_after(scope.workspace_id(), cursor, CATCH_UP_BATCH)
                        .await
                    {
                        Ok(batch) if batch.is_empty() => break,
                        Ok(batch) => {
                            for event in batch {
                                cursor = event.seq;
                                yield event;
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "event bus catch-up read failed");
                            return;
                        }
                    }
                }
                match wakeups.recv().await {
                    // Lagged only drops wakeups; the next catch-up read
                    // delivers everything from the table.
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}
