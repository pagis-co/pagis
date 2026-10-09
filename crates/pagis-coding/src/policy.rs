//! Pagis policy for each Harness Permission of a Coding Session on a
//! Host (ADR-0033).
//!
//! [`PolicyDecisions`] reads the live host Grant of the owning Agent at
//! each permission, so a narrower Grant applies to the next permission
//! of a running session. The broker's [`evaluate`] decides. Each decision
//! writes one audit fact, [`PERMISSION_DECIDED_EVENT`], on the bus.
//!
//! A permission that policy does not allow waits. In the `person` mode
//! the Person answers it on an approval card ([`crate::person`]). In the
//! `agent` mode it waits in [`AgentAsks`] for the verdict of the
//! supervising Agent, who allows it once, denies it, or gives it to the
//! Person. It does not wait forever: a Run of the owning Agent in the
//! session's Thread that started after the permission came, and that
//! ends with no verdict, gives it to the Person.
//!
//! A form question goes to the supervising Agent in each mode, and waits
//! in [`AgentAsks`] for its answer. A Run of the owning Agent in the
//! session's Thread that started after the question came, and that ends
//! with no answer, answers it `cancel`. A form that holds a property that
//! Pagis cannot check is declined at once.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use pagis_broker::{Decider, HarnessToolKind, PolicyOutcome, evaluate};
use pagis_core::{
    AgentId, CodingSession, EventBus, EventScope, EventStream, Grant, GrantStore, HostStore,
    MessageStore, NewEvent, RequestStore, RunId, RunState, RunStore, SessionApprovalMode,
    WorkspaceId,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::agent::Verdict;
use crate::form::Form;
use crate::person::{Permission, PersonAsks};
use crate::{
    AgentAsks, DecidedBy, Pending, PermissionAnswer, PermissionAsk, PermissionOptionKind,
    QuestionAnswer, QuestionAsk, SessionDecisions, ToolKind, Waited, WaitsFor,
};

/// The audit fact of each decision on a Harness Permission.
pub const PERMISSION_DECIDED_EVENT: &str = "coding_session.permission_decided";

/// The note of the card when the Agent ended its turn with no verdict.
pub const NO_DECISION_NOTE: &str = "The sprite ended its turn without a decision.";

/// The note of the answer `cancel` when the Agent ended its turn with no
/// answer to a question.
pub const NO_ANSWER_NOTE: &str = "The sprite ended its turn without an answer.";

/// The active Runs of the Agent in one Channel that one read sees. An
/// Agent has far fewer active Runs in one Thread.
const ACTIVE_RUNS: u32 = 100;

/// The states of a Run that is not over.
const ACTIVE_STATES: [RunState; 5] = [
    RunState::Queued,
    RunState::Running,
    RunState::Reflecting,
    RunState::WaitingForUser,
    RunState::WaitingForApproval,
];

/// What [`PolicyDecisions`] is built from.
pub struct PolicyDecisionsDeps {
    /// The live host Grant holds the mode and the Host Allow Rules.
    pub grants: Arc<dyn GrantStore>,
    /// The Request of each permission that asks the Person.
    pub requests: Arc<dyn RequestStore>,
    /// The session's Thread holds the approval card.
    pub messages: Arc<dyn MessageStore>,
    /// The Host gives the machine name of the card.
    pub hosts: Arc<dyn HostStore>,
    /// The Runs of the Agent in the session's Thread, which a permission
    /// that waits for the Agent watches.
    pub runs: Arc<dyn RunStore>,
    /// Where a permission waits for the Agent's verdict.
    pub agent: Arc<AgentAsks>,
    /// The audit facts, the events of the card, the decisions, and the
    /// ends of the Runs.
    pub bus: Arc<dyn EventBus>,
}

/// Applies Pagis policy to each Harness Permission, and gives each
/// question of a harness to the supervising Agent.
pub struct PolicyDecisions {
    grants: Arc<dyn GrantStore>,
    runs: Arc<dyn RunStore>,
    agent: Arc<AgentAsks>,
    bus: Arc<dyn EventBus>,
    person: PersonAsks,
}

impl PolicyDecisions {
    pub fn new(deps: PolicyDecisionsDeps) -> Self {
        Self {
            grants: deps.grants,
            runs: deps.runs,
            agent: deps.agent,
            bus: Arc::clone(&deps.bus),
            person: PersonAsks {
                requests: deps.requests,
                messages: deps.messages,
                hosts: deps.hosts,
                bus: deps.bus,
            },
        }
    }

    /// The live host Grant of the owning Agent on the session's machine.
    /// A failed read is logged and reads as no Grant, which gives the
    /// narrowest policy.
    async fn host_grant(&self, session: &CodingSession) -> Option<Grant> {
        match host_grant(self.grants.as_ref(), session).await {
            Ok(grant) => grant,
            Err(error) => {
                tracing::warn!(session = %session.id, %error, "the host Grant of a Coding Session was not read");
                None
            }
        }
    }
}

/// The live host Grant of the owning Agent on the session's machine. A
/// session with no Host has none.
pub(crate) async fn host_grant(
    grants: &dyn GrantStore,
    session: &CodingSession,
) -> Result<Option<Grant>, pagis_core::StoreError> {
    let Some(host_id) = &session.host_id else {
        return Ok(None);
    };
    grants
        .live_for_resource(
            &session.workspace_id,
            &session.agent_id,
            Grant::HOST_KIND,
            host_id.as_str(),
        )
        .await
}

/// The mode that answers a permission now: the narrower of the session's
/// mode and the widest mode on the live host Grant. With no live Grant
/// the mode is `person`.
pub(crate) fn effective_mode(
    grant: Option<&Grant>,
    session: &CodingSession,
) -> SessionApprovalMode {
    grant
        .map_or(SessionApprovalMode::Person, Grant::session_approval_mode)
        .min(session.approval_mode)
}

#[async_trait]
impl SessionDecisions for PolicyDecisions {
    /// Allows once when policy allows and the harness offers `allow_once`.
    /// When it offers no `allow_once` option, or the audit fact is not
    /// written, the answer is `cancelled`. Pagis never selects an
    /// "always" option, so the authority stays in Pagis.
    async fn permission(
        &self,
        session: &CodingSession,
        ask: PermissionAsk,
    ) -> Pending<PermissionAnswer> {
        let grant = self.host_grant(session).await;
        // With no live Grant the mode is `person` and there are no rules.
        let mode = effective_mode(grant.as_ref(), session);
        let rules = grant.as_ref().map(Grant::allow_rules).unwrap_or_default();
        let kind = harness_tool_kind(ask.kind);
        let command = ask
            .raw_input
            .as_ref()
            .and_then(|input| input.get("command"))
            .and_then(Value::as_str);
        let locations: Vec<String> = ask
            .locations
            .iter()
            .map(|location| location.to_string_lossy().into_owned())
            .collect();
        // The directory that the harness runs in. A session always has it
        // once it is open; an empty directory holds nothing.
        let directory = session.working_directory.as_deref().unwrap_or_default();
        let outcome = evaluate(mode, kind, &locations, command, directory, &rules);
        let fact = Fact {
            workspace_id: session.workspace_id.clone(),
            agent_id: session.agent_id.clone(),
            payload: json!({
                "session_id": session.id,
                "agent_id": session.agent_id,
                "host_id": session.host_id,
                "tool_call_id": ask.tool_call_id,
                "tool_kind": kind.as_str(),
                "command": command,
                "locations": locations,
                "grant_revision": grant.as_ref().map(|grant| grant.revision),
            }),
        };
        match outcome {
            PolicyOutcome::Allow(decider) => {
                let offered = ask.options.contains(&PermissionOptionKind::AllowOnce);
                let event = if offered {
                    fact.event(
                        Some(decider),
                        Outcome::Allowed,
                        Some(PermissionOptionKind::AllowOnce),
                    )
                } else {
                    fact.event(Some(decider), Outcome::Cancelled, None)
                };
                let answer = match self.bus.publish(event).await {
                    Ok(_) if offered => PermissionAnswer::AllowOnce,
                    Ok(_) => PermissionAnswer::Cancel,
                    Err(error) => {
                        tracing::error!(session = %session.id, %error, "the audit fact of a Harness Permission was not written, so it is cancelled");
                        PermissionAnswer::Cancel
                    }
                };
                Pending::Decided {
                    answer,
                    decider: Some(decider),
                }
            }
            PolicyOutcome::AskAgent => {
                let permission = Permission {
                    tool_call_id: ask.tool_call_id.clone(),
                    title: ask.title.clone(),
                    kind,
                    command: command.map(str::to_string),
                    locations,
                };
                self.ask_agent(session, permission, ask.options, fact).await
            }
            PolicyOutcome::AskPerson => {
                let permission = Permission {
                    tool_call_id: ask.tool_call_id.clone(),
                    title: ask.title.clone(),
                    kind,
                    command: command.map(str::to_string),
                    locations,
                };
                match self
                    .person
                    .ask(session, &permission, &ask.options, fact.clone(), None)
                    .await
                {
                    Some(answer) => Pending::Waits {
                        waits_for: WaitsFor::Person,
                        answer,
                    },
                    None => {
                        let event = fact.event(None, Outcome::Cancelled, None);
                        if let Err(error) = self.bus.publish(event).await {
                            tracing::error!(session = %session.id, %error, "the audit fact of a cancelled Harness Permission was not written");
                        }
                        // Nobody decided: the card was not posted.
                        Pending::Decided {
                            answer: PermissionAnswer::Cancel,
                            decider: None,
                        }
                    }
                }
            }
        }
    }

    /// Gives the question to the supervising Agent, whatever the mode,
    /// and waits for its answer. The Agent answers from what it knows, or
    /// asks the Person with `ask_user` first. The answer is `accept` with
    /// the Agent's values, or `cancel` when a woken Run ends without one.
    async fn question(&self, session: &CodingSession, ask: QuestionAsk) -> Pending<QuestionAnswer> {
        let Some(form) = Form::parse(&ask.schema) else {
            // Pagis cannot check a value for the form, so nobody answers.
            return Pending::Decided {
                answer: QuestionAnswer::Decline,
                decider: None,
            };
        };
        let watch = self.turn_watch(session).await;
        let (waiting, answer) = self.agent.wait_question(&session.id, form);
        Pending::Waits {
            waits_for: WaitsFor::Agent,
            answer: Box::pin(async move {
                let _waiting = waiting;
                tokio::select! {
                    biased;
                    answer = answer => match answer {
                        Ok(answer) => Waited::Answered {
                            answer: QuestionAnswer::Accept(answer.values),
                            by: DecidedBy {
                                decider: Some(Decider::Agent),
                                note: None,
                                run_id: Some(answer.run_id),
                            },
                        },
                        // The daemon stops: nobody answers, so the
                        // question waits until the session drops it.
                        Err(_) => std::future::pending().await,
                    },
                    () = watch.ended() => Waited::Answered {
                        answer: QuestionAnswer::Cancel,
                        by: DecidedBy {
                            decider: None,
                            note: Some(NO_ANSWER_NOTE.to_string()),
                            run_id: None,
                        },
                    },
                }
            }),
        }
    }
}

impl PolicyDecisions {
    /// A permission that waits for the Agent's verdict. A drop of the
    /// wait before the verdict writes the audit fact `cancelled`.
    async fn ask_agent(
        &self,
        session: &CodingSession,
        permission: Permission,
        options: Vec<PermissionOptionKind>,
        fact: Fact,
    ) -> Pending<PermissionAnswer> {
        let watch = self.turn_watch(session).await;
        let (waiting, verdict) = self.agent.wait(&session.id);
        let unanswered = Unanswered {
            bus: Arc::clone(&self.bus),
            event: Some(fact.clone().event(None, Outcome::Cancelled, None)),
        };
        let agent = AgentWait {
            bus: Arc::clone(&self.bus),
            person: self.person.clone(),
            session: session.clone(),
            permission,
            options,
            fact,
        };
        Pending::Waits {
            waits_for: WaitsFor::Agent,
            answer: Box::pin(async move {
                let mut unanswered = unanswered;
                let verdict = {
                    let _waiting = waiting;
                    tokio::select! {
                        biased;
                        verdict = verdict => verdict.ok(),
                        () = watch.ended() => Some(Verdict::Escalate(DecidedBy {
                            decider: None,
                            note: Some(NO_DECISION_NOTE.to_string()),
                            run_id: None,
                        })),
                    }
                };
                // The daemon stops: nobody hands a verdict, so the
                // permission waits until the session drops it.
                let Some(verdict) = verdict else {
                    return std::future::pending().await;
                };
                // The verdict writes its own audit fact, or the Person's
                // decision does.
                unanswered.event = None;
                agent.apply(verdict).await
            }),
        }
    }

    /// The watch of the ends of the Runs of the owning Agent in the
    /// session's Thread, for an ask that comes now.
    async fn turn_watch(&self, session: &CodingSession) -> TurnWatch {
        // Subscribe before the read of the active Runs, so no end of a
        // later Run slips past.
        let events = self
            .bus
            .subscribe(EventScope::workspace(&session.workspace_id), None)
            .await;
        TurnWatch {
            runs: Arc::clone(&self.runs),
            session: session.clone(),
            active: self.active_runs(session).await,
            events,
        }
    }

    /// The Runs of the owning Agent in the session's Thread that are not
    /// over. A failed read is logged and reads as none, so the next end
    /// of a Run there gives the permission to the Person.
    async fn active_runs(&self, session: &CodingSession) -> HashSet<RunId> {
        let runs = self
            .runs
            .list(
                &session.workspace_id,
                Some(&session.agent_id),
                Some(&session.channel_id),
                &ACTIVE_STATES,
                None,
                ACTIVE_RUNS,
            )
            .await;
        match runs {
            Ok(runs) => runs
                .into_iter()
                .filter(|run| run.root_message_id.as_ref() == Some(&session.root_message_id))
                .map(|run| run.id)
                .collect(),
            Err(error) => {
                tracing::warn!(session = %session.id, %error, "the active Runs of a Coding Session's Thread were not read");
                HashSet::new()
            }
        }
    }
}

/// A permission that waits for the Agent, and what its verdict needs.
struct AgentWait {
    bus: Arc<dyn EventBus>,
    person: PersonAsks,
    session: CodingSession,
    permission: Permission,
    options: Vec<PermissionOptionKind>,
    fact: Fact,
}

impl AgentWait {
    /// Answers the harness with the Agent's decision, or asks the Person
    /// on an approval card.
    async fn apply(self, verdict: Verdict) -> Waited<PermissionAnswer> {
        match verdict {
            Verdict::Decide {
                allow,
                note,
                run_id,
            } => {
                let (wanted, answer, outcome) = if allow {
                    (
                        PermissionOptionKind::AllowOnce,
                        PermissionAnswer::AllowOnce,
                        Outcome::Allowed,
                    )
                } else {
                    (
                        PermissionOptionKind::RejectOnce,
                        PermissionAnswer::RejectOnce,
                        Outcome::Rejected,
                    )
                };
                let offered = self.options.contains(&wanted);
                let mut event = if offered {
                    self.fact.event(Some(Decider::Agent), outcome, Some(wanted))
                } else {
                    self.fact
                        .event(Some(Decider::Agent), Outcome::Cancelled, None)
                };
                event.payload["note"] = json!(note);
                event.payload["run_id"] = json!(run_id);
                let answer = match self.bus.publish(event).await {
                    Ok(_) if offered => answer,
                    Ok(_) => PermissionAnswer::Cancel,
                    Err(error) => {
                        tracing::error!(session = %self.session.id, %error, "the audit fact of a Harness Permission was not written, so it is cancelled");
                        PermissionAnswer::Cancel
                    }
                };
                Waited::Answered {
                    answer,
                    by: DecidedBy {
                        decider: Some(Decider::Agent),
                        note: Some(note),
                        run_id: Some(run_id),
                    },
                }
            }
            Verdict::Escalate(by) => {
                let asked = self
                    .person
                    .ask(
                        &self.session,
                        &self.permission,
                        &self.options,
                        self.fact.clone(),
                        by.note.as_deref(),
                    )
                    .await;
                let answer: BoxFuture<'static, Waited<PermissionAnswer>> = match asked {
                    Some(answer) => answer,
                    None => {
                        let event = self.fact.event(None, Outcome::Cancelled, None);
                        if let Err(error) = self.bus.publish(event).await {
                            tracing::error!(session = %self.session.id, %error, "the audit fact of a cancelled Harness Permission was not written");
                        }
                        // Nobody decided: the card was not posted.
                        Box::pin(std::future::ready(Waited::answered(
                            PermissionAnswer::Cancel,
                            None,
                        )))
                    }
                };
                Waited::Escalated { by, answer }
            }
        }
    }
}

/// Watches the ends of the Runs of the owning Agent in the session's
/// Thread, for an ask that waits for the Agent.
struct TurnWatch {
    runs: Arc<dyn RunStore>,
    session: CodingSession,
    /// The Runs that were not over when the ask came. The Wake-up of the
    /// ask waits behind them (ADR-0006), so their ends do not count.
    active: HashSet<RunId>,
    events: EventStream,
}

impl TurnWatch {
    /// Ends when a Run of the Agent in the Thread, that started after the
    /// ask came, reaches a terminal state. It never ends when the
    /// bus ends.
    async fn ended(mut self) {
        while let Some(event) = self.events.next().await {
            if event.event_type != "run.state_changed"
                || event.agent_id.as_ref() != Some(&self.session.agent_id)
                || event.channel_id.as_ref() != Some(&self.session.channel_id)
            {
                continue;
            }
            let terminal = serde_json::from_value::<RunState>(event.payload["to"].clone())
                .is_ok_and(|state| state.is_terminal());
            let Some(run_id) = event.run_id.filter(|run_id| !self.active.contains(run_id)) else {
                continue;
            };
            if !terminal {
                continue;
            }
            match self.runs.get(&self.session.workspace_id, &run_id).await {
                Ok(Some(run))
                    if run.root_message_id.as_ref() == Some(&self.session.root_message_id) =>
                {
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(session = %self.session.id, run = %run_id, %error, "a Run that ended was not read");
                }
            }
        }
        std::future::pending().await
    }
}

/// The fields of the audit fact of one permission, before its decision.
#[derive(Clone)]
pub(crate) struct Fact {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    payload: Value,
}

/// How a Harness Permission ended, in the audit fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// The harness got the `allow_once` option.
    Allowed,
    /// The harness got the `reject_once` option.
    Rejected,
    /// The harness got `cancelled`.
    Cancelled,
    /// The harness stopped waiting before the Person decided, and the
    /// Request of the card expired. The harness got `cancelled`.
    Expired,
}

impl Fact {
    /// The audit fact with its decision. A permission that nobody
    /// decided, such as one that a cancel ended, has no decider.
    pub(crate) fn event(
        mut self,
        decider: Option<Decider>,
        outcome: Outcome,
        option_kind: Option<PermissionOptionKind>,
    ) -> NewEvent {
        self.payload["decider"] = json!(decider);
        self.payload["outcome"] = json!(outcome);
        self.payload["option_kind"] = json!(option_kind);
        NewEvent {
            workspace_id: self.workspace_id,
            event_type: PERMISSION_DECIDED_EVENT.to_string(),
            agent_id: Some(self.agent_id),
            run_id: None,
            channel_id: None,
            payload: self.payload,
        }
    }
}

/// Writes the `cancelled` audit fact of a waiting permission when the
/// session drops its wait.
struct Unanswered {
    bus: Arc<dyn EventBus>,
    event: Option<NewEvent>,
}

impl Drop for Unanswered {
    fn drop(&mut self) {
        let Some(event) = self.event.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                "the audit fact of a cancelled Harness Permission was not written: no runtime"
            );
            return;
        };
        let bus = Arc::clone(&self.bus);
        runtime.spawn(async move {
            if let Err(error) = bus.publish(event).await {
                tracing::warn!(%error, "the audit fact of a cancelled Harness Permission was not written");
            }
        });
    }
}

/// The broker's tool kind of a tool kind of the ACP client.
fn harness_tool_kind(kind: ToolKind) -> HarnessToolKind {
    match kind {
        ToolKind::Read => HarnessToolKind::Read,
        ToolKind::Edit => HarnessToolKind::Edit,
        ToolKind::Delete => HarnessToolKind::Delete,
        ToolKind::Move => HarnessToolKind::Move,
        ToolKind::Search => HarnessToolKind::Search,
        ToolKind::Execute => HarnessToolKind::Execute,
        ToolKind::Think => HarnessToolKind::Think,
        ToolKind::Fetch => HarnessToolKind::Fetch,
        ToolKind::SwitchMode => HarnessToolKind::SwitchMode,
        ToolKind::Other => HarnessToolKind::Other,
    }
}
