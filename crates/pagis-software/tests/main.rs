//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod contribute;
mod docker_real;
mod fork;
mod git_store;
mod index;
mod manifest;
mod materialize;
mod pack;
mod publish;
mod run;
mod search;
mod support;
mod validate;
mod widget;
