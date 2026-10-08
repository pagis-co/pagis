//! Pagis policy for each Harness Permission of a Coding Session on a
//! Host (ADR-0033).
//!
//! [`PolicyDecisions`] reads the live host Grant of the owning Agent at
//! each permission, so a narrower Grant applies to the next permission
//! of a running session. The broker's [`evaluate`] decides. Each decision
//! writes one audit fact, [`PERMISSION_DECIDED_EVENT`], on the bus.
//!
//! A permission that policy does not allow waits. In the `person` mode
//! the Person answers it on an approval card ([`crate::person`]). The
//! Agent's decision is not built, so nothing answers a permission in the
//! `agent` mode: the wait ends when the turn is cancelled or the harness
//! withdraws the request.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{Decider, HarnessToolKind, PolicyOutcome, evaluate};
use pagis_core::{
    AgentId, CodingSession, EventBus, Grant, GrantStore, HostStore, MessageStore, NewEvent,
    RequestStore, SessionApprovalMode, WorkspaceId,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::person::{Permission, PersonAsks};
use crate::{
    Pending, PermissionAnswer, PermissionAsk, PermissionOptionKind, QuestionAnswer, QuestionAsk,
    SessionDecisions, ToolKind, WaitsFor,
};

/// The audit fact of each decision on a Harness Permission.
pub const PERMISSION_DECIDED_EVENT: &str = "coding_session.permission_decided";

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
    /// The audit facts, the events of the card, and the decisions.
    pub bus: Arc<dyn EventBus>,
}

/// Applies Pagis policy to each Harness Permission, and cancels each
/// question of a harness.
pub struct PolicyDecisions {
    grants: Arc<dyn GrantStore>,
    bus: Arc<dyn EventBus>,
    person: PersonAsks,
}

impl PolicyDecisions {
    pub fn new(deps: PolicyDecisionsDeps) -> Self {
        Self {
            grants: deps.grants,
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
        let host_id = session.host_id.as_ref()?;
        match self
            .grants
            .live_for_resource(
                &session.workspace_id,
                &session.agent_id,
                Grant::HOST_KIND,
                host_id.as_str(),
            )
            .await
        {
            Ok(grant) => grant,
            Err(error) => {
                tracing::warn!(session = %session.id, %error, "the host Grant of a Coding Session was not read");
                None
            }
        }
    }
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
        let mode = grant
            .as_ref()
            .map_or(SessionApprovalMode::Person, Grant::session_approval_mode)
            .min(session.approval_mode);
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
            PolicyOutcome::AskAgent => self.wait(WaitsFor::Agent, fact),
            PolicyOutcome::AskPerson => {
                let permission = Permission {
                    tool_call_id: &ask.tool_call_id,
                    title: ask.title.as_deref(),
                    kind,
                    command,
                    locations: &locations,
                };
                match self
                    .person
                    .ask(session, &permission, &ask.options, fact.clone())
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

    /// The question in the daemon is not built, so each question is
    /// cancelled at once.
    async fn question(
        &self,
        _session: &CodingSession,
        _ask: QuestionAsk,
    ) -> Pending<QuestionAnswer> {
        Pending::Decided {
            answer: QuestionAnswer::Cancel,
            decider: None,
        }
    }
}

impl PolicyDecisions {
    /// A permission that waits for `waits_for`. Nothing answers it, so
    /// the wait ends only when the session drops it, and the audit fact
    /// then says `cancelled`.
    fn wait(&self, waits_for: WaitsFor, fact: Fact) -> Pending<PermissionAnswer> {
        let unanswered = Unanswered {
            bus: Arc::clone(&self.bus),
            event: Some(fact.event(None, Outcome::Cancelled, None)),
        };
        Pending::Waits {
            waits_for,
            answer: Box::pin(async move {
                let _unanswered = unanswered;
                std::future::pending().await
            }),
        }
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
