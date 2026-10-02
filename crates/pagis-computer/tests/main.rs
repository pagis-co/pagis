//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod bollard_adapter;
mod docker_real;
mod egress;
mod logs;
mod manager;
mod media_relay;
mod output_cap;
mod remote_access_turn;
