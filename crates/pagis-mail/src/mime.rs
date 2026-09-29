//! The MIME half of the transport (ADR-0019): the bytes a host
//! gives back become one [`Message`], and one [`OutgoingMessage`]
//! becomes the bytes a host accepts.
//!
//! HTML becomes text here, and an attachment keeps its name and its
//! size only: the bytes stay at the host, so a tool answer cannot grow
//! with a file the model never asked for.

use mail_builder::MessageBuilder;
use mail_parser::{Address, HeaderName, MessageParser, MimeHeaders};

use crate::sender::AUTHENTICATION_RESULTS;
use crate::transport::{
    Attachment, Message, MessageId, MessageSummary, OutgoingMessage, TransportError,
    TransportErrorCode,
};

/// How much of the text body one summary carries (ADR-0019).
const SNIPPET_CHARS: usize = 200;

/// One outgoing message, ready for the submission port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltMessage {
    /// The `Message-ID` the bytes carry, which the send reports back.
    pub message_id: String,
    /// Every envelope recipient, `To`, `Cc` and `Bcc` in that order.
    /// `Bcc` is an envelope recipient and never a header.
    pub recipients: Vec<String>,
    pub raw: Vec<u8>,
}

/// Read the bytes of one message into the shape the tools answer with.
/// `id` is the folder and UID the bytes came from; the message carries
/// no id of its own that Pagis uses.
pub fn read_mime(raw: &[u8], id: MessageId) -> Result<Message, TransportError> {
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or(TransportError(TransportErrorCode::Unreadable))?;

    // `body_text` converts an HTML-only body to text, so one call
    // covers both shapes (ADR-0019).
    let text = parsed.body_text(0).unwrap_or_default().into_owned();
    let attachments: Vec<Attachment> = parsed
        .attachments()
        .map(|part| Attachment {
            name: part
                .attachment_name()
                .unwrap_or("attachment")
                .trim()
                .to_string(),
            bytes: part.body.len() as u64,
        })
        .collect();

    let message_id = parsed.message_id().unwrap_or_default().to_string();
    let in_reply_to = parsed
        .in_reply_to()
        .as_text_list()
        .and_then(|answered| answered.last().map(|answered| answered.to_string()))
        .filter(|answered| !answered.is_empty())
        .map(|answered| bracket(&answered));
    let thread_id = parsed
        .references()
        .as_text_list()
        .and_then(|references| references.first())
        .filter(|root| !root.is_empty())
        .map(|root| bracket(root))
        .unwrap_or_else(|| {
            if message_id.is_empty() {
                id.to_string()
            } else {
                bracket(&message_id)
            }
        });

    let summary = MessageSummary {
        from: parsed
            .from()
            .and_then(one_address)
            .unwrap_or_default()
            .to_string(),
        from_mailboxes: parsed
            .header_values(HeaderName::From)
            .map(|value| {
                value
                    .as_address()
                    .map_or(0, |address| address.iter().count())
                    .max(1)
            })
            .sum(),
        to: parsed.to().map(every_address).unwrap_or_default(),
        date: parsed
            .date()
            .map(|date| date.to_timestamp() * 1_000)
            .unwrap_or_default(),
        subject: parsed.subject().unwrap_or_default().to_string(),
        snippet: text.trim().chars().take(SNIPPET_CHARS).collect::<String>(),
        has_attachments: !attachments.is_empty(),
        message_id: if message_id.is_empty() {
            String::new()
        } else {
            bracket(&message_id)
        },
        in_reply_to,
        thread_id,
        id,
        authentication_results: parsed
            .headers_raw()
            .filter(|(name, _)| name.eq_ignore_ascii_case(AUTHENTICATION_RESULTS))
            .map(|(_, value)| value.trim().to_string())
            .collect(),
    };

    let headers = parsed
        .headers_raw()
        .map(|(name, value)| (name.to_string(), value.trim().to_string()))
        .collect();

    Ok(Message {
        summary,
        headers,
        text,
        attachments,
    })
}

/// Build the bytes of one outgoing message. A reply is a send with
/// `in_reply_to` (ADR-0019): it sets `In-Reply-To` and `References`,
/// which is what threads the answer at the recipient.
pub fn build_mime(from: &str, message: &OutgoingMessage) -> Result<BuiltMessage, TransportError> {
    let recipients: Vec<String> = message
        .to
        .iter()
        .chain(&message.cc)
        .chain(&message.bcc)
        .map(|address| address.trim().to_string())
        .filter(|address| !address.is_empty())
        .collect();
    if recipients.is_empty() {
        return Err(TransportError(TransportErrorCode::Rejected));
    }

    // A reserved effect names the identity its outcome is reconciled against,
    // so the send carries that identity instead of a fresh one.
    let message_id = match &message.message_id {
        Some(reserved) => bracket(reserved.trim_matches(['<', '>'])),
        None => new_message_id(from),
    };
    let mut builder = MessageBuilder::new()
        .message_id(message_id.trim_matches(['<', '>']))
        .from(from)
        .to(message.to.clone())
        .subject(message.subject.as_str())
        .text_body(message.body.as_str());
    if !message.cc.is_empty() {
        builder = builder.cc(message.cc.clone());
    }
    if let Some(answered) = &message.in_reply_to {
        let answered = answered.trim_matches(['<', '>']);
        builder = builder.in_reply_to(answered).references(answered);
    }

    let raw = builder
        .write_to_vec()
        .map_err(|_| TransportError(TransportErrorCode::Rejected))?;
    Ok(BuiltMessage {
        message_id,
        recipients,
        raw,
    })
}

/// A `Message-ID` of this mailbox's own domain, so the send reports
/// the id it wrote and a later reply threads on it.
fn new_message_id(from: &str) -> String {
    let domain = from.rsplit_once('@').map_or("pagis.invalid", |(_, it)| it);
    format!(
        "<{}@{domain}>",
        ulid::Ulid::new().to_string().to_lowercase()
    )
}

/// The angle brackets a `Message-ID` reads with. `mail-parser` strips
/// them and the tools show the header form.
fn bracket(id: &str) -> String {
    if id.starts_with('<') {
        id.to_string()
    } else {
        format!("<{id}>")
    }
}

fn one_address(address: &Address<'_>) -> Option<String> {
    address
        .first()
        .and_then(|addr| addr.address.as_ref())
        .map(|address| address.to_string())
}

fn every_address(address: &Address<'_>) -> Vec<String> {
    address
        .iter()
        .filter_map(|addr| addr.address.as_ref())
        .map(|address| address.to_string())
        .collect()
}
