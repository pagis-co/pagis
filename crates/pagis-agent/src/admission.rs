//! Admission rules for the queued and running Runs of one Agent.
//! The value holds no task, clock, store, or Run signal.

use std::collections::VecDeque;

use pagis_core::{ChannelId, MessageId, Run, RunId, TriggerKind};

use crate::AgentLoopConfig;

/// The slot pool a Run takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Conversation,
    Arrival,
}

impl Pool {
    pub fn of(kind: TriggerKind) -> Self {
        match kind {
            TriggerKind::Arrival | TriggerKind::Review => Self::Arrival,
            TriggerKind::Conversation
            | TriggerKind::Message
            | TriggerKind::Schedule
            | TriggerKind::Event => Self::Conversation,
        }
    }
}

/// Runs with the same key execute one at a time. A root of `None`
/// names the Channel's top level. Each background Run has its own key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadKey {
    Conversation {
        channel_id: ChannelId,
        root_message_id: Option<MessageId>,
    },
    Background(RunId),
}

/// The fields of one Run that affect admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRun {
    pub id: RunId,
    thread: ThreadKey,
    pool: Pool,
    message: bool,
}

impl AdmissionRun {
    pub fn new(id: RunId, thread: ThreadKey, kind: TriggerKind) -> Self {
        Self {
            id,
            thread,
            pool: Pool::of(kind),
            message: matches!(kind, TriggerKind::Message | TriggerKind::Conversation),
        }
    }
}

impl From<&Run> for AdmissionRun {
    fn from(run: &Run) -> Self {
        let thread = if run.trigger_kind == TriggerKind::Conversation {
            let delivery = crate::conversations::ConversationDelivery::of(run)
                .expect("Conversation Run has its shared message reference");
            ThreadKey::Conversation {
                channel_id: delivery.channel_id,
                root_message_id: None,
            }
        } else {
            match Pool::of(run.trigger_kind) {
                Pool::Arrival => ThreadKey::Background(run.id.clone()),
                Pool::Conversation => ThreadKey::Conversation {
                    channel_id: run
                        .channel_id
                        .clone()
                        .expect("conversation Run has a Channel"),
                    root_message_id: run.root_message_id.clone(),
                },
            }
        };
        Self::new(run.id.clone(), thread, run.trigger_kind)
    }
}

/// What the caller does after a cancel decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelAction {
    /// Remove the Run from the RunQueue and mark it canceled.
    Queued,
    /// Signal cancellation. The Run holds its slot and Thread until `end`.
    Running,
    /// This Agent has no queued or running Run with this id.
    Unknown,
}

/// The admission state of one Agent, with the caps of its loop config.
#[derive(Debug, Clone)]
pub struct Admission {
    queued: VecDeque<AdmissionRun>,
    running: Vec<AdmissionRun>,
    conversation_cap: usize,
    arrival_cap: usize,
}

impl Admission {
    pub fn new(config: &AgentLoopConfig) -> Self {
        Self {
            queued: VecDeque::new(),
            running: Vec::new(),
            conversation_cap: config.max_concurrent_runs,
            arrival_cap: config.max_arrival_runs,
        }
    }

    fn cap(&self, pool: Pool) -> usize {
        match pool {
            Pool::Conversation => self.conversation_cap,
            Pool::Arrival => self.arrival_cap,
        }
    }

    fn running_in(&self, pool: Pool) -> usize {
        self.running.iter().filter(|run| run.pool == pool).count()
    }

    /// The next eligible Run. A message precedes a proactive Run.
    /// Runs of equal priority keep their queue order, across both pools.
    pub fn next(&self) -> Option<RunId> {
        self.queued
            .iter()
            .filter(|run| self.active(&run.thread).is_none())
            .filter(|run| self.running_in(run.pool) < self.cap(run.pool))
            .min_by_key(|run| !run.message)
            .map(|run| run.id.clone())
    }

    /// Slots that neither a running nor a queued Run holds.
    pub fn free_slots(&self, pool: Pool) -> usize {
        let queued = self.queued.iter().filter(|run| run.pool == pool).count();
        self.cap(pool)
            .saturating_sub(self.running_in(pool).saturating_add(queued))
    }

    /// Add a Run at the end of the queue and name the next Run to start.
    pub fn enqueue(&mut self, run: AdmissionRun) -> Option<RunId> {
        self.queued.push_back(run);
        self.next()
    }

    /// Move the next Run to running. `true` tells the caller to execute it.
    /// Another Run or an absent Run leaves the value unchanged.
    pub fn start(&mut self, run_id: &RunId) -> bool {
        if self.next().as_ref() != Some(run_id) {
            return false;
        }
        let position = self
            .queued
            .iter()
            .position(|run| &run.id == run_id)
            .expect("next Run is queued");
        self.running
            .push(self.queued.remove(position).expect("position valid"));
        true
    }

    /// Release a running Run's slot and Thread, and name the next Run.
    pub fn end(&mut self, run_id: &RunId) -> Option<RunId> {
        self.running.retain(|run| &run.id != run_id);
        self.next()
    }

    pub fn cancel(&mut self, run_id: &RunId) -> CancelAction {
        if self.running.iter().any(|run| &run.id == run_id) {
            return CancelAction::Running;
        }
        if let Some(position) = self.queued.iter().position(|run| &run.id == run_id) {
            self.queued.remove(position);
            return CancelAction::Queued;
        }
        CancelAction::Unknown
    }

    /// The Run that receives a message for this Thread.
    pub fn active(&self, thread: &ThreadKey) -> Option<RunId> {
        self.running
            .iter()
            .find(|run| &run.thread == thread)
            .map(|run| run.id.clone())
    }

    /// True when the Agent has no queued or running Run.
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty() && self.running.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Admission, AdmissionRun, CancelAction, Pool, ThreadKey};
    use crate::AgentLoopConfig;
    use pagis_core::{ChannelId, MessageId, RunId, TriggerKind};

    fn id(value: &str) -> RunId {
        RunId::from(value.to_string())
    }

    fn thread(channel: &str, root: Option<&str>) -> ThreadKey {
        ThreadKey::Conversation {
            channel_id: ChannelId::from(channel.to_string()),
            root_message_id: root.map(|value| MessageId::from(value.to_string())),
        }
    }

    fn run(value: &str, kind: TriggerKind, key: ThreadKey) -> AdmissionRun {
        AdmissionRun::new(id(value), key, kind)
    }

    fn message(value: &str, channel: &str, root: Option<&str>) -> AdmissionRun {
        run(value, TriggerKind::Message, thread(channel, root))
    }

    fn proactive(value: &str) -> AdmissionRun {
        run(value, TriggerKind::Schedule, thread(value, None))
    }

    fn arrival(value: &str) -> AdmissionRun {
        run(
            value,
            TriggerKind::Arrival,
            ThreadKey::Background(id(value)),
        )
    }

    fn admission(conversation: usize, arrival: usize) -> Admission {
        Admission::new(&AgentLoopConfig {
            max_concurrent_runs: conversation,
            max_arrival_runs: arrival,
            ..AgentLoopConfig::default()
        })
    }

    #[test]
    fn trigger_pool_table() {
        for (kind, expected) in [
            (TriggerKind::Message, Pool::Conversation),
            (TriggerKind::Schedule, Pool::Conversation),
            (TriggerKind::Event, Pool::Conversation),
            (TriggerKind::Arrival, Pool::Arrival),
            (TriggerKind::Review, Pool::Arrival),
        ] {
            assert_eq!(Pool::of(kind), expected, "{kind:?}");
        }
    }

    #[test]
    fn run_thread_table() {
        for (kind, channel, root, expected) in [
            (
                TriggerKind::Message,
                Some("channel"),
                None,
                thread("channel", None),
            ),
            (
                TriggerKind::Schedule,
                Some("channel"),
                Some("root"),
                thread("channel", Some("root")),
            ),
            (
                TriggerKind::Event,
                Some("channel"),
                None,
                thread("channel", None),
            ),
            (
                TriggerKind::Arrival,
                None,
                None,
                ThreadKey::Background(id("run")),
            ),
            (
                TriggerKind::Review,
                Some("channel"),
                Some("root"),
                ThreadKey::Background(id("run")),
            ),
        ] {
            let stored: pagis_core::Run = serde_json::from_value(serde_json::json!({
                "id": "run",
                "title": "Work",
                "workspace_id": "workspace",
                "agent_id": "agent",
                "channel_id": channel,
                "root_message_id": root,
                "trigger_kind": kind,
                "state": "queued",
                "hop_count": 0,
                "created_at": 0,
            }))
            .expect("Run");
            assert_eq!(
                AdmissionRun::from(&stored),
                run("run", kind, expected),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn next_run_table() {
        struct Case {
            name: &'static str,
            caps: (usize, usize),
            running: Vec<AdmissionRun>,
            queued: Vec<AdmissionRun>,
            next: Option<&'static str>,
        }
        let cases = [
            Case {
                name: "empty",
                caps: (3, 1),
                running: vec![],
                queued: vec![],
                next: None,
            },
            Case {
                name: "conversation disabled",
                caps: (0, 1),
                running: vec![],
                queued: vec![proactive("a")],
                next: None,
            },
            Case {
                name: "arrival disabled",
                caps: (3, 0),
                running: vec![],
                queued: vec![arrival("a")],
                next: None,
            },
            Case {
                name: "conversation full",
                caps: (1, 1),
                running: vec![proactive("a")],
                queued: vec![proactive("b")],
                next: None,
            },
            Case {
                name: "arrival full",
                caps: (3, 1),
                running: vec![arrival("a")],
                queued: vec![arrival("b")],
                next: None,
            },
            Case {
                name: "arrival does not block conversation",
                caps: (1, 1),
                running: vec![arrival("a")],
                queued: vec![message("b", "b", None)],
                next: Some("b"),
            },
            Case {
                name: "conversation does not block arrival",
                caps: (1, 1),
                running: vec![proactive("a")],
                queued: vec![arrival("b")],
                next: Some("b"),
            },
            Case {
                name: "same Thread is busy",
                caps: (3, 1),
                running: vec![message("a", "channel", Some("root"))],
                queued: vec![message("b", "channel", Some("root"))],
                next: None,
            },
            Case {
                name: "top level is busy",
                caps: (3, 1),
                running: vec![message("a", "channel", None)],
                queued: vec![message("b", "channel", None)],
                next: None,
            },
            Case {
                name: "different roots",
                caps: (3, 1),
                running: vec![message("a", "channel", Some("root"))],
                queued: vec![message("b", "channel", Some("other"))],
                next: Some("b"),
            },
            Case {
                name: "top level and Thread are separate",
                caps: (3, 1),
                running: vec![message("a", "channel", None)],
                queued: vec![message("b", "channel", Some("root"))],
                next: Some("b"),
            },
            Case {
                name: "different Channels",
                caps: (3, 1),
                running: vec![message("a", "channel", Some("root"))],
                queued: vec![message("b", "other", Some("root"))],
                next: Some("b"),
            },
            Case {
                name: "message before Schedule",
                caps: (3, 1),
                running: vec![],
                queued: vec![proactive("a"), message("b", "b", None)],
                next: Some("b"),
            },
            Case {
                name: "message before Event",
                caps: (3, 1),
                running: vec![],
                queued: vec![
                    run("a", TriggerKind::Event, thread("a", None)),
                    message("b", "b", None),
                ],
                next: Some("b"),
            },
            Case {
                name: "message before arrival",
                caps: (3, 1),
                running: vec![],
                queued: vec![arrival("a"), message("b", "b", None)],
                next: Some("b"),
            },
            Case {
                name: "message order",
                caps: (3, 1),
                running: vec![],
                queued: vec![message("a", "a", None), message("b", "b", None)],
                next: Some("a"),
            },
            Case {
                name: "proactive order across pools",
                caps: (3, 1),
                running: vec![],
                queued: vec![arrival("a"), proactive("b")],
                next: Some("a"),
            },
            Case {
                name: "skip busy message",
                caps: (3, 1),
                running: vec![message("a", "busy", None)],
                queued: vec![message("b", "busy", None), proactive("c")],
                next: Some("c"),
            },
            Case {
                name: "queued Runs do not block start",
                caps: (1, 1),
                running: vec![],
                queued: vec![proactive("a"), proactive("b")],
                next: Some("a"),
            },
        ];
        for case in cases {
            let mut state = admission(case.caps.0, case.caps.1);
            for running in case.running {
                let run_id = running.id.clone();
                state.enqueue(running);
                assert!(state.start(&run_id), "{}", case.name);
            }
            for queued in case.queued {
                state.enqueue(queued);
            }
            let expected = case.next.map(id);
            assert_eq!(state.next(), expected, "{}", case.name);
            assert_eq!(state.next(), expected, "next does not change {}", case.name);
        }
    }

    #[test]
    fn free_slots_table() {
        for (name, running, queued, expected) in [
            ("empty", vec![], vec![], (3, 1)),
            ("running conversation", vec![proactive("a")], vec![], (2, 1)),
            ("running arrival", vec![arrival("a")], vec![], (3, 0)),
            ("queued conversation", vec![], vec![proactive("a")], (2, 1)),
            ("queued arrival", vec![], vec![arrival("a")], (3, 0)),
            (
                "running and queued",
                vec![proactive("a"), arrival("b")],
                vec![proactive("c")],
                (1, 0),
            ),
            (
                "busy queued Thread holds a slot",
                vec![message("a", "channel", None)],
                vec![message("b", "channel", None)],
                (1, 1),
            ),
            (
                "saturate both pools",
                vec![proactive("a"), arrival("b")],
                vec![proactive("c"), proactive("d"), proactive("e"), arrival("f")],
                (0, 0),
            ),
        ] {
            let mut state = admission(3, 1);
            for entry in running {
                let run_id = entry.id.clone();
                state.enqueue(entry);
                assert!(state.start(&run_id), "{name}");
            }
            for entry in queued {
                state.enqueue(entry);
            }
            assert_eq!(
                (
                    state.free_slots(Pool::Conversation),
                    state.free_slots(Pool::Arrival)
                ),
                expected,
                "{name}"
            );
        }
        let state = admission(0, 0);
        assert_eq!(state.free_slots(Pool::Conversation), 0);
        assert_eq!(state.free_slots(Pool::Arrival), 0);
    }

    #[test]
    fn start_and_end_table() {
        for kind in [
            TriggerKind::Message,
            TriggerKind::Schedule,
            TriggerKind::Event,
            TriggerKind::Arrival,
            TriggerKind::Review,
        ] {
            let mut state = admission(1, 1);
            let pool = Pool::of(kind);
            let key = thread("channel", None);
            assert!(state.is_empty());
            assert!(!state.start(&id("absent")));
            assert_eq!(state.enqueue(run("a", kind, key.clone())), Some(id("a")));
            assert_eq!(state.enqueue(run("b", kind, key.clone())), Some(id("a")));
            assert!(!state.start(&id("b")), "only next starts");
            assert!(state.start(&id("a")));
            assert!(!state.start(&id("a")), "a Run starts once");
            assert_eq!(state.active(&key), Some(id("a")));
            assert_eq!(state.next(), None);
            assert_eq!(state.free_slots(pool), 0);
            assert_eq!(state.end(&id("absent")), None);
            assert_eq!(
                state.end(&id("b")),
                None,
                "end does not remove a queued Run"
            );
            assert_eq!(state.end(&id("a")), Some(id("b")));
            assert_eq!(state.active(&key), None);
            assert!(state.start(&id("b")));
            assert_eq!(state.free_slots(pool), 0);
            assert_eq!(state.end(&id("b")), None);
            assert_eq!(state.free_slots(pool), 1);
            assert!(state.is_empty());
        }
    }

    #[test]
    fn cancel_table() {
        for (running, expected) in [(false, CancelAction::Queued), (true, CancelAction::Running)] {
            for kind in [
                TriggerKind::Message,
                TriggerKind::Arrival,
                TriggerKind::Review,
            ] {
                let mut state = admission(1, 1);
                let key = thread("channel", None);
                state.enqueue(run("a", kind, key.clone()));
                if running {
                    assert!(state.start(&id("a")));
                }
                state.enqueue(run("b", kind, key.clone()));
                assert_eq!(state.cancel(&id("absent")), CancelAction::Unknown);
                assert_eq!(state.cancel(&id("a")), expected);
                if running {
                    assert_eq!(state.cancel(&id("a")), CancelAction::Running);
                    assert_eq!(state.active(&key), Some(id("a")));
                    assert_eq!(
                        state.next(),
                        None,
                        "cancel keeps the running slot and Thread"
                    );
                    assert_eq!(state.end(&id("a")), Some(id("b")));
                } else {
                    assert_eq!(state.cancel(&id("a")), CancelAction::Unknown);
                    assert_eq!(state.next(), Some(id("b")));
                }
                assert_eq!(state.free_slots(Pool::of(kind)), 0);
                assert_eq!(state.cancel(&id("b")), CancelAction::Queued);
                assert_eq!(state.free_slots(Pool::of(kind)), 1);
                assert!(state.is_empty());
            }
        }
    }

    #[test]
    fn active_thread_table() {
        let mut state = admission(3, 2);
        for entry in [
            message("a", "channel", None),
            message("b", "channel", Some("root")),
            arrival("c"),
        ] {
            let run_id = entry.id.clone();
            state.enqueue(entry);
            assert!(state.start(&run_id));
        }
        for (key, expected) in [
            (thread("channel", None), Some(id("a"))),
            (thread("channel", Some("root")), Some(id("b"))),
            (thread("channel", Some("other")), None),
            (thread("other", None), None),
            (ThreadKey::Background(id("c")), Some(id("c"))),
        ] {
            assert_eq!(state.active(&key), expected, "{key:?}");
        }
        state.enqueue(message("queued", "other", None));
        assert_eq!(state.active(&thread("other", None)), None);
    }
}
