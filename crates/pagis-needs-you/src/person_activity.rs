//! The activity of the Person in a client (ADR-0030): the time of the
//! last `activity` frame of each Workspace.
//!
//! A visible client sends an `activity` frame on the event socket after
//! input. A new Needs-You item waits while the Person used a client in
//! the last [`HOLD`], so the phone does not ring while the Person reads
//! Pagis at the desk.
//!
//! The time is in the daemon's memory and is not stored, as the Presence
//! of a Host is not (ADR-0015). A restart of the daemon forgets it, so an
//! item after a restart goes at once.

use std::collections::HashMap;
use std::sync::Mutex;

use pagis_core::{UnixMillis, WorkspaceId};

/// How long a new item waits after the last activity of the Person.
pub const HOLD: UnixMillis = 120_000;

/// Until when a new item waits at `now`, after the last activity at
/// `last_active_at`: `last_active_at + HOLD` while less than [`HOLD`]
/// passed, else `None`, and the item goes at once.
pub fn hold_until(last_active_at: UnixMillis, now: UnixMillis) -> Option<UnixMillis> {
    let until = last_active_at + HOLD;
    (now < until).then_some(until)
}

/// The time of the last activity of each Workspace, in memory.
#[derive(Default)]
pub struct PersonActivity {
    last: Mutex<HashMap<WorkspaceId, UnixMillis>>,
}

impl PersonActivity {
    pub fn new() -> Self {
        Self::default()
    }

    /// The Person used a client of `workspace_id` at `at`. A time before
    /// the one that is kept changes nothing.
    pub fn record(&self, workspace_id: &WorkspaceId, at: UnixMillis) {
        let mut last = self.last.lock().expect("person activity lock");
        let kept = last.entry(workspace_id.clone()).or_insert(at);
        *kept = (*kept).max(at);
    }

    /// Until when a new item of `workspace_id` waits at `now`, by the
    /// rule of [`hold_until`]. A Workspace with no activity holds
    /// nothing.
    pub fn hold_until(&self, workspace_id: &WorkspaceId, now: UnixMillis) -> Option<UnixMillis> {
        let last_active_at = *self
            .last
            .lock()
            .expect("person activity lock")
            .get(workspace_id)?;
        hold_until(last_active_at, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-25 12:00 UTC.
    const NOW: UnixMillis = 1_790_337_600_000;
    const SECOND: UnixMillis = 1_000;

    fn workspace(id: &str) -> WorkspaceId {
        WorkspaceId::from(id.to_string())
    }

    #[test]
    fn an_item_waits_until_120_s_after_the_last_activity() {
        assert_eq!(hold_until(NOW, NOW), Some(NOW + HOLD));
        assert_eq!(hold_until(NOW, NOW + 60 * SECOND), Some(NOW + HOLD));
        assert_eq!(hold_until(NOW, NOW + HOLD - 1), Some(NOW + HOLD));
    }

    #[test]
    fn an_item_goes_at_once_120_s_after_the_last_activity() {
        assert_eq!(hold_until(NOW, NOW + HOLD), None);
        assert_eq!(hold_until(NOW, NOW + 3_600 * SECOND), None);
    }

    #[test]
    fn a_workspace_with_no_activity_holds_nothing() {
        let activity = PersonActivity::new();

        assert_eq!(activity.hold_until(&workspace("w-1"), NOW), None);
    }

    #[test]
    fn the_activity_of_one_workspace_holds_nothing_of_another() {
        let activity = PersonActivity::new();

        activity.record(&workspace("w-1"), NOW);

        assert_eq!(
            activity.hold_until(&workspace("w-1"), NOW + SECOND),
            Some(NOW + HOLD)
        );
        assert_eq!(activity.hold_until(&workspace("w-2"), NOW + SECOND), None);
    }

    #[test]
    fn a_later_activity_moves_the_hold_and_an_earlier_one_does_not() {
        let activity = PersonActivity::new();
        let w = workspace("w-1");

        activity.record(&w, NOW);
        activity.record(&w, NOW + 30 * SECOND);
        activity.record(&w, NOW + 10 * SECOND);

        assert_eq!(
            activity.hold_until(&w, NOW + HOLD),
            Some(NOW + 30 * SECOND + HOLD)
        );
    }
}
