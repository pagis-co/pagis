//! The Harness Permissions that wait for the supervising Agent, in the
//! `agent` mode (ADR-0033).
//!
//! Pagis policy puts each permission that asks the Agent here, and the
//! core tools `coding_session_decide` and `coding_session_escalate` hand
//! the Agent's verdict to it. A session can hold more than one, and a
//! verdict goes to the oldest.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pagis_core::{CodingSessionId, RunId};
use tokio::sync::oneshot;

use crate::DecidedBy;

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

/// The permissions that wait for the Agent, by session.
#[derive(Default)]
pub struct AgentAsks {
    waiting: Mutex<HashMap<CodingSessionId, VecDeque<Waiter>>>,
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
