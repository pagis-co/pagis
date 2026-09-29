//! The text seam (ADR-0020): carry one text out, read the texts that
//! came in, and hold the carrier's messaging object in place.
//!
//! The other two seams are [`NumberCatalog`] on the REST key and
//! [`CallTransport`] on the SIP credential (ADR-0020). Texting is REST
//! in both directions and it holds a per-number collector cursor, which
//! is a duty of its own with a failure mode of its own, so it is the
//! third seam and not a pair of methods on the catalog.
//!
//! No carrier offers a channel a daemon behind NAT can dial, so inbound
//! texts are polled with an opaque cursor the collector keeps on the
//! number. A carrier that does not carry texts declares texting absent
//! and answers [`TextError::TextingAbsent`]; nothing is emulated
//! (ADR-0005).
//!
//! [`NumberCatalog`]: crate::NumberCatalog
//! [`CallTransport`]: crate::CallTransport

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{PhoneNumber, TextDeliveryStatus, UnixMillis};
use serde_json::{Map, Value};

use crate::catalog::CarrierKey;

/// What a text transport can and cannot do (ADR-0005, ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextCapabilities {
    /// Whether the carrier carries a text at all. A carrier without it
    /// refuses every call on this seam.
    pub texting: bool,
    /// Whether an inbound text brings media the daemon can fetch.
    pub inbound_media: bool,
}

impl TextCapabilities {
    /// A carrier that carries no text.
    pub const ABSENT: Self = Self {
        texting: false,
        inbound_media: false,
    };
}

/// What the carrier had to have in place before the number texts
/// (ADR-0020): the messaging object of the number, and the ids of the
/// Workspace-level objects the carrier made beside it.
///
/// The caller persists both halves: `messaging_object_id` on the
/// number with `PhoneNumberStore::set_messaging_object_id`, and
/// `connection_config` merged into the carrier Connection's `config`
/// map, where the SIP username already lives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prepared {
    /// The carrier's messaging object for this number: the Telnyx
    /// messaging profile id, or the Twilio Messaging Service the
    /// number sits in the sender pool of. `None` when the carrier
    /// needs no object per number.
    pub messaging_object_id: Option<String>,
    /// The Workspace-level ids to write on the Connection, under the
    /// keys the carrier's own client names.
    pub connection_config: Map<String, Value>,
}

/// What the carrier answered a send with (ADR-0020). The daemon sends
/// the body whole and the carrier segments it, so the segment count
/// comes back from the carrier and is never computed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentText {
    /// The carrier's own id for the message. The delivery poll reads
    /// it, and ingest deduplicates on it.
    pub carrier_id: String,
    /// How many segments the carrier billed.
    pub segments: u32,
}

/// One text that arrived for an Agent Phone Number (ADR-0020). The
/// carrier concatenates the segments before it stores the text, so the
/// daemon reassembles nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundText {
    /// The carrier's own id for the message.
    pub carrier_id: String,
    pub from_e164: String,
    pub to_e164: String,
    pub body: String,
    pub received_at: UnixMillis,
    /// The MMS media, as the carrier addresses it. The collector
    /// fetches each one with [`TextTransport::fetch_media`] and stores
    /// it as an Artifact before it raises the event.
    pub media_urls: Vec<String>,
}

/// Why the carrier did not do what it was asked (ADR-0020). The
/// variants are stable, because they reach the user, the tool result
/// and the tests; the carrier's own code travels inside them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    /// The carrier carries no text (ADR-0005). Plivo answers this to
    /// every call.
    #[error("this carrier does not carry texts")]
    TextingAbsent,
    /// The account may not text this destination. It is a per-send
    /// failure the carrier reports, and the daemon reads no geo
    /// permission in advance (ADR-0020).
    #[error("the carrier does not text this destination: {code}")]
    DestinationNotEnabled { code: String },
    /// The carrier refused the request because of its rate limit. It
    /// can also say when to try again.
    #[error("the carrier rate-limited this account")]
    RateLimited { retry_after: Option<Duration> },
    /// The carrier refused, in its own code and words.
    #[error("the carrier refused this: {code}: {message}")]
    Carrier { code: String, message: String },
    /// The carrier was not reached: DNS, the socket, a timeout, or an
    /// answer this client cannot read.
    #[error("the carrier was not reached: {0}")]
    Unreachable(String),
}

impl TextError {
    /// The one stable word for this failure.
    pub fn as_str(&self) -> &'static str {
        match self {
            TextError::TextingAbsent => "texting_absent",
            TextError::DestinationNotEnabled { .. } => "destination_not_enabled",
            TextError::RateLimited { .. } => "rate_limited",
            TextError::Carrier { .. } => "carrier",
            TextError::Unreachable(_) => "unreachable",
        }
    }

    /// The carrier's own code, when the carrier gave one.
    pub fn carrier_code(&self) -> Option<&str> {
        match self {
            TextError::DestinationNotEnabled { code } => Some(code),
            TextError::Carrier { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// Carry texts at the carrier (ADR-0020).
///
/// Every method signs with the carrier's REST [`CarrierKey`], as
/// [`NumberCatalog`] does; the SIP credential stays on the call seam.
///
/// [`NumberCatalog`]: crate::NumberCatalog
#[async_trait]
pub trait TextTransport: Send + Sync {
    /// What this transport declares. The desk, the tools and the
    /// collector read it; nothing else decides whether a number texts.
    fn capabilities(&self) -> TextCapabilities;

    /// Put the carrier's messaging objects in place for one number.
    /// It runs at assignment, as the SIP credential connection is
    /// prepared, and again on daemon start when the object is
    /// missing. `connection_config` is the carrier Connection's own
    /// `config` map, so a Workspace-level object that is already there
    /// is reused instead of made twice.
    async fn prepare(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        connection_config: &Value,
    ) -> Result<Prepared, TextError>;

    /// Send one text from the number to a counterpart. The body goes
    /// whole, up to the carrier's limit, and the carrier segments it.
    async fn send(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        to_e164: &str,
        body: &str,
    ) -> Result<SentText, TextError>;

    /// Whether one sent text arrived, in the four states of ADR-0020.
    async fn delivery_status(
        &self,
        key: &CarrierKey,
        carrier_id: &str,
    ) -> Result<TextDeliveryStatus, TextError>;

    /// The texts that arrived for the number after `cursor`, and the
    /// cursor to come back with. The cursor is opaque: the carrier
    /// decides what it holds, and the collector persists it in the
    /// number's `text_cursor` field.
    async fn poll_inbound(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        cursor: Option<&str>,
    ) -> Result<(Vec<InboundText>, Option<String>), TextError>;

    /// The bytes of one media file of an inbound text.
    async fn fetch_media(&self, key: &CarrierKey, url: &str) -> Result<Vec<u8>, TextError>;
}

/// One [`TextTransport`] per carrier provider (ADR-0020), keyed as
/// [`NumberCatalogs`] is.
///
/// A provider with no entry carries no text: [`Self::capabilities`]
/// answers [`TextCapabilities::ABSENT`] for it, so a carrier with no
/// text client needs no placeholder entry.
///
/// [`NumberCatalogs`]: crate::NumberCatalogs
#[derive(Default, Clone)]
pub struct TextTransports(HashMap<String, Arc<dyn TextTransport>>);

impl TextTransports {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, provider: &str, transport: Arc<dyn TextTransport>) -> Self {
        self.0.insert(provider.to_string(), transport);
        self
    }

    /// One transport under one provider; the tests use it.
    pub fn single(provider: &str, transport: Arc<dyn TextTransport>) -> Self {
        Self::new().with(provider, transport)
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn TextTransport>> {
        self.0.get(provider).cloned()
    }

    /// What the provider's transport declares. A provider with no
    /// entry declares texting absent.
    pub fn capabilities(&self, provider: &str) -> TextCapabilities {
        self.get(provider)
            .map(|transport| transport.capabilities())
            .unwrap_or(TextCapabilities::ABSENT)
    }

    /// Whether the provider carries a text at all.
    pub fn texts(&self, provider: &str) -> bool {
        self.capabilities(provider).texting
    }
}

impl std::fmt::Debug for TextTransports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut providers: Vec<&str> = self.0.keys().map(String::as_str).collect();
        providers.sort_unstable();
        f.debug_tuple("TextTransports").field(&providers).finish()
    }
}
