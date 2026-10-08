//! The forward of a Web Push to the push service of a device. The relay
//! holds one [`Transport`] for each platform that it serves.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::registration::{Environment, Platform};

/// The device of one registration: its platform and its device token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub platform: Platform,
    pub token: String,
}

/// One Web Push as the relay forwards it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The `aes128gcm` body, unchanged. Only the client decrypts it.
    pub body: Vec<u8>,
    /// How long the push service keeps the message for a device that is
    /// not reachable: the `TTL` header.
    pub ttl: Duration,
    pub urgency: Urgency,
    /// A newer message of the same topic replaces one that the push
    /// service did not deliver: the `Topic` header.
    pub topic: Option<String>,
}

/// The `Urgency` header (RFC 8030 section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    VeryLow,
    Low,
    Normal,
    High,
}

impl Urgency {
    /// The urgency of a header value, or `None` for another value.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "very-low" => Some(Self::VeryLow),
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// The header value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VeryLow => "very-low",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// What the push service of the device did with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The push service took the message.
    Delivered,
    /// The device token is no longer valid. The relay removes the
    /// registration.
    Gone,
    /// The push service did not take the message, for this reason.
    Failed(String),
}

/// The client of the push service of one platform.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Forward `message` to the device of `registration`.
    async fn send(&self, registration: &Registration, message: &Message) -> Delivery;
}

/// The transport of each platform that the relay serves. A platform, or
/// an APNs environment of `ios`, with no transport takes no
/// registration.
#[derive(Clone, Default)]
pub struct Transports {
    ios_production: Option<Arc<dyn Transport>>,
    ios_sandbox: Option<Arc<dyn Transport>>,
    android: Option<Arc<dyn Transport>>,
}

impl Transports {
    /// Serve the `ios` platform in the APNs `environment`.
    pub fn with_ios(mut self, environment: Environment, transport: Arc<dyn Transport>) -> Self {
        match environment {
            Environment::Production => self.ios_production = Some(transport),
            Environment::Sandbox => self.ios_sandbox = Some(transport),
        }
        self
    }

    /// Serve the `android` platform.
    pub fn with_android(mut self, transport: Arc<dyn Transport>) -> Self {
        self.android = Some(transport);
        self
    }

    pub(crate) fn of(&self, platform: Platform) -> Option<&dyn Transport> {
        let transport = match platform {
            Platform::Ios(Environment::Production) => &self.ios_production,
            Platform::Ios(Environment::Sandbox) => &self.ios_sandbox,
            Platform::Android => &self.android,
        };
        transport.as_deref()
    }
}

/// `error` and each error under it, joined by `: `.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        text.push_str(": ");
        text.push_str(&error.to_string());
        source = error.source();
    }
    text
}
