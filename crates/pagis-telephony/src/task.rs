//! Tasks that can be cancelled and joined on either runtime.

#[cfg(not(target_arch = "wasm32"))]
pub use tokio::{spawn, task::JoinHandle};

#[cfg(target_arch = "wasm32")]
pub use host::{JoinHandle, spawn};

#[cfg(target_arch = "wasm32")]
mod host {
    use futures::future::{AbortHandle, Abortable};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::sync::oneshot;

    pub struct JoinHandle<T> {
        abort: AbortHandle,
        done: oneshot::Receiver<T>,
    }

    impl<T> JoinHandle<T> {
        pub fn abort(&self) {
            self.abort.abort();
        }
    }

    impl<T> Future for JoinHandle<T> {
        type Output = Result<T, oneshot::error::RecvError>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.done).poll(cx)
        }
    }

    pub fn spawn<F: Future + 'static>(future: F) -> JoinHandle<F::Output> {
        let (abort, registration) = AbortHandle::new_pair();
        let (done, wait) = oneshot::channel();
        pagis_rt::spawn(async move {
            if let Ok(value) = Abortable::new(future, registration).await {
                let _ = done.send(value);
            }
        });
        JoinHandle { abort, done: wait }
    }
}
