//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod advisories;
mod desktop;
mod desktop_linux;
mod emergency;
mod gate;
mod gog;
mod image;
mod pins;
mod release;
mod secrets;
mod security_policy;
mod server_image;
mod server_package;
mod support;
mod what_pagis_encrypts;
