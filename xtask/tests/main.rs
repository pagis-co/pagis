//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod advisories;
mod desktop;
mod desktop_linux;
mod emergency;
mod gate;
mod gog;
mod image;
mod mobile;
mod pins;
mod relay_image;
mod release;
mod screend;
mod secrets;
mod security_policy;
mod seed_worktree_target;
mod server_image;
mod server_package;
mod support;
