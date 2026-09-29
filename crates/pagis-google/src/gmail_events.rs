//! The Gmail collector for `mail.message_received`
//! (ADR-0006, ADR-0019).
//!
//! Arrival rules keep a separate baseline from account learning. This
//! collector uses a search cursor: a high-water received time plus the ids it saw near that
//! edge. It searches from an overlapping window, paginates until it
//! reaches mail it already knows, and deduplicates by message id.
//!
//! The cursor is opaque outside this file. Account learning uses native
//! Gmail history in `gmail_sync`; it does not emit historical arrivals.
//!
//! Nothing here stores a body, a snippet, or an attachment. The Agent
//! reaches those only through a live `mail__get_message`.
//!
//! The event kind is the one mail kind, and the `mailbox` field is the
//! alias of the Connection the mail arrived on, which is the same name
//! the Agent passes to a `mail__*` tool (ADR-0019).

use pagis_core::NormalizedEvent;
use serde::{Deserialize, Serialize};

use crate::{GmailSearch, GogRunner, GoogleCall, GoogleProvider, ProviderError};

/// How far behind the high-water mark each search starts. Gmail orders
/// by received time, and a message can land a moment out of order, so
/// the window is re-read and the duplicates are dropped downstream.
pub const OVERLAP: i64 = 120_000;

/// The most pages one collection pass reads. A mailbox that received
/// more than this since the last pass catches up on the next pass,
/// because the cursor only advances over what was read.
const MAX_PAGES: usize = 10;

/// The most messages one page asks for.
const PAGE_SIZE: u32 = 100;

/// How many recent ids the cursor carries. It only has to cover the
/// overlap window; the Incoming Event table is the durable dedup key.
const MAX_RECENT_IDS: usize = 500;

/// The collector's place in one mailbox. It is serialized into the
/// Trigger module's opaque cursor column and read nowhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailCursor {
    /// The newest received time this collector has accepted.
    pub high_water_at: i64,
    /// The message ids seen at or after `high_water_at - OVERLAP`.
    #[serde(default)]
    pub recent_ids: Vec<String>,
}

impl GmailCursor {
    fn decode(cursor: Option<&str>) -> Option<Self> {
        serde_json::from_str(cursor?).ok()
    }

    fn encode(&self) -> String {
        serde_json::to_string(self).expect("cursor serializes")
    }
}

/// What one collection pass acquired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub cursor: String,
    pub events: Vec<NormalizedEvent>,
    /// True for the pass that established the baseline. It emits no
    /// Incoming Event: mail from before the first rule is not news.
    pub baseline: bool,
}

/// One mailbox's collector.
pub struct GmailCollector<R> {
    provider: GoogleProvider<R>,
    /// The alias of the Connection this mailbox is. It stamps every
    /// event, so a rule can name one account (ADR-0019).
    mailbox: String,
}

impl<R: GogRunner> GmailCollector<R> {
    pub fn new(provider: GoogleProvider<R>, mailbox: impl Into<String>) -> Self {
        Self {
            provider,
            mailbox: mailbox.into(),
        }
    }

    /// Read the mail that arrived since the cursor. `now` is the clock
    /// the baseline uses when the mailbox is empty.
    pub async fn collect(
        &self,
        cursor: Option<&str>,
        now: i64,
    ) -> Result<Collection, ProviderError> {
        let previous = GmailCursor::decode(cursor);
        let from = previous
            .as_ref()
            .map(|cursor| cursor.high_water_at - OVERLAP);
        let mut messages = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let value = self
                .provider
                .invoke(&GoogleCall::GmailSearch(GmailSearch {
                    query: query(from),
                    max: Some(PAGE_SIZE),
                    page: page.clone(),
                }))
                .await?;
            let (found, next) = parse_page(&value);
            let reached_known = found
                .iter()
                .any(|message| from.is_some_and(|from| message.received_at < from));
            messages.extend(found);
            page = next;
            if page.is_none() || reached_known {
                break;
            }
        }

        let Some(previous) = previous else {
            // The first pass sets the baseline and emits nothing.
            let high_water_at = messages
                .iter()
                .map(|message| message.received_at)
                .max()
                .unwrap_or(now);
            return Ok(Collection {
                cursor: cursor_from(high_water_at, &messages).encode(),
                events: Vec::new(),
                baseline: true,
            });
        };

        let events: Vec<NormalizedEvent> = messages
            .iter()
            .filter(|message| message.received_at > previous.high_water_at - OVERLAP)
            .filter(|message| !previous.recent_ids.contains(&message.message_id))
            .map(|message| message.to_event(&self.mailbox))
            .collect();
        let high_water_at = messages
            .iter()
            .map(|message| message.received_at)
            .max()
            .unwrap_or(previous.high_water_at)
            .max(previous.high_water_at);
        Ok(Collection {
            cursor: cursor_from(high_water_at, &messages).encode(),
            events,
            baseline: false,
        })
    }
}

/// The cursor after one pass: the new high-water mark, and the ids
/// inside the window the next pass re-reads.
fn cursor_from(high_water_at: i64, messages: &[Message]) -> GmailCursor {
    let mut recent: Vec<(i64, String)> = messages
        .iter()
        .filter(|message| message.received_at >= high_water_at - OVERLAP)
        .map(|message| (message.received_at, message.message_id.clone()))
        .collect();
    recent.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    recent.truncate(MAX_RECENT_IDS);
    GmailCursor {
        high_water_at,
        recent_ids: recent.into_iter().map(|(_, id)| id).collect(),
    }
}

/// The Gmail search this collector runs. It is broad on purpose: one
/// pass serves every rule on the Connection, and the typed filters are
/// evaluated locally afterwards.
fn query(from: Option<i64>) -> String {
    let mut query = crate::manifest::BROWSE_QUERY.to_string();
    if let Some(from) = from {
        // Gmail's `after:` takes whole seconds and is exclusive, so the
        // window starts one second early rather than one message late.
        let seconds = (from / 1000).saturating_sub(1).max(0);
        query.push_str(&format!(" after:{seconds}"));
    }
    query
}

/// One message, reduced to the fields the declaration allows
/// (ADR-0019). There is no body and no snippet here.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    message_id: String,
    thread_id: Option<String>,
    in_reply_to: Option<String>,
    from: Option<String>,
    to: Vec<String>,
    subject: Option<String>,
    has_attachments: bool,
    received_at: i64,
}

impl Message {
    fn to_event(&self, mailbox: &str) -> NormalizedEvent {
        let mut metadata = serde_json::Map::new();
        metadata.insert("mailbox".into(), mailbox.into());
        metadata.insert("message_id".into(), self.message_id.clone().into());
        for (name, value) in [
            ("thread_id", &self.thread_id),
            ("in_reply_to", &self.in_reply_to),
            ("subject", &self.subject),
        ] {
            if let Some(value) = value {
                metadata.insert(name.into(), value.clone().into());
            }
        }
        if let Some(from) = &self.from {
            metadata.insert("from".into(), from.clone().into());
            if let Some(domain) = domain_of(from) {
                metadata.insert("from_domain".into(), domain.into());
            }
        }
        metadata.insert("to".into(), self.to.clone().into());
        metadata.insert("has_attachments".into(), self.has_attachments.into());
        metadata.insert("received_at".into(), self.received_at.into());
        NormalizedEvent {
            provider_event_id: self.message_id.clone(),
            metadata: serde_json::Value::Object(metadata),
            occurred_at: self.received_at,
            // Gmail is the user's account, so its Wake-up lands where
            // the rule says (ADR-0019).
            landing: None,
        }
    }
}

/// The registrable-looking domain of a `From` header value. It is
/// untrusted text: the matcher compares it, and nothing else reads it.
pub(crate) fn domain_of(from: &str) -> Option<String> {
    let address = from
        .rsplit_once('<')
        .map(|(_, rest)| rest.trim_end_matches('>'))
        .unwrap_or(from);
    let (_, domain) = address.trim().rsplit_once('@')?;
    let domain = domain.trim().trim_end_matches('>').to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

/// One page of a `gog gmail search` document. The shape is read
/// tolerantly: the collector pins the fields it needs, not the whole
/// provider document.
fn parse_page(value: &serde_json::Value) -> (Vec<Message>, Option<String>) {
    let messages = page_items(value).iter().filter_map(parse_message).collect();
    let next = ["next_page", "nextPageToken", "next_page_token"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|found| found.as_str()))
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    (messages, next)
}

/// The entries of one page of a `gog gmail search` document.
fn page_items(value: &serde_json::Value) -> Vec<serde_json::Value> {
    ["messages", "items", "results", "data"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|found| found.as_array()))
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default()
}

/// The Gmail message id of one entry of a search page.
fn message_id(value: &serde_json::Value) -> Option<String> {
    string(value, &["id", "message_id", "messageId"])
}

/// The Gmail messages that one page of a `gog gmail search` document
/// lists: the id of each, and its thread where the entry names one. The
/// page is read as the collector reads it.
pub(crate) fn listed_messages(value: &serde_json::Value) -> Vec<(String, Option<String>)> {
    page_items(value)
        .iter()
        .filter_map(|entry| {
            Some((
                message_id(entry)?,
                string(entry, &["thread_id", "threadId"]),
            ))
        })
        .collect()
}

fn parse_message(value: &serde_json::Value) -> Option<Message> {
    let message_id = message_id(value)?;
    let received_at = ["internal_date", "internalDate", "received_at", "date"]
        .iter()
        .find_map(|key| value.get(*key).and_then(parse_instant))?;
    Some(Message {
        message_id,
        thread_id: string(value, &["thread_id", "threadId"]),
        in_reply_to: string(value, &["in_reply_to", "inReplyTo"]),
        from: string(value, &["from", "sender"]),
        to: recipients(value),
        subject: string(value, &["subject"]),
        has_attachments: ["has_attachments", "hasAttachments"]
            .iter()
            .find_map(|key| value.get(*key).and_then(|found| found.as_bool()))
            .unwrap_or(false),
        received_at,
    })
}

/// The `To` header, however the provider writes it: a list, or one
/// string with the addresses separated by commas.
fn recipients(value: &serde_json::Value) -> Vec<String> {
    let Some(found) = value.get("to") else {
        return Vec::new();
    };
    if let Some(list) = found.as_array() {
        return list
            .iter()
            .filter_map(|entry| entry.as_str())
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
    }
    found
        .as_str()
        .map(|line| {
            line.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|found| found.as_str()))
        .filter(|found| !found.is_empty())
        .map(str::to_string)
}

/// Gmail reports the received time as epoch milliseconds, epoch
/// seconds, or a date header. All three become milliseconds.
fn parse_instant(value: &serde_json::Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(scale(number));
    }
    let text = value.as_str()?;
    if let Ok(number) = text.parse::<i64>() {
        return Some(scale(number));
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.timestamp_millis())
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc2822(text).map(|parsed| parsed.timestamp_millis())
        })
        .ok()
}

/// Ten-digit values are seconds; thirteen-digit values are already
/// milliseconds.
fn scale(number: i64) -> i64 {
    if number.abs() < 100_000_000_000 {
        number * 1000
    } else {
        number
    }
}
