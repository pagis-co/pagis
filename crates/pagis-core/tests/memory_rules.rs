#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

// The native unit tests also run in Node through this test binary.
pub use pagis_core::memory::rules::*;
pub use pagis_core::*;
#[path = "../src/memory/rules/tests.rs"]
mod tests;
