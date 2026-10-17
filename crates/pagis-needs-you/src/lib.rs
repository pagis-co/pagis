//! The Needs-You Queue and the Notification rules of one Workspace.
//!
//! An Embedder runs [`NotificationRule`] with its own time and timer.
//! At the end of a hold it reads [`Notifications::queued`] and gives
//! the result to [`NotificationRule::on_hold_end`]. For [`Action::Send`]
//! it calls [`Notifications::notify`]. [`run_notifications`] uses the
//! same methods with a timer in memory.

mod loops;
mod needs_you;
mod notification_rule;
mod notifications;
mod person_activity;
mod queue_changes;
mod sender;
mod vapid;

pub use loops::{run_needs_you, run_notifications};
pub use needs_you::{
    NEEDS_YOU_ADDED, NEEDS_YOU_REMOVED, NeedsYou, NeedsYouApproval, NeedsYouAssignment,
    NeedsYouCall, NeedsYouFailed, NeedsYouItem, NeedsYouKeypad, NeedsYouMail, NeedsYouQueue,
    NeedsYouWaiting,
};
pub use notification_rule::{Action, HeldItem, NotificationRule};
pub use notifications::{Notifications, QueuedItem, options, payload, test_options, test_payload};
pub use person_activity::{HOLD, PersonActivity, PersonActivityReader, hold_until};
pub use queue_changes::{QueueChanges, QueueReader};
pub use sender::NotificationSender;
pub use vapid::{VAPID_KEY_SECRET, vapid_key};
