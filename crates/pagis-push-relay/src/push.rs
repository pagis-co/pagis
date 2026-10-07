//! The headers of a Web Push (RFC 8030 section 5): the content coding of
//! the body and the delivery options that the relay forwards.

use std::time::Duration;

use axum::http::{HeaderMap, header};

use crate::transport::Urgency;

/// The longest `Topic` (RFC 8030 section 5.4).
const MAX_TOPIC: usize = 32;

/// `true` when the body is one RFC 8291 `aes128gcm` record.
pub(crate) fn is_aes128gcm(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("aes128gcm"))
}

/// The delivery options of a Web Push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) ttl: Duration,
    pub(crate) urgency: Urgency,
    pub(crate) topic: Option<String>,
}

/// Read `TTL`, `Urgency` and `Topic`. The message of an error names the
/// header and its form.
pub(crate) fn options(headers: &HeaderMap) -> Result<Options, String> {
    let ttl = text(headers, "ttl")?
        .filter(|ttl| !ttl.is_empty() && ttl.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|ttl| ttl.parse().ok())
        .ok_or("TTL is required: the seconds that the push service keeps the message")?;
    let urgency = match text(headers, "urgency")? {
        None => Urgency::Normal,
        Some(urgency) => {
            Urgency::parse(urgency).ok_or("Urgency must be very-low, low, normal or high")?
        }
    };
    let topic = match text(headers, "topic")? {
        None => None,
        Some(topic) if is_topic(topic) => Some(topic.to_string()),
        Some(_) => {
            return Err(
                "Topic must be 1 to 32 characters of A-Z, a-z, 0-9, '-' and '_'".to_string(),
            );
        }
    };
    Ok(Options {
        ttl: Duration::from_secs(ttl),
        urgency,
        topic,
    })
}

/// The trimmed value of the header `name`, or `None` when it is absent.
fn text<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, String> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map_err(|_| format!("{name} must be visible ASCII"))
        })
        .transpose()
}

fn is_topic(topic: &str) -> bool {
    let url_safe = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
    !topic.is_empty() && topic.len() <= MAX_TOPIC && topic.bytes().all(url_safe)
}
