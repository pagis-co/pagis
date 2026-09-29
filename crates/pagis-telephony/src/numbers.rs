//! The number desk (ADR-0018): the one path that buys, assigns,
//! unassigns and releases an Agent Phone Number.
//!
//! Buying writes a purchase intent before it asks the carrier, so a
//! daemon that restarts in the middle of a purchase reconciles the
//! intent against the carrier's own list instead of buying a second
//! time. Every act that changes who holds a line refuses while a Call
//! on that number is active, and every act writes to the audit log.
//!
//! The desk also holds the two emergency guards (ADR-0018): the broker
//! guard, before the call tool acts, and the dial guard, in the dial
//! path. Both apply the one predicate in [`crate::emergency`], and a
//! refusal is audited and makes no Call record.
//!
//! The desk also starts the line of the carrier (ADR-0020): the line
//! registers the carrier's SIP credential once for every number, so
//! buying, assigning, unassigning or releasing a number does not change
//! it. The line starts when the daemon starts or the carrier is set up,
//! and restarts when the SIP credential changes.

use std::sync::Arc;

use pagis_core::{
    AgentId, AgentStatus, AgentStore, Connection, ConnectionId, ConnectionStore, EventBus,
    NewEvent, PhoneNumber, PhoneNumberId, PhoneNumberStatus, PhoneNumberStore, PurchaseIntent,
    PurchaseIntentId, PurchaseIntentState, RunId, SecretStore, StoreError, WorkspaceId, now_ms,
};

use crate::calls::ActiveCalls;
use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberCatalogs,
    NumberSearch,
};
use crate::emergency::{EmergencyRefused, check_dial};
use crate::endpoint::{Endpoints, RegistrationState};
use crate::events::StandingCallRule;
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
    /// The number is an emergency number. Pagis never calls one, and
    /// no setting, override or approval changes that (ADR-0018).
    #[error(transparent)]
    EmergencyRefused(#[from] EmergencyRefused),
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
    pub agents: Arc<dyn AgentStore>,
    /// One catalog per carrier provider.
    pub catalogs: Arc<NumberCatalogs>,
    /// One text transport per carrier provider (ADR-0020). The desk
    /// prepares the carrier's messaging objects with it at assignment.
    pub text_transports: Arc<TextTransports>,
    pub secrets: Arc<dyn SecretStore>,
    pub calls: Arc<dyn ActiveCalls>,
    pub bus: Arc<dyn EventBus>,
    /// The lines: one for each carrier Connection.
    pub endpoints: Arc<Endpoints>,
    /// The Standing Call Rule (ADR-0020): assigning a line writes it,
    /// so the Agent wakes on every call it answers, and taking the
    /// line back archives it.
    pub standing_rule: Arc<dyn StandingCallRule>,
}

pub struct NumberDesk {
    numbers: Arc<dyn PhoneNumberStore>,
    connections: Arc<dyn ConnectionStore>,
    org_workspace_id: WorkspaceId,
    agents: Arc<dyn AgentStore>,
    catalogs: Arc<NumberCatalogs>,
    text_transports: Arc<TextTransports>,
    secrets: Arc<dyn SecretStore>,
    calls: Arc<dyn ActiveCalls>,
    bus: Arc<dyn EventBus>,
    endpoints: Arc<Endpoints>,
    standing_rule: Arc<dyn StandingCallRule>,
}

impl NumberDesk {
    pub fn new(deps: NumberDeskDeps) -> Self {
        Self {
            numbers: deps.numbers,
            connections: deps.connections,
            org_workspace_id: deps.org_workspace_id,
            agents: deps.agents,
            catalogs: deps.catalogs,
            text_transports: deps.text_transports,
            secrets: deps.secrets,
            calls: deps.calls,
            bus: deps.bus,
            endpoints: deps.endpoints,
            standing_rule: deps.standing_rule,
        }
    }

    pub async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, NumberError> {
        Ok(self.numbers.list(workspace_id).await?)
    }

    /// Where the line of the number's carrier stands with the
    /// registrar, or `None` when no Agent holds the number: a call to it
    /// is refused whatever the line does. A carrier with no line is
    /// `unregistered`.
    pub fn registration(&self, number: &PhoneNumber) -> Option<RegistrationState> {
        (number.status == PhoneNumberStatus::Assigned).then(|| {
            self.endpoints
                .state(&number.connection_id)
                .unwrap_or(RegistrationState::Unregistered)
        })
    }

    /// Prepare the Org's carrier for Pagis's media and dialed numbers,
    /// so a carrier that was set up by hand answers the media Pagis
    /// offers, and start the carrier's line. The daemon calls it at
    /// start and when an Administrator sets up the carrier. A line that
    /// runs already keeps its registration and its calls.
    pub async fn start_carrier(&self) -> Result<(), NumberError> {
        if let Some(connection) = self.carrier().await? {
            self.prepare_sip_connection(&connection).await;
            self.endpoints.start(&connection.id).await;
        }
        Ok(())
    }

    /// Prepare the texting of every assigned number of one Workspace
    /// whose messaging object is missing (ADR-0020): a preparation that
    /// failed at assignment, or a number assigned before the carrier
    /// carried texts, is put in place on the next start.
    pub async fn prepare_assigned_texting(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<(), NumberError> {
        for mut number in self.numbers.list(workspace_id).await? {
            if number.status == PhoneNumberStatus::Assigned && number.messaging_object_id.is_none()
            {
                self.prepare_texting(&mut number).await;
            }
        }
        Ok(())
    }

    /// The carrier's SIP credential changed, or the carrier Connection
    /// is gone: the carrier's SIP Connection is prepared for Pagis's
    /// media and dialed numbers, and the carrier's line registers again
    /// with the credential now. A Connection that is gone has no line.
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

    /// Make the carrier's credential SIP Connection answer SDES-SRTP
    /// and send the dialed number in E.164 (ADR-0020). A carrier that
    /// cannot be prepared is logged and the line still registers: a
    /// call then ends with `media_failed`, or is refused for a dialed
    /// number that is not E.164, and the log says what to fix.
    async fn prepare_sip_connection(&self, connection: &Connection) {
        let Some((username, _domain)) = crate::endpoint::sip_identity(connection) else {
            return;
        };
        let (catalog, key) = match self.carrier_client(connection) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "the carrier's SIP Connection was not prepared");
                return;
            }
        };
        match catalog.prepare_sip_connection(&key, &username).await {
            Ok(true) => {
                tracing::info!(
                    username,
                    "the carrier now sends SRTP media and E.164 dialed numbers"
                )
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, username, "the carrier's SIP Connection was not prepared")
            }
        }
    }

    /// Put the carrier's messaging objects in place for one number
    /// (ADR-0020). It runs at assignment, and again on daemon start
    /// when the object is missing.
    ///
    /// A carrier that carries no text prepares nothing. A preparation
    /// that fails is logged and the assignment stands: the line still
    /// calls, and the next start asks again. The number carries the
    /// messaging object away with it, so the caller gives back the
    /// record the store holds.
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

    /// The Org's carrier Connection, whatever its status. One
    /// Connection carries every number of every Workspace (ADR-0018).
    pub async fn carrier(&self) -> Result<Option<Connection>, NumberError> {
        Ok(self
            .connections
            .list(&self.org_workspace_id)
            .await?
            .into_iter()
            .find(|connection| is_carrier(&connection.provider)))
    }

    /// Voice-capable numbers the carrier offers now, with today's
    /// price. A search needs the Connection at `connected`.
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

    /// Buy one number and, when the user bought it from an Agent's
    /// page, give it to that Agent. The intent is written first, so the
    /// purchase is idempotent across a restart.
    pub async fn buy(
        &self,
        workspace_id: &WorkspaceId,
        e164: &str,
        agent_id: Option<&AgentId>,
    ) -> Result<PhoneNumber, NumberError> {
        let e164 = normalize_e164(e164)?;
        let (connection, catalog, key) = self.connected_carrier().await?;
        if let Some(agent_id) = agent_id {
            self.active_agent(workspace_id, agent_id).await?;
            if self
                .numbers
                .for_agent(workspace_id, agent_id)
                .await?
                .is_some()
            {
                return Err(NumberError::Conflict(
                    "that Agent already holds a number. Unassign it first.".to_string(),
                ));
            }
        }
        let intent = PurchaseIntent {
            id: PurchaseIntentId::generate(),
            workspace_id: workspace_id.clone(),
            connection_id: connection.id.clone(),
            e164: e164.clone(),
            agent_id: agent_id.cloned(),
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

    /// Record a number the carrier account already holds. The
    /// carrier is asked whether the account has it; nothing is bought.
    /// Adopted from an Agent's page, the number is the Agent's at once
    /// and its calls reach the Agent now.
    pub async fn adopt(
        &self,
        workspace_id: &WorkspaceId,
        e164: &str,
        agent_id: Option<&AgentId>,
    ) -> Result<PhoneNumber, NumberError> {
        let e164 = normalize_e164(e164)?;
        let (connection, catalog, key) = self.connected_carrier().await?;
        if let Some(agent_id) = agent_id {
            self.active_agent(workspace_id, agent_id).await?;
            if self
                .numbers
                .for_agent(workspace_id, agent_id)
                .await?
                .is_some()
            {
                return Err(NumberError::Conflict(
                    "that Agent already holds a number. Unassign it first.".to_string(),
                ));
            }
        }
        if self
            .numbers
            .list(workspace_id)
            .await?
            .iter()
            .any(|number| number.e164 == e164 && number.status != PhoneNumberStatus::Released)
        {
            return Err(NumberError::Conflict(
                "Pagis already holds that number".to_string(),
            ));
        }
        let Some(purchased) = catalog.find_purchased(&key, &e164).await? else {
            return Err(NumberError::Validation(
                "your carrier account does not hold that number".to_string(),
            ));
        };
        let at = now_ms();
        let mut number = PhoneNumber::new(
            PhoneNumberId::generate(),
            workspace_id.clone(),
            connection.id.clone(),
            e164,
            purchased.provider_number_id,
            agent_id.cloned(),
            at,
        );
        self.numbers.create(&number).await.map_err(store_error)?;
        self.audit("phone_number.adopted", &number, None).await?;
        if let Some(agent_id) = agent_id {
            self.audit("phone_number.assigned", &number, None).await?;
            self.line_is_held(&mut number, agent_id).await;
        }
        Ok(number)
    }

    /// Settle every purchase intent that outlived the daemon. An intent
    /// the carrier honoured becomes a number record; an intent it never
    /// saw is abandoned. Nothing is bought here (ADR-0018).
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

    /// Give an unassigned number to an Agent.
    pub async fn assign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        agent_id: &AgentId,
    ) -> Result<PhoneNumber, NumberError> {
        let number = self.live_number(workspace_id, id).await?;
        self.active_agent(workspace_id, agent_id).await?;
        if number.status == PhoneNumberStatus::Assigned {
            return Err(NumberError::Conflict(
                "that number is already assigned. Unassign it first.".to_string(),
            ));
        }
        if self
            .numbers
            .for_agent(workspace_id, agent_id)
            .await?
            .is_some()
        {
            return Err(NumberError::Conflict(
                "that Agent already holds a number. Unassign it first.".to_string(),
            ));
        }
        let at = now_ms();
        if !self
            .numbers
            .assign(workspace_id, id, agent_id, at)
            .await
            .map_err(store_error)?
        {
            return Err(NumberError::NotFound);
        }
        let mut assigned = PhoneNumber {
            agent_id: Some(agent_id.clone()),
            status: PhoneNumberStatus::Assigned,
            assigned_at: Some(at),
            ..number
        };
        self.audit("phone_number.assigned", &assigned, None).await?;
        self.line_is_held(&mut assigned, agent_id).await;
        Ok(assigned)
    }

    /// Take the line back from its Agent. The Workspace keeps the
    /// number and keeps paying for it.
    pub async fn unassign(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<PhoneNumber, NumberError> {
        let number = self.live_number(workspace_id, id).await?;
        let held_by = number.agent_id.clone();
        if !self.numbers.unassign(workspace_id, id).await? {
            return Err(NumberError::Conflict(
                "that number is not assigned".to_string(),
            ));
        }
        let unassigned = PhoneNumber {
            agent_id: None,
            status: PhoneNumberStatus::Unassigned,
            assigned_at: None,
            ..number
        };
        self.archive_standing_rule(&unassigned, held_by.as_ref())
            .await;
        self.audit("phone_number.unassigned", &unassigned, held_by)
            .await?;
        Ok(unassigned)
    }

    /// Archiving an Agent takes its line back (ADR-0018). It is the one
    /// unassignment the user does not ask for by number.
    pub async fn unassign_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, NumberError> {
        let Some(number) = self.numbers.for_agent(workspace_id, agent_id).await? else {
            return Ok(None);
        };
        if self.calls.is_active(&number.id).await {
            return Err(NumberError::CallActive);
        }
        let unassigned = self
            .numbers
            .unassign_for_agent(workspace_id, agent_id)
            .await?
            .ok_or(NumberError::NotFound)?;
        self.archive_standing_rule(&unassigned, Some(agent_id))
            .await;
        self.audit(
            "phone_number.unassigned",
            &unassigned,
            Some(agent_id.clone()),
        )
        .await?;
        Ok(Some(unassigned))
    }

    /// Give the number back to the carrier. The charge stops, the
    /// record stays for the Calls that point at it, and the number
    /// never comes back.
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
        let held_by = number.agent_id.clone();
        if !self.numbers.release(workspace_id, id).await? {
            return Err(NumberError::NotFound);
        }
        let released = PhoneNumber {
            agent_id: None,
            status: PhoneNumberStatus::Released,
            assigned_at: None,
            ..number
        };
        self.archive_standing_rule(&released, held_by.as_ref())
            .await;
        self.audit("phone_number.released", &released, held_by)
            .await?;
        Ok(released)
    }

    /// The line is one Agent's now, whether it was bought for it,
    /// adopted for it or assigned to it. The Agent wakes on the calls
    /// it answers (ADR-0020), and the carrier's messaging objects are
    /// made. The carrier's line already carries the number, so calls to
    /// it reach the Agent from now on. A rule that cannot be written
    /// leaves the number assigned: the user sees the line working, and
    /// the daemon says what is missing.
    async fn line_is_held(&self, number: &mut PhoneNumber, agent_id: &AgentId) {
        if let Err(error) = self.standing_rule.create(number, agent_id).await {
            tracing::error!(%error, e164 = number.e164, "the standing call rule was not written");
        }
        self.prepare_texting(number).await;
    }

    /// The line leaves the Agent, so the rule that woke it on a call
    /// stops. The number is already taken back, so a rule that
    /// will not archive is logged and never fails the act.
    async fn archive_standing_rule(&self, number: &PhoneNumber, held_by: Option<&AgentId>) {
        let Some(agent_id) = held_by else { return };
        if let Err(error) = self.standing_rule.archive(number, agent_id).await {
            tracing::error!(%error, e164 = number.e164, "the standing call rule was not archived");
        }
    }

    /// Whether a number of this Workspace that is not released still
    /// points at this Connection. Deleting the carrier Connection
    /// refuses while a number of any Workspace does (ADR-0018).
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

    /// The number one Agent holds. The broker reads it: an Agent with
    /// no number is offered no phone tool at all (ADR-0005).
    pub async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, NumberError> {
        Ok(self.numbers.for_agent(workspace_id, agent_id).await?)
    }

    /// The broker guard (ADR-0018): before the `phone.call` tool acts,
    /// the broker calls this with the Agent, the Run and the
    /// `to` argument as the model wrote it. It runs before the E.164
    /// check, so `911` refuses as an emergency number and not as a
    /// malformed argument the model should retry. It gives back the
    /// line the Agent holds, which the dial path takes next.
    pub async fn guard_call_tool(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        to: &str,
    ) -> Result<PhoneNumber, NumberError> {
        let number = self
            .numbers
            .for_agent(workspace_id, agent_id)
            .await?
            .ok_or_else(|| NumberError::Validation("this Agent holds no number".to_string()))?;
        self.guard_dial(&number, Some(run_id), to).await?;
        Ok(number)
    }

    /// The dial guard (ADR-0018): every path that starts or redirects a
    /// Call calls this with the line the Call is from and the number it
    /// is asked to reach. A refusal writes
    /// `phone_number.emergency_refused`, the one durable record that an
    /// Agent tried, and makes no Call record.
    pub async fn guard_dial(
        &self,
        number: &PhoneNumber,
        run_id: Option<&RunId>,
        to: &str,
    ) -> Result<(), NumberError> {
        let Err(refused) = check_dial(to, &number.e164) else {
            return Ok(());
        };
        self.bus
            .publish(NewEvent {
                workspace_id: number.workspace_id.clone(),
                event_type: "phone_number.emergency_refused".to_string(),
                agent_id: number.agent_id.clone(),
                run_id: run_id.cloned(),
                channel_id: None,
                payload: serde_json::json!({
                    "phone_number_id": number.id.as_str(),
                    "e164": number.e164,
                    "to": refused.to,
                    "region": refused.region,
                    "agent_id": number.agent_id.as_ref().map(AgentId::as_str),
                }),
            })
            .await?;
        Err(NumberError::EmergencyRefused(refused))
    }

    async fn record_purchase(
        &self,
        intent: &PurchaseIntent,
        provider_number_id: String,
    ) -> Result<PhoneNumber, NumberError> {
        let at = now_ms();
        // An Agent that was archived, or that took another number,
        // while the purchase was in flight does not get this one. The
        // Workspace keeps it, unassigned and paid for.
        let agent_id = match &intent.agent_id {
            Some(agent_id) => match self
                .numbers
                .for_agent(&intent.workspace_id, agent_id)
                .await?
            {
                Some(_) => None,
                None => Some(agent_id.clone()),
            },
            None => None,
        };
        let mut number = PhoneNumber::new(
            PhoneNumberId::generate(),
            intent.workspace_id.clone(),
            intent.connection_id.clone(),
            intent.e164.clone(),
            provider_number_id,
            agent_id,
            at,
        );
        self.numbers
            .confirm_purchase(&intent.workspace_id, &intent.id, &number, at)
            .await
            .map_err(store_error)?;
        self.audit("phone_number.purchased", &number, None).await?;
        if let Some(agent_id) = number.agent_id.clone() {
            self.audit("phone_number.assigned", &number, None).await?;
            self.line_is_held(&mut number, &agent_id).await;
        }
        Ok(number)
    }

    /// A number of this Workspace that is not released, and that no
    /// Call is running on.
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

    async fn active_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<(), NumberError> {
        let agent = self
            .agents
            .get(workspace_id, agent_id)
            .await?
            .ok_or_else(|| NumberError::Validation("that Agent is not here".to_string()))?;
        if agent.status != AgentStatus::Active {
            return Err(NumberError::Validation(
                "an archived Agent holds no number".to_string(),
            ));
        }
        Ok(())
    }

    /// The carrier Connection and its API key, when a purchase or a
    /// search may proceed.
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

    /// The catalog of the Connection's provider and the key it signs
    /// with. A provider this daemon has no client for is refused with
    /// the reason.
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

    /// One audit line for each act (ADR-0018). It carries the number,
    /// the Agent and nothing else: no carrier handle and no key.
    async fn audit(
        &self,
        event_type: &str,
        number: &PhoneNumber,
        held_by: Option<AgentId>,
    ) -> Result<(), NumberError> {
        let agent_id = number.agent_id.clone().or(held_by);
        self.bus
            .publish(NewEvent {
                workspace_id: number.workspace_id.clone(),
                event_type: event_type.to_string(),
                agent_id: agent_id.clone(),
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({
                    "phone_number_id": number.id.as_str(),
                    "e164": number.e164,
                    "agent_id": agent_id.as_ref().map(AgentId::as_str),
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

/// E.164 and nothing else: a `+`, a country digit, and up to fourteen
/// more. The call tool accepts the same shape (ADR-0018), so a number
/// reads the same everywhere it is stored, dialed and audited.
/// The Trust List holds a number under the same rule, so the shape
/// lives once, in `pagis-core`.
pub fn normalize_e164(value: &str) -> Result<String, NumberError> {
    pagis_core::normalize_e164(value).ok_or_else(|| {
        NumberError::Validation(format!(
            "{} is not an E.164 number, such as +14155550123",
            value.trim()
        ))
    })
}
