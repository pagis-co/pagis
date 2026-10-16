//! Queue changes of one Workspace, with no task or timer.

use std::collections::BTreeSet;
use std::sync::Arc;

use pagis_core::{Event, NewEvent, StoreError, WorkspaceId};

use crate::NeedsYouQueue;
use crate::needs_you::{QUEUE_EVENTS, changes, item_ids};

/// The current queue. The host supplies the records and the time.
pub trait QueueReader: Send + Sync + 'static {
    fn queue(
        &self,
        workspace_id: &WorkspaceId,
    ) -> impl Future<Output = Result<NeedsYouQueue, StoreError>> + Send;
}

/// The last item ids of one Workspace. The queue stays a view of records.
pub struct QueueChanges<Q: QueueReader> {
    queue: Arc<Q>,
    workspace_id: WorkspaceId,
    last: BTreeSet<String>,
}

impl<Q: QueueReader> QueueChanges<Q> {
    /// Read the startup baseline without publishing an event.
    pub async fn baseline(queue: Arc<Q>, workspace_id: WorkspaceId) -> Result<Self, StoreError> {
        let last = item_ids(&queue.queue(&workspace_id).await?);
        Ok(Self {
            queue,
            workspace_id,
            last,
        })
    }

    /// A Workspace made after startup starts with no known items.
    pub(crate) fn empty(queue: Arc<Q>, workspace_id: WorkspaceId) -> Self {
        Self {
            queue,
            workspace_id,
            last: BTreeSet::new(),
        }
    }

    /// Derive this Workspace after an event that can change its queue.
    pub async fn on_event(&mut self, event: &Event) -> Result<Vec<NewEvent>, StoreError> {
        if event.workspace_id != self.workspace_id
            || !QUEUE_EVENTS.contains(&event.event_type.as_str())
        {
            return Ok(Vec::new());
        }
        self.refresh().await
    }

    /// A Shared Workspace event can change a Person's queue too.
    pub(crate) async fn refresh(&mut self) -> Result<Vec<NewEvent>, StoreError> {
        let queue = self.queue.queue(&self.workspace_id).await?;
        let events = changes(&self.workspace_id, &self.last, &queue);
        self.last = item_ids(&queue);
        Ok(events)
    }
}
