//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

#![cfg(not(target_arch = "wasm32"))]

mod audio;
mod call_record;
mod calls;
mod dtmf;
mod endpoint;
mod frame_media;
mod handoff;
mod hub;
mod inbound;
mod listen;
#[path = "../../llm-router/tests/support/realtime.rs"]
mod realtime_transport;

mod model_router;
mod numbers;
mod plivo;
mod realtime_bridge;
mod recording;
mod routing;
mod sdp;
mod session_tools;
mod telnyx_catalog;
mod telnyx_text;
mod text_collector;
mod text_transport;
mod tiers;
mod transport;
mod twilio;
mod twilio_text;

mod workspace_calls;
