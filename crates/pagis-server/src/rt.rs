//! Local provider futures at the axum boundary.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn on_thread<F: std::future::Future>(future: F) -> F {
    future
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn on_thread<F: std::future::Future>(future: F) -> send_wrapper::SendWrapper<F> {
    // Axum requires `Send` futures. The WebAssembly host polls and drops
    // each future on its one thread; the wrapper checks that rule.
    send_wrapper::SendWrapper::new(future)
}
