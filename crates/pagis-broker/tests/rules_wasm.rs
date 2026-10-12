#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

// Use the production parser and its tests without the native test
// harness. This also tests the private simple_commands function.
#[path = "../src/rules.rs"]
mod rules;
