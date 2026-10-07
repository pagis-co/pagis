//! The Web Push protocol of the daemon (ADR-0030): one encrypted Web Push
//! to one Push Subscription, and what the push service answered.
//!
//! Each Web Push is one RFC 8291 `aes128gcm` record that only the client
//! decrypts, with an RFC 8292 VAPID token that the VAPID Key of the
//! installation signs, posted by RFC 8030. A push service of a browser
//! and the Push Relay are equal endpoints.
//!
//! The crate holds no subscription, no payload and no retry rule. The
//! daemon holds them, and reads [`Outcome`] to end a Push Subscription
//! that is gone or to wait before the next send.

mod guard;
mod options;
mod vapid;
mod web_push;

pub use guard::Policy;
pub use options::{Options, Topic, TopicError, Urgency};
pub use web_push::{BuildError, MAX_PLAINTEXT, Outcome, Subscription, WebPush};
