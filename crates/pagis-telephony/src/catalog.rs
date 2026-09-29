//! The number seam (ADR-0020): search, buy and release a number. It
//! uses the carrier's REST API key, and it never touches a call. The
//! call seam, `CallTransport`, is the other half and belongs to the
//! endpoint task.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

/// The carrier's REST credential: an account and a secret. Telnyx
/// signs with a bearer key and leaves the account empty; Twilio and
/// Plivo sign with an account id and a secret, and the account names
/// the account in the path. The secret has no accessor that prints it
/// and no `Debug` that shows it, so it cannot reach a log line by
/// accident.
#[derive(Clone, PartialEq, Eq)]
pub struct CarrierKey {
    account: String,
    secret: String,
}

impl CarrierKey {
    pub fn new(account: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            secret: secret.into(),
        }
    }

    /// The account id, which is not secret. Empty for a carrier whose
    /// secret is the whole credential.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The one reader of the secret. Only a provider client calls it.
    pub fn expose_secret(&self) -> &str {
        &self.secret
    }
}

impl std::fmt::Debug for CarrierKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarrierKey")
            .field("account", &self.account)
            .field("secret", &"redacted")
            .finish()
    }
}

/// What the user asks the carrier for (ADR-0018): a country and an area
/// code or a locality. Pagis asks for voice-capable numbers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberSearch {
    /// The ISO 3166-1 alpha-2 country, e.g. `US`.
    pub country: String,
    /// The area code, e.g. `415`.
    pub area_code: Option<String>,
    /// The city or town, e.g. `San Francisco`.
    pub locality: Option<String>,
    pub limit: u32,
}

/// One number the carrier offers, with the price it charges today. The
/// price is shown and never stored: a price recorded at purchase goes
/// stale (ADR-0018).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableNumber {
    pub e164: String,
    /// What the carrier calls the place, e.g. `San Francisco, CA`.
    pub region: Option<String>,
    /// The monthly charge, as the carrier states it, e.g. `1.00`.
    pub monthly_cost: Option<String>,
    /// The currency of `monthly_cost`, e.g. `USD`.
    pub currency: Option<String>,
}

/// A number the carrier sold. The pair is what Pagis stores: the
/// model-visible `e164` and the opaque carrier handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchasedNumber {
    pub e164: String,
    pub provider_number_id: String,
}

/// Why the carrier did not do what it was asked. The codes are stable,
/// because they reach the user and the tests; no upstream text is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogErrorCode {
    /// The carrier refused the API key.
    Unauthorized,
    /// The number is gone, or the carrier will not sell it here.
    NumberUnavailable,
    /// The carrier did not answer, or answered with a failure it calls
    /// temporary.
    TemporarilyUnavailable,
    /// The carrier answered with something this client cannot read.
    Unreadable,
}

impl CatalogErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogErrorCode::Unauthorized => "unauthorized",
            CatalogErrorCode::NumberUnavailable => "number_unavailable",
            CatalogErrorCode::TemporarilyUnavailable => "temporarily_unavailable",
            CatalogErrorCode::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the carrier did not complete this: {}", .0.as_str())]
pub struct CatalogError(pub CatalogErrorCode);

/// Search, buy and release a number at the carrier (ADR-0020).
///
/// `buy` carries the idempotency key of the purchase intent, and
/// `find_purchased` is how a daemon that restarted mid-purchase learns
/// the answer. Between them, Pagis never buys the same number twice.
#[async_trait]
pub trait NumberCatalog: Send + Sync {
    /// Voice-capable numbers the carrier offers now, with today's
    /// price.
    async fn search(
        &self,
        key: &CarrierKey,
        search: &NumberSearch,
    ) -> Result<Vec<AvailableNumber>, CatalogError>;

    /// Buy one number. `idempotency_key` is the purchase intent's id.
    async fn buy(
        &self,
        key: &CarrierKey,
        e164: &str,
        idempotency_key: &str,
    ) -> Result<PurchasedNumber, CatalogError>;

    /// Whether the carrier already holds this number for the account.
    /// Reconciliation reads it, and buys nothing when it answers.
    async fn find_purchased(
        &self,
        key: &CarrierKey,
        e164: &str,
    ) -> Result<Option<PurchasedNumber>, CatalogError>;

    /// Give the number back. The charge stops and the number never
    /// comes back.
    async fn release(&self, key: &CarrierKey, provider_number_id: &str)
    -> Result<(), CatalogError>;

    /// Make the credential SIP Connection with this username take the
    /// media Pagis offers, SDES-SRTP and nothing else, and send the
    /// dialed number of an inbound call in E.164 (ADR-0020). A carrier
    /// that has encrypted media off answers cleartext, and no audio
    /// flows; a carrier that sends another number format sends a
    /// number the line cannot route. Returns whether the connection was
    /// changed. A username the account does not hold is
    /// `NumberUnavailable`.
    async fn prepare_sip_connection(
        &self,
        key: &CarrierKey,
        username: &str,
    ) -> Result<bool, CatalogError>;
}

/// One [`NumberCatalog`] per carrier provider. The desk and the
/// connector pick the catalog by the carrier Connection's provider; a
/// provider with no catalog is refused with a stated reason.
#[derive(Default, Clone)]
pub struct NumberCatalogs(HashMap<String, Arc<dyn NumberCatalog>>);

impl NumberCatalogs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, provider: &str, catalog: Arc<dyn NumberCatalog>) -> Self {
        self.0.insert(provider.to_string(), catalog);
        self
    }

    /// One catalog under one provider; the tests use it.
    pub fn single(provider: &str, catalog: Arc<dyn NumberCatalog>) -> Self {
        Self::new().with(provider, catalog)
    }

    pub fn get(&self, provider: &str) -> Option<Arc<dyn NumberCatalog>> {
        self.0.get(provider).cloned()
    }
}
