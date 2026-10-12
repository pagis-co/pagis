//! Tasks and timers for the host.

use std::future::Future;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub use tokio::{spawn, time::Instant, time::error::Elapsed, time::sleep};

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
    // The future stays on the isolate's one thread. Domain futures
    // keep their Send contract.
    send_wrapper::SendWrapper::new(gloo_timers::future::sleep(duration))
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
