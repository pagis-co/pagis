//! The runtime contract on native targets and under Node.

use std::time::Duration;

use futures::future;
use pagis_rt::{Instant, sleep, spawn, timeout};

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn timeout_returns_ready_value() {
    assert_eq!(
        timeout(Duration::ZERO, future::ready(42)).await.unwrap(),
        42
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn timeout_ends_pending_future_and_drops_it() {
    struct Pending(tokio::sync::oneshot::Sender<()>);
    impl Drop for Pending {
        fn drop(&mut self) {
            assert!(!self.0.is_closed());
        }
    }
    let (sender, mut receiver) = tokio::sync::oneshot::channel();
    let pending = Pending(sender);
    let duration = Duration::from_millis(25);
    let started = Instant::now();
    let error = timeout(duration, async move {
        let _held = pending;
        future::pending::<()>().await;
    })
    .await
    .expect_err("the future times out");
    assert!(started.elapsed() >= duration);
    assert_eq!(error.to_string(), "deadline has elapsed");
    assert_eq!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn task_sleeps_and_runs_after_caller_continues() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let duration = Duration::from_millis(25);
    let started = Instant::now();
    spawn(async move {
        sleep(duration).await;
        sender.send(42).unwrap();
    });
    assert_eq!(
        timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap(),
        42
    );
    assert!(started.elapsed() >= duration);
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(start_paused = true)]
async fn native_timeout_uses_the_tokio_clock_from_creation() {
    let timeout = timeout(Duration::from_secs(1), future::pending::<()>());
    tokio::time::advance(Duration::from_secs(2)).await;
    let started = Instant::now();
    timeout.await.expect_err("the deadline has passed");
    assert_eq!(Instant::now(), started);
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn wasm_task_accepts_local_values() {
    let local = std::rc::Rc::new(42);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn(async move {
        sender.send(*local).unwrap();
    });
    assert_eq!(
        timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap(),
        42
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn sleep_until_waits_for_the_deadline_and_accepts_a_past_deadline() {
    let deadline = Instant::now() + Duration::from_micros(25_900);
    pagis_rt::sleep_until(deadline).await;
    assert!(Instant::now() >= deadline);
    timeout(Duration::from_millis(100), pagis_rt::sleep_until(deadline))
        .await
        .unwrap();
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn interval_ticks_at_once_then_waits_and_does_not_burst_after_a_delay() {
    let period = Duration::from_millis(25);
    let mut ticks = pagis_rt::interval(period);
    timeout(Duration::from_millis(100), ticks.tick())
        .await
        .unwrap();
    let started = Instant::now();
    ticks.tick().await;
    assert!(started.elapsed() >= Duration::from_millis(20));
    sleep(period * 4).await;
    ticks.tick().await;
    let resumed = Instant::now();
    ticks.tick().await;
    assert!(resumed.elapsed() >= Duration::from_millis(20));
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test(start_paused = true)]
async fn native_interval_uses_the_paused_clock_and_a_cancelled_tick_keeps_its_place() {
    let period = Duration::from_millis(20);
    let started = Instant::now();
    let mut ticks = pagis_rt::interval(period);
    assert_eq!(ticks.tick().await, started);
    assert!(
        timeout(Duration::from_millis(5), ticks.tick())
            .await
            .is_err()
    );
    assert_eq!(ticks.tick().await, started + period);
    tokio::time::advance(period * 5).await;
    ticks.tick().await;
    let resumed = Instant::now();
    assert_eq!(ticks.tick().await, resumed + period);
}
