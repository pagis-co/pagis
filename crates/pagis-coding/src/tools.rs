//! The core tools of the Coding Sessions (ADR-0033).
//!
//! A start on a Host runs here after the broker has settled the machine,
//! checked the start and asked the Person. A start in the Agent's own
//! Computer asks nobody, because the container is the sandbox: it checks
//! the harness, the directory under `/data/agent`, the limit of open
//! sessions and the model route of the harness: a provider of its routes
//! with an Org key, and a model of the Agent's alias on a route that needs
//! one.
//! Each other tool acts on one session of
//! the calling Agent: a session of another Agent reads as absent, as
//! another Agent's Call does (ADR-0020). A resume of a session on a Host
//! also needs the live host Grant of the Agent on the session's machine. A decision needs a
//! Harness Permission that waits for the Agent, and the `agent` mode
//! still in force on the live host Grant. An answer needs a question that
//! waits for the Agent, and values that match its form. A change of the
//! Harness Mode needs a mode that the session lists, and the allowance of
//! the live host Grant for a mode that acts without asking.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{
    AuthorizedCall, CoreTool, Decider, SIGN_IN_REQUIRED, SessionStarts, ToolExecutor, ToolResult,
    ToolRoute, WorktreeRequest,
};
use pagis_core::{
    CodingSession, CodingSessionId, CodingSessionPlace, CodingSessionState as State,
    CodingSessionStore, EventSource, Grant, GrantStore, HarnessSetting, HostStore,
    SessionApprovalMode, StoreError, harness, wrap_untrusted,
};
use serde_json::json;

use crate::agent::{AnswerRefusal, Verdict};
use crate::model_route::{ModelRoutes, RouteFailure};
use crate::policy::{effective_mode, host_grant};
use crate::report::harness_output;
use crate::sessions::UNATTENDED_MODE_NOT_ALLOWED;
use crate::starts::{MAX_OPEN_SESSIONS, choice};
use crate::{
    AgentAsks, ChoiceFailure, CloseReason, CodingSessions, DecidedBy, NewCodingSession, Place,
    PromptOutcome, ResumeFailure, SessionError, SetModeFailure, SetSettingsFailure, Setting,
    SettingChoices, StartFailure,
};

/// The home of the Agent in its own Computer. A session there works in a
/// directory under it.
const COMPUTER_HOME: &str = "/data/agent";

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
const NO_PENDING_DECISION: &str = "no_pending_decision";
const NO_PENDING_QUESTION: &str = "no_pending_question";
const INVALID_VALUES: &str = "invalid_values";
const NO_MODES: &str = "no_modes";

/// Executes the Coding Session tools.
pub struct CodingToolRuntime {
    sessions: Arc<CodingSessions>,
    starts: Arc<dyn SessionStarts>,
    /// The records and the transcripts that the reads answer from.
    store: Arc<dyn CodingSessionStore>,
    /// The Hosts give the machine names.
    hosts: Arc<dyn HostStore>,
    /// The live host Grant lets a resume run, and holds the mode of a
    /// decision.
    grants: Arc<dyn GrantStore>,
    /// The Harness Permissions that wait for the Agent's verdict, and the
    /// questions that wait for its answer.
    agent: Arc<AgentAsks>,
    /// The model routes of the harnesses in a Computer, from the Org's
    /// provider keys and the Agent's model alias.
    routes: Arc<ModelRoutes>,
}

impl CodingToolRuntime {
    pub fn new(
        sessions: Arc<CodingSessions>,
        starts: Arc<dyn SessionStarts>,
        store: Arc<dyn CodingSessionStore>,
        hosts: Arc<dyn HostStore>,
        grants: Arc<dyn GrantStore>,
        agent: Arc<AgentAsks>,
        routes: Arc<ModelRoutes>,
    ) -> Self {
        Self {
            sessions,
            starts,
            store,
            hosts,
            grants,
            agent,
            routes,
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
            place: Place::Host(host.id.clone()),
            harness_id: start.harness_id,
            worktree: start.branch.map(|branch| WorktreeRequest {
                repo: start.directory.clone(),
                branch,
                base: WORKTREE_BASE.to_string(),
            }),
            directory: start.directory,
            approval_mode: start.mode,
            harness_mode: start.harness_mode,
            settings: SettingChoices {
                model: start.model,
                thought_level: start.thought_level,
            },
            title: text(call, "title"),
            prompt: text(call, "prompt"),
        };
        let session = self.sessions.start(new).await.map_err(start_failure)?;
        Ok(started(&session))
    }

    /// Starts the session in the Agent's own Computer, and returns when
    /// the harness has the first prompt. The checks go in this order: the
    /// harness runs in a Computer, the directory is under `/data/agent`,
    /// the Agent holds fewer than `MAX_OPEN_SESSIONS` open sessions on
    /// every place, the harness has a model route, and a route that names
    /// the model takes no other `model`. The harness runs
    /// in the Harness Mode of its Computer launch, and the Agent decides
    /// what the harness still asks.
    async fn computer_start(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let harness_id = text(call, "harness");
        let Some(entry) = harness::computer_sessions().find(|entry| entry.id == harness_id) else {
            let known = harness::computer_sessions()
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolResult::error(
                "unknown_harness",
                format!("{harness_id:?} does not run in your computer. Use one of: {known}."),
            ));
        };
        let directory = text(call, "directory");
        let settings = SettingChoices {
            model: choice(&call.arguments, "model")?,
            thought_level: choice(&call.arguments, "thought_level")?,
        };
        if !in_computer_home(&directory) {
            return Err(ToolResult::error(
                "bad_directory",
                format!(
                    "{directory:?} is not under {COMPUTER_HOME}. Name an absolute directory \
                     under {COMPUTER_HOME}, such as {COMPUTER_HOME}/app."
                ),
            ));
        }
        let open = self
            .store
            .count_open(&call.workspace_id, &call.agent_id)
            .await
            .map_err(unavailable)?;
        if open >= MAX_OPEN_SESSIONS {
            return Err(ToolResult::error(
                "session_limit",
                format!(
                    "You have {open} open coding sessions, and {MAX_OPEN_SESSIONS} is the most. \
                     Close one of your coding sessions first."
                ),
            ));
        }
        let route = self
            .routes
            .route(&call.workspace_id, &call.agent_id, entry.id)
            .await
            .map_err(|failure| match failure {
                RouteFailure::NoRoute(refusal) => {
                    ToolResult::error(refusal.code(), refusal.to_string())
                }
                RouteFailure::Unavailable(message) => {
                    ToolResult::error("temporarily_unavailable", message)
                }
            })?;
        // The route names the model, and the harness lists models that the
        // route does not serve.
        if route.fixes_model()
            && let Some(model) = &settings.model
        {
            return Err(ToolResult::error(
                Setting::Model.not_offered(),
                format!(
                    "{} in your computer reaches {} and uses the model {} of your model alias,                      so it takes no other model such as {model:?}. Omit model.",
                    entry.label,
                    route.provider().name(),
                    route.model().unwrap_or_default()
                ),
            ));
        }
        let new = NewCodingSession {
            workspace_id: call.workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            run_id: call.run_id.clone(),
            place: Place::Computer(call.agent_id.clone()),
            harness_id: entry.id.to_string(),
            directory,
            worktree: None,
            approval_mode: SessionApprovalMode::Agent,
            harness_mode: entry
                .computer
                .and_then(|launch| launch.mode)
                .map(str::to_string),
            settings,
            title: text(call, "title"),
            prompt: text(call, "prompt"),
        };
        let session = self.sessions.start(new).await.map_err(start_failure)?;
        Ok(started(&session))
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
        let mut output = harness_output(self.store.as_ref(), &session)
            .await
            .map_err(unavailable)?;
        // The harness names its modes, so they go inside the envelope.
        output["harness_mode"] = match &session.harness_mode {
            Some(id) => json!({"id": id, "name": mode_name(&session, id)}),
            None => serde_json::Value::Null,
        };
        output["harness_modes"] = json!(session.harness_modes);
        // The harness names its models and its thought levels too.
        output["model"] = current_choice(session.model.as_ref());
        output["models"] = json!(session.model.as_ref().map(|model| &model.choices));
        output["thought_level"] = current_choice(session.thought_level.as_ref());
        output["thought_levels"] =
            json!(session.thought_level.as_ref().map(|level| &level.choices));
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

    /// Resumes an `interrupted` session. A session on a Host needs a live
    /// host Grant of the Agent on its machine: the Person approved this
    /// harness in this directory on this machine, and a revoked Grant
    /// takes that back. A session in the Agent's own Computer needs none.
    async fn resume(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let granted = match &session.host_id {
            _ if session.place == CodingSessionPlace::Computer => true,
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

    /// Allows once or denies the Harness Permission that waits for the
    /// Agent. The checks go in this order: the session is the Agent's
    /// own, a permission waits for the Agent, and the `agent` mode is
    /// still in force. A narrower mode on the live host Grant gives the
    /// permission to the Person. A decision never writes a rule.
    async fn decide(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        if !self.agent.waits(&session.id) {
            return Err(no_pending_decision());
        }
        let grant = host_grant(self.grants.as_ref(), &session)
            .await
            .map_err(unavailable)?;
        if effective_mode(grant.as_ref(), &session) < SessionApprovalMode::Agent {
            // The Person decides. The daemon moved the decision, so the
            // escalation has no decider and no note.
            self.agent
                .hand(&session.id, Verdict::Escalate(DecidedBy::default()))
                .map_err(|_| no_pending_decision())?;
            return Err(ToolResult::error(
                "escalated",
                "The widest mode on this machine changed. The person decides on the card.",
            ));
        }
        let allow = call.arguments["decision"] == "allow";
        let verdict = Verdict::Decide {
            allow,
            note: text(call, "note"),
            run_id: call.run_id.clone(),
        };
        self.agent
            .hand(&session.id, verdict)
            .map_err(|_| no_pending_decision())?;
        Ok(ToolResult::success(if allow {
            "The harness does this one action once. The next request asks again."
        } else {
            "The harness does not do it."
        }))
    }

    /// Gives the Harness Permission that waits for the Agent to the
    /// Person, with the Agent's note on the card. It returns at once: the
    /// Run does not wait for the Person.
    async fn escalate(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let by = DecidedBy {
            decider: Some(Decider::Agent),
            note: Some(text(call, "note")),
            run_id: Some(call.run_id.clone()),
        };
        self.agent
            .hand(&session.id, Verdict::Escalate(by))
            .map_err(|_| no_pending_decision())?;
        Ok(ToolResult::success(
            "The person decides on the card in the session's Thread.",
        ))
    }

    /// Answers the question that waits for the Agent. The checks go in
    /// this order: the session is the Agent's own, a question waits for
    /// the Agent, and the values match its form. A failed check leaves
    /// the question waiting.
    async fn answer(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let values = call.arguments["values"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        match self.agent.answer(&session.id, values, call.run_id.clone()) {
            Ok(()) => Ok(ToolResult::success("The harness has your answer.")),
            Err(AnswerRefusal::NoQuestion) => Err(ToolResult::error(
                NO_PENDING_QUESTION,
                "no question of this coding session waits for your answer",
            )),
            // The name of a field is harness text, so the problem goes
            // inside the envelope of the session (ADR-0005).
            Err(AnswerRefusal::Invalid(error)) => Err(ToolResult::plain_error(
                INVALID_VALUES,
                format!(
                    "The values do not match the form of the question, and the question still \
                     waits. Answer again with values that match the form. The problem:\n{}",
                    wrap_untrusted(
                        &EventSource::coding_session(session.id.clone()).to_string(),
                        &error.to_string(),
                    )
                ),
            )),
        }
    }

    /// Changes the Harness Mode of the session. The checks go in this
    /// order: the session is the Agent's own, it has a stream, its harness
    /// offered modes, the mode is one of them, and a mode that acts
    /// without asking on a Host has the allowance of the live host Grant.
    async fn set_mode(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let mode = text(call, "mode");
        let source = EventSource::coding_session(session.id.clone()).to_string();
        self.sessions
            .set_mode(&call.workspace_id, &session.id, &mode)
            .await
            .map_err(|failure| match failure {
                SetModeFailure::NotFound => ToolResult::error(
                    SESSION_NOT_FOUND,
                    format!("you have no coding session {}", session.id),
                ),
                SetModeFailure::NotOpen(state) => not_open(state),
                SetModeFailure::NoModes => ToolResult::error(
                    NO_MODES,
                    "the harness of this coding session offered no Harness Modes",
                ),
                // The ids are harness text (ADR-0005).
                SetModeFailure::NotOffered { offered } => ToolResult::plain_error(
                    "invalid_request",
                    format!(
                        "The session does not list this mode. Use one of the modes that it \
                         lists:\n{}",
                        wrap_untrusted(&source, &offered.join(", "))
                    ),
                ),
                SetModeFailure::UnattendedModeNotAllowed { .. } => {
                    ToolResult::error(UNATTENDED_MODE_NOT_ALLOWED, failure.to_string())
                }
                SetModeFailure::Harness(message) => harness_error(session.id.clone(), &message),
                SetModeFailure::Store(error) => unavailable(error),
            })?;
        // The daemon's own state and the Agent's own words.
        Ok(ToolResult::success(
            json!({"session_id": session.id.as_str(), "mode": mode}).to_string(),
        ))
    }

    /// Changes the Harness Model or the thought level of the session, the
    /// model first. The checks go in this order: the session is the
    /// Agent's own, it has a stream, and the harness offers each choice.
    async fn set_model(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let session = self.own_session(call).await?;
        let choices = SettingChoices {
            model: choice(&call.arguments, "model")?,
            thought_level: choice(&call.arguments, "thought_level")?,
        };
        self.sessions
            .set_settings(&call.workspace_id, &session.id, choices.clone())
            .await
            .map_err(|failure| match failure {
                SetSettingsFailure::NotFound => ToolResult::error(
                    SESSION_NOT_FOUND,
                    format!("you have no coding session {}", session.id),
                ),
                SetSettingsFailure::NotOpen(state) => not_open(state),
                SetSettingsFailure::Choice(failure) => choice_failure(&session.id, failure, false),
                SetSettingsFailure::Store(error) => unavailable(error),
            })?;
        // The daemon's own state and the Agent's own words.
        Ok(ToolResult::success(
            json!({
                "session_id": session.id.as_str(),
                "model": choices.model,
                "thought_level": choices.thought_level,
            })
            .to_string(),
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

/// The tool result of a session that started: the daemon's own state, so
/// it takes no envelope.
fn started(session: &CodingSession) -> ToolResult {
    ToolResult::success(
        json!({
            "session_id": session.id.as_str(),
            "state": session.state.as_str(),
            "directory": session.working_directory.as_deref().unwrap_or(&session.directory),
            "branch": session.worktree_branch,
        })
        .to_string(),
    )
}

/// Whether `directory` is `/data/agent` or an absolute path under it,
/// with no `..` that leaves it.
fn in_computer_home(directory: &str) -> bool {
    let path = std::path::Path::new(directory);
    path.starts_with(COMPUTER_HOME)
        && path
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

/// One text argument of a call. The broker checked the required ones.
fn text(call: &AuthorizedCall, field: &str) -> String {
    call.arguments[field]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The name that the harness gave a mode of the session, else its id.
fn mode_name<'a>(session: &'a CodingSession, mode_id: &'a str) -> &'a str {
    session
        .harness_modes
        .iter()
        .find(|mode| mode.id == mode_id)
        .map_or(mode_id, |mode| mode.name.as_str())
}

/// The current choice of a Harness Setting with the name that the
/// harness gave it, or null when the harness offers no such setting.
fn current_choice(setting: Option<&HarnessSetting>) -> serde_json::Value {
    match setting {
        Some(setting) => json!({"id": setting.current, "name": setting.current_name()}),
        None => serde_json::Value::Null,
    }
}

/// The tool error of a choice that the harness did not take. The choices
/// and a failure of the harness are harness text (ADR-0005). After a
/// failed start, the Agent starts a new session.
fn choice_failure(session_id: &CodingSessionId, failure: ChoiceFailure, start: bool) -> ToolResult {
    match failure {
        ChoiceFailure::NotOffered {
            setting,
            choice,
            offered,
        } => {
            let words = setting.words();
            let outcome = if start {
                format!(
                    "so the session failed before its first prompt. Start a new session with \
                     one of the {words}s that it offers"
                )
            } else {
                format!("Use one of the {words}s that it offers")
            };
            let message = if offered.is_empty() {
                format!(
                    "The harness offers no choice of {words}, {}. Omit {}.",
                    if start {
                        "so the session failed before its first prompt"
                    } else {
                        "so the session keeps its own"
                    },
                    setting.as_str()
                )
            } else {
                format!(
                    "The harness does not offer the {words} {choice:?}, {outcome}:\n{}",
                    wrap_untrusted(
                        &EventSource::coding_session(session_id.clone()).to_string(),
                        &offered
                    )
                )
            };
            ToolResult::plain_error(setting.not_offered(), message)
        }
        ChoiceFailure::Harness(message) => harness_error(session_id.clone(), &message),
    }
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

fn no_pending_decision() -> ToolResult {
    ToolResult::error(
        NO_PENDING_DECISION,
        "no permission request of this coding session waits for your decision",
    )
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
        ResumeFailure::UnattendedModeNotAllowed { .. } => {
            ToolResult::error(UNATTENDED_MODE_NOT_ALLOWED, failure.to_string())
        }
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
        StartFailure::UnknownHarness(_) | StartFailure::NotInComputer(_) => {
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
        // The daemon's own words, which name where the Person signs in.
        StartFailure::SignInRequired { .. } => {
            ToolResult::plain_error(SIGN_IN_REQUIRED, failure.to_string())
        }
        StartFailure::UnattendedModeNotAllowed { .. } => {
            ToolResult::error(UNATTENDED_MODE_NOT_ALLOWED, failure.to_string())
        }
        // The ids of the modes are harness text (ADR-0005).
        StartFailure::ModeNotOffered {
            session_id,
            mode,
            offered,
        } => ToolResult::plain_error(
            "mode_not_offered",
            format!(
                "The harness does not offer the Harness Mode {mode:?}, so the session failed \
                 before its first prompt. Start a new session in one of the modes that it \
                 offered:\n{}",
                wrap_untrusted(
                    &EventSource::coding_session(session_id).to_string(),
                    &offered.join(", ")
                )
            ),
        ),
        StartFailure::ChoiceNotOffered {
            session_id,
            setting,
            choice,
            offered,
        } => choice_failure(
            &session_id,
            ChoiceFailure::NotOffered {
                setting,
                choice,
                offered,
            },
            true,
        ),
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
                CoreTool::ComputerCodingSessionStart => self.computer_start(&call).await,
                CoreTool::CodingSessionSend => self.send(&call).await,
                CoreTool::CodingSessionRead => self.read(&call).await,
                CoreTool::CodingSessionCancel => self.cancel(&call).await,
                CoreTool::CodingSessionClose => self.close(&call).await,
                CoreTool::CodingSessionList => self.list(&call).await,
                CoreTool::CodingSessionResume => self.resume(&call).await,
                CoreTool::CodingSessionDecide => self.decide(&call).await,
                CoreTool::CodingSessionEscalate => self.escalate(&call).await,
                CoreTool::CodingSessionAnswer => self.answer(&call).await,
                CoreTool::CodingSessionSetMode => self.set_mode(&call).await,
                CoreTool::CodingSessionSetModel => self.set_model(&call).await,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_computer_session_works_under_the_home_of_the_agent_only() {
        for inside in ["/data/agent", "/data/agent/app", "/data/agent/code/app/"] {
            assert!(in_computer_home(inside), "{inside}");
        }
        for outside in [
            "/data/agentx",
            "/data",
            "/etc",
            "data/agent/app",
            "/data/agent/../../etc",
            "",
        ] {
            assert!(!in_computer_home(outside), "{outside}");
        }
    }

    #[test]
    fn a_start_or_a_resume_that_the_live_grant_no_longer_allows_answers_its_error() {
        let session_id = CodingSessionId::from("s1".to_string());
        let started = start_failure(StartFailure::UnattendedModeNotAllowed {
            session_id: session_id.clone(),
            harness: "pi".to_string(),
            machine: "Air".to_string(),
        });
        let resumed = resume_failure(
            &session_id,
            ResumeFailure::UnattendedModeNotAllowed {
                harness: "pi".to_string(),
                machine: "Air".to_string(),
            },
        );

        for result in [started, resumed] {
            assert!(result.is_error);
            assert_eq!(result.code.as_deref(), Some("unattended_mode_not_allowed"));
            let content: serde_json::Value = serde_json::from_str(&result.content).unwrap();
            assert_eq!(
                content["error"]["message"],
                "pi acts without asking, and the user does not allow modes that act without \
                 asking for you on Air. The session is closed. Ask the user to allow them."
            );
        }
    }
}
