//! Who answers the permission requests and the questions of a Coding
//! Session.
//!
//! [`CodingSessions`](crate::CodingSessions) gives each session an
//! [`AskHandler`](crate::AskHandler) that records each ask and its answer
//! in the transcript and moves the session through `needs_decision`. The
//! answer itself comes from [`SessionDecisions`].

use async_trait::async_trait;
use futures::future::BoxFuture;
use pagis_broker::Decider;
use pagis_core::{CodingSession, RunId};
use serde::{Deserialize, Serialize};

use crate::{PermissionAnswer, PermissionAsk, QuestionAnswer, QuestionAsk};

/// Answers the asks of the harness of a Coding Session.
///
/// Each method gives the answer at once, or says at once who answers, so
/// the transcript names who the session waits for, and gives the answer
/// as a future that can wait for hours. The session drops the future
/// when the harness withdraws the ask or the turn is cancelled.
#[async_trait]
pub trait SessionDecisions: Send + Sync {
    async fn permission(
        &self,
        session: &CodingSession,
        ask: PermissionAsk,
    ) -> Pending<PermissionAnswer>;

    async fn question(&self, session: &CodingSession, ask: QuestionAsk) -> Pending<QuestionAnswer>;
}

/// The answer to an ask.
pub enum Pending<T> {
    /// The daemon answers at once, and the session does not wait.
    /// `decider` names the Pagis policy that decided a permission. A
    /// question has none.
    Decided { answer: T, decider: Option<Decider> },
    /// The ask waits for its answer, and the session is `needs_decision`
    /// until it comes.
    Waits {
        waits_for: WaitsFor,
        answer: BoxFuture<'static, Waited<T>>,
    },
}

impl<T: Send + 'static> Pending<T> {
    /// An ask that waits for `waits_for`, with an answer of `waits_for`
    /// that is ready now.
    pub fn ready(waits_for: WaitsFor, answer: T) -> Self {
        Self::Waits {
            waits_for,
            answer: Box::pin(std::future::ready(Waited::answered(
                answer,
                Some(waits_for.decider()),
            ))),
        }
    }
}

/// What came of an ask that waits.
pub enum Waited<T> {
    /// The ask has its answer.
    Answered { answer: T, by: DecidedBy },
    /// The supervising Agent, or the daemon for it, gave the ask to the
    /// Person. The ask waits on for `answer`, and the session waits for
    /// the Person.
    Escalated {
        by: DecidedBy,
        answer: BoxFuture<'static, Waited<T>>,
    },
}

impl<T> Waited<T> {
    /// An answer of `decider`, with no note.
    pub fn answered(answer: T, decider: Option<Decider>) -> Self {
        Self::Answered {
            answer,
            by: DecidedBy {
                decider,
                ..DecidedBy::default()
            },
        }
    }
}

/// Who gave a decision and why, for its transcript row and its audit
/// fact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecidedBy {
    /// None for a decision that nobody made, such as an expiry, or an
    /// escalation of the daemon.
    pub decider: Option<Decider>,
    /// The Agent's reason, or the daemon's.
    pub note: Option<String>,
    /// The Run of the Agent that decided.
    pub run_id: Option<RunId>,
}

/// Who answers an ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitsFor {
    /// The Person.
    Person,
    /// The supervising Agent.
    Agent,
}

impl WaitsFor {
    /// The decider of an answer that this party gives.
    pub fn decider(self) -> Decider {
        match self {
            WaitsFor::Person => Decider::Person,
            WaitsFor::Agent => Decider::Agent,
        }
    }
}

/// Refuses each ask: it rejects each permission once and cancels each
/// question, in the name of the Person.
pub struct RefuseDecisions;

#[async_trait]
impl SessionDecisions for RefuseDecisions {
    async fn permission(
        &self,
        _session: &CodingSession,
        _ask: PermissionAsk,
    ) -> Pending<PermissionAnswer> {
        Pending::ready(WaitsFor::Person, PermissionAnswer::RejectOnce)
    }

    async fn question(
        &self,
        _session: &CodingSession,
        _ask: QuestionAsk,
    ) -> Pending<QuestionAnswer> {
        Pending::ready(WaitsFor::Person, QuestionAnswer::Cancel)
    }
}
