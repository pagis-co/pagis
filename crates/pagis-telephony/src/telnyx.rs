//! Telnyx v2 REST, the number half of the carrier (ADR-0020). This
//! client holds the API key and never touches a call: signaling and
//! media are the SIP credential's work, in the endpoint task.
//!
//! A number order is asynchronous at Telnyx, so `buy` places the order
//! and then reads the account's own list until the number appears. When
//! it does not appear in time, the purchase intent stays pending and
//! reconciliation settles it later. Pagis never orders twice.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberSearch,
    PurchasedNumber,
};

const API_BASE: &str = "https://api.telnyx.com/v2";
/// How long the client waits for one Telnyx request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How many times a fresh order is looked for before the purchase is
/// left to reconciliation.
const ORDER_POLLS: usize = 5;
const ORDER_POLL_DELAY: Duration = Duration::from_secs(2);

pub struct TelnyxNumberCatalog {
    http: reqwest::Client,
    base_url: String,
    order_poll_delay: Duration,
}

impl TelnyxNumberCatalog {
    pub fn new() -> Result<Self, CatalogError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|_| CatalogError(CatalogErrorCode::TemporarilyUnavailable))?,
            base_url: API_BASE.to_string(),
            order_poll_delay: ORDER_POLL_DELAY,
        })
    }

    /// Point the client at another base URL and shorten the order wait.
    /// The contract tests use it; production uses [`TelnyxNumberCatalog::new`].
    pub fn with_base_url(base_url: impl Into<String>, order_poll_delay: Duration) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            order_poll_delay,
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        key: &CarrierKey,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, CatalogError> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(key.expose_secret())
            .query(query)
            .send()
            .await
            .map_err(transport_error)?;
        read(response).await
    }
}

/// A Telnyx list answer. Only the fields Pagis reads are named; the
/// carrier can add its own without breaking this client.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct AvailableNumberDto {
    phone_number: String,
    #[serde(default)]
    cost_information: Option<CostDto>,
    #[serde(default)]
    region_information: Vec<RegionDto>,
}

#[derive(Debug, Deserialize)]
struct CostDto {
    #[serde(default)]
    monthly_cost: Option<String>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegionDto {
    #[serde(default)]
    region_type: Option<String>,
    #[serde(default)]
    region_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PhoneNumberDto {
    id: String,
    phone_number: String,
}

#[derive(Debug, Deserialize)]
struct CredentialConnectionDto {
    id: String,
    #[serde(default)]
    encrypted_media: Option<String>,
    #[serde(default)]
    inbound: Option<InboundSettingsDto>,
}

#[derive(Debug, Deserialize)]
struct InboundSettingsDto {
    #[serde(default)]
    dnis_number_format: Option<String>,
}

/// The value of `encrypted_media` that makes Telnyx answer SRTP.
const ENCRYPTED_MEDIA_SRTP: &str = "SRTP";
/// The value of `inbound.dnis_number_format` that makes Telnyx send the
/// dialed number in E.164, with the `+`. The line routes an inbound
/// call by that number, so it must not depend on the account default.
const DNIS_NUMBER_FORMAT_E164: &str = "+e164";

impl From<AvailableNumberDto> for AvailableNumber {
    fn from(dto: AvailableNumberDto) -> Self {
        let region = |wanted: &str| {
            dto.region_information
                .iter()
                .find(|region| region.region_type.as_deref() == Some(wanted))
                .and_then(|region| region.region_name.clone())
        };
        let locality = region("locality");
        let state = region("state");
        let region = match (locality, state) {
            (Some(locality), Some(state)) => Some(format!("{locality}, {state}")),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        };
        Self {
            e164: dto.phone_number,
            region,
            monthly_cost: dto
                .cost_information
                .as_ref()
                .and_then(|cost| cost.monthly_cost.clone()),
            currency: dto
                .cost_information
                .as_ref()
                .and_then(|cost| cost.currency.clone()),
        }
    }
}

#[async_trait]
impl NumberCatalog for TelnyxNumberCatalog {
    async fn search(
        &self,
        key: &CarrierKey,
        search: &NumberSearch,
    ) -> Result<Vec<AvailableNumber>, CatalogError> {
        let mut query = vec![
            ("filter[country_code]".to_string(), search.country.clone()),
            ("filter[phone_number_type]".to_string(), "local".to_string()),
            // Voice-capable numbers only (ADR-0018).
            ("filter[features][]".to_string(), "voice".to_string()),
            ("filter[limit]".to_string(), search.limit.to_string()),
        ];
        if let Some(area_code) = &search.area_code {
            query.push((
                "filter[national_destination_code]".to_string(),
                area_code.clone(),
            ));
        }
        if let Some(locality) = &search.locality {
            query.push(("filter[locality]".to_string(), locality.clone()));
        }
        let found: Envelope<Vec<AvailableNumberDto>> =
            self.get(key, "/available_phone_numbers", &query).await?;
        Ok(found.data.into_iter().map(AvailableNumber::from).collect())
    }

    async fn buy(
        &self,
        key: &CarrierKey,
        e164: &str,
        idempotency_key: &str,
    ) -> Result<PurchasedNumber, CatalogError> {
        let response = self
            .http
            .post(format!("{}/number_orders", self.base_url))
            .bearer_auth(key.expose_secret())
            .json(&serde_json::json!({
                "phone_numbers": [{ "phone_number": e164 }],
                // The intent id travels to the carrier, so one order is
                // recognisable in the carrier's own records.
                "customer_reference": idempotency_key,
            }))
            .send()
            .await
            .map_err(transport_error)?;
        let _: Envelope<serde_json::Value> = read(response).await?;
        // The order is asynchronous. The account's own list is the
        // answer, and the number carries the id Pagis stores.
        for attempt in 0..ORDER_POLLS {
            if attempt > 0 {
                tokio::time::sleep(self.order_poll_delay).await;
            }
            if let Some(purchased) = self.find_purchased(key, e164).await? {
                return Ok(purchased);
            }
        }
        Err(CatalogError(CatalogErrorCode::TemporarilyUnavailable))
    }

    async fn find_purchased(
        &self,
        key: &CarrierKey,
        e164: &str,
    ) -> Result<Option<PurchasedNumber>, CatalogError> {
        let held: Envelope<Vec<PhoneNumberDto>> = self
            .get(
                key,
                "/phone_numbers",
                &[("filter[phone_number]".to_string(), e164.to_string())],
            )
            .await?;
        Ok(held
            .data
            .into_iter()
            .find(|number| number.phone_number == e164)
            .map(|number| PurchasedNumber {
                e164: number.phone_number,
                provider_number_id: number.id,
            }))
    }

    async fn release(
        &self,
        key: &CarrierKey,
        provider_number_id: &str,
    ) -> Result<(), CatalogError> {
        let response = self
            .http
            .delete(format!(
                "{}/phone_numbers/{provider_number_id}",
                self.base_url
            ))
            .bearer_auth(key.expose_secret())
            .send()
            .await
            .map_err(transport_error)?;
        // A number the carrier does not hold is released already.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let _: serde_json::Value = read(response).await?;
        Ok(())
    }

    async fn prepare_sip_connection(
        &self,
        key: &CarrierKey,
        username: &str,
    ) -> Result<bool, CatalogError> {
        let found: Envelope<Vec<CredentialConnectionDto>> = self
            .get(
                key,
                "/credential_connections",
                &[("filter[user_name]".to_string(), username.to_string())],
            )
            .await?;
        let connection = found
            .data
            .into_iter()
            .next()
            .ok_or(CatalogError(CatalogErrorCode::NumberUnavailable))?;
        let dnis_number_format = connection
            .inbound
            .as_ref()
            .and_then(|inbound| inbound.dnis_number_format.as_deref());
        if connection.encrypted_media.as_deref() == Some(ENCRYPTED_MEDIA_SRTP)
            && dnis_number_format == Some(DNIS_NUMBER_FORMAT_E164)
        {
            return Ok(false);
        }
        let response = self
            .http
            .patch(format!(
                "{}/credential_connections/{}",
                self.base_url, connection.id
            ))
            .bearer_auth(key.expose_secret())
            .json(&serde_json::json!({
                "encrypted_media": ENCRYPTED_MEDIA_SRTP,
                "inbound": { "dnis_number_format": DNIS_NUMBER_FORMAT_E164 },
            }))
            .send()
            .await
            .map_err(transport_error)?;
        let _: serde_json::Value = read(response).await?;
        Ok(true)
    }
}

/// One answer, turned into either a document or a stable code. No
/// upstream text reaches the caller.
async fn read<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, CatalogError> {
    let status = response.status();
    if !status.is_success() {
        return Err(CatalogError(match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                CatalogErrorCode::Unauthorized
            }
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::CONFLICT => {
                CatalogErrorCode::NumberUnavailable
            }
            other if other.is_client_error() => CatalogErrorCode::NumberUnavailable,
            _ => CatalogErrorCode::TemporarilyUnavailable,
        }));
    }
    response
        .json()
        .await
        .map_err(|_| CatalogError(CatalogErrorCode::Unreadable))
}

fn transport_error(_error: reqwest::Error) -> CatalogError {
    CatalogError(CatalogErrorCode::TemporarilyUnavailable)
}
