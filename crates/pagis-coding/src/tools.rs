//! The core tools of the Coding Sessions (ADR-0033).
//!
//! A start runs here after the broker has settled the machine, checked
//! the start and asked the Person. Each other tool acts on one session of
//! the calling Agent: a session of another Agent reads as absent, as
//! another Agent's Call does (ADR-0020). A resume also needs the live
//! host Grant of the Agent on the session's machine.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{
    AuthorizedCall, CoreTool, SessionStarts, ToolExecutor, ToolResult, ToolRoute, WorktreeRequest,
};
use pagis_core::{
    CodingSession, CodingSessionId, CodingSessionState as State, CodingSessionStore, EventSource,
    Grant, GrantStore, HostStore, StoreError, harness, wrap_untrusted,
};
use serde_json::json;

use crate::report::harness_output;
use crate::{
    CloseReason, CodingSessions, NewCodingSession, PromptOutcome, ResumeFailure, SessionError,
    StartFailure,
};

/// The ref that the worktree of a session starts from: the commit that
/// the directory has checked out.
const WORKTREE_BASE: &str = "HEAD";

/// The ended sessions that a list holds, newest first.
const ENDED_LISTED: u32 = 20;

/// The open sessions of one state that a list reads. The start holds an
/// Agent at `MAX_OPEN_SESSIONS`, so this page holds each one.
const OPEN_LISTED: u32 = 100;

/// The states of an open session.
const OPEN_STATES: [State; 5] = [
    State::Starting,
    State::Working,
    State::NeedsDecision,
    State::Idle,
    State::Interrupted,
];

const SESSION_NOT_FOUND: &str = "session_not_found";
const SESSION_NOT_OPEN: &str = "session_not_open";

/// Executes the Coding Session tools.
pub struct CodingToolRuntime {
    sessions: Arc<CodingSessions>,
    starts: Arc<dyn SessionStarts>,
    /// The records and the transcripts that the reads answer from.
    store: Arc<dyn CodingSessionStore>,
    /// The Hosts give the machine names.
    hosts: Arc<dyn HostStore>,
    /// The live host Grant lets a resume run.
    grants: Arc<dyn GrantStore>,
}

impl CodingToolRuntime {
    pub fn new(
        sessions: Arc<CodingSessions>,
        starts: Arc<dyn SessionStarts>,
        store: Arc<dyn CodingSessionStore>,
        hosts: Arc<dyn HostStore>,
        grants: Arc<dyn GrantStore>,
    ) -> Self {
        Self {
            sessions,
            starts,
            store,
            hosts,
            grants,
        }
    }

    /// Whether a route is this runtime's to execute.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(route, ToolRoute::Core { tool } if tool.is_coding_session())
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
            title: text(call, "title"),
            prompt: text(call, "prompt"),
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

    /// Sends a prompt, or queues it while a turn runs.
    async fn send(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let outcome = self
            .sessions
            .prompt(&call.workspace_id, &session.id, text(call, "prompt"))
            .await
            .map_err(|error| session_error(&session.id, error))?;
        Ok(ToolResult::success(match outcome {
            PromptOutcome::Sent => "The turn started.",
            PromptOutcome::Queued => "A turn runs. Your prompt goes when it ends.",
        }))
    }

    /// The daemon's own fields of the session, and its harness text inside
    /// one untrusted envelope (ADR-0005).
    async fn read(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let machine = match &session.host_id {
            Some(host_id) => self
                .hosts
                .get(&call.workspace_id, host_id)
                .await
                .map_err(unavailable)?
                .map(|host| host.name),
            None => None,
        };
        let output = harness_output(self.store.as_ref(), &session)
            .await
            .map_err(unavailable)?;
        let source = EventSource::coding_session(session.id.clone()).to_string();
        Ok(ToolResult::success(
            json!({
                "session_id": session.id.as_str(),
                "harness": harness_name(&session.harness_id),
                "machine": machine,
                "title": session.title,
                "state": session.state.as_str(),
                "end_reason": session.end_reason,
                "usage": session.usage,
                "updated_at": session.updated_at,
                "harness_output": wrap_untrusted(&source, &output.to_string()),
            })
            .to_string(),
        ))
    }

    /// Cancels the turn that runs. The session then takes a new prompt.
    async fn cancel(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        match session.state {
            State::Working | State::NeedsDecision => {
                self.sessions
                    .cancel(&call.workspace_id, &session.id)
                    .await
                    .map_err(|error| session_error(&session.id, error))?;
                Ok(ToolResult::success(
                    "The turn is cancelled. The session takes a new prompt when it is idle.",
                ))
            }
            State::Idle => Ok(ToolResult::success("No turn runs.")),
            state => Err(not_open(state)),
        }
    }

    /// Closes the session for good.
    async fn close(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        self.sessions
            .close(&call.workspace_id, &session.id, CloseReason::Closed)
            .await
            .map_err(|error| session_error(&session.id, error))?;
        Ok(ToolResult::success("The session is closed."))
    }

    /// Resumes an `interrupted` session, when the Agent still holds a
    /// live host Grant on its machine. The Person approved this harness
    /// in this directory on this machine, and a revoked Grant takes that
    /// back.
    async fn resume(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let granted = match &session.host_id {
            Some(host_id) => self
                .grants
                .live_for_resource(
                    &call.workspace_id,
                    &call.agent_id,
                    Grant::HOST_KIND,
                    host_id.as_str(),
                )
                .await
                .map_err(unavailable)?
                .is_some(),
            None => false,
        };
        if !granted {
            return Err(ToolResult::error(
                "permission_revoked",
                "the user revoked your access to the computer of this coding session; ask the \
                 user to allow it again",
            ));
        }
        self.sessions
            .resume(&call.workspace_id, &session.id)
            .await
            .map_err(|failure| resume_failure(&session.id, failure))?;
        Ok(ToolResult::success(
            "The session resumed. It is idle and takes a new prompt.",
        ))
    }

    /// The Agent's own sessions: each open one, then the newest that
    /// ended. The title and the directory are the Agent's own words, so
    /// they take no envelope.
    async fn list(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let mut open = Vec::new();
        for state in OPEN_STATES {
            open.extend(self.of_agent(call, state, OPEN_LISTED).await?);
        }
        let mut ended = self.of_agent(call, State::Closed, ENDED_LISTED).await?;
        ended.extend(self.of_agent(call, State::Failed, ENDED_LISTED).await?);
        // The ids are ordered by time, as the store orders them.
        open.sort_by(|a, b| b.id.as_str().cmp(a.id.as_str()));
        ended.sort_by(|a, b| b.id.as_str().cmp(a.id.as_str()));
        ended.truncate(ENDED_LISTED as usize);

        let machines: HashMap<_, _> = self
            .hosts
            .list(&call.workspace_id)
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|host| (host.id, host.name))
            .collect();
        let items: Vec<_> = open
            .into_iter()
            .chain(ended)
            .map(|session| {
                json!({
                    "session_id": session.id.as_str(),
                    "title": session.title,
                    "harness": harness_name(&session.harness_id),
                    "machine": session.host_id.as_ref().and_then(|host_id| machines.get(host_id)),
                    "directory": session.directory,
                    "state": session.state.as_str(),
                    "updated_at": session.updated_at,
                })
            })
            .collect();
        Ok(ToolResult::success(json!({ "items": items }).to_string()))
    }

    /// The sessions of the calling Agent in one state, newest first.
    async fn of_agent(
        &self,
        call: &AuthorizedCall,
        state: State,
        limit: u32,
    ) -> Result<Vec<CodingSession>, ToolResult> {
        self.store
            .list(
                &call.workspace_id,
                Some(&call.agent_id),
                Some(state),
                None,
                limit,
            )
            .await
            .map_err(unavailable)
    }

    /// The session that the call names, when the calling Agent owns it.
    async fn own_session(&self, call: &AuthorizedCall) -> Result<CodingSession, ToolResult> {
        let id = text(call, "session");
        self.store
            .get(&call.workspace_id, &CodingSessionId::from(id.clone()))
            .await
            .map_err(unavailable)?
            .filter(|session| session.agent_id == call.agent_id)
            .ok_or_else(|| {
                ToolResult::error(
                    SESSION_NOT_FOUND,
                    format!("you have no coding session {id}"),
                )
            })
    }
}

/// One text argument of a call. The broker checked the required ones.
fn text(call: &AuthorizedCall, field: &str) -> String {
    call.arguments[field]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The display name of a harness. A harness that a later release removed
/// from the catalog reads as its id.
fn harness_name(harness_id: &str) -> String {
    harness::entry(harness_id)
        .map_or_else(|| harness_id.to_string(), |entry| entry.label.to_string())
}

fn unavailable(error: StoreError) -> ToolResult {
    ToolResult::error("temporarily_unavailable", error.to_string())
}

fn not_open(state: State) -> ToolResult {
    let message = match state {
        State::Interrupted => "the coding session is interrupted; resume it with \
                               coding_session_resume, or close it"
            .to_string(),
        state => format!("the coding session is {}", state.as_str()),
    };
    ToolResult::error(SESSION_NOT_OPEN, message)
}

/// The tool error of a resume that did nothing.
fn resume_failure(session_id: &CodingSessionId, failure: ResumeFailure) -> ToolResult {
    match failure {
        ResumeFailure::NotFound => ToolResult::error(
            SESSION_NOT_FOUND,
            format!("you have no coding session {session_id}"),
        ),
        ResumeFailure::NotInterrupted(_) | ResumeFailure::Resuming => {
            ToolResult::error("session_not_interrupted", failure.to_string())
        }
        ResumeFailure::NotStarted | ResumeFailure::CannotResume { .. } => {
            ToolResult::error("cannot_resume", failure.to_string())
        }
        ResumeFailure::UnknownHarness(_) => {
            ToolResult::error("unknown_harness", failure.to_string())
        }
        ResumeFailure::HostNotFound => ToolResult::error("machine_not_found", failure.to_string()),
        // The words of every host tool for a machine that is away.
        ResumeFailure::HostNotConnected { .. } => {
            ToolResult::plain_error("host_not_connected", failure.to_string())
        }
        ResumeFailure::Open(failure) => ToolResult::error(failure.code.as_str(), failure.message),
        ResumeFailure::Harness(message) => harness_error(session_id.clone(), &message),
        ResumeFailure::Store(error) => unavailable(error),
    }
}

/// The tool error of a call on a session that did nothing.
fn session_error(session_id: &CodingSessionId, error: SessionError) -> ToolResult {
    match error {
        SessionError::NotFound => ToolResult::error(
            SESSION_NOT_FOUND,
            format!("you have no coding session {session_id}"),
        ),
        SessionError::NotOpen(state) => not_open(state),
        SessionError::Harness(message) => harness_error(session_id.clone(), &message),
        SessionError::Store(error) => unavailable(error),
    }
}

/// A failure that the harness reported. The message is harness text, so
/// it reaches the model inside the untrusted envelope (ADR-0005).
fn harness_error(session_id: CodingSessionId, message: &str) -> ToolResult {
    ToolResult::plain_error(
        "harness_error",
        wrap_untrusted(
            &EventSource::coding_session(session_id).to_string(),
            message,
        ),
    )
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
        StartFailure::Harness {
            session_id,
            message,
        } => harness_error(session_id, &message),
        StartFailure::Rules { .. } => {
            ToolResult::error("temporarily_unavailable", failure.to_string())
        }
        StartFailure::Store(error) => unavailable(error),
    }
}

#[async_trait]
impl ToolExecutor for CodingToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        let answer = match &call.route {
            ToolRoute::Core { tool } => match tool {
                CoreTool::CodingSessionStart => self.start(&call).await,
                CoreTool::CodingSessionSend => self.send(&call).await,
                CoreTool::CodingSessionRead => self.read(&call).await,
                CoreTool::CodingSessionCancel => self.cancel(&call).await,
                CoreTool::CodingSessionClose => self.close(&call).await,
                CoreTool::CodingSessionList => self.list(&call).await,
                CoreTool::CodingSessionResume => self.resume(&call).await,
                _ => Err(not_a_session_tool(&call)),
            },
            _ => Err(not_a_session_tool(&call)),
        };
        answer.unwrap_or_else(|refused| refused)
    }
}

fn not_a_session_tool(call: &AuthorizedCall) -> ToolResult {
    ToolResult::error(
        "temporarily_unavailable",
        format!("{} is not a coding session tool", call.tool_name),
    )
}
