//! The text seam of a carrier that carries no text (ADR-0020).
//!
//! Plivo never returns the body of an inbound text, so Pagis does not
//! text on Plivo at all. A capability that is absent is absent
//! (ADR-0005): this transport declares texting absent and answers
//! `texting_absent` to every call, and the Plivo entry of the Provider
//! Catalog says the same, so the user reads it before a number is
//! bought.

use async_trait::async_trait;
use pagis_core::{PhoneNumber, TextDeliveryStatus};
use serde_json::Value;

use crate::catalog::CarrierKey;
use crate::text::{InboundText, Prepared, SentText, TextCapabilities, TextError, TextTransport};

/// A transport that carries no text.
#[derive(Debug, Default)]
pub struct NoTextTransport;

#[async_trait]
impl TextTransport for NoTextTransport {
    fn capabilities(&self) -> TextCapabilities {
        TextCapabilities::ABSENT
    }

    async fn prepare(
        &self,
        _key: &CarrierKey,
        _number: &PhoneNumber,
        _connection_config: &Value,
    ) -> Result<Prepared, TextError> {
        Err(TextError::TextingAbsent)
    }

    async fn send(
        &self,
        _key: &CarrierKey,
        _number: &PhoneNumber,
        _to_e164: &str,
        _body: &str,
    ) -> Result<SentText, TextError> {
        Err(TextError::TextingAbsent)
    }

    async fn delivery_status(
        &self,
        _key: &CarrierKey,
        _carrier_id: &str,
    ) -> Result<TextDeliveryStatus, TextError> {
        Err(TextError::TextingAbsent)
    }

    async fn poll_inbound(
        &self,
        _key: &CarrierKey,
        _number: &PhoneNumber,
        _cursor: Option<&str>,
    ) -> Result<(Vec<InboundText>, Option<String>), TextError> {
        Err(TextError::TextingAbsent)
    }

    async fn fetch_media(&self, _key: &CarrierKey, _url: &str) -> Result<Vec<u8>, TextError> {
        Err(TextError::TextingAbsent)
    }
}
