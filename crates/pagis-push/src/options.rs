//! The delivery options of a Web Push: the headers of RFC 8030 that the
//! push service reads.

use std::time::Duration;

/// How a push service delivers one Web Push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// How long the push service keeps the Web Push for a client that is
    /// not reachable: the `TTL` header, in whole seconds.
    pub ttl: Duration,
    pub urgency: Urgency,
    /// A newer Web Push of the same topic replaces one that the push
    /// service did not deliver.
    pub topic: Option<Topic>,
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
    pub(crate) fn header(self) -> &'static str {
        match self {
            Urgency::VeryLow => "very-low",
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "high",
        }
    }
}

/// The `Topic` header: at most 32 characters of the URL-safe base64
/// alphabet (RFC 8030 section 5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic(String);

/// The longest topic that RFC 8030 allows.
const MAX_TOPIC: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a topic is 1 to 32 characters of A-Z, a-z, 0-9, '-' and '_', not {0:?}")]
pub struct TopicError(String);

impl Topic {
    pub fn new(topic: &str) -> Result<Self, TopicError> {
        let url_safe = topic
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if topic.is_empty() || topic.len() > MAX_TOPIC || !url_safe {
            return Err(TopicError(topic.to_string()));
        }
        Ok(Self(topic.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
