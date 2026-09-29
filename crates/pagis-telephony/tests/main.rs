//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod audio;
mod call_record;
mod call_tool;
mod calls;
mod dtmf;
mod emergency;
mod endpoint;
mod hub;
mod inbound;
mod listen;
mod model_router;
mod numbers;
mod plivo;
mod realtime_bridge;
mod recording;
mod routing;
mod sdp;
mod telnyx_catalog;
mod telnyx_text;
mod text_transport;
mod tiers;
mod transport;
mod twilio;
mod twilio_text;
