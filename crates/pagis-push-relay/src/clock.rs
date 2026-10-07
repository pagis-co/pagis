//! The time that the relay reads. A test sets it, so a check of the
//! VAPID token and the count of a UTC day run at a known time.

use std::time::SystemTime;

/// Where the relay reads its now.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The machine's wall clock, what the binary runs on.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
