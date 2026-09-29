//! The Twilio text seam (ADR-0020): send a text, read the delivery
//! receipt, poll the texts that came in, and fetch inbound media.
//!
//! Twilio is the one carrier of the three that gives the body of an
//! inbound text to a poller: it stores every inbound message with its
//! body, whatever the number's webhook setting, and the list endpoint
//! returns it. So the daemon needs no public URL and no relay here; it
//! lists, filters and remembers where it stopped.
//!
//! The client signs with HTTP Basic, as the number half does: the
//! Account SID is the account of the [`CarrierKey`] and it also names
//! the account in every path of the 2010-04-01 API; the secret is the
//! Auth Token. Two hosts carry the work: `api.twilio.com` for messages
//! and media, and `messaging.twilio.com` for the Messaging Service.
//!
//! [`CarrierKey`]: crate::CarrierKey

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use chrono::SecondsFormat;
use pagis_core::{PhoneNumber, TextDeliveryStatus};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::catalog::CarrierKey;
use crate::text::{InboundText, Prepared, SentText, TextCapabilities, TextError, TextTransport};

/// The host of the 2010-04-01 API: messages and media.
const API_BASE: &str = "https://api.twilio.com";
/// The host of the Messaging Service API.
const MESSAGING_BASE: &str = "https://messaging.twilio.com";
/// How long the client waits for one Twilio request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How many messages one page of the inbound poll holds. Twilio's
/// default is 50 and its maximum is 1000.
const PAGE_SIZE: u32 = 50;
/// How many pages one poll follows. A poll every ten seconds never
/// needs more, and the cap keeps a carrier that always offers another
/// page from holding the collector.
const MAX_PAGES: usize = 20;
/// Twilio's code for an account that may not text this region: its
/// geo-permission refusal.
const GEO_PERMISSION_CODE: &str = "21408";
/// Twilio's code for a request it did not process, which the caller
/// may retry.
const RATE_LIMIT_CODE: &str = "20429";
/// The name the Messaging Service carries in the user's Twilio
/// console, so the user reads who made it.
const SERVICE_NAME: &str = "Pagis";

/// The `config` key of the carrier Connection that holds the
/// Workspace's Twilio Messaging Service (ADR-0020). The service is
/// made once, on the first number that texts, and every later number
/// of the Workspace joins its sender pool.
pub const TWILIO_MESSAGING_SERVICE_KEY: &str = "twilio_messaging_service_sid";

/// Carry texts on Twilio.
pub struct TwilioTextTransport {
    http: reqwest::Client,
    api_base: String,
    messaging_base: String,
}

impl TwilioTextTransport {
    pub fn new() -> Result<Self, TextError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| TextError::Unreachable(error.to_string()))?,
            api_base: API_BASE.to_string(),
            messaging_base: MESSAGING_BASE.to_string(),
        })
    }

    /// Point both hosts at one base URL. The contract tests use it;
    /// production uses [`TwilioTextTransport::new`].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            http: reqwest::Client::new(),
            api_base: base_url.clone(),
            messaging_base: base_url,
        }
    }

    /// The URL of one resource of the account the key names.
    fn account_url(&self, key: &CarrierKey, tail: &str) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}/{tail}",
            self.api_base,
            key.account()
        )
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        key: &CarrierKey,
        url: &str,
        query: &[(String, String)],
    ) -> Result<T, TextError> {
        let response = self
            .http
            .get(url)
            .basic_auth(key.account(), Some(key.expose_secret()))
            .query(query)
            .send()
            .await
            .map_err(unreachable)?;
        read(response).await
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        key: &CarrierKey,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, TextError> {
        let response = self
            .http
            .post(url)
            .basic_auth(key.account(), Some(key.expose_secret()))
            .form(form)
            .send()
            .await
            .map_err(unreachable)?;
        read(response).await
    }

    /// The Workspace's Messaging Service, made once. A sid already on
    /// the Connection is reused, so a second number of the same
    /// Workspace makes no second service.
    async fn messaging_service(
        &self,
        key: &CarrierKey,
        connection_config: &Value,
    ) -> Result<(String, Map<String, Value>), TextError> {
        if let Some(sid) = messaging_service_sid(connection_config) {
            return Ok((sid, Map::new()));
        }
        let made: ServiceDto = self
            .post(
                key,
                &format!("{}/v1/Services", self.messaging_base),
                &[("FriendlyName", SERVICE_NAME)],
            )
            .await?;
        let mut config = Map::new();
        config.insert(
            TWILIO_MESSAGING_SERVICE_KEY.to_string(),
            Value::String(made.sid.clone()),
        );
        Ok((made.sid, config))
    }

    /// Put the number in the service's sender pool. A number the pool
    /// already holds is in place, so the answer that says so is not a
    /// failure.
    async fn add_to_pool(
        &self,
        key: &CarrierKey,
        service_sid: &str,
        provider_number_id: &str,
    ) -> Result<(), TextError> {
        let response = self
            .http
            .post(format!(
                "{}/v1/Services/{service_sid}/PhoneNumbers",
                self.messaging_base
            ))
            .basic_auth(key.account(), Some(key.expose_secret()))
            .form(&[("PhoneNumberSid", provider_number_id)])
            .send()
            .await
            .map_err(unreachable)?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Ok(());
        }
        let _: PoolMemberDto = read(response).await?;
        Ok(())
    }

    /// The media URLs of one inbound text. Each one needs the same
    /// Basic auth [`TwilioTextTransport::fetch_media`] signs with.
    async fn media_urls(
        &self,
        key: &CarrierKey,
        message_sid: &str,
    ) -> Result<Vec<String>, TextError> {
        let listed: MediaPageDto = self
            .get(
                key,
                &self.account_url(key, &format!("Messages/{message_sid}/Media.json")),
                &[],
            )
            .await?;
        Ok(listed
            .media_list
            .into_iter()
            .map(|media| {
                self.account_url(key, &format!("Messages/{message_sid}/Media/{}", media.sid))
            })
            .collect())
    }
}

#[async_trait]
impl TextTransport for TwilioTextTransport {
    fn capabilities(&self) -> TextCapabilities {
        TextCapabilities {
            texting: true,
            inbound_media: true,
        }
    }

    async fn prepare(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        connection_config: &Value,
    ) -> Result<Prepared, TextError> {
        let (service_sid, config) = self.messaging_service(key, connection_config).await?;
        self.add_to_pool(key, &service_sid, &number.provider_number_id)
            .await?;
        Ok(Prepared {
            // The number's messaging object is the service it sends
            // through: `send` reads it back from the number.
            messaging_object_id: Some(service_sid),
            connection_config: config,
        })
    }

    async fn send(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        to_e164: &str,
        body: &str,
    ) -> Result<SentText, TextError> {
        let mut form = vec![
            ("From", number.e164.as_str()),
            ("To", to_e164),
            ("Body", body),
        ];
        // The Messaging Service carries the A2P campaign association,
        // so a prepared number sends through it beside `From`.
        if let Some(service_sid) = number.messaging_object_id.as_deref() {
            form.push(("MessagingServiceSid", service_sid));
        }
        let sent: MessageDto = self
            .post(key, &self.account_url(key, "Messages.json"), &form)
            .await?;
        Ok(SentText {
            carrier_id: sent.sid,
            segments: count(sent.num_segments.as_deref()),
        })
    }

    async fn delivery_status(
        &self,
        key: &CarrierKey,
        carrier_id: &str,
    ) -> Result<TextDeliveryStatus, TextError> {
        let message: MessageDto = self
            .get(
                key,
                &self.account_url(key, &format!("Messages/{carrier_id}.json")),
                &[],
            )
            .await?;
        Ok(delivery_status(&message))
    }

    async fn poll_inbound(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        cursor: Option<&str>,
    ) -> Result<(Vec<InboundText>, Option<String>), TextError> {
        let previous = cursor.and_then(Cursor::parse);
        let mut query = vec![
            ("To".to_string(), number.e164.clone()),
            ("PageSize".to_string(), PAGE_SIZE.to_string()),
        ];
        if let Some(previous) = &previous {
            // Twilio holds no `Direction` filter, so the list is
            // narrowed by time here and by direction below.
            query.push(("DateSent>=".to_string(), previous.iso.clone()));
        }

        let mut listed = Vec::new();
        let mut seen_sids = HashSet::new();
        let mut next: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page: MessagePageDto = match &next {
                None => {
                    self.get(key, &self.account_url(key, "Messages.json"), &query)
                        .await?
                }
                // A page shifts as new messages arrive, so the poll
                // follows the carrier's own next link and never asks
                // for a page number of its own.
                Some(uri) => {
                    self.get(key, &format!("{}{uri}", self.api_base), &[])
                        .await?
                }
            };
            for message in page.messages {
                if seen_sids.insert(message.sid.clone()) {
                    listed.push(message);
                }
            }
            match page.next_page_uri.filter(|uri| !uri.is_empty()) {
                Some(uri) => next = Some(uri),
                None => break,
            }
        }

        let mut fresh: Vec<(i64, String, MessageDto)> = listed
            .into_iter()
            .filter(|message| message.direction.as_deref() == Some("inbound"))
            .filter_map(|message| {
                let (at, iso) = sent_at(&message)?;
                Some((at, iso, message))
            })
            .filter(|(at, _, message)| match &previous {
                Some(previous) => {
                    *at > previous.at
                        || (*at == previous.at && !previous.seen.contains(&message.sid))
                }
                None => true,
            })
            .collect();
        // Twilio answers newest first; the collector wants the order
        // the texts arrived in.
        fresh.sort_by_key(|item| item.0);

        let next_cursor = match fresh.last() {
            None => cursor.map(str::to_string),
            Some((newest, iso, _)) => {
                let mut at_newest: Vec<String> = fresh
                    .iter()
                    .filter(|(at, _, _)| at == newest)
                    .map(|(_, _, message)| message.sid.clone())
                    .collect();
                if let Some(previous) = &previous
                    && previous.at == *newest
                {
                    at_newest.extend(previous.seen.iter().cloned());
                }
                at_newest.sort_unstable();
                at_newest.dedup();
                Some(format!("{iso}|{}", at_newest.join(",")))
            }
        };

        let mut texts = Vec::with_capacity(fresh.len());
        for (at, _, message) in fresh {
            let media_urls = match media_count(message.num_media.as_deref()) {
                0 => Vec::new(),
                _ => self.media_urls(key, &message.sid).await?,
            };
            texts.push(InboundText {
                carrier_id: message.sid,
                from_e164: message.from.unwrap_or_default(),
                to_e164: message.to.unwrap_or_else(|| number.e164.clone()),
                body: message.body.unwrap_or_default(),
                received_at: at,
                media_urls,
            });
        }
        Ok((texts, next_cursor))
    }

    async fn fetch_media(&self, key: &CarrierKey, url: &str) -> Result<Vec<u8>, TextError> {
        let response = self
            .http
            .get(url)
            .basic_auth(key.account(), Some(key.expose_secret()))
            .send()
            .await
            .map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(failure(response).await);
        }
        Ok(response.bytes().await.map_err(unreachable)?.to_vec())
    }
}

/// Where the inbound poll stopped: the moment of the newest text it
/// read, and the ids it read at that moment. Twilio filters by time
/// and two texts share one second often enough, so the ids at the
/// edge are the part that keeps a text from being read twice.
struct Cursor {
    /// The moment, in milliseconds, for the comparison.
    at: i64,
    /// The same moment as Twilio's own filter reads it.
    iso: String,
    seen: HashSet<String>,
}

impl Cursor {
    /// `<ISO-8601 moment>|<sid>,<sid>`. A cursor this client did not
    /// write reads as no cursor, so the poll starts over instead of
    /// refusing.
    fn parse(raw: &str) -> Option<Self> {
        let (moment, sids) = raw.split_once('|').unwrap_or((raw, ""));
        let (at, iso) = parse_moment(moment)?;
        Some(Self {
            at,
            iso,
            seen: sids
                .split(',')
                .filter(|sid| !sid.is_empty())
                .map(str::to_string)
                .collect(),
        })
    }
}

/// When one message was sent, in milliseconds and as Twilio's filter
/// reads it. A message the carrier has not sent yet carries the moment
/// it was made.
fn sent_at(message: &MessageDto) -> Option<(i64, String)> {
    message
        .date_sent
        .as_deref()
        .and_then(parse_moment)
        .or_else(|| message.date_created.as_deref().and_then(parse_moment))
}

/// Twilio writes RFC 2822 in an answer and reads ISO-8601 in a filter.
fn parse_moment(raw: &str) -> Option<(i64, String)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let moment = chrono::DateTime::parse_from_rfc2822(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .ok()?;
    Some((
        moment.timestamp_millis(),
        moment.to_utc().to_rfc3339_opts(SecondsFormat::Secs, true),
    ))
}

/// Twilio's thirteen message states, in the four of ADR-0020. A
/// message the carrier did not carry is `failed`, whichever of its
/// three words it used, and it carries the carrier's code.
fn delivery_status(message: &MessageDto) -> TextDeliveryStatus {
    match message.status.as_deref().unwrap_or_default() {
        "queued" | "accepted" | "scheduled" | "sending" | "receiving" => TextDeliveryStatus::Queued,
        "sent" | "partially_delivered" => TextDeliveryStatus::Sent,
        "delivered" | "received" | "read" => TextDeliveryStatus::Delivered,
        // `undelivered`, `failed` and `canceled` all mean the text did
        // not reach the handset.
        _ => TextDeliveryStatus::Failed {
            code: message.error_code.map(|code| code.to_string()),
            reason: message.error_message.clone(),
        },
    }
}

/// A segment count Twilio writes as a decimal string. An absent or
/// unreadable count is one segment.
fn count(raw: Option<&str>) -> u32 {
    raw.and_then(|raw| raw.trim().parse().ok()).unwrap_or(1)
}

/// How many media files the message carries. An absent count is none.
fn media_count(raw: Option<&str>) -> u32 {
    raw.and_then(|raw| raw.trim().parse().ok()).unwrap_or(0)
}

/// One answer, turned into a document or into a stable error.
async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, TextError> {
    if !response.status().is_success() {
        return Err(failure(response).await);
    }
    response
        .json()
        .await
        .map_err(|error| TextError::Unreachable(error.to_string()))
}

/// Why Twilio refused, in the words of the seam. A refusal the account
/// may retry is `rate_limited`; a region the account may not text is
/// `destination_not_enabled`; everything else keeps Twilio's own code.
async fn failure(response: reqwest::Response) -> TextError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let fault: FaultDto = response.json().await.unwrap_or_default();
    let code = fault
        .code
        .map(|code| code.to_string())
        .unwrap_or_else(|| status.as_u16().to_string());
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || code == RATE_LIMIT_CODE {
        return TextError::RateLimited { retry_after };
    }
    if code == GEO_PERMISSION_CODE {
        return TextError::DestinationNotEnabled { code };
    }
    TextError::Carrier {
        code,
        message: fault
            .message
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("refused").to_string()),
    }
}

fn unreachable(error: reqwest::Error) -> TextError {
    TextError::Unreachable(error.to_string())
}

/// The Workspace's Messaging Service sid on the carrier Connection,
/// when one is already there.
fn messaging_service_sid(connection_config: &Value) -> Option<String> {
    connection_config
        .get(TWILIO_MESSAGING_SERVICE_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|sid| !sid.is_empty())
        .map(str::to_string)
}

/// The Twilio answers this client reads. Only the fields Pagis needs
/// are named, so the carrier can add its own.
#[derive(Debug, Deserialize)]
struct ServiceDto {
    sid: String,
}

/// The sender pool answer. Its body says only that the number joined.
#[derive(Debug, Deserialize)]
struct PoolMemberDto {}

#[derive(Debug, Deserialize)]
struct MessagePageDto {
    #[serde(default)]
    messages: Vec<MessageDto>,
    #[serde(default)]
    next_page_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageDto {
    sid: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    date_sent: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
    #[serde(default)]
    num_segments: Option<String>,
    #[serde(default)]
    num_media: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MediaPageDto {
    #[serde(default)]
    media_list: Vec<MediaDto>,
}

#[derive(Debug, Deserialize)]
struct MediaDto {
    sid: String,
}

#[derive(Debug, Default, Deserialize)]
struct FaultDto {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}
