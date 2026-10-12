#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

// The native unit tests also run in Node through this test binary.
pub use pagis_memory::rules::*;
#[path = "../src/rules/tests.rs"]
mod tests;
