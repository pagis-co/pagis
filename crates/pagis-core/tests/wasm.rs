#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use pagis_core::{MessageId, now_ms};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn now_ms_reads_the_javascript_clock() {
    let javascript_now = js_sys::Date::now() as i64;
    let now = now_ms();
    assert!(now.abs_diff(javascript_now) < 1_000);
}

#[wasm_bindgen_test]
fn record_ids_are_unique_and_sorted() {
    let ids: Vec<String> = (0..1_000)
        .map(|_| MessageId::generate().to_string())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));

    let javascript_now = js_sys::Date::now() as u64;
    for id in ids {
        let timestamp = id.parse::<ulid::Ulid>().unwrap().timestamp_ms();
        assert!(timestamp.abs_diff(javascript_now) < 1_000);
    }
}

#[wasm_bindgen_test]
fn computer_values_and_an_opaque_lease_need_no_host_runtime() {
    use pagis_core::ComputerLease;
    use pagis_core::computer_action::{Action, DisplayFrame, InputOp};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Held(Arc<AtomicBool>);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    let held = Arc::new(AtomicBool::new(true));
    let lease: ComputerLease = Box::new(Held(held.clone()));
    assert!(held.load(Ordering::SeqCst));
    drop(lease);
    assert!(!held.load(Ordering::SeqCst));

    let input = InputOp::Move { x: 20.0, y: 30.0 };
    assert_eq!(
        serde_json::to_value(input).unwrap(),
        serde_json::json!({"op": "move", "x": 20.0, "y": 30.0})
    );
    let action = Action::Move { x: 20.0, y: 30.0 };
    assert_eq!(action.clone(), action);
    let frame = DisplayFrame {
        png: vec![1, 2],
        scale_x: 2.0,
        scale_y: 1.5,
    };
    assert_eq!(frame.png, vec![1, 2]);
    assert_eq!((frame.scale_x, frame.scale_y), (2.0, 1.5));
}
