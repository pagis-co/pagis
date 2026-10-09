//! The asks of a harness that wait for the supervising Agent
//! (ADR-0033).
//!
//! In the `agent` mode, Pagis policy puts each Harness Permission that
//! asks the Agent here, and the core tools `coding_session_decide` and
//! `coding_session_escalate` hand the Agent's verdict to it. A session can
//! hold more than one, and a verdict goes to the oldest.
//!
//! In each mode, each form question of a harness waits here for the
//! Agent, and the core tool `coding_session_answer` hands it the Agent's
//! values. A session holds at most one: the session asks its questions
//! one at a time, in arrival order.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pagis_core::{CodingSessionId, RunId};
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use crate::DecidedBy;
use crate::form::{Form, ValueError};

/// The verdict on a permission that waits for the Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The Agent allows the action once, or denies it.
    Decide {
        allow: bool,
        note: String,
        run_id: RunId,
    },
    /// The permission goes to the Person on an approval card.
    Escalate(DecidedBy),
}

/// One permission that waits for the Agent.
struct Waiter {
    key: u64,
    verdict: oneshot::Sender<Verdict>,
}

/// The Agent's answer to a question: the values of the form, and the
/// Run that gave them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Answer {
    pub(crate) values: Map<String, Value>,
    pub(crate) run_id: RunId,
}

/// Why the Agent's answer did not reach the question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnswerRefusal {
    /// No question of the session waits for the Agent.
    NoQuestion,
    /// The values do not match the form. The question still waits.
    Invalid(ValueError),
}

/// The one question of a session that waits for the Agent.
struct QuestionWaiter {
    key: u64,
    form: Form,
    answer: oneshot::Sender<Answer>,
}

/// The asks that wait for the Agent, by session.
#[derive(Default)]
pub struct AgentAsks {
    waiting: Mutex<HashMap<CodingSessionId, VecDeque<Waiter>>>,
    questions: Mutex<HashMap<CodingSessionId, QuestionWaiter>>,
    next_key: AtomicU64,
}

impl AgentAsks {
    /// Whether a permission of the session waits for the Agent.
    pub(crate) fn waits(&self, session_id: &CodingSessionId) -> bool {
        self.waiting
            .lock()
            .expect("the waiting permissions")
            .get(session_id)
            .is_some_and(|waiters| waiters.iter().any(|waiter| !waiter.verdict.is_closed()))
    }

    /// Gives `verdict` to the oldest permission of the session that waits
    /// for the Agent. It gives the verdict back when none waits.
    pub(crate) fn hand(
        &self,
        session_id: &CodingSessionId,
        verdict: Verdict,
    ) -> Result<(), Verdict> {
        let mut waiting = self.waiting.lock().expect("the waiting permissions");
        let Some(waiters) = waiting.get_mut(session_id) else {
            return Err(verdict);
        };
        let mut verdict = verdict;
        // A permission whose wait ended meanwhile takes no verdict.
        let handed = loop {
            let Some(waiter) = waiters.pop_front() else {
                break Err(verdict);
            };
            match waiter.verdict.send(verdict) {
                Ok(()) => break Ok(()),
                Err(back) => verdict = back,
            }
        };
        if waiters.is_empty() {
            waiting.remove(session_id);
        }
        handed
    }

    /// Puts a permission of the session in the queue. The drop of the
    /// [`Waiting`] takes it out.
    pub(crate) fn wait(
        self: &Arc<Self>,
        session_id: &CodingSessionId,
    ) -> (Waiting, oneshot::Receiver<Verdict>) {
        let (verdict, received) = oneshot::channel();
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        self.waiting
            .lock()
            .expect("the waiting permissions")
            .entry(session_id.clone())
            .or_default()
            .push_back(Waiter { key, verdict });
        let waiting = Waiting {
            asks: Arc::clone(self),
            session_id: session_id.clone(),
            key,
        };
        (waiting, received)
    }
}

impl AgentAsks {
    /// Puts the question of the session with `form` in place, as the one
    /// that waits for the Agent. The drop of the [`WaitingQuestion`]
    /// takes it out.
    pub(crate) fn wait_question(
        self: &Arc<Self>,
        session_id: &CodingSessionId,
        form: Form,
    ) -> (WaitingQuestion, oneshot::Receiver<Answer>) {
        let (answer, received) = oneshot::channel();
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        let replaced = self
            .questions
            .lock()
            .expect("the waiting questions")
            .insert(session_id.clone(), QuestionWaiter { key, form, answer });
        if replaced.is_some() {
            tracing::warn!(session = %session_id, "a question of a Coding Session took the place of one that still waited");
        }
        let waiting = WaitingQuestion {
            asks: Arc::clone(self),
            session_id: session_id.clone(),
            key,
        };
        (waiting, received)
    }

    /// Hands `values` to the question of the session that waits for the
    /// Agent, when they match its form. The question waits on when they
    /// do not.
    pub(crate) fn answer(
        &self,
        session_id: &CodingSessionId,
        values: Map<String, Value>,
        run_id: RunId,
    ) -> Result<(), AnswerRefusal> {
        let mut questions = self.questions.lock().expect("the waiting questions");
        // A question whose wait ended meanwhile takes no answer.
        let waiter = questions
            .get(session_id)
            .filter(|waiter| !waiter.answer.is_closed())
            .ok_or(AnswerRefusal::NoQuestion)?;
        waiter.form.check(&values).map_err(AnswerRefusal::Invalid)?;
        let waiter = questions
            .remove(session_id)
            .expect("the question of the session");
        waiter
            .answer
            .send(Answer { values, run_id })
            .map_err(|_| AnswerRefusal::NoQuestion)
    }
}

/// The place of the question of a session, while it waits for the Agent.
pub(crate) struct WaitingQuestion {
    asks: Arc<AgentAsks>,
    session_id: CodingSessionId,
    key: u64,
}

impl Drop for WaitingQuestion {
    fn drop(&mut self) {
        let mut questions = self.asks.questions.lock().expect("the waiting questions");
        if questions
            .get(&self.session_id)
            .is_some_and(|waiter| waiter.key == self.key)
        {
            questions.remove(&self.session_id);
        }
    }
}

/// The place of one permission in the queue, while it waits for the
/// Agent.
pub(crate) struct Waiting {
    asks: Arc<AgentAsks>,
    session_id: CodingSessionId,
    key: u64,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut waiting = self.asks.waiting.lock().expect("the waiting permissions");
        if let Some(waiters) = waiting.get_mut(&self.session_id) {
            waiters.retain(|waiter| waiter.key != self.key);
            if waiters.is_empty() {
                waiting.remove(&self.session_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide() -> Verdict {
        Verdict::Decide {
            allow: true,
            note: "Safe.".to_string(),
            run_id: RunId::from("run-1".to_string()),
        }
    }

    #[test]
    fn a_verdict_goes_to_the_oldest_permission_of_the_session() {
        let asks = Arc::new(AgentAsks::default());
        let session = CodingSessionId::from("cs-1".to_string());
        let other = CodingSessionId::from("cs-2".to_string());
        let (_first, mut first) = asks.wait(&session);
        let (_second, mut second) = asks.wait(&session);

        assert!(asks.waits(&session));
        assert!(!asks.waits(&other));
        assert_eq!(asks.hand(&other, decide()), Err(decide()));
        assert_eq!(asks.hand(&session, decide()), Ok(()));

        assert_eq!(first.try_recv().ok(), Some(decide()));
        assert!(second.try_recv().is_err());
        assert!(asks.waits(&session));
    }

    fn branch_form() -> Form {
        Form::parse(&serde_json::json!({
            "type": "object",
            "properties": {"branch": {"type": "string"}},
            "required": ["branch"],
        }))
        .expect("a form that Pagis checks")
    }

    fn values(values: Value) -> Map<String, Value> {
        let Value::Object(values) = values else {
            panic!("the values are a JSON object");
        };
        values
    }

    #[test]
    fn values_that_do_not_match_the_form_leave_the_question_waiting() {
        let asks = Arc::new(AgentAsks::default());
        let session = CodingSessionId::from("cs-1".to_string());
        let run_id = RunId::from("run-1".to_string());
        let (_waiting, mut answer) = asks.wait_question(&session, branch_form());

        let refused = asks.answer(&session, values(serde_json::json!({})), run_id.clone());
        assert!(
            matches!(&refused, Err(AnswerRefusal::Invalid(error)) if error.property == "branch"),
            "{refused:?}"
        );
        assert!(answer.try_recv().is_err());

        let main = values(serde_json::json!({"branch": "main"}));
        assert_eq!(asks.answer(&session, main.clone(), run_id.clone()), Ok(()));
        assert_eq!(
            answer.try_recv().ok(),
            Some(Answer {
                values: main.clone(),
                run_id: run_id.clone(),
            })
        );
        assert_eq!(
            asks.answer(&session, main, run_id),
            Err(AnswerRefusal::NoQuestion)
        );
    }

    #[test]
    fn a_question_whose_wait_ended_takes_no_answer() {
        let asks = Arc::new(AgentAsks::default());
        let session = CodingSessionId::from("cs-1".to_string());
        let main = values(serde_json::json!({"branch": "main"}));
        let run_id = RunId::from("run-1".to_string());
        let (waiting, _answer) = asks.wait_question(&session, branch_form());

        drop(waiting);

        assert_eq!(
            asks.answer(&session, main, run_id),
            Err(AnswerRefusal::NoQuestion)
        );
    }

    #[test]
    fn a_permission_whose_wait_ended_takes_no_verdict() {
        let asks = Arc::new(AgentAsks::default());
        let session = CodingSessionId::from("cs-1".to_string());
        let (waiting, received) = asks.wait(&session);
        drop(received);
        let (_open, mut open) = asks.wait(&session);

        assert_eq!(asks.hand(&session, decide()), Ok(()));
        assert_eq!(open.try_recv().ok(), Some(decide()));

        drop(waiting);
        assert!(!asks.waits(&session));
        assert_eq!(asks.hand(&session, decide()), Err(decide()));
    }
}
