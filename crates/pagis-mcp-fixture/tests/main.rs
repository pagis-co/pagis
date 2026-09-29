//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod support;

mod http_host;
mod output_limits;
mod stdio_host;
