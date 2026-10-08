//! The connect flow (ADR-0012): the one path that puts a
//! Connection row in the table and takes it to `connected`.
//!
//! Three kinds of provider arrive here. Google is an OAuth exchange the
//! user finishes in a browser. The telephony carrier is an API key the
//! user pastes, which the daemon proves against the carrier before it
//! keeps it. A Mailbox Provider is a mail domain: Migadu with an
//! API key the daemon proves by listing the domain's mailboxes, or a
//! manual host with no key at all (ADR-0019). They all end at the same
//! place: a row at `connected`.
//!
//! The flow has two halves because the user is between them. First the
//! record: the alias a tool call names and the display name. Then the
//! authorization.
//!
//! Google connects only through the Installation OAuth Client, the one
//! Web OAuth client that an Administrator sets up for the Org
//! (ADR-0012). The person types nothing: the daemon answers with the
//! start route on its own Public Origin and returns at once, the person
//! picks the account in Google's account chooser and allows Pagis, and
//! the redirect comes back to the same origin. Only the browser and the
//! Person that the start route bound finish it. The daemon owns the
//! refresh token from then on, sealed with that person's Tenant Data
//! Key. [`google`] holds that half.

use std::sync::Arc;

use pagis_core::{
    Connection, ConnectionId, ConnectionStore, SecretStore, StoreError, WorkspaceId, now_ms,
};
use pagis_google::GoogleCapability;
use pagis_mail::{
    Endpoint, HostAccount, HostErrorCode, MAIL_TRANSPORT, MANUAL_PROVIDER, MIGADU_IMAP,
    MIGADU_PROVIDER, MIGADU_SMTP, MailboxCapabilities, MailboxHost, MailboxProvider,
    host_api_key_secret_name,
};
use pagis_telephony::{
    CARRIER_ACCOUNT_KEY, CarrierKey, CatalogErrorCode, NumberCatalogs, NumberSearch,
    SIP_DOMAIN_KEY, SIP_USERNAME_KEY, carrier_key_secret_name, is_carrier,
    sip_password_secret_name,
};

mod catalog;
mod google;

pub use catalog::{
    CALENDAR, FieldKind, InstallationSetup, MAIL, MAILBOXES, PersonProvider, ProviderEntry,
    ProviderField, ProviderKind, SetupKind, SetupPart, TELEPHONY, TEXTING, absent_capabilities,
    capabilities, catalog, entry, installation_setup, installation_setups,
    is_installation_provider, person_catalog,
};
pub use google::{
    AUTHORIZE_WINDOW_MS, GoogleBroker, Initiator, Opener, OrgWebClient, PendingAuthorizations,
    StartRefusal,
};
pub use pagis_google::ProviderErrorCode;

/// What the user typed into the connect flow. The secrets reach the
/// provider or the secret store once and are dropped; the type carries
/// no `Debug`, so none of them reaches a log line.
pub struct NewConnection {
    pub workspace_id: WorkspaceId,
    /// The name a tool call uses to pick this account.
    pub alias: String,
    pub display_name: String,
    pub credentials: NewCredentials,
}

/// What one provider needs to be reachable. The provider follows from
/// the shape, so a request cannot name one provider and carry the
/// other's secrets.
pub enum NewCredentials {
    /// A Google account. It carries nothing: the account comes from the
    /// consent, and the client is the installation's (ADR-0012).
    Google,
    /// A carrier account: the provider, the account id
    /// that is not secret and the secret that is. The secret goes to
    /// the secret store, never to the database. Telnyx has no account
    /// id, so its `account` is empty.
    Carrier {
        provider: String,
        account: String,
        secret: String,
    },
    /// The Migadu account, its API key and the mail domain the account
    /// owns (ADR-0019). The key goes to the secret store; the mail path
    /// never reads it.
    Migadu {
        account: String,
        api_key: String,
        domain: String,
    },
    /// A mail host with no API (ADR-0019): the user makes each mailbox
    /// at the host, so Pagis holds the domain and the two endpoints and
    /// no host credential at all.
    Manual {
        domain: String,
        imap: Endpoint,
        smtp: Endpoint,
    },
}

impl NewCredentials {
    /// The `provider` the Connection records; its catalog entry
    /// declares the fields this shape was read from.
    pub fn provider(&self) -> &str {
        match self {
            NewCredentials::Google => pagis_google::GOOGLE_PROVIDER,
            NewCredentials::Carrier { provider, .. } => provider,
            NewCredentials::Migadu { .. } => MIGADU_PROVIDER,
            NewCredentials::Manual { .. } => MANUAL_PROVIDER,
        }
    }
}

/// What one authorization step answers.
#[derive(Debug)]
pub enum Authorization {
    /// The Connection reached its new state now. A carrier key and a
    /// mail host key end here.
    Done(Connection),
    /// The person has to consent at Google in their own browser.
    /// `url` is the start route on the Public Origin, which sends a
    /// browser of the initiating Person to Google. The Connection is
    /// `connecting` until the redirect arrives on the Public Origin.
    AtGoogle { connection: Connection, url: String },
}

impl Authorization {
    /// The Connection as it stands now, whichever shape this is.
    pub fn connection(&self) -> &Connection {
        match self {
            Authorization::Done(connection) => connection,
            Authorization::AtGoogle { connection, .. } => connection,
        }
    }

    /// The start route to open, for the Google flow.
    pub fn url(&self) -> Option<&str> {
        match self {
            Authorization::Done(_) => None,
            Authorization::AtGoogle { url, .. } => Some(url),
        }
    }

    pub fn into_connection(self) -> Connection {
        match self {
            Authorization::Done(connection) => connection,
            Authorization::AtGoogle { connection, .. } => connection,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("connection not found")]
    NotFound,
    /// The provider refused. The code is the stable one the broker uses;
    /// no upstream text reaches it.
    #[error("the provider did not complete this: {}", .0.as_str())]
    Provider(ProviderErrorCode),
}

/// Everything the connect flow reaches: the Connection table, the
/// Google broker, the carrier for telephony, the mail host for a
/// Mailbox Provider, and the secret store under all of them.
pub struct ConnectorDeps {
    pub connections: Arc<dyn ConnectionStore>,
    /// One number catalog per carrier provider.
    pub catalogs: Arc<NumberCatalogs>,
    /// The host API of a Mailbox Provider (ADR-0019). Migadu is the one
    /// host with an API; the manual host needs no client, because
    /// the user does the work at the host.
    pub mail_host: Arc<dyn MailboxHost>,
    pub secrets: Arc<dyn SecretStore>,
    /// The Google half. It answers whether this installation holds a
    /// Web OAuth client, and it runs the server-side
    /// authorization-code flow.
    pub google: Arc<GoogleBroker>,
}

/// Creates Connections and takes them to `connected`.
pub struct Connector {
    connections: Arc<dyn ConnectionStore>,
    catalogs: Arc<NumberCatalogs>,
    mail_host: Arc<dyn MailboxHost>,
    secrets: Arc<dyn SecretStore>,
    google: Arc<GoogleBroker>,
}

impl Connector {
    pub fn new(deps: ConnectorDeps) -> Self {
        Self {
            connections: deps.connections,
            catalogs: deps.catalogs,
            mail_host: deps.mail_host,
            secrets: deps.secrets,
            google: deps.google,
        }
    }

    /// The Google half, for the callback route and the
    /// administrator's settings route.
    pub fn google(&self) -> &Arc<GoogleBroker> {
        &self.google
    }

    /// Record one Connection. A Google record starts at `disconnected`:
    /// it carries no account and no authorization until
    /// [`Connector::authorize`] sends the person to Google and the
    /// consent comes back. A carrier record starts at
    /// `connected`, because the API key it carries is the whole
    /// authorization and it is proved before the row exists.
    ///
    /// The alias names the stored key too, so one Connection owns one
    /// secret and the user invents one name, not two.
    pub async fn create(&self, new: NewConnection) -> Result<Connection, ConnectError> {
        validate_alias(&new.alias)?;
        let display_name = new.display_name.trim();
        if display_name.is_empty() {
            return Err(ConnectError::Validation(
                "a connection needs a display name".to_string(),
            ));
        }
        let provider = new.credentials.provider().to_string();
        self.enforce_max_instances(&new.workspace_id, &provider)
            .await?;
        match new.credentials {
            NewCredentials::Google => {
                // The installation's own Web client is the one client
                // every person consents against (ADR-0012). Without it
                // no row is written, so the alias stays free.
                if self.google.web_client().await?.is_none() {
                    return Err(ConnectError::Validation(
                        catalog::GOOGLE_NOT_SET_UP.to_string(),
                    ));
                }
                let connection = Connection {
                    id: ConnectionId::generate(),
                    workspace_id: new.workspace_id,
                    provider: provider.to_string(),
                    alias: new.alias.clone(),
                    display_name: display_name.to_string(),
                    status: Connection::DISCONNECTED.to_string(),
                    authorized_capabilities: Vec::new(),
                    // The account lands with the first consent.
                    config: serde_json::json!({ "client": new.alias }),
                    created_at: now_ms(),
                };
                self.connections
                    .create(&connection)
                    .await
                    .map_err(store_error)?;
                Ok(connection)
            }
            NewCredentials::Carrier {
                provider,
                account,
                secret,
            } => {
                self.enforce_one_carrier(&new.workspace_id).await?;
                let key = CarrierKey::new(account.trim(), secret.trim());
                self.prove_carrier_key(&provider, &key).await?;
                let mut config = serde_json::json!({});
                if !key.account().is_empty() {
                    config[CARRIER_ACCOUNT_KEY] = serde_json::Value::String(key.account().into());
                }
                let connection = Connection {
                    id: ConnectionId::generate(),
                    workspace_id: new.workspace_id,
                    provider: provider.clone(),
                    alias: new.alias.clone(),
                    display_name: display_name.to_string(),
                    status: Connection::CONNECTED.to_string(),
                    authorized_capabilities: Vec::new(),
                    config,
                    created_at: now_ms(),
                };
                self.connections
                    .create(&connection)
                    .await
                    .map_err(store_error)?;
                if let Err(error) = self.store_carrier_key(&connection, key.expose_secret()) {
                    // A record with no key reaches no carrier, and
                    // leaving it would take the alias for good.
                    self.connections
                        .delete_and_revoke(&connection.workspace_id, &connection.id, now_ms())
                        .await?;
                    return Err(error);
                }
                Ok(connection)
            }
            NewCredentials::Migadu {
                account,
                api_key,
                domain,
            } => {
                let account = HostAccount::new(
                    mail_domain(&domain)?,
                    required(&account, "the mail account is required")?,
                    required(&api_key, "the mail host API key is required")?,
                );
                // The key must list the domain's mailboxes before the
                // row exists: a key the host refuses reaches neither
                // the record nor the secret store (ADR-0019).
                self.mail_host.list(&account).await.map_err(mail_error)?;
                let settings = MailboxProvider {
                    account: Some(account.account().to_string()),
                    domain: account.domain().to_string(),
                    imap: Endpoint::new(MIGADU_IMAP.0, MIGADU_IMAP.1),
                    smtp: Endpoint::new(MIGADU_SMTP.0, MIGADU_SMTP.1),
                    capabilities: MailboxCapabilities::of(
                        self.mail_host.capabilities(),
                        MAIL_TRANSPORT,
                    ),
                };
                let connection = self.mail_connection(
                    &new.workspace_id,
                    &new.alias,
                    &provider,
                    display_name,
                    settings,
                );
                self.connections
                    .create(&connection)
                    .await
                    .map_err(store_error)?;
                if let Err(error) = self.store_host_key(&connection, account.expose_api_key()) {
                    // A record with no key makes no mailbox, and
                    // leaving it would take the alias for good.
                    self.connections
                        .delete_and_revoke(&connection.workspace_id, &connection.id, now_ms())
                        .await?;
                    return Err(error);
                }
                Ok(connection)
            }
            NewCredentials::Manual { domain, imap, smtp } => {
                // There is no host to ask, so the record is the whole
                // Connection and it is `connected` at once. Each
                // mailbox proves itself at its first login (ADR-0019).
                let settings = MailboxProvider {
                    account: None,
                    domain: mail_domain(&domain)?,
                    imap: mail_endpoint(imap, "IMAP")?,
                    smtp: mail_endpoint(smtp, "SMTP")?,
                    capabilities: MailboxCapabilities::of(
                        pagis_mail::ManualHost::new().capabilities(),
                        MAIL_TRANSPORT,
                    ),
                };
                let connection = self.mail_connection(
                    &new.workspace_id,
                    &new.alias,
                    &provider,
                    display_name,
                    settings,
                );
                self.connections
                    .create(&connection)
                    .await
                    .map_err(store_error)?;
                Ok(connection)
            }
        }
    }

    /// A provider whose entry caps the instances is refused once the
    /// Workspace holds that many. The carrier is the one such provider: one
    /// account carries every number of the Workspace (ADR-0018).
    async fn enforce_max_instances(
        &self,
        workspace_id: &WorkspaceId,
        provider: &str,
    ) -> Result<(), ConnectError> {
        let Some(entry) = catalog::entry(provider) else {
            return Ok(());
        };
        let Some(max_instances) = entry.max_instances else {
            return Ok(());
        };
        let held = self
            .connections
            .list(workspace_id)
            .await?
            .iter()
            .filter(|connection| connection.provider == provider)
            .count();
        if held >= max_instances as usize {
            let noun = if max_instances == 1 {
                "connection"
            } else {
                "connections"
            };
            return Err(ConnectError::Conflict(format!(
                "this workspace already has {max_instances} {} {noun}",
                entry.label
            )));
        }
        Ok(())
    }

    /// A Workspace has one carrier Connection, of any provider: one
    /// account carries every number (ADR-0018).
    async fn enforce_one_carrier(&self, workspace_id: &WorkspaceId) -> Result<(), ConnectError> {
        let held = self
            .connections
            .list(workspace_id)
            .await?
            .into_iter()
            .find(|connection| is_carrier(&connection.provider));
        match held {
            Some(connection) => Err(ConnectError::Conflict(format!(
                "this workspace already has a carrier, {}; one account carries every number",
                connection.display_name
            ))),
            None => Ok(()),
        }
    }

    /// One Mailbox Provider record. It lands `connected`: a Migadu key
    /// is proved before this runs, and a manual host has nothing to
    /// prove (ADR-0019).
    fn mail_connection(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
        provider: &str,
        display_name: &str,
        settings: MailboxProvider,
    ) -> Connection {
        Connection {
            id: ConnectionId::generate(),
            workspace_id: workspace_id.clone(),
            provider: provider.to_string(),
            alias: alias.to_string(),
            display_name: display_name.to_string(),
            status: Connection::CONNECTED.to_string(),
            authorized_capabilities: Vec::new(),
            config: settings.config(),
            created_at: now_ms(),
        }
    }

    /// Keep one Connection's mail host API key. The name carries the
    /// Workspace, so the alias of one person never names another's key.
    fn store_host_key(&self, connection: &Connection, api_key: &str) -> Result<(), ConnectError> {
        self.secrets
            .set(&host_api_key_secret_name(&connection.alias), api_key)
            .map_err(|error| {
                tracing::error!(%error, "storing the mail host API key failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })
    }

    /// Prove the host API key again, and keep a new one when the user
    /// supplies it. A key the host refuses leaves the Connection
    /// `unavailable`: no mailbox is made through it, and the mailboxes
    /// it already made keep working, because the mail path uses each
    /// mailbox's own password (ADR-0019).
    async fn reconnect_mail(
        &self,
        connection: Connection,
        api_key: Option<&str>,
    ) -> Result<Connection, ConnectError> {
        if connection.provider == MANUAL_PROVIDER {
            // There is no key and no host to ask.
            self.set_status(&connection, Connection::CONNECTED).await?;
            return Ok(Connection {
                status: Connection::CONNECTED.to_string(),
                ..connection
            });
        }
        let settings = pagis_mail::mailbox_provider(&connection).ok_or_else(|| {
            ConnectError::Validation("this connection has no mail domain".to_string())
        })?;
        let key = match api_key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => key.to_string(),
            None => self
                .secrets
                .get(&host_api_key_secret_name(&connection.alias))
                .ok()
                .flatten()
                .ok_or_else(|| {
                    ConnectError::Validation("the mail host API key is required".to_string())
                })?,
        };
        let account = HostAccount::new(settings.domain, settings.account.unwrap_or_default(), &key);
        if let Err(error) = self.mail_host.list(&account).await {
            if error.0 == HostErrorCode::Unauthorized {
                self.set_status(&connection, Connection::UNAVAILABLE)
                    .await?;
            }
            return Err(mail_error(error));
        }
        if api_key.is_some() {
            self.store_host_key(&connection, &key)?;
        }
        self.set_status(&connection, Connection::CONNECTED).await?;
        Ok(Connection {
            status: Connection::CONNECTED.to_string(),
            ..connection
        })
    }

    /// Ask the carrier one harmless question with the key. A key the
    /// carrier refuses never reaches the secret store. A provider this
    /// daemon has no client for is refused with the reason.
    async fn prove_carrier_key(
        &self,
        provider: &str,
        key: &CarrierKey,
    ) -> Result<(), ConnectError> {
        if key.expose_secret().is_empty() {
            return Err(ConnectError::Validation(
                "the carrier API key is required".to_string(),
            ));
        }
        let catalog = self.catalogs.get(provider).ok_or_else(|| {
            ConnectError::Validation(format!(
                "this daemon has no client for the carrier provider {provider}"
            ))
        })?;
        let probe = NumberSearch {
            country: "US".to_string(),
            area_code: None,
            locality: None,
            limit: 1,
        };
        match catalog.search(key, &probe).await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.0 {
                CatalogErrorCode::Unauthorized => {
                    ConnectError::Validation("the carrier refused that API key".to_string())
                }
                _ => ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable),
            }),
        }
    }

    /// Keep one Connection's carrier API key, under a name that carries
    /// the Workspace.
    fn store_carrier_key(&self, connection: &Connection, secret: &str) -> Result<(), ConnectError> {
        self.secrets
            .set(
                &carrier_key_secret_name(&connection.provider, &connection.alias),
                secret.trim(),
            )
            .map_err(|error| {
                tracing::error!(%error, "storing the carrier API key failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })
    }

    /// Authorize one Connection for the capabilities the user grants.
    /// This is the connect flow's third step, the repair for
    /// `reauth_required`, and the way scopes widen.
    ///
    /// A Google Connection answers [`Authorization::AtGoogle`] and
    /// returns at once: the person
    /// consents in their own browser, and the redirect that comes back
    /// to [`GoogleBroker::finish`] is what makes the Connection
    /// `connected`. The authorization belongs to `initiator`: only a
    /// browser with a Session of that Person gets to Google, except on a
    /// local installation with one Person. Every other Connection is
    /// finished when this returns.
    pub async fn authorize(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        capabilities: &[String],
        api_key: Option<&str>,
        initiator: &Initiator,
    ) -> Result<Authorization, ConnectError> {
        let connection = self
            .connections
            .get(workspace_id, id)
            .await?
            .ok_or(ConnectError::NotFound)?;
        // The entry says what the Connection is; the repair follows it.
        let gives = catalog::capabilities(&connection.provider);
        if gives.iter().any(|capability| capability == TELEPHONY) {
            return self
                .reconnect_carrier(connection, api_key)
                .await
                .map(Authorization::Done);
        }
        if gives.iter().any(|capability| capability == MAILBOXES) {
            return self
                .reconnect_mail(connection, api_key)
                .await
                .map(Authorization::Done);
        }
        let capabilities = parse_capabilities(capabilities)?;
        let url = self
            .google
            .authorize(&connection, &capabilities, initiator)
            .await?;
        self.set_status(&connection, Connection::CONNECTING).await?;
        Ok(Authorization::AtGoogle {
            connection: Connection {
                status: Connection::CONNECTING.to_string(),
                ..connection
            },
            url,
        })
    }

    /// Prove the carrier key again, and keep a new one when the user
    /// supplies it. There is no browser step: the key is the whole
    /// authorization. The account id stays what the record holds; only
    /// the secret is replaced.
    async fn reconnect_carrier(
        &self,
        connection: Connection,
        api_key: Option<&str>,
    ) -> Result<Connection, ConnectError> {
        let secret = match api_key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => key.to_string(),
            None => self
                .secrets
                .get(&carrier_key_secret_name(
                    &connection.provider,
                    &connection.alias,
                ))
                .ok()
                .flatten()
                .ok_or_else(|| {
                    ConnectError::Validation("the carrier API key is required".to_string())
                })?,
        };
        let account = connection.config[CARRIER_ACCOUNT_KEY]
            .as_str()
            .unwrap_or_default();
        let key = CarrierKey::new(account, secret);
        if let Err(error) = self.prove_carrier_key(&connection.provider, &key).await {
            self.set_status(&connection, Connection::REAUTH_REQUIRED)
                .await?;
            return Err(error);
        }
        if api_key.is_some() {
            self.store_carrier_key(&connection, key.expose_secret())?;
        }
        self.set_status(&connection, Connection::CONNECTED).await?;
        Ok(Connection {
            status: Connection::CONNECTED.to_string(),
            ..connection
        })
    }

    /// Keep the SIP credential of the carrier (ADR-0020): the
    /// username and the registrar in the record, the password in the
    /// secret store. Nothing is proved here; the carrier's line
    /// registers with it at once, and each number's registration state
    /// is the proof the user sees.
    pub async fn set_sip_credential(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        username: &str,
        password: &str,
        domain: &str,
    ) -> Result<Connection, ConnectError> {
        let connection = self
            .connections
            .get(workspace_id, id)
            .await?
            .ok_or(ConnectError::NotFound)?;
        if !catalog::capabilities(&connection.provider)
            .iter()
            .any(|capability| capability == TELEPHONY)
        {
            return Err(ConnectError::Validation(
                "only a telephony connection has a SIP credential".to_string(),
            ));
        }
        let username = username.trim();
        let domain = domain.trim().to_lowercase();
        if username.is_empty() || password.is_empty() {
            return Err(ConnectError::Validation(
                "the SIP username and password are both required".to_string(),
            ));
        }
        if !is_host_name(&domain) {
            return Err(ConnectError::Validation(
                "the SIP domain is a host name, such as sip.example.com".to_string(),
            ));
        }
        // The password lands first: a record that names a credential
        // whose password is missing would register nothing and say
        // `no_credential`, which is the honest state.
        self.secrets
            .set(
                &sip_password_secret_name(&connection.provider, &connection.alias),
                password,
            )
            .map_err(|error| {
                tracing::error!(%error, "storing the SIP password failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?;
        let mut config = connection.config.clone();
        config[SIP_USERNAME_KEY] = serde_json::Value::String(username.to_string());
        config[SIP_DOMAIN_KEY] = serde_json::Value::String(domain);
        if !self
            .connections
            .set_config(workspace_id, id, &config)
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(Connection {
            config,
            ..connection
        })
    }

    /// Forget the SIP credential of the carrier: the password leaves the
    /// secret store and the username and the registrar leave the record.
    /// Each held number then registers with nothing and says
    /// `no_credential`.
    pub async fn clear_sip_credential(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Connection, ConnectError> {
        let connection = self
            .connections
            .get(workspace_id, id)
            .await?
            .ok_or(ConnectError::NotFound)?;
        self.secrets
            .delete(&sip_password_secret_name(
                &connection.provider,
                &connection.alias,
            ))
            .map_err(|error| {
                tracing::error!(%error, "forgetting the SIP password failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?;
        let mut config = connection.config.clone();
        if let Some(map) = config.as_object_mut() {
            map.remove(SIP_USERNAME_KEY);
            map.remove(SIP_DOMAIN_KEY);
        }
        if !self
            .connections
            .set_config(workspace_id, id, &config)
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(Connection {
            config,
            ..connection
        })
    }

    async fn set_status(&self, connection: &Connection, status: &str) -> Result<(), ConnectError> {
        if !self
            .connections
            .set_status(&connection.workspace_id, &connection.id, status)
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(())
    }
}

/// A mail host refusal the user can fix keeps its words; the rest
/// report the stable code and nothing from upstream.
fn mail_error(error: pagis_mail::HostError) -> ConnectError {
    match error.0 {
        HostErrorCode::Unauthorized => {
            ConnectError::Validation("the mail host refused that API key".to_string())
        }
        HostErrorCode::MailboxUnknown | HostErrorCode::Refused => ConnectError::Validation(
            "the mail host does not hold that domain for this account".to_string(),
        ),
        _ => ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable),
    }
}

/// A value the user must type. It reaches no provider when it is
/// missing, so the message names what to type.
fn required(value: &str, message: &str) -> Result<String, ConnectError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ConnectError::Validation(message.to_string()));
    }
    Ok(value.to_string())
}

/// The mail domain of a Mailbox Provider. Addresses are made on it, so
/// it is a host name and it is held in lowercase.
fn mail_domain(domain: &str) -> Result<String, ConnectError> {
    let domain = required(domain, "the mail domain is required")?.to_lowercase();
    if !is_host_name(&domain) || !domain.contains('.') {
        return Err(ConnectError::Validation(
            "the mail domain is a host name, such as example.com".to_string(),
        ));
    }
    Ok(domain)
}

/// One endpoint the transport reaches. `which` names it in the message
/// the user reads.
fn mail_endpoint(endpoint: Endpoint, which: &str) -> Result<Endpoint, ConnectError> {
    let host = endpoint.host.trim().to_lowercase();
    if !is_host_name(&host) {
        return Err(ConnectError::Validation(format!(
            "the {which} host is a host name, such as imap.example.com"
        )));
    }
    if endpoint.port == 0 {
        return Err(ConnectError::Validation(format!(
            "the {which} port is required"
        )));
    }
    Ok(Endpoint::new(host, endpoint.port))
}

/// A name the daemon can resolve: labels of ASCII letters, digits and
/// hyphens, and 253 characters at most.
fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn store_error(error: StoreError) -> ConnectError {
    match error {
        StoreError::Conflict(message) => ConnectError::Conflict(message),
        other => ConnectError::Store(other),
    }
}

/// The alias is model-visible and lands in a `gog` client name, so it
/// stays to the character set both read the same way.
fn validate_alias(alias: &str) -> Result<(), ConnectError> {
    let valid = !alias.is_empty()
        && alias.len() <= 64
        && alias.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        });
    if valid {
        Ok(())
    } else {
        Err(ConnectError::Validation(
            "an alias uses lowercase letters, numbers, hyphens, or underscores".to_string(),
        ))
    }
}

/// An empty request authorizes the read-only profile a Connection starts
/// at; the user widens it by naming the capabilities they grant.
fn parse_capabilities(names: &[String]) -> Result<Vec<GoogleCapability>, ConnectError> {
    if names.is_empty() {
        return Ok(GoogleCapability::READ_ONLY.to_vec());
    }
    names
        .iter()
        .map(|name| {
            GoogleCapability::parse(name)
                .ok_or_else(|| ConnectError::Validation(format!("{name} is not a capability")))
        })
        .collect()
}
