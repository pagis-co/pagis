//! The calls that run right now. One active call per number
//! (ADR-0020), and the session tools exist only while a call is live:
//! a number with no live call has no `hang_up`, not one that fails.
//! [`LiveCalls`] is also the [`ActiveCalls`] seam the number desk
//! reads before it moves a line.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_broker::ToolDef;
use pagis_core::{CallId, PhoneNumberId, WorkspaceId};

use crate::calls::ActiveCalls;
use crate::hub::MediaHub;

struct LiveCall {
    call_id: CallId,
    tools: Vec<ToolDef>,
}

#[derive(Default)]
pub struct LiveCalls {
    calls: Mutex<HashMap<PhoneNumberId, LiveCall>>,
    /// The media hub of each live call. The key holds the Workspace and
    /// the Call, so a lookup finds a call only under its own Workspace
    /// (ADR-0023): Listen-Live and hang up find the call of the Person
    /// who asks, and a Call of another Workspace is a Call that is not
    /// live. The bridge puts the hub here when the call starts, and the
    /// guard takes it away when the call ends.
    hubs: Mutex<HashMap<(WorkspaceId, CallId), Arc<MediaHub>>>,
}

/// A number is already on a call, so a second one does not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a call on this number is active")]
pub struct CallActive {
    pub call_id: CallId,
}

impl LiveCalls {
    /// Start a call on a number. The call is live until the guard
    /// drops, so a bridge that fails or a task that is aborted still
    /// frees the line.
    pub fn begin(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        number_id: &PhoneNumberId,
        call_id: &CallId,
        tools: Vec<ToolDef>,
    ) -> Result<LiveCallGuard, CallActive> {
        let mut calls = self.calls.lock().unwrap();
        if let Some(live) = calls.get(number_id) {
            return Err(CallActive {
                call_id: live.call_id.clone(),
            });
        }
        calls.insert(
            number_id.clone(),
            LiveCall {
                call_id: call_id.clone(),
                tools,
            },
        );
        Ok(LiveCallGuard {
            live: Arc::clone(self),
            number_id: number_id.clone(),
            hub_key: (workspace_id.clone(), call_id.clone()),
        })
    }

    /// Make the call listenable. The bridge calls this as soon
    /// as the media hub exists, so a person in the Thread hears the
    /// call from that moment.
    pub fn attach_hub(&self, workspace_id: &WorkspaceId, call_id: &CallId, hub: Arc<MediaHub>) {
        self.hubs
            .lock()
            .unwrap()
            .insert((workspace_id.clone(), call_id.clone()), hub);
    }

    /// The call ended: a listener that joins now finds no hub.
    pub fn detach_hub(&self, workspace_id: &WorkspaceId, call_id: &CallId) {
        self.hubs
            .lock()
            .unwrap()
            .remove(&(workspace_id.clone(), call_id.clone()));
    }

    /// The media hub of a live call of the Workspace, or `None` when no
    /// call of that id runs now in that Workspace.
    pub fn hub(&self, workspace_id: &WorkspaceId, call_id: &CallId) -> Option<Arc<MediaHub>> {
        self.hubs
            .lock()
            .unwrap()
            .get(&(workspace_id.clone(), call_id.clone()))
            .cloned()
    }

    /// Bind another tool list on the number's live call, for a fact
    /// that only the answered call knows: the negotiated leg may have
    /// no telephone events, and then `send_digits` is absent. Returns
    /// `false` when no call is live on the number.
    pub fn rebind(&self, number_id: &PhoneNumberId, tools: Vec<ToolDef>) -> bool {
        let mut calls = self.calls.lock().unwrap();
        match calls.get_mut(number_id) {
            Some(live) => {
                live.tools = tools;
                true
            }
            None => false,
        }
    }

    /// The tools bound on the number's live call, or `None` when no
    /// call is live: outside a call the session tools are absent.
    pub fn session_tools(&self, number_id: &PhoneNumberId) -> Option<Vec<ToolDef>> {
        self.calls
            .lock()
            .unwrap()
            .get(number_id)
            .map(|live| live.tools.clone())
    }
}

#[async_trait]
impl ActiveCalls for LiveCalls {
    async fn is_active(&self, number_id: &PhoneNumberId) -> bool {
        self.calls.lock().unwrap().contains_key(number_id)
    }
}

/// The line is on a call until this drops.
pub struct LiveCallGuard {
    live: Arc<LiveCalls>,
    number_id: PhoneNumberId,
    hub_key: (WorkspaceId, CallId),
}

impl Drop for LiveCallGuard {
    fn drop(&mut self) {
        self.live.calls.lock().unwrap().remove(&self.number_id);
        self.live.hubs.lock().unwrap().remove(&self.hub_key);
    }
}
