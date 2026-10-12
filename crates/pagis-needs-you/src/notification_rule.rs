//! Notification decisions of one Workspace, with a caller-supplied time.

use std::collections::HashSet;
use std::sync::Arc;

use pagis_core::{Event, UnixMillis, WorkspaceId};
use serde::Deserialize;

use crate::{NEEDS_YOU_ADDED, NEEDS_YOU_REMOVED, PersonActivity, QueuedItem};

/// The queue item at the end of a hold, or no item when it left the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldItem {
    pub item_id: String,
    pub queued: Option<QueuedItem>,
}

/// Work for the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Send(QueuedItem),
    Hold { item_id: String, until: UnixMillis },
    EndHold { item_id: String },
}

#[derive(Deserialize)]
struct Removed {
    item_id: String,
}

/// The held items of one Workspace. This rule reads no clock and starts no task.
pub struct NotificationRule {
    workspace_id: WorkspaceId,
    activity: Arc<PersonActivity>,
    held: HashSet<String>,
}

impl NotificationRule {
    pub fn new(workspace_id: WorkspaceId, activity: Arc<PersonActivity>) -> Self {
        Self {
            workspace_id,
            activity,
            held: HashSet::new(),
        }
    }

    /// Send an entering item, hold it while active, or end a leaving item's hold.
    pub fn on_event(&mut self, event: &Event, now: UnixMillis) -> Vec<Action> {
        if event.workspace_id != self.workspace_id {
            return Vec::new();
        }
        match event.event_type.as_str() {
            NEEDS_YOU_ADDED => {
                let added = match serde_json::from_value::<QueuedItem>(event.payload.clone()) {
                    Ok(added) => added,
                    Err(error) => {
                        tracing::error!(%error, workspace_id = %self.workspace_id, "a needs_you.added event was not read");
                        return Vec::new();
                    }
                };
                let item_id = added.item.id().to_string();
                match self.activity.hold_until(&self.workspace_id, now) {
                    Some(until) => {
                        self.held.insert(item_id.clone());
                        vec![Action::Hold { item_id, until }]
                    }
                    None => {
                        let mut actions = self.end_hold(&item_id);
                        actions.push(Action::Send(added));
                        actions
                    }
                }
            }
            NEEDS_YOU_REMOVED => match serde_json::from_value::<Removed>(event.payload.clone()) {
                Ok(removed) => self.end_hold(&removed.item_id),
                Err(error) => {
                    tracing::error!(%error, workspace_id = %self.workspace_id, "a needs_you.removed event was not read");
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    /// Check the activity again when the host reports the end of an item's hold.
    pub fn on_hold_end(&mut self, item: HeldItem, now: UnixMillis) -> Vec<Action> {
        let item_id = item.item_id.as_str();
        if !self.held.contains(item_id) {
            return Vec::new();
        }
        if let Some(until) = self.activity.hold_until(&self.workspace_id, now) {
            return vec![Action::Hold {
                item_id: item_id.to_string(),
                until,
            }];
        }
        let mut actions = self.end_hold(item_id);
        if let Some(queued) = item.queued {
            actions.push(Action::Send(queued));
        }
        actions
    }

    fn end_hold(&mut self, item_id: &str) -> Vec<Action> {
        if self.held.remove(item_id) {
            vec![Action::EndHold {
                item_id: item_id.to_string(),
            }]
        } else {
            Vec::new()
        }
    }
}
