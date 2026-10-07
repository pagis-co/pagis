//! The Push Relay (ADR-0030).
//!
//! The project runs one Push Relay for the store apps of the Mobile App.
//! An installation of the Mobile App registers its APNs or FCM device
//! token and the VAPID key of its server, and the relay gives it an
//! opaque `https` Web Push endpoint. A Web Push to that endpoint goes to
//! the [`Transport`] of the platform. The relay runs apart from every
//! Pagis installation: it has its own settings and its own SQLite file,
//! and it depends on no daemon crate.

mod api;
mod apns;
mod clock;
mod fcm;
mod forwarded;
mod limit;
mod push;
mod registration;
mod settings;
mod store;
mod transport;
mod vapid;

pub use api::router;
pub use apns::{ApnsBaseUrls, ApnsError, ApnsTransport};
pub use clock::{Clock, SystemClock};
pub use fcm::{FCM_BASE_URL, FcmError, FcmTransport, ServiceAccount, TokenSource};
pub use forwarded::TrustedProxy;
pub use registration::{Environment, Platform};
pub use settings::{ApnsSettings, FcmSettings, PublicOrigin, Settings, SettingsError};
pub use store::{connect, connect_memory};
pub use transport::{Delivery, Message, Registration, Transport, Transports, Urgency};
