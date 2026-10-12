//! Realtime values can move between threads on native targets. On
//! WebAssembly they stay on the host's thread, as in `llm-router`.

#[cfg(not(target_arch = "wasm32"))]
pub use std::marker::{Send as VoiceSend, Sync as VoiceSync};

#[cfg(target_arch = "wasm32")]
pub trait VoiceSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> VoiceSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait VoiceSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> VoiceSync for T {}
