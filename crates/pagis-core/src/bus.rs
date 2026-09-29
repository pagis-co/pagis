//! The in-process event bus seam: table-append-first publication with
//! live subscriptions that survive lag by catching up from the table.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::WorkspaceId;
use crate::event::{Event, NewEvent};
use crate::store::StoreError;

pub type EventStream = BoxStream<'static, Event>;

/// Whose events a subscriber receives.
///
/// Every subscription of a client names the Workspace, so a socket
/// cannot receive an event of another tenant, live or on `last_seq`
/// replay. The installation scope exists for the in-process consumers
/// that serve the whole daemon: the agent dispatcher, the run loop, the
/// takeover note and the scheduler. Nothing that answers a request uses
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventScope {
    /// One Workspace's events and no other.
    Workspace(WorkspaceId),
    /// Every Workspace's events, for a daemon-lifetime consumer.
    Installation,
}

impl EventScope {
    /// One Workspace's events, by reference.
    pub fn workspace(workspace_id: &WorkspaceId) -> Self {
        EventScope::Workspace(workspace_id.clone())
    }

    /// The Workspace the scope filters on, or `None` for the whole
    /// installation. The event log takes this as its filter.
    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        match self {
            EventScope::Workspace(workspace_id) => Some(workspace_id),
            EventScope::Installation => None,
        }
    }

    /// True when the event belongs to the scope.
    pub fn covers(&self, event: &Event) -> bool {
        match self {
            EventScope::Workspace(workspace_id) => &event.workspace_id == workspace_id,
            EventScope::Installation => true,
        }
    }
}

#[async_trait]
pub trait EventBus: Send + Sync {
    /// Persist the event to the log, then wake subscribers.
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError>;

    /// Subscribe to the events of `scope` with seq strictly greater than
    /// `after_seq` (`None` = only events published after this call). A
    /// subscriber that falls behind the in-memory channel is caught up
    /// from the table; events are delivered in seq order without gaps.
    async fn subscribe(&self, scope: EventScope, after_seq: Option<i64>) -> EventStream;
}
