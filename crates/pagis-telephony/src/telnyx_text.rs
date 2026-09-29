//! Telnyx texting (ADR-0020): the messaging profile, the number's
//! attachment to it, the send, the delivery read and the inbound poll.
//!
//! Telnyx returns the body of an inbound text in the webhook only, so
//! the inbound half of this transport reads a queue instead of a
//! message list: the relay function writes each `message.received`
//! payload into a Telnyx KV namespace, and this client lists the keys
//! of the number, reads each value and deletes it. The queue is the
//! cursor, so the seam's cursor stays `None`.
//!
//! The Workspace-level objects are made one time and reused: one
//! messaging profile named `pagis-<workspace>` and one KV namespace of
//! the same name. Their ids travel back on [`Prepared`] and the caller
//! writes them on the carrier Connection under
//! [`TELNYX_MESSAGING_PROFILE_KEY`] and [`TELNYX_KV_NAMESPACE_KEY`].
//! The relay's public URL comes the other way, under
//! [`TELNYX_RELAY_URL_KEY`]: when it is in the Connection's config,
//! `prepare` points the profile's webhook at it, on a fresh profile
//! and on one that is already there.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{PhoneNumber, TextDeliveryStatus, UnixMillis, WorkspaceId, now_ms};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::catalog::CarrierKey;
use crate::emergency::region_of;
use crate::text::{InboundText, Prepared, SentText, TextCapabilities, TextError, TextTransport};

const API_BASE: &str = "https://api.telnyx.com/v2";
/// How long the client waits for one Telnyx request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How many inbound keys one poll reads. Telnyx allows 1 to 1000.
const POLL_LIMIT: u32 = 100;

/// The Connection `config` key that holds the Workspace's Telnyx
/// messaging profile id.
pub const TELNYX_MESSAGING_PROFILE_KEY: &str = "telnyx_messaging_profile_id";
/// The Connection `config` key that holds the Workspace's Telnyx KV
/// namespace id, the queue the relay writes inbound texts into.
pub const TELNYX_KV_NAMESPACE_KEY: &str = "telnyx_kv_namespace_id";
/// The Connection `config` key that holds the public URL of the
/// shipped relay function. `prepare` reads it and makes it the
/// messaging profile's webhook. The step that ships the relay and
/// writes this key is not built.
pub const TELNYX_RELAY_URL_KEY: &str = "telnyx_relay_url";

/// The Telnyx webhook version the relay function answers.
const WEBHOOK_API_VERSION: &str = "2";
/// The name every Pagis-made Workspace object starts with. A profile
/// with this prefix is reused instead of made twice.
const OBJECT_NAME_PREFIX: &str = "pagis-";
/// The Telnyx codes that mean the account may not text this
/// destination: the destination country is outside the messaging
/// profile's whitelist.
const DESTINATION_NOT_ENABLED_CODES: [&str; 1] = ["40309"];

/// The Telnyx text half of the carrier (ADR-0020). It signs with the
/// REST API key, as [`TelnyxNumberCatalog`] does.
///
/// [`TelnyxNumberCatalog`]: crate::TelnyxNumberCatalog
pub struct TelnyxTextTransport {
    http: reqwest::Client,
    base_url: String,
    /// The KV namespace id of each Workspace, as the last lookup read
    /// it. The seam hands `poll_inbound` no Connection config, so the
    /// namespace is found by its name and held here for the polls that
    /// follow.
    namespaces: Mutex<HashMap<String, String>>,
}

impl TelnyxTextTransport {
    pub fn new() -> Result<Self, TextError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| TextError::Unreachable(error.to_string()))?,
            base_url: API_BASE.to_string(),
            namespaces: Mutex::new(HashMap::new()),
        })
    }

    /// Point the client at another base URL. The contract tests use it;
    /// production uses [`TelnyxTextTransport::new`].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            namespaces: Mutex::new(HashMap::new()),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// The Workspace's messaging profile, reused when the account
    /// already holds one Pagis made.
    async fn find_profile(
        &self,
        key: &CarrierKey,
        name: &str,
    ) -> Result<Option<String>, TextError> {
        let response = self
            .http
            .get(self.url("/messaging_profiles"))
            .bearer_auth(key.expose_secret())
            .query(&[("filter[name]", OBJECT_NAME_PREFIX)])
            .send()
            .await
            .map_err(unreachable)?;
        let found: Envelope<Vec<NamedDto>> = read(response).await?;
        // The Workspace's own name only: another `pagis-` profile of
        // the account belongs to another Workspace.
        Ok(found
            .data
            .into_iter()
            .find(|profile| profile.name.as_deref() == Some(name))
            .map(|profile| profile.id))
    }

    async fn create_profile(
        &self,
        key: &CarrierKey,
        name: &str,
        destinations: &[String],
        webhook_url: Option<&str>,
    ) -> Result<String, TextError> {
        let mut body = json!({
            "name": name,
            "whitelisted_destinations": destinations,
        });
        if let Some(url) = webhook_url {
            body["webhook_url"] = json!(url);
            body["webhook_api_version"] = json!(WEBHOOK_API_VERSION);
        }
        let response = self
            .http
            .post(self.url("/messaging_profiles"))
            .bearer_auth(key.expose_secret())
            .json(&body)
            .send()
            .await
            .map_err(unreachable)?;
        let made: Envelope<NamedDto> = read(response).await?;
        Ok(made.data.id)
    }

    /// Point an existing profile's webhook at the relay. It runs on
    /// every prepare, so a relay URL that arrives after the profile
    /// exists reaches the profile on the next prepare.
    async fn point_webhook(
        &self,
        key: &CarrierKey,
        profile_id: &str,
        webhook_url: &str,
    ) -> Result<(), TextError> {
        let response = self
            .http
            .patch(self.url(&format!("/messaging_profiles/{profile_id}")))
            .bearer_auth(key.expose_secret())
            .json(&json!({
                "webhook_url": webhook_url,
                "webhook_api_version": WEBHOOK_API_VERSION,
            }))
            .send()
            .await
            .map_err(unreachable)?;
        let _: Value = read(response).await?;
        Ok(())
    }

    async fn attach_number(
        &self,
        key: &CarrierKey,
        provider_number_id: &str,
        profile_id: &str,
    ) -> Result<(), TextError> {
        let response = self
            .http
            .patch(self.url(&format!("/phone_numbers/{provider_number_id}/messaging")))
            .bearer_auth(key.expose_secret())
            .json(&json!({ "messaging_profile_id": profile_id }))
            .send()
            .await
            .map_err(unreachable)?;
        let _: Value = read(response).await?;
        Ok(())
    }

    /// The Workspace's KV namespace, or `None` when the account holds
    /// none of that name yet. A found id is held for the next poll.
    async fn find_namespace(
        &self,
        key: &CarrierKey,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<String>, TextError> {
        if let Some(id) = self.cached_namespace(workspace_id) {
            return Ok(Some(id));
        }
        let name = object_name(workspace_id);
        let response = self
            .http
            .get(self.url("/storage/kvs"))
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(unreachable)?;
        let held: Envelope<Vec<NamedDto>> = read(response).await?;
        let found = held
            .data
            .into_iter()
            .find(|namespace| namespace.name.as_deref() == Some(name.as_str()))
            .map(|namespace| namespace.id);
        if let Some(id) = &found {
            self.remember_namespace(workspace_id, id);
        }
        Ok(found)
    }

    async fn create_namespace(
        &self,
        key: &CarrierKey,
        workspace_id: &WorkspaceId,
    ) -> Result<String, TextError> {
        let response = self
            .http
            .post(self.url("/storage/kvs"))
            .bearer_auth(key.expose_secret())
            .json(&json!({ "name": object_name(workspace_id) }))
            .send()
            .await
            .map_err(unreachable)?;
        let made: Envelope<NamedDto> = read(response).await?;
        self.remember_namespace(workspace_id, &made.data.id);
        Ok(made.data.id)
    }

    fn cached_namespace(&self, workspace_id: &WorkspaceId) -> Option<String> {
        self.namespaces
            .lock()
            .expect("kv namespace cache lock")
            .get(workspace_id.as_str())
            .cloned()
    }

    fn remember_namespace(&self, workspace_id: &WorkspaceId, namespace_id: &str) {
        self.namespaces
            .lock()
            .expect("kv namespace cache lock")
            .insert(workspace_id.to_string(), namespace_id.to_string());
    }

    async fn read_key(
        &self,
        key: &CarrierKey,
        namespace_id: &str,
        kv_key: &str,
    ) -> Result<Value, TextError> {
        let response = self
            .http
            .get(self.url(&format!(
                "/storage/kvs/{namespace_id}/keys/{}",
                encode_key(kv_key)
            )))
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(unreachable)?;
        read(response).await
    }

    async fn delete_key(
        &self,
        key: &CarrierKey,
        namespace_id: &str,
        kv_key: &str,
    ) -> Result<(), TextError> {
        let response = self
            .http
            .delete(self.url(&format!(
                "/storage/kvs/{namespace_id}/keys/{}",
                encode_key(kv_key)
            )))
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(unreachable)?;
        checked(response).await?;
        Ok(())
    }
}

#[async_trait]
impl TextTransport for TelnyxTextTransport {
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
        let name = object_name(&number.workspace_id);
        let webhook_url = connection_config
            .get(TELNYX_RELAY_URL_KEY)
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty());
        let held = match connection_config
            .get(TELNYX_MESSAGING_PROFILE_KEY)
            .and_then(Value::as_str)
        {
            Some(id) if !id.is_empty() => Some(id.to_string()),
            _ => self.find_profile(key, &name).await?,
        };
        let profile_id = match held {
            Some(profile_id) => {
                if let Some(url) = webhook_url {
                    self.point_webhook(key, &profile_id, url).await?;
                }
                profile_id
            }
            None => {
                self.create_profile(
                    key,
                    &name,
                    &whitelisted_destinations(&number.e164),
                    webhook_url,
                )
                .await?
            }
        };
        self.attach_number(key, &number.provider_number_id, &profile_id)
            .await?;
        let namespace_id = match connection_config
            .get(TELNYX_KV_NAMESPACE_KEY)
            .and_then(Value::as_str)
        {
            Some(id) if !id.is_empty() => {
                self.remember_namespace(&number.workspace_id, id);
                id.to_string()
            }
            _ => match self.find_namespace(key, &number.workspace_id).await? {
                Some(id) => id,
                None => self.create_namespace(key, &number.workspace_id).await?,
            },
        };
        let mut config = Map::new();
        config.insert(
            TELNYX_MESSAGING_PROFILE_KEY.to_string(),
            json!(profile_id.clone()),
        );
        config.insert(TELNYX_KV_NAMESPACE_KEY.to_string(), json!(namespace_id));
        Ok(Prepared {
            messaging_object_id: Some(profile_id),
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
        // The number carries the messaging profile, so `from` is the
        // whole address Telnyx needs. The body goes whole, and Telnyx
        // picks the alphabet and counts the segments.
        let response = self
            .http
            .post(self.url("/messages"))
            .bearer_auth(key.expose_secret())
            .json(&json!({
                "from": number.e164,
                "to": to_e164,
                "text": body,
                "encoding": "auto",
            }))
            .send()
            .await
            .map_err(unreachable)?;
        let sent: Envelope<MessageDto> = read(response).await?;
        Ok(SentText {
            carrier_id: sent.data.id,
            segments: sent.data.parts.unwrap_or(1),
        })
    }

    async fn delivery_status(
        &self,
        key: &CarrierKey,
        carrier_id: &str,
    ) -> Result<TextDeliveryStatus, TextError> {
        let response = self
            .http
            .get(self.url(&format!("/messages/{carrier_id}")))
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(unreachable)?;
        let message: Envelope<MessageDto> = read(response).await?;
        Ok(delivery_status(&message.data))
    }

    async fn poll_inbound(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        _cursor: Option<&str>,
    ) -> Result<(Vec<InboundText>, Option<String>), TextError> {
        // No namespace means no relay yet: nothing arrived, and
        // nothing failed either.
        let Some(namespace_id) = self.find_namespace(key, &number.workspace_id).await? else {
            return Ok((Vec::new(), None));
        };
        let prefix = key_prefix(&number.e164);
        let response = self
            .http
            .get(self.url(&format!("/storage/kvs/{namespace_id}/keys")))
            .bearer_auth(key.expose_secret())
            .query(&[("prefix", prefix), ("limit", POLL_LIMIT.to_string())])
            .send()
            .await
            .map_err(unreachable)?;
        let listed: Envelope<Vec<KeyDto>> = read(response).await?;
        // The key carries the millisecond the text arrived, so key
        // order is arrival order.
        let mut keys: Vec<String> = listed.data.into_iter().map(|entry| entry.key).collect();
        keys.sort();
        let mut texts = Vec::with_capacity(keys.len());
        for kv_key in keys {
            let payload = self.read_key(key, &namespace_id, &kv_key).await?;
            if let Some(text) = inbound_text(&payload) {
                texts.push(text);
            }
            // A key that was read, and a key this client cannot read,
            // both leave the queue: a payload Pagis does not
            // understand would block every poll after it.
            self.delete_key(key, &namespace_id, &kv_key).await?;
        }
        Ok((texts, None))
    }

    async fn fetch_media(&self, key: &CarrierKey, url: &str) -> Result<Vec<u8>, TextError> {
        let response = self
            .http
            .get(url)
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(unreachable)?;
        let response = checked(response).await?;
        Ok(response
            .bytes()
            .await
            .map_err(|error| TextError::Unreachable(error.to_string()))?
            .to_vec())
    }
}

/// The name of the Workspace's messaging profile and KV namespace. A
/// KV name takes lowercase letters, digits and hyphens only.
fn object_name(workspace_id: &WorkspaceId) -> String {
    format!(
        "{OBJECT_NAME_PREFIX}{}",
        workspace_id.as_str().to_lowercase()
    )
}

/// The KV prefix of one number's queue: `text/<digits>/`, as the relay
/// function writes it. A KV key takes no `+`.
fn key_prefix(e164: &str) -> String {
    let digits: String = e164.chars().filter(char::is_ascii_digit).collect();
    format!("text/{digits}/")
}

/// One KV key in a URL. The key alphabet is `a-z A-Z 0-9 - _ / = .`,
/// and the API reads the whole key as one path segment.
fn encode_key(kv_key: &str) -> String {
    kv_key
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('=', "%3D")
}

/// The countries the messaging profile may text: the country of the
/// number. The record holds no region, because the E.164 number says
/// where the line is, and [`region_of`] reads it the same way for the
/// emergency guard. The North American plan is one calling code over
/// two countries the catalog sells, so a `+1` line texts both.
fn whitelisted_destinations(e164: &str) -> Vec<String> {
    match region_of(e164) {
        Some("US") => vec!["US".to_string(), "CA".to_string()],
        Some(region) => vec![region.to_string()],
        // A calling code the table does not know. Telnyx needs one
        // destination, and US is the market the catalog sells.
        None => vec!["US".to_string()],
    }
}

/// The four states of ADR-0020, from the recipient's Telnyx status.
fn delivery_status(message: &MessageDto) -> TextDeliveryStatus {
    let status = message
        .to
        .first()
        .and_then(|to| to.status.as_deref())
        .unwrap_or_default();
    match status {
        "queued" | "sending" => TextDeliveryStatus::Queued,
        "delivered" => TextDeliveryStatus::Delivered,
        "expired" | "sending_failed" | "delivery_failed" => {
            let error = message.errors.first();
            TextDeliveryStatus::Failed {
                code: error.and_then(|error| error.code.clone()),
                reason: error.and_then(|error| error.detail.clone().or(error.title.clone())),
            }
        }
        // `sent`, `delivery_unconfirmed`, and any state Telnyx adds:
        // the carrier took the text and reported no failure.
        _ => TextDeliveryStatus::Sent,
    }
}

/// One `message.received` payload, as the relay wrote it into KV.
fn inbound_text(payload: &Value) -> Option<InboundText> {
    let received: ReceivedDto = serde_json::from_value(payload.clone()).ok()?;
    let payload = received.data.payload;
    Some(InboundText {
        carrier_id: payload.id?,
        from_e164: payload.from.and_then(|from| from.phone_number)?,
        to_e164: payload
            .to
            .into_iter()
            .find_map(|to| to.phone_number)
            .unwrap_or_default(),
        body: payload.text.unwrap_or_default(),
        received_at: payload
            .received_at
            .as_deref()
            .and_then(millis_of)
            .unwrap_or_else(now_ms),
        media_urls: payload
            .media
            .into_iter()
            .filter_map(|media| media.url)
            .collect(),
    })
}

fn millis_of(timestamp: &str) -> Option<UnixMillis> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// A Telnyx answer, turned into a document or into a stable failure.
async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, TextError> {
    let response = checked(response).await?;
    response
        .json()
        .await
        .map_err(|error| TextError::Unreachable(error.to_string()))
}

/// The status line, turned into the failure the seam names. The
/// carrier's own code travels inside it.
async fn checked(response: reqwest::Response) -> Result<reqwest::Response, TextError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        return Err(TextError::RateLimited { retry_after });
    }
    let errors: ErrorsDto = response.json().await.unwrap_or_default();
    let first = errors.errors.into_iter().next().unwrap_or_default();
    let code = first.code.unwrap_or_else(|| status.as_u16().to_string());
    if DESTINATION_NOT_ENABLED_CODES.contains(&code.as_str()) {
        return Err(TextError::DestinationNotEnabled { code });
    }
    let message = first
        .detail
        .or(first.title)
        .unwrap_or_else(|| status.to_string());
    Err(TextError::Carrier { code, message })
}

fn unreachable(error: reqwest::Error) -> TextError {
    TextError::Unreachable(error.to_string())
}

/// A Telnyx answer. Only the fields Pagis reads are named; the carrier
/// can add its own without breaking this client.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct NamedDto {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KeyDto {
    key: String,
}

#[derive(Debug, Deserialize)]
struct MessageDto {
    id: String,
    #[serde(default)]
    parts: Option<u32>,
    #[serde(default)]
    to: Vec<RecipientDto>,
    #[serde(default)]
    errors: Vec<ErrorDto>,
}

#[derive(Debug, Deserialize)]
struct RecipientDto {
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorsDto {
    #[serde(default)]
    errors: Vec<ErrorDto>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorDto {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReceivedDto {
    data: ReceivedDataDto,
}

#[derive(Debug, Deserialize)]
struct ReceivedDataDto {
    payload: ReceivedPayloadDto,
}

#[derive(Debug, Deserialize)]
struct ReceivedPayloadDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    from: Option<PartyDto>,
    #[serde(default)]
    to: Vec<PartyDto>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    received_at: Option<String>,
    #[serde(default)]
    media: Vec<MediaDto>,
}

#[derive(Debug, Deserialize)]
struct PartyDto {
    #[serde(default)]
    phone_number: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MediaDto {
    #[serde(default)]
    url: Option<String>,
}
