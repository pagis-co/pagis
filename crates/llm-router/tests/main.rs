//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod common;

mod anthropic;
mod audio_chat;
mod computer_use;
mod modalities;
mod models;
mod openai;
mod realtime;
mod registry;
mod router;
mod speech;
mod video;
