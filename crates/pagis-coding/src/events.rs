//! The Incoming Events of a Coding Session and its Session Rule
//! (ADR-0033, ADR-0006).
//!
//! When a session starts, the daemon makes its Session Rule: one Event
//! Subscription for each kind of [`SessionNews`], in the session's Thread.
//! Each end of a turn, each decision that waits and the end of the session
//! go to the Trigger module as one batch with one event, and a Wake-up
//! starts a Run of the owning Agent in that Thread. When the session
//! closes or fails, the daemon ends the rule after the last event, so the
//! Wake-up of that event stays.
//!
//! The Trigger module lives above this crate, so the crate stops at two
//! seams: [`SessionRules`] and [`SessionEvents`].

use async_trait::async_trait;
use pagis_broker::{
    CODING_SESSION_ENDED, CODING_SESSION_NEEDS_DECISION, CODING_SESSION_TURN_ENDED,
};
use pagis_core::{
    CodingSession, CodingSessionState, EventMatcher, IngestBatch, NormalizedEvent, UnixMillis,
};
use serde_json::{Map, Value};

use crate::StopReason;

/// What the Wake-up of a Session Rule tells the Agent to do. The event
/// names the session and never the text of the harness, so the Agent
/// reads the session with a tool.
pub const SESSION_RULE_INSTRUCTION: &str = "Your coding session changed. Read it with \
     `coding_session_read`, then supervise it: send the next prompt, decide, answer its \
     question, or report to the user.";

/// The name of the Session Rule of a session.
pub fn session_rule_name(session: &CodingSession) -> String {
    format!("Coding session: {}", session.title)
}

/// What happened to a session that its Agent must hear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionNews {
    /// A turn ended, and the session is `idle`. `seq` is the row of the
    /// end of the turn.
    TurnEnded { stop_reason: StopReason, seq: i64 },
    /// A Harness Permission or a question waits, and the session is
    /// `needs_decision`. `seq` is the row of the ask.
    NeedsDecision {
        decision_kind: DecisionKind,
        seq: i64,
    },
    /// The session is `closed` or `failed`. The record gives the state
    /// and the end reason.
    Ended,
    /// The session is `interrupted`. The record holds no end reason, so
    /// the news holds the reason of the interruption.
    Interrupted { reason: InterruptReason },
}

impl SessionNews {
    /// The Incoming Event kind of the news.
    pub fn event_kind(self) -> &'static str {
        match self {
            SessionNews::TurnEnded { .. } => CODING_SESSION_TURN_ENDED,
            SessionNews::NeedsDecision { .. } => CODING_SESSION_NEEDS_DECISION,
            SessionNews::Ended | SessionNews::Interrupted { .. } => CODING_SESSION_ENDED,
        }
    }
}

/// Why a session is `interrupted`. The harness keeps its own session on
/// its place, so the Agent can resume it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    /// The Host or its session socket went away.
    HostLost,
    /// The Agent's Computer stopped while its harness ran.
    ComputerStopped,
    /// The daemon stopped, and every ACP connection with it.
    DaemonRestart,
}

impl InterruptReason {
    pub fn as_str(self) -> &'static str {
        match self {
            InterruptReason::HostLost => "host_lost",
            InterruptReason::ComputerStopped => "computer_stopped",
            InterruptReason::DaemonRestart => "daemon_restart",
        }
    }
}

/// The kind of a decision that waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Permission,
    Question,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Permission => "permission",
            DecisionKind::Question => "question",
        }
    }
}

/// The Incoming Event of one piece of news of a session on `machine`.
///
/// The identity of a row event is `<session id>:<seq>`, and of an end is
/// `<session id>:ended:<state>`, so a replay wakes nobody twice. A
/// resumed session can be interrupted again, so the identity of an
/// interruption also holds the time of the record that it wrote:
/// `<session id>:ended:interrupted:<updated_at>`. The metadata holds the
/// daemon's own fields and the Agent's title, and no text of the harness.
pub fn session_event(
    session: &CodingSession,
    machine: &str,
    news: SessionNews,
    occurred_at: UnixMillis,
) -> NormalizedEvent {
    let mut metadata = Map::new();
    metadata.insert("coding_session_id".into(), session.id.as_str().into());
    metadata.insert("title".into(), session.title.clone().into());
    metadata.insert("harness".into(), session.harness_id.clone().into());
    metadata.insert("machine".into(), machine.into());
    let provider_event_id = match news {
        SessionNews::TurnEnded { stop_reason, seq } => {
            metadata.insert("stop_reason".into(), stop_reason_text(stop_reason).into());
            metadata.insert("seq".into(), seq.into());
            format!("{}:{seq}", session.id)
        }
        SessionNews::NeedsDecision { decision_kind, seq } => {
            metadata.insert("decision_kind".into(), decision_kind.as_str().into());
            metadata.insert("seq".into(), seq.into());
            format!("{}:{seq}", session.id)
        }
        SessionNews::Ended => {
            metadata.insert("state".into(), session.state.as_str().into());
            metadata.insert(
                "reason".into(),
                session.end_reason.clone().unwrap_or_default().into(),
            );
            format!("{}:ended:{}", session.id, session.state.as_str())
        }
        SessionNews::Interrupted { reason } => {
            metadata.insert(
                "state".into(),
                CodingSessionState::Interrupted.as_str().into(),
            );
            metadata.insert("reason".into(), reason.as_str().into());
            format!("{}:ended:interrupted:{}", session.id, session.updated_at)
        }
    };
    NormalizedEvent {
        provider_event_id,
        metadata: Value::Object(metadata),
        occurred_at,
        // The rule targets the session's Thread.
        landing: None,
    }
}

/// The one batch of one piece of news. A Coding Session source has no
/// cursor and no baseline, and the batch reaches the rules of the owning
/// Agent alone.
pub(crate) fn session_batch(
    session: &CodingSession,
    machine: &str,
    news: SessionNews,
    at: UnixMillis,
) -> IngestBatch {
    IngestBatch {
        workspace_id: session.workspace_id.clone(),
        source: pagis_core::EventSource::coding_session(session.id.clone()),
        agent_id: Some(session.agent_id.clone()),
        event_kind: news.event_kind().to_string(),
        cursor: None,
        events: vec![session_event(session, machine, news, at)],
        received_at: at,
        baseline: false,
    }
}

fn stop_reason_text(stop_reason: StopReason) -> &'static str {
    match stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
    }
}

/// The matcher of the Coding Session kinds. The source of a Session Rule
/// names the one session, so every event of that source matches.
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionEventMatcher;

impl EventMatcher for SessionEventMatcher {
    fn matches(&self, _filter: &Value, _metadata: &Value) -> bool {
        true
    }
}

/// Why the Session Rule of a session was not written or ended, or why an
/// event did not reach it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SessionRuleError(pub String);

/// The Session Rule of each session: made at the start, and ended when
/// the session closes or fails.
#[async_trait]
pub trait SessionRules: Send + Sync {
    async fn create(&self, session: &CodingSession) -> Result<(), SessionRuleError>;
    async fn end(&self, session: &CodingSession) -> Result<(), SessionRuleError>;
}

/// The `ingest` seam over the Trigger module.
#[async_trait]
pub trait SessionEvents: Send + Sync {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), SessionRuleError>;
}

#[cfg(test)]
mod tests {
    use pagis_core::{
        AgentId, ChannelId, CodingSessionId, CodingSessionPlace, CodingSessionUsage, HostId,
        MessageId, RunId, SessionApprovalMode, WorkspaceId,
    };
    use serde_json::json;

    use super::*;

    fn session(state: CodingSessionState, end_reason: Option<&str>) -> CodingSession {
        let message_id = MessageId::generate();
        CodingSession {
            id: CodingSessionId::from("cs_1".to_string()),
            workspace_id: WorkspaceId::generate(),
            agent_id: AgentId::generate(),
            harness_id: "claude".to_string(),
            harness_version: "1.0.0".to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(HostId::generate()),
            directory: "/Users/bo/code/app".to_string(),
            working_directory: None,
            worktree_branch: None,
            approval_mode: SessionApprovalMode::Person,
            harness_mode: None,
            harness_modes: Vec::new(),
            model: None,
            thought_level: None,
            title: "Fix the login".to_string(),
            state,
            end_reason: end_reason.map(str::to_string),
            end_detail: Some("exit code 1\npanicked".to_string()),
            acp_session_id: None,
            channel_id: ChannelId::generate(),
            root_message_id: message_id.clone(),
            message_id,
            run_id: RunId::generate(),
            usage: CodingSessionUsage::default(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
        }
    }

    #[test]
    fn a_row_event_is_named_by_the_session_and_the_seq_of_its_row() {
        let idle = session(CodingSessionState::Idle, None);
        let turn = session_event(
            &idle,
            "Air",
            SessionNews::TurnEnded {
                stop_reason: StopReason::MaxTokens,
                seq: 7,
            },
            10,
        );
        assert_eq!(turn.provider_event_id, "cs_1:7");
        assert_eq!(turn.landing, None);
        assert_eq!(
            turn.metadata,
            json!({
                "coding_session_id": "cs_1",
                "title": "Fix the login",
                "harness": "claude",
                "machine": "Air",
                "stop_reason": "max_tokens",
                "seq": 7
            })
        );

        let waiting = session(CodingSessionState::NeedsDecision, None);
        let decision = session_event(
            &waiting,
            "Air",
            SessionNews::NeedsDecision {
                decision_kind: DecisionKind::Question,
                seq: 9,
            },
            10,
        );
        assert_eq!(decision.provider_event_id, "cs_1:9");
        assert_eq!(decision.metadata["decision_kind"], "question");
        assert_eq!(decision.metadata["seq"], 9);
    }

    #[test]
    fn an_end_is_named_by_the_session_and_its_state_and_holds_no_end_detail() {
        let failed = session(CodingSessionState::Failed, Some("harness_exited"));
        let ended = session_event(&failed, "Air", SessionNews::Ended, 10);
        assert_eq!(ended.provider_event_id, "cs_1:ended:failed");
        assert_eq!(ended.metadata["state"], "failed");
        assert_eq!(ended.metadata["reason"], "harness_exited");
        assert!(
            !ended.metadata.to_string().contains("panicked"),
            "the end detail is harness text"
        );
    }

    #[test]
    fn an_interruption_holds_its_reason_and_the_time_of_its_record() {
        let mut interrupted = session(CodingSessionState::Interrupted, None);
        interrupted.updated_at = 42;
        let news = SessionNews::Interrupted {
            reason: InterruptReason::HostLost,
        };

        let event = session_event(&interrupted, "Air", news, 10);

        assert_eq!(event.provider_event_id, "cs_1:ended:interrupted:42");
        assert_eq!(event.metadata["state"], "interrupted");
        assert_eq!(event.metadata["reason"], "host_lost");
        assert_eq!(news.event_kind(), CODING_SESSION_ENDED);
        interrupted.updated_at = 43;
        assert_ne!(
            session_event(&interrupted, "Air", news, 10).provider_event_id,
            event.provider_event_id,
            "a second interruption of a resumed session wakes the Agent again"
        );
    }

    #[test]
    fn the_batch_goes_to_the_rules_of_the_owning_agent_alone() {
        let idle = session(CodingSessionState::Idle, None);
        let news = SessionNews::TurnEnded {
            stop_reason: StopReason::EndTurn,
            seq: 3,
        };
        let batch = session_batch(&idle, "Air", news, 10);
        assert_eq!(batch.event_kind, CODING_SESSION_TURN_ENDED);
        assert_eq!(batch.agent_id.as_ref(), Some(&idle.agent_id));
        assert_eq!(batch.source.coding_session_id(), Some(&idle.id));
        assert_eq!(batch.cursor, None);
        assert!(!batch.baseline);
        assert_eq!(batch.events.len(), 1);
    }

    #[test]
    fn the_matcher_matches_an_empty_filter() {
        let metadata = session_event(
            &session(CodingSessionState::Closed, Some("closed")),
            "Air",
            SessionNews::Ended,
            10,
        )
        .metadata;
        assert!(SessionEventMatcher.matches(&json!({}), &metadata));
    }
}
