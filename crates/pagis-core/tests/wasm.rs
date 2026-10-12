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
