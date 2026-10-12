//! Tasks, timers and response bodies for the host.

pub(crate) use pagis_rt::{Elapsed, sleep, spawn, timeout};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn body<S>(stream: S) -> S {
    stream
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn body<S>(stream: S) -> send_wrapper::SendWrapper<S> {
    // A response body stays on the isolate's one thread.
    send_wrapper::SendWrapper::new(stream)
}
