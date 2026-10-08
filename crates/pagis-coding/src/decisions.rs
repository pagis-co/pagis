//! Who answers the permission requests and the questions of a Coding
//! Session.
//!
//! [`CodingSessions`](crate::CodingSessions) gives each session an
//! [`AskHandler`](crate::AskHandler) that records each ask and its answer
//! in the transcript and moves the session through `needs_decision`. The
//! answer itself comes from [`SessionDecisions`].

use async_trait::async_trait;
use futures::future::BoxFuture;
use pagis_core::CodingSession;
use serde::{Deserialize, Serialize};

use crate::{PermissionAnswer, PermissionAsk, QuestionAnswer, QuestionAsk};

/// Answers the asks of the harness of a Coding Session.
///
/// Each method says at once who answers, so the transcript names who the
/// session waits for, and gives the answer as a future that can wait for
/// hours. The session drops the future when the harness withdraws the
/// ask or the turn is cancelled.
#[async_trait]
pub trait SessionDecisions: Send + Sync {
    async fn permission(
        &self,
        session: &CodingSession,
        ask: PermissionAsk,
    ) -> Pending<PermissionAnswer>;

    async fn question(&self, session: &CodingSession, ask: QuestionAsk) -> Pending<QuestionAnswer>;
}

/// An ask that waits for its answer.
pub struct Pending<T> {
    pub waits_for: WaitsFor,
    pub answer: BoxFuture<'static, T>,
}

impl<T: Send + 'static> Pending<T> {
    /// An answer that is ready now.
    pub fn ready(waits_for: WaitsFor, answer: T) -> Self {
        Self {
            waits_for,
            answer: Box::pin(std::future::ready(answer)),
        }
    }
}

/// Who answers an ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitsFor {
    /// The Person.
    Person,
    /// The supervising Agent.
    Agent,
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
