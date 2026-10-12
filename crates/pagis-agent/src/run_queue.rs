//! The queued Run records of one Agent.

use std::collections::VecDeque;

use pagis_core::{Run, RunId};

/// A host keeps queued Run records and returns them in enqueue order.
/// Admission selects the id to remove; the queue makes no admission decision.
pub trait RunQueue {
    fn enqueue(&mut self, run: Run);
    fn queued(&self) -> impl Iterator<Item = &Run>;
    fn remove(&mut self, run_id: &RunId) -> Option<Run>;
}

/// The daemon keeps the Run records in its resident actor's memory.
impl RunQueue for VecDeque<Run> {
    fn enqueue(&mut self, run: Run) {
        self.push_back(run);
    }

    fn queued(&self) -> impl Iterator<Item = &Run> {
        self.iter()
    }

    fn remove(&mut self, run_id: &RunId) -> Option<Run> {
        let position = self.iter().position(|run| &run.id == run_id)?;
        VecDeque::remove(self, position)
    }
}

#[cfg(test)]
mod tests {
    use super::RunQueue;
    use pagis_core::{Run, RunId};
    use std::collections::VecDeque;

    fn run(id: &str) -> Run {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": "Work",
            "workspace_id": "workspace",
            "agent_id": "agent",
            "trigger_kind": "message",
            "state": "queued",
            "hop_count": 0,
            "created_at": 0,
        }))
        .expect("Run")
    }

    #[test]
    fn memory_queue_keeps_order_and_removes_the_named_run() {
        let mut queue = VecDeque::new();
        for id in ["a", "b", "c"] {
            queue.enqueue(run(id));
        }
        assert_eq!(
            queue
                .queued()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(
            RunQueue::remove(&mut queue, &RunId::from("absent".to_string())),
            None
        );
        assert_eq!(
            RunQueue::remove(&mut queue, &RunId::from("b".to_string())),
            Some(run("b"))
        );
        assert_eq!(
            queue
                .queued()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(
            RunQueue::remove(&mut queue, &RunId::from("a".to_string())),
            Some(run("a"))
        );
        assert_eq!(
            RunQueue::remove(&mut queue, &RunId::from("c".to_string())),
            Some(run("c"))
        );
        assert_eq!(queue.queued().count(), 0);
    }
}
