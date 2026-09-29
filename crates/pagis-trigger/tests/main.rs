//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod arrival;
mod common;
mod forget;
mod ingest;
mod one_shot;
mod recurring;
mod run_now;
mod targets;
