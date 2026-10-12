//! Tasks, timers and response bodies for the host.

use std::future::Future;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use tokio::{spawn, time::error::Elapsed, time::sleep};

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_bindgen_futures::spawn_local as spawn;

#[cfg(target_arch = "wasm32")]
#[derive(Debug, thiserror::Error)]
#[error("deadline has elapsed")]
pub(crate) struct Elapsed;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> impl Future<Output = Result<F::Output, Elapsed>> {
    tokio::time::timeout(duration, future)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sleep(duration: Duration) -> impl Future<Output = ()> + Send {
    // The isolate has one thread. Stream adapters keep their Send contract.
    send_wrapper::SendWrapper::new(gloo_timers::future::sleep(duration))
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn timeout<F: Future>(
    duration: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    let timer = sleep(duration);
    futures::pin_mut!(future, timer);
    match futures::future::select(future, timer).await {
        futures::future::Either::Left((value, _)) => Ok(value),
        futures::future::Either::Right(_) => Err(Elapsed),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn body<S>(stream: S) -> S {
    stream
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn body<S>(stream: S) -> send_wrapper::SendWrapper<S> {
    // A response body stays on the isolate's one thread.
    send_wrapper::SendWrapper::new(stream)
}
