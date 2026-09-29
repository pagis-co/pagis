//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod contracts;
mod gmail_collector;
mod gmail_sync;
mod manifest;
mod oauth;
mod pinned_gog;
