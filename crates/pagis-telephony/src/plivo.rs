//! Plivo v1 REST, the number half of the carrier (ADR-0020). This
//! client holds the Auth ID and the Auth Token and never touches a
//! call: signaling and media are the SIP credential's work, in the
//! endpoint task.
//!
//! Plivo names a number by the number itself, so a purchase is a POST
//! to the number's own path and the handle Pagis stores is the E.164
//! digits. The answer is immediate; there is no order to poll.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberSearch,
    PurchasedNumber,
};

const API_BASE: &str = "https://api.plivo.com";
/// How long the client waits for one Plivo request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Plivo prices every number in US dollars and does not name the
/// currency in the answer.
const CURRENCY: &str = "USD";

pub struct PlivoNumberCatalog {
    http: reqwest::Client,
    base_url: String,
}

impl PlivoNumberCatalog {
    pub fn new() -> Result<Self, CatalogError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|_| CatalogError(CatalogErrorCode::TemporarilyUnavailable))?,
            base_url: API_BASE.to_string(),
        })
    }

    /// Point the client at another base URL. The contract tests use it;
    /// production uses [`PlivoNumberCatalog::new`].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// One request, signed with HTTP Basic: the Auth ID is the user and
    /// the Auth Token is the password.
    fn request(
        &self,
        method: reqwest::Method,
        key: &CarrierKey,
        path: &str,
    ) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .basic_auth(key.account(), Some(key.expose_secret()))
    }
}

/// A Plivo list answer. Only the fields Pagis reads are named; the
/// carrier can add its own without breaking this client.
#[derive(Debug, Deserialize)]
struct ObjectList<T> {
    #[serde(default = "Vec::new")]
    objects: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct AvailableNumberDto {
    /// The E.164 digits, with no `+`.
    number: String,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    monthly_rental_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeldNumberDto {
    number: String,
}

#[derive(Debug, Deserialize)]
struct BuyAnswer {
    #[serde(default = "Vec::new")]
    numbers: Vec<BoughtNumberDto>,
}

#[derive(Debug, Deserialize)]
struct BoughtNumberDto {
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

impl From<AvailableNumberDto> for AvailableNumber {
    fn from(dto: AvailableNumberDto) -> Self {
        let region = match (dto.city, dto.region) {
            (Some(city), Some(region)) => Some(format!("{city}, {region}")),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        };
        Self {
            e164: format!("+{}", dto.number),
            region,
            monthly_cost: dto.monthly_rental_rate,
            currency: Some(CURRENCY.to_string()),
        }
    }
}

/// The E.164 number as Plivo names it: the digits, with no `+`.
fn digits(e164: &str) -> &str {
    e164.trim_start_matches('+')
}

#[async_trait]
impl NumberCatalog for PlivoNumberCatalog {
    async fn search(
        &self,
        key: &CarrierKey,
        search: &NumberSearch,
    ) -> Result<Vec<AvailableNumber>, CatalogError> {
        let mut query = vec![
            ("country_iso".to_string(), search.country.clone()),
            ("type".to_string(), "local".to_string()),
            // Voice-capable numbers only (ADR-0018).
            ("services".to_string(), "voice".to_string()),
            ("limit".to_string(), search.limit.to_string()),
        ];
        if let Some(area_code) = &search.area_code {
            query.push(("pattern".to_string(), area_code.clone()));
        }
        if let Some(locality) = &search.locality {
            query.push(("city".to_string(), locality.clone()));
        }
        let response = self
            .request(
                reqwest::Method::GET,
                key,
                &format!("/v1/Account/{}/PhoneNumber/", key.account()),
            )
            .query(&query)
            .send()
            .await
            .map_err(transport_error)?;
        let found: ObjectList<AvailableNumberDto> = read(response).await?;
        Ok(found
            .objects
            .into_iter()
            .map(AvailableNumber::from)
            .collect())
    }

    async fn buy(
        &self,
        key: &CarrierKey,
        e164: &str,
        _idempotency_key: &str,
    ) -> Result<PurchasedNumber, CatalogError> {
        // Plivo has no idempotency field. The desk's purchase intent is
        // what keeps Pagis from buying the same number twice, and
        // `find_purchased` settles a purchase the daemon lost.
        let response = self
            .request(
                reqwest::Method::POST,
                key,
                &format!(
                    "/v1/Account/{}/PhoneNumber/{}/",
                    key.account(),
                    digits(e164)
                ),
            )
            // Plivo takes an optional application id here. Pagis
            // attaches none: the call path is the SIP credential's.
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(transport_error)?;
        let bought: BuyAnswer = read(response).await?;
        let number = bought
            .numbers
            .first()
            .ok_or(CatalogError(CatalogErrorCode::Unreadable))?;
        // A number that waits for activation documents is bought. Any
        // other state is a number the carrier did not sell.
        let sold = matches!(
            number
                .status
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("success") | Some("pending")
        );
        if !sold {
            return Err(CatalogError(CatalogErrorCode::NumberUnavailable));
        }
        let digits = number
            .number
            .clone()
            .unwrap_or_else(|| digits(e164).to_string());
        Ok(PurchasedNumber {
            e164: format!("+{digits}"),
            provider_number_id: digits,
        })
    }

    async fn find_purchased(
        &self,
        key: &CarrierKey,
        e164: &str,
    ) -> Result<Option<PurchasedNumber>, CatalogError> {
        let wanted = digits(e164);
        let response = self
            .request(
                reqwest::Method::GET,
                key,
                &format!("/v1/Account/{}/Number/", key.account()),
            )
            .query(&[("number_startswith", wanted)])
            .send()
            .await
            .map_err(transport_error)?;
        let held: ObjectList<HeldNumberDto> = read(response).await?;
        // Plivo matches a prefix, so the exact number is picked here.
        Ok(held
            .objects
            .into_iter()
            .find(|number| number.number == wanted)
            .map(|number| PurchasedNumber {
                e164: format!("+{}", number.number),
                provider_number_id: number.number,
            }))
    }

    async fn release(
        &self,
        key: &CarrierKey,
        provider_number_id: &str,
    ) -> Result<(), CatalogError> {
        let response = self
            .request(
                reqwest::Method::DELETE,
                key,
                &format!(
                    "/v1/Account/{}/Number/{}/",
                    key.account(),
                    digits(provider_number_id)
                ),
            )
            .send()
            .await
            .map_err(transport_error)?;
        // A number the carrier does not hold is released already.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        status_code(response.status()).map_or(Ok(()), Err)
    }

    /// Plivo has no per-endpoint setting for encrypted media, so there
    /// is nothing to prepare: the endpoint answers what the offer asks
    /// for, or the call ends with `media_failed` and says so.
    async fn prepare_sip_connection(
        &self,
        _key: &CarrierKey,
        _username: &str,
    ) -> Result<bool, CatalogError> {
        Ok(false)
    }
}

/// One answer, turned into either a document or a stable code. No
/// upstream text reaches the caller.
async fn read<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, CatalogError> {
    if let Some(error) = status_code(response.status()) {
        return Err(error);
    }
    response
        .json()
        .await
        .map_err(|_| CatalogError(CatalogErrorCode::Unreadable))
}

/// The stable code one HTTP status maps onto, or `None` when the
/// carrier did what it was asked.
fn status_code(status: reqwest::StatusCode) -> Option<CatalogError> {
    if status.is_success() {
        return None;
    }
    Some(CatalogError(match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            CatalogErrorCode::Unauthorized
        }
        other if other.is_client_error() => CatalogErrorCode::NumberUnavailable,
        _ => CatalogErrorCode::TemporarilyUnavailable,
    }))
}

fn transport_error(_error: reqwest::Error) -> CatalogError {
    CatalogError(CatalogErrorCode::TemporarilyUnavailable)
}
