//! The host runtime contract, on native targets and under Node.

#[path = "../src/rt.rs"]
mod rt;

use std::time::Duration;

use futures::{StreamExt, future, stream};
use web_time::Instant;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn timeout_returns_elapsed_after_deadline() {
    let deadline = Duration::from_millis(25);
    let started = Instant::now();
    let error: pagis_rt::Elapsed = rt::timeout(deadline, future::pending::<()>())
        .await
        .expect_err("a pending future must time out");
    assert!(started.elapsed() >= deadline);
    assert_eq!(error.to_string(), "deadline has elapsed");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn timeout_returns_ready_value() {
    assert_eq!(
        rt::timeout(Duration::ZERO, future::ready(42))
            .await
            .unwrap(),
        42
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(start_paused = true)]
async fn timeout_starts_deadline_when_created() {
    let timeout = rt::timeout(Duration::from_secs(1), future::pending::<()>());
    tokio::time::advance(Duration::from_secs(2)).await;
    let started = tokio::time::Instant::now();
    timeout.await.expect_err("the deadline has passed");
    assert_eq!(tokio::time::Instant::now(), started);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn sleep_waits_for_duration() {
    let duration = Duration::from_millis(25);
    let started = Instant::now();
    rt::sleep(duration).await;
    assert!(started.elapsed() >= duration);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn spawn_runs_task() {
    let (sent, received) = tokio::sync::oneshot::channel();
    rt::spawn(async move {
        rt::sleep(Duration::from_millis(5)).await;
        sent.send(42).unwrap();
    });
    assert_eq!(
        rt::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap(),
        42
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn body_keeps_stream_items_and_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let body = rt::body(stream::iter([
        Ok::<_, &'static str>(1),
        Err("body error"),
        Ok(2),
    ]));
    assert_send(&body);
    assert_eq!(
        body.collect::<Vec<_>>().await,
        [Ok(1), Err("body error"), Ok(2)]
    );
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn body_and_spawn_accept_local_values() {
    use std::rc::Rc;

    let value = Rc::new(42);
    let local = value.clone();
    let body = rt::body(stream::once(async move { *local })).boxed();
    let (sent, received) = tokio::sync::oneshot::channel();
    rt::spawn(async move {
        sent.send((body.collect::<Vec<_>>().await, *value)).unwrap();
    });
    assert_eq!(
        rt::timeout(Duration::from_secs(1), received)
            .await
            .unwrap()
            .unwrap(),
        (vec![42], 42)
    );
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
fn router_builds_on_host() {
    llm_router::Router::new(llm_router::RouterConfig::new()).unwrap();
}
