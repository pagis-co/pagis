//! Twilio 2010-04-01 REST, the number half of the carrier (ADR-0020).
//! This client holds the API credential and never touches a call:
//! signaling and media are the SIP credential's work, in the endpoint
//! task.
//!
//! Twilio signs with HTTP Basic. The Account SID is the account of the
//! [`CarrierKey`], and it also names the account in every path; the
//! secret is the Auth Token. A purchase is synchronous here, so `buy`
//! reads the carrier handle from the answer and polls nothing.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberSearch,
    PurchasedNumber,
};

const API_BASE: &str = "https://api.twilio.com";
/// How long the client waits for one Twilio request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct TwilioNumberCatalog {
    http: reqwest::Client,
    base_url: String,
}

impl TwilioNumberCatalog {
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
    /// production uses [`TwilioNumberCatalog::new`].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// The path of one resource of the account the key names.
    fn account_path(&self, key: &CarrierKey, tail: &str) -> String {
        format!(
            "{}/2010-04-01/Accounts/{}/{tail}",
            self.base_url,
            key.account()
        )
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        key: &CarrierKey,
        tail: &str,
        query: &[(String, String)],
    ) -> Result<T, CatalogError> {
        let response = self
            .http
            .get(self.account_path(key, tail))
            .basic_auth(key.account(), Some(key.expose_secret()))
            .query(query)
            .send()
            .await
            .map_err(transport_error)?;
        read(response).await
    }
}

/// A Twilio search answer. Only the fields Pagis reads are named; the
/// carrier can add its own without breaking this client.
#[derive(Debug, Deserialize)]
struct AvailableAnswer {
    #[serde(default)]
    available_phone_numbers: Vec<AvailableNumberDto>,
}

#[derive(Debug, Deserialize)]
struct AvailableNumberDto {
    phone_number: String,
    #[serde(default)]
    locality: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeldAnswer {
    #[serde(default)]
    incoming_phone_numbers: Vec<PhoneNumberDto>,
}

#[derive(Debug, Deserialize)]
struct PhoneNumberDto {
    sid: String,
    phone_number: String,
}

impl From<AvailableNumberDto> for AvailableNumber {
    fn from(dto: AvailableNumberDto) -> Self {
        let region = match (dto.locality, dto.region) {
            (Some(locality), Some(state)) => Some(format!("{locality}, {state}")),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        };
        Self {
            e164: dto.phone_number,
            region,
            // Twilio states no price per number in the search answer.
            monthly_cost: None,
            currency: None,
        }
    }
}

impl From<PhoneNumberDto> for PurchasedNumber {
    fn from(dto: PhoneNumberDto) -> Self {
        Self {
            e164: dto.phone_number,
            provider_number_id: dto.sid,
        }
    }
}

#[async_trait]
impl NumberCatalog for TwilioNumberCatalog {
    async fn search(
        &self,
        key: &CarrierKey,
        search: &NumberSearch,
    ) -> Result<Vec<AvailableNumber>, CatalogError> {
        let mut query = vec![
            // Voice-capable numbers only (ADR-0018).
            ("VoiceEnabled".to_string(), "true".to_string()),
            ("PageSize".to_string(), search.limit.to_string()),
        ];
        if let Some(area_code) = &search.area_code {
            query.push(("AreaCode".to_string(), area_code.clone()));
        }
        if let Some(locality) = &search.locality {
            query.push(("InLocality".to_string(), locality.clone()));
        }
        let found: AvailableAnswer = self
            .get(
                key,
                &format!("AvailablePhoneNumbers/{}/Local.json", search.country),
                &query,
            )
            .await?;
        Ok(found
            .available_phone_numbers
            .into_iter()
            .map(AvailableNumber::from)
            .collect())
    }

    async fn buy(
        &self,
        key: &CarrierKey,
        e164: &str,
        idempotency_key: &str,
    ) -> Result<PurchasedNumber, CatalogError> {
        let response = self
            .http
            .post(self.account_path(key, "IncomingPhoneNumbers.json"))
            .basic_auth(key.account(), Some(key.expose_secret()))
            .form(&[
                ("PhoneNumber", e164),
                // The intent id travels to the carrier, so one purchase
                // is recognisable in the carrier's own records.
                ("FriendlyName", idempotency_key),
            ])
            .send()
            .await
            .map_err(transport_error)?;
        // The purchase is synchronous, so the answer carries the handle
        // Pagis stores.
        let bought: PhoneNumberDto = read(response).await?;
        Ok(bought.into())
    }

    async fn find_purchased(
        &self,
        key: &CarrierKey,
        e164: &str,
    ) -> Result<Option<PurchasedNumber>, CatalogError> {
        let held: HeldAnswer = self
            .get(
                key,
                "IncomingPhoneNumbers.json",
                &[("PhoneNumber".to_string(), e164.to_string())],
            )
            .await?;
        Ok(held
            .incoming_phone_numbers
            .into_iter()
            .find(|number| number.phone_number == e164)
            .map(PurchasedNumber::from))
    }

    async fn release(
        &self,
        key: &CarrierKey,
        provider_number_id: &str,
    ) -> Result<(), CatalogError> {
        let response = self
            .http
            .delete(self.account_path(
                key,
                &format!("IncomingPhoneNumbers/{provider_number_id}.json"),
            ))
            .basic_auth(key.account(), Some(key.expose_secret()))
            .send()
            .await
            .map_err(transport_error)?;
        // A number the carrier does not hold is released already.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let status = response.status();
        if !status.is_success() {
            return Err(CatalogError(code_of(status)));
        }
        Ok(())
    }

    /// Twilio keys SRTP by the SIP Domain's secure-media setting, which
    /// the user sets in the console; this client has no handle on it.
    /// Nothing is prepared, and a cleartext answer ends the call with
    /// `media_failed`, which names the setting.
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
    let status = response.status();
    if !status.is_success() {
        return Err(CatalogError(code_of(status)));
    }
    response
        .json()
        .await
        .map_err(|_| CatalogError(CatalogErrorCode::Unreadable))
}

fn code_of(status: reqwest::StatusCode) -> CatalogErrorCode {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            CatalogErrorCode::Unauthorized
        }
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::CONFLICT => {
            CatalogErrorCode::NumberUnavailable
        }
        other if other.is_client_error() => CatalogErrorCode::NumberUnavailable,
        _ => CatalogErrorCode::TemporarilyUnavailable,
    }
}

fn transport_error(_error: reqwest::Error) -> CatalogError {
    CatalogError(CatalogErrorCode::TemporarilyUnavailable)
}
