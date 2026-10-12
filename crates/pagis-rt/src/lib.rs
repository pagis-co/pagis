//! Tasks and timers for the host.

use std::future::Future;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub use tokio::{
    spawn,
    task::spawn_blocking,
    time::{Instant, Interval, error::Elapsed, sleep, sleep_until},
};

/// Run blocking work on the host's one thread and return its value.
/// The call completes before this function returns. It needs no
/// `Send` or `'static` bound because no value leaves this thread.
/// On native targets, Tokio runs the call on its blocking pool.
#[cfg(target_arch = "wasm32")]
pub fn spawn_blocking<F: FnOnce() -> T, T>(
    f: F,
) -> std::future::Ready<Result<T, std::convert::Infallible>> {
    std::future::ready(Ok(f()))
}

/// Tick at once, then once per period. A missed tick delays the next
/// tick by one period, so a task does not send a burst to catch up.
#[cfg(not(target_arch = "wasm32"))]
pub fn interval(period: Duration) -> Interval {
    let mut ticks = tokio::time::interval(period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticks
}

#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_futures::spawn_local as spawn;
#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

#[cfg(target_arch = "wasm32")]
#[derive(Debug, thiserror::Error)]
#[error("deadline has elapsed")]
pub struct Elapsed;

/// Limit a future to the given duration. The native deadline starts
/// when this function is called, as Tokio's deadline does.
#[cfg(not(target_arch = "wasm32"))]
pub fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> impl Future<Output = Result<F::Output, Elapsed>> {
    tokio::time::timeout(duration, future)
}

/// Wait for the given duration on the host's one thread.
#[cfg(target_arch = "wasm32")]
pub fn sleep(duration: Duration) -> impl Future<Output = ()> + Send {
    sleep_until(Instant::now() + duration)
}

/// Wait until the host clock reaches the deadline.
#[cfg(target_arch = "wasm32")]
pub async fn sleep_until(deadline: Instant) {
    // A host timer uses whole milliseconds and can fire before the
    // clock reaches the deadline. Wait again for what remains.
    while let Some(duration) = deadline.checked_duration_since(Instant::now()) {
        if duration.is_zero() {
            return;
        }
        // The timer stays on the isolate's one thread. Domain futures
        // keep their Send contract.
        send_wrapper::SendWrapper::new(gloo_timers::future::sleep(duration)).await;
    }
}

/// A timer that delays missed ticks. Cancelling a tick keeps its place.
#[cfg(target_arch = "wasm32")]
pub struct Interval {
    next: Instant,
    period: Duration,
}

/// Tick at once, then once per period. A missed tick delays the next
/// tick by one period, so a task does not send a burst to catch up.
#[cfg(target_arch = "wasm32")]
pub fn interval(period: Duration) -> Interval {
    assert!(!period.is_zero(), "the interval period must be positive");
    Interval {
        next: Instant::now(),
        period,
    }
}

#[cfg(target_arch = "wasm32")]
impl Interval {
    pub async fn tick(&mut self) -> Instant {
        sleep_until(self.next).await;
        let deadline = self.next;
        let now = Instant::now();
        // Match the native timer's tolerance for timer precision.
        self.next = if now > deadline + Duration::from_millis(5) {
            now + self.period
        } else {
            deadline + self.period
        };
        deadline
    }
}

/// Limit a future to the given duration on the host's one thread.
#[cfg(target_arch = "wasm32")]
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
    let timer = sleep(duration);
    futures::pin_mut!(future, timer);
    match futures::future::select(future, timer).await {
        futures::future::Either::Left((value, _)) => Ok(value),
        futures::future::Either::Right(_) => Err(Elapsed),
    }
}
