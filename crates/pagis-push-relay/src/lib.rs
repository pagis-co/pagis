//! The Push Relay (ADR-0030).
//!
//! The project runs one Push Relay for the store apps of the Mobile App.
//! An installation of the Mobile App registers its APNs or FCM device
//! token and the VAPID key of its server, and the relay gives it an
//! opaque `https` Web Push endpoint. The relay runs apart from every
//! Pagis installation: it has its own settings and its own SQLite file,
//! and it depends on no daemon crate.

mod api;
mod forwarded;
mod limit;
mod registration;
mod settings;
mod store;

pub use api::router;
pub use forwarded::TrustedProxy;
pub use settings::{PublicOrigin, Settings, SettingsError};
pub use store::{connect, connect_memory};
