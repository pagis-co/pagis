//! The Web Push protocol of the daemon (ADR-0030): one encrypted Web Push
//! to one Push Subscription, and what the push service answered.
//!
//! Each Web Push is one RFC 8291 `aes128gcm` record that only the client
//! decrypts, with an RFC 8292 VAPID token that the VAPID Key of the
//! installation signs, posted by RFC 8030. A push service of a browser
//! and the Push Relay are equal endpoints.
//!
//! [`WebPush::request`] makes the request without network I/O, and
//! [`Outcome::of`] reads the answer with the time of the caller. On
//! native targets, `WebPush::send` posts the request through the address
//! guard. On wasm, the host sends it under the rules of [`WebPush::request`].
//!
//! The crate holds no subscription, no payload and no retry rule. The
//! daemon holds them, and reads [`Outcome`] to end a Push Subscription
//! that is gone or to wait before the next send.

#[cfg(not(target_arch = "wasm32"))]
mod guard;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod options;
mod policy;
mod vapid;
mod web_push;

pub use options::{Options, Topic, TopicError, Urgency};
pub use policy::Policy;
pub use web_push::{BuildError, MAX_PLAINTEXT, Outcome, Subscription, WebPush};
