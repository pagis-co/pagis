//! The failed-attempt count of one Workspace's Keypad Code (ADR-0021).
//!
//! The code belongs to the Workspace, so its guessing limit does too:
//! one count holds the wrong codes that callers entered on every Call
//! and on every Agent Phone Number of the Workspace. After
//! [`FAILURES_BEFORE_DELAY`] failures, keypad elevation is suspended
//! for a delay. Each further failure doubles the delay, up to
//! [`LONGEST_DELAY`]. A delay suspends elevation only: Pagis still
//! answers the Call, at Unknown.
//!
//! A correct code outside a delay clears the count, and so does the
//! Person in Settings.

use std::time::Duration;

use async_trait::async_trait;

use crate::id::WorkspaceId;
use crate::store::StoreError;
use crate::time::UnixMillis;

/// How many failed attempts a Workspace has before the first delay.
pub const FAILURES_BEFORE_DELAY: u32 = 6;
/// The delay that the failure at [`FAILURES_BEFORE_DELAY`] starts.
pub const FIRST_DELAY: Duration = Duration::from_secs(60);
/// The longest delay. Further failures do not make it longer.
pub const LONGEST_DELAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The wrong codes of one Workspace, and the end of its delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeypadFailures {
    /// The wrong codes since a correct code or the Person last cleared
    /// the count.
    pub failed_attempts: u32,
    /// The end of the latest delay. `None` until the first delay
    /// starts. It stays when the delay ends, so the Person still sees
    /// that one ran.
    pub suspended_until: Option<UnixMillis>,
}

impl KeypadFailures {
    /// True while a delay runs: no prompt plays and no code is checked.
    pub fn suspended_at(&self, now: UnixMillis) -> bool {
        self.suspended_until.is_some_and(|until| now < until)
    }

    /// The count after one more wrong code at `now`.
    pub fn after_failure(self, now: UnixMillis) -> Self {
        let failed_attempts = self.failed_attempts.saturating_add(1);
        let suspended_until = match delay_after(failed_attempts) {
            Some(delay) => Some(now.saturating_add(delay.as_millis() as UnixMillis)),
            None => self.suspended_until,
        };
        Self {
            failed_attempts,
            suspended_until,
        }
    }
}

/// How long keypad elevation is suspended after this many failed
/// attempts. `None` below [`FAILURES_BEFORE_DELAY`].
pub fn delay_after(failed_attempts: u32) -> Option<Duration> {
    let doublings = failed_attempts.checked_sub(FAILURES_BEFORE_DELAY)?;
    // The delay passes the cap after eleven doublings, so the shift
    // stops at 31 and never overflows.
    let delay = FIRST_DELAY.saturating_mul(1 << doublings.min(31));
    Some(delay.min(LONGEST_DELAY))
}

/// The failed-attempt count of each Workspace.
///
/// No tool reaches it. The tier gate of an inbound Call reads and
/// counts through it, and the Person clears it in Settings.
#[async_trait]
pub trait KeypadFailureStore: Send + Sync {
    /// The count of one Workspace. A Workspace with no failure reads as
    /// the default: no failure and no delay.
    async fn get(&self, workspace_id: &WorkspaceId) -> Result<KeypadFailures, StoreError>;
    /// Count one wrong code at `now` and answer the new count. The read
    /// and the write are one step, so two failures at the same moment,
    /// on two lines of the Workspace, both count.
    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<KeypadFailures, StoreError>;
    /// Clear the count of one Workspace, and end its delay.
    async fn clear(&self, workspace_id: &WorkspaceId) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60 * 1_000;
    const NOW: UnixMillis = 1_790_305_200_000;

    /// The count after `failures` wrong codes, one minute apart.
    fn after(failures: u32) -> KeypadFailures {
        (0..failures).fold(KeypadFailures::default(), |count, index| {
            count.after_failure(NOW + i64::from(index) * MINUTE)
        })
    }

    #[test]
    fn five_failures_start_no_delay() {
        let failures = after(FAILURES_BEFORE_DELAY - 1);

        assert_eq!(failures.failed_attempts, 5);
        assert_eq!(failures.suspended_until, None);
        assert!(!failures.suspended_at(NOW + 5 * MINUTE));
    }

    #[test]
    fn the_sixth_failure_starts_a_delay_of_one_minute() {
        let at = NOW + 5 * MINUTE;
        let failures = after(FAILURES_BEFORE_DELAY);

        assert_eq!(failures.failed_attempts, 6);
        assert_eq!(failures.suspended_until, Some(at + MINUTE));
        assert!(failures.suspended_at(at));
        assert!(failures.suspended_at(at + MINUTE - 1));
        assert!(!failures.suspended_at(at + MINUTE), "the delay ended");
    }

    #[test]
    fn each_further_failure_doubles_the_delay_up_to_a_day() {
        assert_eq!(delay_after(5), None);
        assert_eq!(delay_after(6), Some(Duration::from_secs(60)));
        assert_eq!(delay_after(7), Some(Duration::from_secs(2 * 60)));
        assert_eq!(delay_after(8), Some(Duration::from_secs(4 * 60)));
        assert_eq!(delay_after(16), Some(Duration::from_secs(1_024 * 60)));
        assert_eq!(
            delay_after(17),
            Some(LONGEST_DELAY),
            "2048 minutes is over the cap"
        );
        assert_eq!(delay_after(u32::MAX), Some(LONGEST_DELAY));

        let seventh = after(6).after_failure(NOW + 10 * MINUTE);
        assert_eq!(seventh.suspended_until, Some(NOW + 12 * MINUTE));
    }

    #[test]
    fn a_count_with_no_delay_is_not_suspended() {
        assert!(!KeypadFailures::default().suspended_at(NOW));
    }
}
