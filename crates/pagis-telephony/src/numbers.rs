//! Workspace Number purchase, carrier setup and release (ADR-0041).

use std::sync::Arc;

use pagis_core::{
    Connection, ConnectionId, ConnectionStore, EventBus, NewEvent, PhoneNumber, PhoneNumberId,
    PhoneNumberStatus, PhoneNumberStore, PurchaseIntent, PurchaseIntentId, PurchaseIntentState,
    SecretStore, StoreError, WorkspaceId, now_ms,
};

use crate::calls::ActiveCalls;
use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberCatalogs,
    NumberSearch,
};
use crate::endpoint::{Endpoints, RegistrationState};
use crate::text::TextTransports;
use crate::{CARRIER_ACCOUNT_KEY, carrier_key_secret_name, is_carrier};

/// The most numbers one search returns.
const SEARCH_LIMIT: u32 = 10;

#[derive(Debug, thiserror::Error)]
pub enum NumberError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("phone number not found")]
    NotFound,
    /// A Call on the number is running. Nothing about a number changes
    /// while somebody is on it.
    #[error("a call on this number is active")]
    CallActive,
    /// The Org has no carrier Connection, so there is no carrier
    /// account to buy from.
    #[error("the installation has no telephony connection")]
    NoCarrier,
    /// The carrier Connection names a provider this daemon has no
    /// client for.
    #[error("this daemon has no client for the carrier provider {0}")]
    UnknownProvider(String),
    /// The carrier Connection is not `connected`. Settings shows the
    /// reason and a reconnect action in place of the search.
    #[error("the telephony connection is {0}")]
    CarrierUnavailable(String),
    #[error(transparent)]
    Carrier(#[from] CatalogError),
}

/// Everything the desk needs. The stores and the carrier are seams, so
/// a test drives the whole lifecycle without a network.
pub struct NumberDeskDeps {
    pub numbers: Arc<dyn PhoneNumberStore>,
    pub connections: Arc<dyn ConnectionStore>,
    /// The Org's Workspace, which holds the carrier Connection. One
    /// carrier serves every person, and each number lives in the
    /// Workspace of the person who holds it (ADR-0018).
    pub org_workspace_id: WorkspaceId,
    /// One catalog per carrier provider.
    pub catalogs: Arc<NumberCatalogs>,
    /// One text transport per carrier provider (ADR-0020). The desk
    /// prepares the carrier's messaging objects with it at purchase.
    pub text_transports: Arc<TextTransports>,
    pub secrets: Arc<dyn SecretStore>,
    pub calls: Arc<dyn ActiveCalls>,
    pub bus: Arc<dyn EventBus>,
    /// The lines: one for each carrier Connection.
    pub endpoints: Arc<Endpoints>,
}

pub struct NumberDesk {
    purchase: tokio::sync::Mutex<()>,
    numbers: Arc<dyn PhoneNumberStore>,
    connections: Arc<dyn ConnectionStore>,
    org_workspace_id: WorkspaceId,
    catalogs: Arc<NumberCatalogs>,
    text_transports: Arc<TextTransports>,
    secrets: Arc<dyn SecretStore>,
    calls: Arc<dyn ActiveCalls>,
    bus: Arc<dyn EventBus>,
    endpoints: Arc<Endpoints>,
}

impl NumberDesk {
    pub fn new(deps: NumberDeskDeps) -> Self {
        Self {
            purchase: tokio::sync::Mutex::new(()),
            numbers: deps.numbers,
            connections: deps.connections,
            org_workspace_id: deps.org_workspace_id,
            catalogs: deps.catalogs,
            text_transports: deps.text_transports,
            secrets: deps.secrets,
            calls: deps.calls,
            bus: deps.bus,
            endpoints: deps.endpoints,
        }
    }
    pub async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, NumberError> {
        Ok(self.numbers.list(workspace_id).await?)
    }
    pub fn registration(&self, number: &PhoneNumber) -> Option<RegistrationState> {
        (number.status == PhoneNumberStatus::Active).then(|| {
            self.endpoints
                .state(&number.connection_id)
                .unwrap_or(RegistrationState::Unregistered)
        })
    }
    pub async fn start_carrier(&self) -> Result<(), NumberError> {
        if let Some(connection) = self.carrier().await? {
            self.prepare_sip_connection(&connection).await;
            self.endpoints.start(&connection.id).await;
        }
        Ok(())
    }
    pub async fn prepare_texting_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<(), NumberError> {
        for mut number in self.numbers.list(workspace_id).await? {
            if number.status == PhoneNumberStatus::Active && number.messaging_object_id.is_none() {
                self.prepare_texting(&mut number).await;
            }
        }
        Ok(())
    }
    pub async fn sip_credential_changed(&self, connection_id: &ConnectionId) {
        match self
            .connections
            .get(&self.org_workspace_id, connection_id)
            .await
        {
            Ok(Some(connection)) => self.prepare_sip_connection(&connection).await,
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, "reading the carrier Connection failed");
            }
        }
        self.endpoints.restart_for_connection(connection_id).await;
    }
    async fn prepare_sip_connection(&self, connection: &Connection) {
        let Some((username, domain)) = crate::endpoint::sip_identity(connection) else {
            return;
        };
        let (catalog, key) = match self.carrier_client(connection) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "the carrier's SIP Connection was not prepared");
                return;
            }
        };
        match catalog
            .prepare_sip_connection(&key, &username, &domain)
            .await
        {
            Ok(true) => {
                tracing::info!(
                    username,
                    "the carrier's SIP Connection was prepared for Pagis's media and dialed numbers"
                )
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, username, "the carrier's SIP Connection was not prepared")
            }
        }
    }
    async fn prepare_texting(&self, number: &mut PhoneNumber) {
        let connection = match self
            .connections
            .get(&self.org_workspace_id, &number.connection_id)
            .await
        {
            Ok(Some(connection)) => connection,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "reading the carrier Connection failed");
                return;
            }
        };
        let Some(transport) = self.text_transports.get(&connection.provider) else {
            return;
        };
        if !transport.capabilities().texting {
            return;
        }
        let key = match self.carrier_key(&connection) {
            Ok(key) => key,
            Err(error) => {
                tracing::warn!(%error, "the carrier's texting was not prepared");
                return;
            }
        };
        let prepared = match transport.prepare(&key, number, &connection.config).await {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(%error, e164 = number.e164, "the carrier's texting was not prepared");
                return;
            }
        };
        if let Some(object_id) = prepared.messaging_object_id {
            match self
                .numbers
                .set_messaging_object_id(&number.workspace_id, &number.id, &object_id)
                .await
            {
                Ok(_) => number.messaging_object_id = Some(object_id),
                Err(error) => {
                    tracing::error!(%error, "storing the carrier's messaging object failed");
                }
            }
        }
        if prepared.connection_config.is_empty() {
            return;
        }
        // The account-level ids join the Connection's own config,
        // where the SIP username already lives, so the next number of
        // any Workspace reuses the objects instead of making them
        // again.
        let mut config = connection.config.clone();
        let Some(map) = config.as_object_mut() else {
            return;
        };
        map.extend(prepared.connection_config);
        if let Err(error) = self
            .connections
            .set_config(&connection.workspace_id, &connection.id, &config)
            .await
        {
            tracing::error!(%error, "storing the carrier's messaging ids failed");
        }
    }
    pub async fn carrier(&self) -> Result<Option<Connection>, NumberError> {
        Ok(self
            .connections
            .list(&self.org_workspace_id)
            .await?
            .into_iter()
            .find(|connection| is_carrier(&connection.provider)))
    }
    pub async fn search(
        &self,
        country: &str,
        area_code: Option<&str>,
        locality: Option<&str>,
    ) -> Result<Vec<AvailableNumber>, NumberError> {
        let (_carrier, catalog, key) = self.connected_carrier().await?;
        let search = NumberSearch {
            country: country.trim().to_uppercase(),
            area_code: trimmed(area_code),
            locality: trimmed(locality),
            limit: SEARCH_LIMIT,
        };
        if search.country.len() != 2 {
            return Err(NumberError::Validation(
                "a search needs a two-letter country, such as US".to_string(),
            ));
        }
        Ok(catalog.search(&key, &search).await?)
    }
    pub async fn buy(
        &self,
        workspace_id: &WorkspaceId,
        e164: &str,
    ) -> Result<PhoneNumber, NumberError> {
        let _purchase = self.purchase.lock().await;
        let e164 = normalize_e164(e164)?;
        let (connection, catalog, key) = self.connected_carrier().await?;
        if self
            .numbers
            .list(workspace_id)
            .await?
            .iter()
            .any(|number| number.status == PhoneNumberStatus::Active)
        {
            return Err(NumberError::Conflict(
                "this Workspace already has a Workspace Number".to_string(),
            ));
        }
        let intent = PurchaseIntent {
            id: PurchaseIntentId::generate(),
            workspace_id: workspace_id.clone(),
            connection_id: connection.id.clone(),
            e164: e164.clone(),
            state: PurchaseIntentState::Pending,
            created_at: now_ms(),
            settled_at: None,
        };
        self.numbers
            .create_intent(&intent)
            .await
            .map_err(store_error)?;

        let purchased = match catalog.buy(&key, &e164, intent.id.as_str()).await {
            Ok(purchased) => purchased,
            Err(error) => {
                // The carrier may still have sold it: the intent stays
                // pending, and reconciliation asks the carrier later.
                if error.0 == CatalogErrorCode::NumberUnavailable {
                    self.numbers
                        .abandon_intent(&intent.workspace_id, &intent.id, now_ms())
                        .await?;
                }
                return Err(NumberError::Carrier(error));
            }
        };
        self.record_purchase(&intent, purchased.provider_number_id)
            .await
    }
    pub async fn reconcile_purchases(
        &self,
        pending_at_boot: Vec<PurchaseIntent>,
    ) -> Result<(), NumberError> {
        for intent in pending_at_boot {
            let Some(connection) = self
                .connections
                .get(&self.org_workspace_id, &intent.connection_id)
                .await?
            else {
                self.numbers
                    .abandon_intent(&intent.workspace_id, &intent.id, now_ms())
                    .await?;
                continue;
            };
            let (catalog, key) = match self.carrier_client(&connection) {
                Ok(client) => client,
                Err(_) => {
                    // The key is gone. The intent stays pending, so the
                    // next start asks again once the user reconnects.
                    tracing::warn!(
                        e164 = intent.e164,
                        "the carrier key for a pending purchase is missing"
                    );
                    continue;
                }
            };
            match catalog.find_purchased(&key, &intent.e164).await {
                Ok(Some(purchased)) => {
                    self.record_purchase(&intent, purchased.provider_number_id)
                        .await?;
                }
                Ok(None) => {
                    self.numbers
                        .abandon_intent(&intent.workspace_id, &intent.id, now_ms())
                        .await?;
                }
                Err(error) => {
                    // The carrier is unreachable. The intent stays
                    // pending, so the next start asks again.
                    tracing::warn!(
                        code = error.0.as_str(),
                        e164 = intent.e164,
                        "reconciling a phone number purchase failed"
                    );
                }
            }
        }
        Ok(())
    }
    pub async fn release(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<PhoneNumber, NumberError> {
        let number = self.live_number(workspace_id, id).await?;
        let connection = self
            .connections
            .get(&self.org_workspace_id, &number.connection_id)
            .await?
            .ok_or(NumberError::NoCarrier)?;
        let (catalog, key) = self.carrier_client(&connection)?;
        catalog.release(&key, &number.provider_number_id).await?;
        if !self.numbers.release(workspace_id, id).await? {
            return Err(NumberError::NotFound);
        }
        let released = PhoneNumber {
            status: PhoneNumberStatus::Released,
            ..number
        };
        self.audit("phone_number.released", &released).await?;
        Ok(released)
    }
    pub async fn connection_in_use(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<bool, NumberError> {
        Ok(self
            .numbers
            .any_live_for_connection(workspace_id, connection_id)
            .await?)
    }
    async fn record_purchase(
        &self,
        intent: &PurchaseIntent,
        provider_number_id: String,
    ) -> Result<PhoneNumber, NumberError> {
        let at = now_ms();
        let mut number = PhoneNumber::new(
            PhoneNumberId::generate(),
            intent.workspace_id.clone(),
            intent.connection_id.clone(),
            intent.e164.clone(),
            provider_number_id,
            at,
        );
        self.numbers
            .confirm_purchase(&intent.workspace_id, &intent.id, &number, at)
            .await
            .map_err(store_error)?;
        self.audit("phone_number.purchased", &number).await?;
        self.prepare_texting(&mut number).await;
        Ok(number)
    }
    async fn live_number(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<PhoneNumber, NumberError> {
        let number = self
            .numbers
            .get(workspace_id, id)
            .await?
            .ok_or(NumberError::NotFound)?;
        if number.status == PhoneNumberStatus::Released {
            return Err(NumberError::Conflict(
                "that number went back to the carrier".to_string(),
            ));
        }
        if self.calls.is_active(&number.id).await {
            return Err(NumberError::CallActive);
        }
        Ok(number)
    }
    async fn connected_carrier(
        &self,
    ) -> Result<(Connection, Arc<dyn NumberCatalog>, CarrierKey), NumberError> {
        let connection = self.carrier().await?.ok_or(NumberError::NoCarrier)?;
        if connection.status != Connection::CONNECTED {
            return Err(NumberError::CarrierUnavailable(connection.status.clone()));
        }
        let (catalog, key) = self.carrier_client(&connection)?;
        Ok((connection, catalog, key))
    }
    fn carrier_client(
        &self,
        connection: &Connection,
    ) -> Result<(Arc<dyn NumberCatalog>, CarrierKey), NumberError> {
        let catalog = self
            .catalogs
            .get(&connection.provider)
            .ok_or_else(|| NumberError::UnknownProvider(connection.provider.clone()))?;
        Ok((catalog, self.carrier_key(connection)?))
    }
    fn carrier_key(&self, connection: &Connection) -> Result<CarrierKey, NumberError> {
        let name = carrier_key_secret_name(&connection.provider, &connection.alias);
        let account = connection.config[CARRIER_ACCOUNT_KEY]
            .as_str()
            .unwrap_or_default();
        match self.secrets.get(&name) {
            Ok(Some(secret)) => Ok(CarrierKey::new(account, secret)),
            Ok(None) => Err(NumberError::CarrierUnavailable(
                Connection::REAUTH_REQUIRED.to_string(),
            )),
            Err(error) => {
                tracing::error!(%error, "reading the carrier API key failed");
                Err(NumberError::CarrierUnavailable(
                    Connection::REAUTH_REQUIRED.to_string(),
                ))
            }
        }
    }
    async fn audit(&self, event_type: &str, number: &PhoneNumber) -> Result<(), NumberError> {
        self.bus
            .publish(NewEvent {
                workspace_id: number.workspace_id.clone(),
                event_type: event_type.to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({
                    "phone_number_id": number.id.as_str(),
                    "e164": number.e164,
                }),
            })
            .await?;
        Ok(())
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn store_error(error: StoreError) -> NumberError {
    match error {
        StoreError::Conflict(message) => NumberError::Conflict(message),
        other => NumberError::Store(other),
    }
}

pub fn normalize_e164(value: &str) -> Result<String, NumberError> {
    pagis_core::normalize_e164(value).ok_or_else(|| {
        NumberError::Validation(format!(
            "{} is not an E.164 number, such as +14155550123",
            value.trim()
        ))
    })
}
