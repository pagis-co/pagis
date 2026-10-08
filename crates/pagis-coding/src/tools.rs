//! The core tools of the Coding Sessions (ADR-0033). The broker has
//! settled the machine, checked the start and asked the Person; this is
//! what runs after the approval.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{
    AuthorizedCall, CoreTool, SessionStarts, ToolExecutor, ToolResult, ToolRoute, WorktreeRequest,
};
use pagis_core::{EventSource, wrap_untrusted};
use serde_json::json;

use crate::{CodingSessions, NewCodingSession, StartFailure};

/// The ref that the worktree of a session starts from: the commit that
/// the directory has checked out.
const WORKTREE_BASE: &str = "HEAD";

/// Executes the Coding Session tools.
pub struct CodingToolRuntime {
    sessions: Arc<CodingSessions>,
    starts: Arc<dyn SessionStarts>,
}

impl CodingToolRuntime {
    pub fn new(sessions: Arc<CodingSessions>, starts: Arc<dyn SessionStarts>) -> Self {
        Self { sessions, starts }
    }

    /// Whether a route is this runtime's to execute.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(
            route,
            ToolRoute::Core {
                tool: CoreTool::CodingSessionStart
            }
        )
    }

    /// Starts the session on the machine of the call, and returns when the
    /// harness has the first prompt.
    async fn start(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let host = call.host.as_ref().ok_or_else(|| {
            ToolResult::error("host_not_connected", "the start names no computer")
        })?;
        // The checks hold at the start too: another start of the Agent can
        // end while the Person reads the card.
        let start = self
            .starts
            .describe(&call.workspace_id, &call.agent_id, host, &call.arguments)
            .await?;
        let text = |field: &str| {
            call.arguments[field]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        let new = NewCodingSession {
            workspace_id: call.workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            run_id: call.run_id.clone(),
            host_id: host.id.clone(),
            harness_id: start.harness_id,
            worktree: start.branch.map(|branch| WorktreeRequest {
                repo: start.directory.clone(),
                branch,
                base: WORKTREE_BASE.to_string(),
            }),
            directory: start.directory,
            approval_mode: start.mode,
            title: text("title"),
            prompt: text("prompt"),
        };
        let session = self.sessions.start(new).await.map_err(start_failure)?;
        // The daemon's own state, so it takes no envelope.
        Ok(ToolResult::success(
            json!({
                "session_id": session.id.as_str(),
                "state": session.state.as_str(),
                "directory": session.working_directory.as_deref().unwrap_or(&session.directory),
                "branch": session.worktree_branch,
            })
            .to_string(),
        ))
    }
}

/// The tool error of a start that failed.
fn start_failure(failure: StartFailure) -> ToolResult {
    match failure {
        StartFailure::UnknownHarness(_) => {
            ToolResult::error("unknown_harness", failure.to_string())
        }
        StartFailure::RunNotFound(_) => ToolResult::error("not_found", failure.to_string()),
        // The Host left the Workspace while the Person read the card.
        StartFailure::HostNotFound(_) => {
            ToolResult::error("machine_not_found", failure.to_string())
        }
        StartFailure::NoConversation => ToolResult::error("unsupported_scope", failure.to_string()),
        StartFailure::Open { failure, .. } => {
            ToolResult::error(failure.code.as_str(), failure.message)
        }
        // The message is harness text, so it reaches the model inside the
        // untrusted envelope (ADR-0005).
        StartFailure::Harness {
            session_id,
            message,
        } => ToolResult::plain_error(
            "harness_error",
            wrap_untrusted(
                &EventSource::coding_session(session_id).to_string(),
                &message,
            ),
        ),
        StartFailure::Store(error) => {
            ToolResult::error("temporarily_unavailable", error.to_string())
        }
    }
}

#[async_trait]
impl ToolExecutor for CodingToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        if !Self::owns(&call.route) {
            return ToolResult::error(
                "temporarily_unavailable",
                format!("{} is not a coding session tool", call.tool_name),
            );
        }
        self.start(&call).await.unwrap_or_else(|refused| refused)
    }
}
