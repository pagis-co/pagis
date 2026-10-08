//! The connect flow contract: what the row records, what reaches the
//! provider, and what a refused or abandoned exchange leaves behind.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_connect::{
    ConnectError, Connector, ConnectorDeps, GoogleBroker, Initiator, NewConnection, NewCredentials,
    Opener, OrgWebClient, StartRefusal,
};
use pagis_core::{
    ClientKind, Connection, ConnectionId, ConnectionStore, MemorySecretStore, Org, OrgId, OrgStore,
    SealedSecret, SecretStore, Session, SessionId, SessionStore, StoreError, TenantKeys,
    UnixMillis, UserId, WorkspaceId, now_ms,
};
use pagis_google::ProviderErrorCode;
use pagis_mail::fake::FakeMailboxHost;
use pagis_telephony::fake::FakeNumberCatalog;
use pagis_telephony::{CARRIER_ACCOUNT_KEY, NumberCatalogs, TELNYX_PROVIDER, TWILIO_PROVIDER};

/// An in-memory `ConnectionStore` that enforces the one uniqueness the
/// schema owns: a workspace alias is taken once.
#[derive(Default)]
struct MemoryConnections {
    rows: Mutex<Vec<Connection>>,
    refresh_tokens: Mutex<std::collections::HashMap<(WorkspaceId, ConnectionId), SealedSecret>>,
}

/// One Org, so the Google half has somewhere to keep the
/// installation's Web OAuth client id.
struct MemoryOrgs {
    rows: Mutex<Vec<Org>>,
}

impl Default for MemoryOrgs {
    fn default() -> Self {
        Self {
            rows: Mutex::new(vec![Org {
                id: OrgId::from("org-1".to_string()),
                name: "Org".to_string(),
                google_client_id: None,
                workspace_id: pagis_core::WorkspaceId::generate(),
                created_at: now_ms(),
            }]),
        }
    }
}

#[async_trait]
impl OrgStore for MemoryOrgs {
    async fn create(&self, org: &Org) -> Result<(), StoreError> {
        self.rows.lock().unwrap().push(org.clone());
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Org>, StoreError> {
        Ok(self.rows.lock().unwrap().clone())
    }

    async fn set_google_client_id(
        &self,
        id: &OrgId,
        client_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        match rows.iter_mut().find(|org| &org.id == id) {
            Some(org) => {
                org.google_client_id = client_id.map(str::to_string);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[async_trait]
impl ConnectionStore for MemoryConnections {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().unwrap();
        if rows
            .iter()
            .any(|row| row.workspace_id == connection.workspace_id && row.alias == connection.alias)
        {
            return Err(StoreError::Conflict(format!(
                "alias {} is taken",
                connection.alias
            )));
        }
        rows.push(connection.clone());
        Ok(())
    }

    async fn set_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        match rows
            .iter_mut()
            .find(|row| &row.workspace_id == workspace_id && &row.id == id)
        {
            Some(row) => {
                row.status = status.to_string();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_authorization(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        match rows
            .iter_mut()
            .find(|row| &row.workspace_id == workspace_id && &row.id == id)
        {
            Some(row) => {
                row.status = Connection::CONNECTED.to_string();
                row.authorized_capabilities = capabilities.to_vec();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn set_config(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        match rows
            .iter_mut()
            .find(|row| &row.workspace_id == workspace_id && &row.id == id)
        {
            Some(row) => {
                row.config = config.clone();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .iter()
            .filter(|row| &row.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .iter()
            .find(|row| &row.workspace_id == workspace_id && &row.id == id)
            .cloned())
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        _revoked_at: i64,
    ) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|row| !(&row.workspace_id == workspace_id && &row.id == id));
        Ok(rows.len() != before)
    }

    async fn set_refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        token: Option<&SealedSecret>,
    ) -> Result<bool, StoreError> {
        let mut tokens = self.refresh_tokens.lock().unwrap();
        let key = (workspace_id.clone(), id.clone());
        match token {
            Some(token) => tokens.insert(key, token.clone()),
            None => tokens.remove(&key),
        };
        Ok(true)
    }

    async fn refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<SealedSecret>, StoreError> {
        Ok(self
            .refresh_tokens
            .lock()
            .unwrap()
            .get(&(workspace_id.clone(), id.clone()))
            .cloned())
    }
}

/// The Sessions of the installation. The Google half reads the
/// Session that started an authorization again at the callback.
#[derive(Default)]
struct MemorySessions {
    rows: Mutex<Vec<Session>>,
}

impl MemorySessions {
    /// A live Session of the Person of `initiator`, under its id.
    fn sign_in(&self, initiator: &Initiator) {
        self.rows.lock().unwrap().push(Session {
            id: initiator.session_id.clone(),
            user_id: initiator.user_id.clone(),
            token_hash: format!("hash-of-{}", initiator.session_id.as_str()),
            client_kind: ClientKind::Browser,
            client_name: None,
            created_at: 0,
            last_used_at: 0,
            expires_at: i64::MAX,
        });
    }
}

#[async_trait]
impl SessionStore for MemorySessions {
    async fn create(&self, session: &Session) -> Result<(), StoreError> {
        self.rows.lock().unwrap().push(session.clone());
        Ok(())
    }

    async fn find_live(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|session| session.token_hash == token_hash && session.expires_at > now)
            .cloned())
    }

    async fn find_live_by_id(
        &self,
        id: &SessionId,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|session| &session.id == id && session.expires_at > now)
            .cloned())
    }

    async fn touch(
        &self,
        _id: &SessionId,
        _at: UnixMillis,
        _expires_at: UnixMillis,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list_live_for_user(
        &self,
        user_id: &UserId,
        now: UnixMillis,
    ) -> Result<Vec<Session>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|session| &session.user_id == user_id && session.expires_at > now)
            .cloned()
            .collect())
    }

    async fn list_live(&self, now: UnixMillis) -> Result<Vec<Session>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|session| session.expires_at > now)
            .cloned()
            .collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<bool, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|session| &session.id != id);
        Ok(rows.len() < before)
    }

    async fn delete_for_user(&self, user_id: &UserId) -> Result<u64, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|session| &session.user_id != user_id);
        Ok((before - rows.len()) as u64)
    }

    async fn delete_expired(&self, now: UnixMillis) -> Result<u64, StoreError> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|session| session.expires_at > now);
        Ok((before - rows.len()) as u64)
    }
}

/// The Person and the Session that start every test authorization.
fn initiator() -> Initiator {
    Initiator {
        user_id: UserId::from("person-1".to_string()),
        session_id: SessionId::from("session-1".to_string()),
    }
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from("ws-1".to_string())
}

/// A Google connection request. The person types nothing for Google.
fn new_google(alias: &str) -> NewConnection {
    NewConnection {
        workspace_id: workspace(),
        alias: alias.to_string(),
        display_name: "Google".to_string(),
        credentials: NewCredentials::Google,
    }
}

/// A carrier connection request with an API key the fake accepts.
fn new_carrier(alias: &str, api_key: &str) -> NewConnection {
    NewConnection {
        workspace_id: workspace(),
        alias: alias.to_string(),
        display_name: "Telnyx".to_string(),
        credentials: NewCredentials::Carrier {
            provider: TELNYX_PROVIDER.to_string(),
            account: String::new(),
            secret: api_key.to_string(),
        },
    }
}

/// The Google half of an installation whose Org holds no Installation
/// OAuth Client: no person connects Google there.
fn unset_google(connections: Arc<MemoryConnections>) -> Arc<GoogleBroker> {
    google_broker(
        connections,
        Arc::new(MemoryOrgs::default()),
        Arc::new(MemorySecretStore::default()),
        Arc::new(MemorySessions::default()),
        "https://oauth2.example.test/token",
    )
}

fn google_broker(
    connections: Arc<MemoryConnections>,
    orgs: Arc<MemoryOrgs>,
    secrets: Arc<MemorySecretStore>,
    sessions: Arc<MemorySessions>,
    token_endpoint: &str,
) -> Arc<GoogleBroker> {
    Arc::new(GoogleBroker::new(
        connections as _,
        Arc::new(OrgWebClient::new(orgs as _, secrets as _)),
        Arc::new(pagis_google::GoogleOAuth::with_endpoints(
            "https://accounts.example.test/authorize",
            token_endpoint,
        )),
        Arc::new(TenantKeys::new(Arc::new(MemorySecretStore::default()))),
        sessions as _,
        Arc::new(pagis_core::SystemClock),
        "https://pagis.example.net",
    ))
}

fn connector() -> (Connector, Arc<MemoryConnections>) {
    let connections = Arc::new(MemoryConnections::default());
    let connector = Connector::new(ConnectorDeps {
        connections: Arc::clone(&connections) as _,
        catalogs: Arc::new(NumberCatalogs::single(
            TELNYX_PROVIDER,
            Arc::new(FakeNumberCatalog::default()),
        )),
        mail_host: Arc::new(FakeMailboxHost::default()),
        secrets: Arc::new(MemorySecretStore::default()),
        google: unset_google(Arc::clone(&connections)),
    });
    (connector, connections)
}

/// No person connects Google where the Org holds no Installation OAuth
/// Client (ADR-0012). The refusal names who sets it up, and no row
/// takes the alias.
#[tokio::test]
async fn a_google_connection_without_the_installation_client_is_refused_and_leaves_no_row() {
    let (connector, connections) = connector();

    let refused = connector
        .create(new_google("work"))
        .await
        .expect_err("the installation holds no Google client");

    match refused {
        ConnectError::Validation(message) => assert!(
            message.contains("sets up Google sign-in in the Administration Interface"),
            "{message}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(connections.list(&workspace()).await.unwrap().is_empty());
}

/// The carrier connection: the key is proved, then stored, and
/// the row lands at `connected` with no browser step.
fn carrier_connector(
    catalog: Arc<FakeNumberCatalog>,
) -> (Connector, Arc<MemoryConnections>, Arc<MemorySecretStore>) {
    let connections = Arc::new(MemoryConnections::default());
    let secrets = Arc::new(MemorySecretStore::default());
    let connector = Connector::new(ConnectorDeps {
        connections: Arc::clone(&connections) as _,
        catalogs: Arc::new(NumberCatalogs::single(TELNYX_PROVIDER, catalog)),
        mail_host: Arc::new(FakeMailboxHost::default()),
        secrets: Arc::clone(&secrets) as _,
        google: unset_google(Arc::clone(&connections)),
    });
    (connector, connections, secrets)
}

#[tokio::test]
async fn connecting_a_carrier_proves_the_key_and_lands_connected() {
    let (connector, connections, secrets) =
        carrier_connector(Arc::new(FakeNumberCatalog::default()));

    let created = connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .expect("create");

    assert_eq!(created.provider, "telnyx");
    assert_eq!(created.status, Connection::CONNECTED);
    // The key is in the secret store, and nowhere in the row.
    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        Some("telnyx-key".to_string())
    );
    assert_eq!(created.config, serde_json::json!({}));
    assert_eq!(connections.list(&workspace()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_key_the_carrier_refuses_leaves_no_record() {
    let catalog = Arc::new(FakeNumberCatalog::default());
    catalog.fail_with(Some(pagis_telephony::CatalogErrorCode::Unauthorized));
    let (connector, connections, secrets) = carrier_connector(catalog);

    let refused = connector.create(new_carrier("carrier", "wrong")).await;

    assert!(matches!(refused, Err(ConnectError::Validation(_))));
    assert!(connections.list(&workspace()).await.unwrap().is_empty());
    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        None
    );
}

/// A carrier that signs with an account id and a secret: the
/// account is not secret and lands in the record; the secret goes to
/// the secret store and nowhere else.
#[tokio::test]
async fn a_carrier_account_lands_in_the_record_and_the_secret_in_the_store() {
    let catalog = Arc::new(FakeNumberCatalog::default());
    let (connector, _, secrets) = carrier_connector(Arc::clone(&catalog));

    let connection = connector
        .create(NewConnection {
            workspace_id: workspace(),
            alias: "carrier".to_string(),
            display_name: "Carrier".to_string(),
            credentials: NewCredentials::Carrier {
                provider: TELNYX_PROVIDER.to_string(),
                account: " AC123 ".to_string(),
                secret: "shh".to_string(),
            },
        })
        .await
        .expect("create");

    assert_eq!(connection.config[CARRIER_ACCOUNT_KEY], "AC123");
    assert!(!connection.config.to_string().contains("shh"));
    let proved = catalog.last_key().expect("the key was proved");
    assert_eq!(proved.account(), "AC123");
    assert_eq!(proved.expose_secret(), "shh");
    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap()
            .as_deref(),
        Some("shh")
    );
}

#[tokio::test]
async fn one_workspace_has_one_carrier() {
    let (connector, _, _) = carrier_connector(Arc::new(FakeNumberCatalog::default()));
    connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .expect("create");

    let refused = connector.create(new_carrier("second", "telnyx-key")).await;

    assert!(matches!(refused, Err(ConnectError::Conflict(_))));
}

/// The one carrier of a Workspace is one of any provider, so a
/// second carrier is refused even when it names another provider.
#[tokio::test]
async fn one_workspace_has_one_carrier_of_any_provider() {
    let catalog = Arc::new(FakeNumberCatalog::default());
    let connections = Arc::new(MemoryConnections::default());
    let connector = Connector::new(ConnectorDeps {
        connections: Arc::clone(&connections) as _,
        catalogs: Arc::new(
            NumberCatalogs::new()
                .with(TELNYX_PROVIDER, Arc::clone(&catalog) as _)
                .with(TWILIO_PROVIDER, Arc::clone(&catalog) as _),
        ),
        mail_host: Arc::new(FakeMailboxHost::default()),
        secrets: Arc::new(MemorySecretStore::default()) as _,
        google: unset_google(Arc::clone(&connections)),
    });
    connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .expect("create");

    let refused = connector
        .create(NewConnection {
            workspace_id: workspace(),
            alias: "twilio".to_string(),
            display_name: "Twilio".to_string(),
            credentials: NewCredentials::Carrier {
                provider: TWILIO_PROVIDER.to_string(),
                account: "AC123".to_string(),
                secret: "token".to_string(),
            },
        })
        .await;

    assert!(matches!(refused, Err(ConnectError::Conflict(_))));
}

#[tokio::test]
async fn a_carrier_key_the_user_replaces_is_proved_again() {
    let catalog = Arc::new(FakeNumberCatalog::default());
    let (connector, _, secrets) = carrier_connector(Arc::clone(&catalog));
    let created = connector
        .create(new_carrier("carrier", "old-key"))
        .await
        .expect("create");

    connector
        .authorize(
            &workspace(),
            &created.id,
            &[],
            Some("new-key"),
            &initiator(),
        )
        .await
        .expect("authorize");

    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        Some("new-key".to_string())
    );

    // A key the carrier now refuses takes the record to reauth.
    catalog.fail_with(Some(pagis_telephony::CatalogErrorCode::Unauthorized));
    let refused = connector
        .authorize(&workspace(), &created.id, &[], Some("stale"), &initiator())
        .await;
    assert!(matches!(refused, Err(ConnectError::Validation(_))));
}

#[tokio::test]
async fn an_alias_a_tool_call_cannot_name_is_refused() {
    let (connector, connections) = connector();
    for alias in ["", "Work Mail", "work mail", "work/mail"] {
        let refused = connector.create(new_carrier(alias, "good-key")).await;
        assert!(
            matches!(refused, Err(ConnectError::Validation(_))),
            "{alias:?} must be refused"
        );
    }
    assert!(connections.list(&workspace()).await.unwrap().is_empty());
}

#[tokio::test]
async fn authorizing_a_connection_that_is_gone_reports_not_found() {
    let (connector, _) = connector();
    let missing = connector
        .authorize(
            &workspace(),
            &ConnectionId::from("nope".to_string()),
            &[],
            None,
            &initiator(),
        )
        .await;
    assert!(matches!(missing, Err(ConnectError::NotFound)));
}

// The SIP credential (ADR-0020): the public half in the record,
// the password in the store, and nothing proved here.

#[tokio::test]
async fn a_sip_credential_lands_in_the_record_and_the_store() {
    let (connector, connections, secrets) =
        carrier_connector(Arc::new(FakeNumberCatalog::default()));
    let created = connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .unwrap();

    let updated = connector
        .set_sip_credential(
            &workspace(),
            &created.id,
            " robin ",
            "sip-secret",
            "SIP.Telnyx.com",
        )
        .await
        .unwrap();

    assert_eq!(
        pagis_telephony::sip_identity(&updated),
        Some(("robin".to_string(), "sip.telnyx.com".to_string()))
    );
    let stored = connections
        .get(&workspace(), &created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.config, updated.config);
    assert!(stored.config.get("sip_password").is_none());
    assert_eq!(
        secrets
            .get(&pagis_telephony::sip_password_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        Some("sip-secret".to_string())
    );
    // The API key is untouched beside it.
    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        Some("telnyx-key".to_string())
    );
}

/// Forgetting the SIP credential takes the password out of the store
/// and the public half out of the record, and leaves the API key.
#[tokio::test]
async fn a_cleared_sip_credential_leaves_the_record_and_the_store() {
    let (connector, connections, secrets) =
        carrier_connector(Arc::new(FakeNumberCatalog::default()));
    let created = connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .unwrap();
    connector
        .set_sip_credential(
            &workspace(),
            &created.id,
            "robin",
            "sip-secret",
            "sip.telnyx.com",
        )
        .await
        .unwrap();

    let cleared = connector
        .clear_sip_credential(&workspace(), &created.id)
        .await
        .unwrap();

    assert_eq!(pagis_telephony::sip_identity(&cleared), None);
    let stored = connections
        .get(&workspace(), &created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pagis_telephony::sip_identity(&stored), None);
    assert_eq!(
        secrets
            .get(&pagis_telephony::sip_password_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        None
    );
    assert_eq!(
        secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        Some("telnyx-key".to_string())
    );
}

#[tokio::test]
async fn a_sip_credential_with_a_part_missing_is_refused_before_anything_is_written() {
    let (connector, connections, secrets) =
        carrier_connector(Arc::new(FakeNumberCatalog::default()));
    let created = connector
        .create(new_carrier("carrier", "telnyx-key"))
        .await
        .unwrap();

    for (username, password, domain) in [
        ("", "secret", "sip.telnyx.com"),
        ("robin", "", "sip.telnyx.com"),
        ("robin", "secret", ""),
        ("robin", "secret", "sip.telnyx.com:5061"),
        ("robin", "secret", "sip telnyx com"),
    ] {
        let refused = connector
            .set_sip_credential(&workspace(), &created.id, username, password, domain)
            .await;
        assert!(
            matches!(refused, Err(ConnectError::Validation(_))),
            "{username:?} {password:?} {domain:?} must be refused"
        );
    }
    let stored = connections
        .get(&workspace(), &created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pagis_telephony::sip_identity(&stored), None);
    assert_eq!(
        secrets
            .get(&pagis_telephony::sip_password_secret_name(
                TELNYX_PROVIDER,
                "carrier"
            ))
            .unwrap(),
        None
    );
}

/// The Mailbox Provider connection (ADR-0019): a Migadu key that lists
/// the domain's mailboxes, or a manual host with no key at all.
fn mail_connector(
    host: Arc<FakeMailboxHost>,
) -> (Connector, Arc<MemoryConnections>, Arc<MemorySecretStore>) {
    let connections = Arc::new(MemoryConnections::default());
    let secrets = Arc::new(MemorySecretStore::default());
    let connector = Connector::new(ConnectorDeps {
        connections: Arc::clone(&connections) as _,
        catalogs: Arc::new(NumberCatalogs::single(
            TELNYX_PROVIDER,
            Arc::new(FakeNumberCatalog::default()),
        )),
        mail_host: host,
        secrets: Arc::clone(&secrets) as _,
        google: unset_google(Arc::clone(&connections)),
    });
    (connector, connections, secrets)
}

fn new_migadu(alias: &str, api_key: &str) -> NewConnection {
    NewConnection {
        workspace_id: workspace(),
        alias: alias.to_string(),
        display_name: "Migadu".to_string(),
        credentials: NewCredentials::Migadu {
            account: "owner@example.com".to_string(),
            api_key: api_key.to_string(),
            domain: "Example.com".to_string(),
        },
    }
}

fn new_manual(alias: &str) -> NewConnection {
    NewConnection {
        workspace_id: workspace(),
        alias: alias.to_string(),
        display_name: "Fastmail".to_string(),
        credentials: NewCredentials::Manual {
            domain: "example.org".to_string(),
            imap: pagis_mail::Endpoint::new("imap.fastmail.com", 993),
            smtp: pagis_mail::Endpoint::new("smtp.fastmail.com", 465),
        },
    }
}

#[tokio::test]
async fn connecting_migadu_lists_the_domain_and_lands_connected() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, connections, secrets) = mail_connector(Arc::clone(&host));

    let created = connector.create(new_migadu("mail", "migadu-key")).await;
    let created = created.expect("create");

    assert_eq!(created.provider, pagis_mail::MIGADU_PROVIDER);
    assert_eq!(created.status, Connection::CONNECTED);
    // The key answered the one harmless question before the row existed.
    assert_eq!(host.calls(), vec![pagis_mail::fake::HostCall::List]);
    let settings = pagis_mail::mailbox_provider(&created).expect("mail settings");
    assert_eq!(settings.domain, "example.com");
    assert_eq!(settings.account.as_deref(), Some("owner@example.com"));
    assert_eq!(
        settings.imap,
        pagis_mail::Endpoint::new("imap.migadu.com", 993)
    );
    assert_eq!(
        settings.smtp,
        pagis_mail::Endpoint::new("smtp.migadu.com", 465)
    );
    // The key is in the secret store under the alias, and nowhere else.
    assert_eq!(
        secrets
            .get(&pagis_mail::host_api_key_secret_name("mail"))
            .unwrap()
            .as_deref(),
        Some("migadu-key")
    );
    let stored = &connections.list(&workspace()).await.unwrap()[0];
    assert!(
        !serde_json::to_string(stored)
            .unwrap()
            .contains("migadu-key")
    );
}

#[tokio::test]
async fn a_migadu_record_declares_what_both_seams_do() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, _, _) = mail_connector(host);

    let created = connector
        .create(new_migadu("mail", "migadu-key"))
        .await
        .expect("create");

    let capabilities = pagis_mail::mailbox_provider(&created)
        .expect("mail settings")
        .capabilities;
    assert_eq!(
        capabilities,
        pagis_mail::MailboxCapabilities {
            idle: true,
            outgoing_cap: true,
            delete_mailbox: true,
            reset_password: true,
        }
    );
}

#[tokio::test]
async fn a_key_the_mail_host_refuses_leaves_no_record_and_no_secret() {
    let host = Arc::new(FakeMailboxHost::default());
    host.fail_with(Some(pagis_mail::HostErrorCode::Unauthorized));
    let (connector, connections, secrets) = mail_connector(host);

    let refused = connector.create(new_migadu("mail", "wrong")).await;

    assert!(matches!(refused, Err(ConnectError::Validation(_))));
    assert!(connections.list(&workspace()).await.unwrap().is_empty());
    assert_eq!(
        secrets
            .get(&pagis_mail::host_api_key_secret_name("mail"))
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_manual_provider_lands_connected_and_keeps_no_key() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, _, secrets) = mail_connector(Arc::clone(&host));

    let created = connector.create(new_manual("mail")).await.expect("create");

    assert_eq!(created.provider, pagis_mail::MANUAL_PROVIDER);
    assert_eq!(created.status, Connection::CONNECTED);
    // No host is asked, because the user makes each mailbox there.
    assert!(host.calls().is_empty());
    let settings = pagis_mail::mailbox_provider(&created).expect("mail settings");
    assert_eq!(settings.account, None);
    assert_eq!(
        settings.imap,
        pagis_mail::Endpoint::new("imap.fastmail.com", 993)
    );
    assert_eq!(
        settings.capabilities,
        pagis_mail::MailboxCapabilities {
            idle: true,
            outgoing_cap: false,
            delete_mailbox: false,
            reset_password: false,
        }
    );
    assert_eq!(
        secrets
            .get(&pagis_mail::host_api_key_secret_name("mail"))
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_mail_domain_that_is_not_a_host_name_is_refused() {
    let (connector, connections, _) = mail_connector(Arc::new(FakeMailboxHost::default()));

    let refused = connector
        .create(NewConnection {
            workspace_id: workspace(),
            alias: "mail".to_string(),
            display_name: "Migadu".to_string(),
            credentials: NewCredentials::Migadu {
                account: "owner@example.com".to_string(),
                api_key: "migadu-key".to_string(),
                domain: "not a domain".to_string(),
            },
        })
        .await;

    assert!(matches!(refused, Err(ConnectError::Validation(_))));
    assert!(connections.list(&workspace()).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_revoked_mail_key_makes_the_connection_unavailable() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, connections, _) = mail_connector(Arc::clone(&host));
    let created = connector
        .create(new_migadu("mail", "migadu-key"))
        .await
        .expect("create");

    // The user revoked the key at the host.
    host.fail_with(Some(pagis_mail::HostErrorCode::Unauthorized));
    let refused = connector
        .authorize(&workspace(), &created.id, &[], None, &initiator())
        .await;

    assert!(matches!(refused, Err(ConnectError::Validation(_))));
    assert_eq!(
        connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Connection::UNAVAILABLE
    );
}

#[tokio::test]
async fn a_replacement_mail_key_is_proved_and_kept() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, connections, secrets) = mail_connector(Arc::clone(&host));
    let created = connector
        .create(new_migadu("mail", "old-key"))
        .await
        .expect("create");
    host.fail_with(Some(pagis_mail::HostErrorCode::Unauthorized));
    let _ = connector
        .authorize(&workspace(), &created.id, &[], None, &initiator())
        .await;

    host.fail_with(None);
    let repaired = connector
        .authorize(
            &workspace(),
            &created.id,
            &[],
            Some("new-key"),
            &initiator(),
        )
        .await
        .expect("authorize");

    assert_eq!(repaired.connection().status, Connection::CONNECTED);
    assert_eq!(
        connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        Connection::CONNECTED
    );
    assert_eq!(
        secrets
            .get(&pagis_mail::host_api_key_secret_name("mail"))
            .unwrap()
            .as_deref(),
        Some("new-key")
    );
}

#[tokio::test]
async fn a_manual_provider_has_no_key_to_prove() {
    let host = Arc::new(FakeMailboxHost::default());
    let (connector, _, _) = mail_connector(Arc::clone(&host));
    let created = connector.create(new_manual("mail")).await.expect("create");

    let authorized = connector
        .authorize(&workspace(), &created.id, &[], None, &initiator())
        .await
        .expect("authorize");

    assert_eq!(authorized.connection().status, Connection::CONNECTED);
    assert!(host.calls().is_empty());
}

/// The carrier key and the mail host key are the installation's, not a
/// Workspace's.
///
/// The carrier connection and the mail domain belong to the Org: one
/// installation buys the numbers and owns the mail domain, and an
/// Administrator alone configures either. The secret name therefore
/// carries no Workspace, and rotating a key is one write rather than one
/// per person. The API is what holds the invariant: the create route
/// refuses a Member, so no second person writes a carrier Connection at
/// all (`crates/pagis/tests/administration.rs`).
#[tokio::test]
async fn the_carrier_key_and_the_mail_host_key_are_the_installations() {
    let (connector, _, secrets) = carrier_connector(Arc::new(FakeNumberCatalog::default()));

    connector
        .create(new_carrier("telnyx", "the-installations-key"))
        .await
        .expect("the administrator connects the carrier");

    let name = pagis_telephony::carrier_key_secret_name(TELNYX_PROVIDER, "telnyx");
    assert_eq!(
        secrets.get(&name).unwrap().as_deref(),
        Some("the-installations-key")
    );
    // The name names the provider and the alias, and no Workspace, so
    // a second Workspace cannot hold a copy of it.
    assert!(!name.contains("workspace/"), "{name}");

    // The mail host key is filed the same way.
    let host = Arc::new(FakeMailboxHost::default());
    let (mail, _, mail_secrets) = mail_connector(Arc::clone(&host));
    mail.create(new_migadu("mail", "the-installations-mail-key"))
        .await
        .expect("the administrator connects the mail domain");

    let mail_name = pagis_mail::host_api_key_secret_name("mail");
    assert_eq!(
        mail_secrets.get(&mail_name).unwrap().as_deref(),
        Some("the-installations-mail-key")
    );
    assert!(!mail_name.contains("workspace/"), "{mail_name}");
}

/// A mailbox password stays the Workspace's, because mailboxes stay per
/// Workspace even though the domain is the Org's. Two people
/// who hold the same address keep separate passwords.
#[tokio::test]
async fn a_mailbox_password_carries_its_workspace() {
    let mine = pagis_mail::mailbox_password_secret_name(&workspace(), "sage@example.com");
    let theirs = pagis_mail::mailbox_password_secret_name(
        &WorkspaceId::from("ws-2".to_string()),
        "sage@example.com",
    );

    assert_ne!(mine, theirs);
    assert!(mine.starts_with("workspace/"), "{mine}");
}

/// The Google flow (ADR-0012): the installation holds one Web OAuth
/// client, the person holds the consent, and the daemon holds the
/// refresh token sealed with that person's Tenant Data Key.
mod installation_client {
    use super::*;

    use std::collections::HashMap;
    use std::net::SocketAddr;

    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;

    /// Every scope the fake grants unless a test says less: the identity
    /// scopes and the scope of every capability.
    const EVERY_SCOPE: &str = "openid https://www.googleapis.com/auth/userinfo.email \
        https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.send \
        https://www.googleapis.com/auth/gmail.modify \
        https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/calendar";

    /// A fake Google token endpoint. These tests never reach
    /// Google: they reach this. It grants every scope, and its ID token
    /// names the account that a test gives each code, or
    /// `alice@example.com`.
    #[derive(Default)]
    struct FakeGoogle {
        calls: Mutex<Vec<String>>,
        refusals: Mutex<usize>,
        /// The Google account that consented, by the code of the
        /// redirect.
        accounts: Mutex<HashMap<String, String>>,
    }

    impl FakeGoogle {
        /// The person who brings `code` consented as `account`.
        fn consent(&self, code: &str, account: &str) {
            self.accounts
                .lock()
                .unwrap()
                .insert(code.to_string(), account.to_string());
        }
    }

    async fn token(
        State(google): State<Arc<FakeGoogle>>,
        body: String,
    ) -> (axum::http::StatusCode, String) {
        google.calls.lock().unwrap().push(body.clone());
        let mut refusals = google.refusals.lock().unwrap();
        if *refusals > 0 {
            *refusals -= 1;
            return (
                axum::http::StatusCode::BAD_REQUEST,
                r#"{"error":"invalid_grant"}"#.to_string(),
            );
        }
        // The answer names the code or the refresh token it was given,
        // so a test can tell one person's tokens from another's.
        let of = |key: &str| {
            body.split('&')
                .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
                .unwrap_or_default()
                .to_string()
        };
        let mark = match of("code").is_empty() {
            false => of("code"),
            true => of("refresh_token").replace("%2F", "/"),
        };
        let account = google
            .accounts
            .lock()
            .unwrap()
            .get(&mark)
            .cloned()
            .unwrap_or_else(|| "alice@example.com".to_string());
        let answer = serde_json::json!({
            "access_token": format!("access-for-{mark}"),
            "refresh_token": format!("refresh-for-{mark}"),
            "expires_in": 3599,
            "scope": EVERY_SCOPE,
            "id_token": id_token(serde_json::json!({
                "aud": "installation-id",
                "email": account,
                "email_verified": true,
            })),
        });
        (axum::http::StatusCode::OK, answer.to_string())
    }

    /// An ID token with these claims. The daemon checks no signature, so
    /// the signature is a placeholder.
    fn id_token(claims: serde_json::Value) -> String {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            URL_SAFE_NO_PAD.encode("not-a-signature"),
        )
    }

    async fn serve(google: Arc<FakeGoogle>) -> SocketAddr {
        let app = Router::new()
            .route("/token", post(token))
            .with_state(google);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    struct Installation {
        connector: Connector,
        connections: Arc<MemoryConnections>,
        broker: Arc<GoogleBroker>,
        google: Arc<FakeGoogle>,
        sessions: Arc<MemorySessions>,
    }

    /// One installation whose Org registered a Google Web client. The
    /// Person of [`initiator`] is signed in.
    async fn installation() -> Installation {
        let google = Arc::new(FakeGoogle::default());
        let addr = serve(Arc::clone(&google)).await;
        let connections = Arc::new(MemoryConnections::default());
        let orgs = Arc::new(MemoryOrgs::default());
        let secrets = Arc::new(MemorySecretStore::default());
        let sessions = Arc::new(MemorySessions::default());
        sessions.sign_in(&initiator());
        let broker = google_broker(
            Arc::clone(&connections),
            Arc::clone(&orgs),
            Arc::clone(&secrets),
            Arc::clone(&sessions),
            &format!("http://{addr}/token"),
        );
        OrgWebClient::new(orgs as _, secrets as _)
            .register("installation-id", "installation-secret")
            .await
            .expect("register the installation's Web client");
        let connector = Connector::new(ConnectorDeps {
            connections: Arc::clone(&connections) as _,
            catalogs: Arc::new(NumberCatalogs::single(
                TELNYX_PROVIDER,
                Arc::new(FakeNumberCatalog::default()),
            )),
            mail_host: Arc::new(FakeMailboxHost::default()),
            secrets: Arc::new(MemorySecretStore::default()),
            google: Arc::clone(&broker),
        });
        Installation {
            connector,
            connections,
            broker,
            google,
            sessions,
        }
    }

    fn google_connection(workspace: &WorkspaceId, alias: &str) -> NewConnection {
        NewConnection {
            workspace_id: workspace.clone(),
            ..new_google(alias)
        }
    }

    /// The `state` of an address.
    fn state_of(url: &str) -> String {
        url.split(['?', '&'])
            .find_map(|pair| pair.strip_prefix("state="))
            .expect("the address carries a state")
            .to_string()
    }

    /// Create a Google Connection and authorize it as [`initiator`].
    /// Answer the Connection and the `state`.
    async fn started(installation: &Installation, capabilities: &[&str]) -> (Connection, String) {
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");
        let capabilities = capabilities
            .iter()
            .map(|capability| capability.to_string())
            .collect::<Vec<_>>();
        let answer = installation
            .connector
            .authorize(&workspace(), &created.id, &capabilities, None, &initiator())
            .await
            .expect("authorize");
        let state = state_of(answer.url().expect("the Google flow answers an address"));
        (created, state)
    }

    /// Authorize a Connection again, as [`initiator`]. Answer the
    /// `state`.
    async fn authorized(installation: &Installation, connection: &Connection) -> String {
        let answer = installation
            .connector
            .authorize(&workspace(), &connection.id, &[], None, &initiator())
            .await
            .expect("authorize");
        state_of(answer.url().expect("the Google flow answers an address"))
    }

    /// A Google Connection starts with no account and asks the person
    /// for nothing. The person consents in their own browser, so the
    /// request comes from any machine.
    #[tokio::test]
    async fn a_google_connection_starts_with_no_account() {
        let installation = installation().await;

        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");

        assert_eq!(created.status, Connection::DISCONNECTED);
        assert_eq!(created.config, serde_json::json!({ "client": "google" }));
    }

    /// The alias is how a tool call picks the account, so a Workspace
    /// takes each alias once.
    #[tokio::test]
    async fn two_connections_get_distinct_aliases_and_a_duplicate_is_refused() {
        let installation = installation().await;
        for alias in ["work", "personal"] {
            installation
                .connector
                .create(google_connection(&workspace(), alias))
                .await
                .expect("create");
        }

        let duplicate = installation
            .connector
            .create(google_connection(&workspace(), "work"))
            .await;

        assert!(matches!(duplicate, Err(ConnectError::Conflict(_))));
        assert_eq!(
            installation
                .connections
                .list(&workspace())
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// A capability Pagis does not define never reaches Google.
    #[tokio::test]
    async fn a_capability_nobody_defines_never_reaches_the_provider() {
        let installation = installation().await;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");

        let refused = installation
            .connector
            .authorize(
                &workspace(),
                &created.id,
                &["drive_read".to_string()],
                None,
                &initiator(),
            )
            .await;

        assert!(matches!(refused, Err(ConnectError::Validation(_))));
        assert!(installation.broker.pending().is_empty());
    }

    /// The authorize step answers the start route on the Public Origin
    /// and returns. It waits for nobody, and the record is `connecting`
    /// until the redirect lands.
    #[tokio::test]
    async fn authorize_answers_the_start_route_and_does_not_wait() {
        let installation = installation().await;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");

        let answer = installation
            .connector
            .authorize(&workspace(), &created.id, &[], None, &initiator())
            .await
            .expect("authorize");

        let url = answer.url().expect("the Google flow answers an address");
        assert_eq!(
            url,
            format!(
                "https://pagis.example.net/api/v1/connections/google/start?state={}",
                state_of(url)
            )
        );
        assert_eq!(answer.connection().status, Connection::CONNECTING);
        // Nothing was asked of Google yet: the person has not consented.
        assert!(installation.google.calls.lock().unwrap().is_empty());
    }

    /// The start route sends a browser of the initiating Person to
    /// Google, and nobody else's. It spends nothing.
    #[tokio::test]
    async fn the_start_route_answers_google_for_the_initiating_person_alone() {
        let installation = installation().await;
        let (_, state) = started(&installation, &["gmail_read"]).await;

        let google = installation
            .broker
            .start(&state, Opener::SignedIn(&initiator().user_id))
            .expect("the initiating Person goes to Google");

        assert!(
            google.starts_with("https://accounts.example.test/authorize?"),
            "{google}"
        );
        assert_eq!(state_of(&google), state);
        // The first consent names no account: the person picks one in
        // Google's account chooser.
        assert!(!google.contains("login_hint"), "{google}");
        assert!(
            google.contains("prompt=select_account%20consent"),
            "{google}"
        );
        assert!(google.contains("code_challenge_method=S256"), "{google}");
        assert!(
            google.contains(
                "redirect_uri=https%3A%2F%2Fpagis.example.net%2Fapi%2Fv1%2Fconnections%2Fgoogle%2Fcallback"
            ),
            "{google}"
        );
        assert_eq!(
            installation.broker.start(
                &state,
                Opener::SignedIn(&UserId::from("person-2".to_string()))
            ),
            Err(StartRefusal::AnotherPerson)
        );
        assert_eq!(
            installation
                .broker
                .start("not-a-state", Opener::SignedIn(&initiator().user_id)),
            Err(StartRefusal::NotWaiting)
        );
        // An installation with one Person asks the browser for no
        // Session, and a `state` that waits for nobody still goes nowhere.
        assert_eq!(
            installation.broker.start(&state, Opener::ThisMachine),
            Ok(google.clone())
        );
        assert_eq!(
            installation
                .broker
                .start("not-a-state", Opener::ThisMachine),
            Err(StartRefusal::NotWaiting)
        );
        assert_eq!(installation.broker.pending().len(), 1);
    }

    /// The Session that started an authorization must be live when
    /// Google sends the browser back. After a sign-out the daemon
    /// redeems no code and keeps no token.
    #[tokio::test]
    async fn a_session_that_ended_finishes_nothing() {
        let installation = installation().await;
        let (created, state) = started(&installation, &["gmail_read"]).await;
        installation
            .sessions
            .delete(&initiator().session_id)
            .await
            .unwrap();

        let refused = installation.broker.finish(&state, "alices-code").await;

        assert!(matches!(refused, Err(ConnectError::Validation(_))));
        assert!(installation.google.calls.lock().unwrap().is_empty());
        let stored = installation
            .connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, Connection::DISCONNECTED);
        assert!(
            installation
                .connections
                .refresh_token(&workspace(), &created.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The first consent of a Connection records the account that
    /// consented, in lower case, and a later consent asks Google for
    /// that account.
    #[tokio::test]
    async fn the_first_consent_records_the_account_that_consented() {
        let installation = installation().await;
        installation.google.consent("bobs-code", "Bob@Example.com");
        let (created, state) = started(&installation, &["gmail_read"]).await;

        let connected = installation
            .broker
            .finish(&state, "bobs-code")
            .await
            .expect("the callback finishes");

        assert_eq!(connected.config["account"], "bob@example.com");
        let stored = installation
            .connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.config["account"], "bob@example.com");
        assert_eq!(stored.config["client"], "google");

        let again = authorized(&installation, &stored).await;
        let google = installation
            .broker
            .start(&again, Opener::SignedIn(&initiator().user_id))
            .unwrap();
        assert!(google.contains("login_hint=bob%40example.com"), "{google}");
        assert!(google.contains("prompt=consent&"), "{google}");
    }

    /// Once a Connection holds an account, the daemon keeps a token only
    /// for that account, whatever account the person picked at Google.
    #[tokio::test]
    async fn a_new_consent_from_another_account_stores_no_token() {
        let installation = installation().await;
        installation
            .google
            .consent("mallorys-code", "mallory@example.com");
        let (created, state) = started(&installation, &["gmail_read"]).await;
        installation
            .broker
            .finish(&state, "alices-code")
            .await
            .expect("Alice connects");
        let alices = installation
            .connections
            .refresh_token(&workspace(), &created.id)
            .await
            .unwrap();
        let stored = installation
            .connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap();

        let again = authorized(&installation, &stored).await;
        let refused = installation.broker.finish(&again, "mallorys-code").await;

        assert!(matches!(refused, Err(ConnectError::Validation(_))));
        let stored = installation
            .connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, Connection::DISCONNECTED);
        assert_eq!(stored.config["account"], "alice@example.com");
        assert_eq!(
            installation
                .connections
                .refresh_token(&workspace(), &created.id)
                .await
                .unwrap(),
            alices,
            "Mallory's token replaced Alice's"
        );
    }

    /// Two Connections of one account would bring each message in
    /// twice, so a consent from an account that another Connection of
    /// the Workspace holds stores no token and records no account.
    #[tokio::test]
    async fn an_account_that_another_connection_holds_stores_no_token() {
        let installation = installation().await;
        let (_, state) = started(&installation, &["gmail_read"]).await;
        installation
            .broker
            .finish(&state, "alices-code")
            .await
            .expect("the first connection");
        let second = installation
            .connector
            .create(google_connection(&workspace(), "work"))
            .await
            .expect("create");
        installation.google.consent("again", "ALICE@example.com");

        let state = authorized(&installation, &second).await;
        let refused = installation.broker.finish(&state, "again").await;

        assert!(matches!(refused, Err(ConnectError::Conflict(_))));
        let stored = installation
            .connections
            .get(&workspace(), &second.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, Connection::DISCONNECTED);
        assert!(stored.config.get("account").is_none(), "{}", stored.config);
        assert!(
            installation
                .connections
                .refresh_token(&workspace(), &second.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The callback finds its authorization through the `state`, trades
    /// the code, seals the refresh token and lands the Connection at
    /// `connected` with the capabilities Google granted.
    #[tokio::test]
    async fn the_callback_finishes_the_connection_through_its_state() {
        let installation = installation().await;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");
        let answer = installation
            .connector
            .authorize(
                &workspace(),
                &created.id,
                &["gmail_read".to_string(), "calendar_read".to_string()],
                None,
                &initiator(),
            )
            .await
            .expect("authorize");
        let state = state_of(answer.url().unwrap());

        let connected = installation
            .broker
            .finish(&state, "alices-code")
            .await
            .expect("the callback finishes");

        assert_eq!(connected.status, Connection::CONNECTED);
        assert_eq!(
            connected.authorized_capabilities,
            vec!["gmail_read".to_string(), "calendar_read".to_string()]
        );
        // The daemon holds the refresh token as ciphertext, and the
        // plaintext is nowhere in the row.
        let sealed = installation
            .connections
            .refresh_token(&workspace(), &created.id)
            .await
            .unwrap()
            .expect("the daemon holds the refresh token");
        assert!(
            !sealed
                .0
                .windows(b"refresh-for-alices-code".len())
                .any(|window| window == b"refresh-for-alices-code"),
            "the refresh token is in the row as plaintext"
        );
        // The state is spent: a replay of the redirect finds nothing.
        assert!(installation.broker.pending().is_empty());
        assert!(
            installation
                .broker
                .finish(&state, "alices-code")
                .await
                .is_err()
        );
    }

    /// An authorization the daemon did not start, and a redirect with no
    /// code, both refuse. The callback is public, so it is the one place
    /// where an unauthenticated request reaches a record.
    #[tokio::test]
    async fn a_state_this_daemon_did_not_mint_reaches_no_record() {
        let installation = installation().await;

        assert!(matches!(
            installation.broker.finish("not-a-state", "code").await,
            Err(ConnectError::Validation(_))
        ));
        assert!(matches!(
            installation.broker.finish("not-a-state", "  ").await,
            Err(ConnectError::Validation(_))
        ));
        assert!(installation.google.calls.lock().unwrap().is_empty());
    }

    /// Two people each connect Google under the alias `google`. One
    /// revoking their access leaves the other's alone: the tokens are
    /// two rows sealed with two keys.
    #[tokio::test]
    async fn one_person_revoking_google_leaves_the_other_connected() {
        let installation = installation().await;
        let alice = WorkspaceId::from("ws-alice".to_string());
        let bob = WorkspaceId::from("ws-bob".to_string());
        let mut ids = Vec::new();
        installation.google.consent("bobs-code", "bob@example.com");
        for (workspace, code) in [(&alice, "alices-code"), (&bob, "bobs-code")] {
            let created = installation
                .connector
                .create(google_connection(workspace, "google"))
                .await
                .expect("create");
            let answer = installation
                .connector
                .authorize(
                    workspace,
                    &created.id,
                    &["gmail_read".to_string()],
                    None,
                    &initiator(),
                )
                .await
                .expect("authorize");
            installation
                .broker
                .finish(&state_of(answer.url().unwrap()), code)
                .await
                .expect("finish");
            ids.push(created.id);
        }
        // Alice revokes: the record and its sealed token go.
        assert!(
            installation
                .connections
                .delete_and_revoke(&alice, &ids[0], 1)
                .await
                .unwrap()
        );

        assert!(
            installation
                .connections
                .get(&alice, &ids[0])
                .await
                .unwrap()
                .is_none()
        );
        let bobs = installation
            .connections
            .get(&bob, &ids[1])
            .await
            .unwrap()
            .expect("Bob's connection survives");
        assert_eq!(bobs.status, Connection::CONNECTED);
        assert!(
            installation
                .connections
                .refresh_token(&bob, &ids[1])
                .await
                .unwrap()
                .is_some(),
            "Bob's token went with Alice's"
        );
    }

    /// Google refusing the exchange leaves the record `disconnected`
    /// with no account, so the person retries from the same card.
    #[tokio::test]
    async fn a_refused_exchange_leaves_the_record_disconnected() {
        let installation = installation().await;
        *installation.google.refusals.lock().unwrap() = 1;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");
        let answer = installation
            .connector
            .authorize(&workspace(), &created.id, &[], None, &initiator())
            .await
            .expect("authorize");

        let refused = installation
            .broker
            .finish(&state_of(answer.url().unwrap()), "alices-code")
            .await;

        assert!(matches!(
            refused,
            Err(ConnectError::Provider(ProviderErrorCode::ReauthRequired))
        ));
        let stored = installation
            .connections
            .get(&workspace(), &created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, Connection::DISCONNECTED);
        assert!(stored.config.get("account").is_none(), "{}", stored.config);
    }

    /// `gog` gets an access token the daemon minted, refreshed from the
    /// sealed token the daemon holds, and it is reused until it runs
    /// out: a run of tool calls is one refresh, not one per call.
    #[tokio::test]
    async fn the_daemon_mints_the_access_token_of_a_connection() {
        let installation = installation().await;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");
        let answer = installation
            .connector
            .authorize(&workspace(), &created.id, &[], None, &initiator())
            .await
            .expect("authorize");
        installation
            .broker
            .finish(&state_of(answer.url().unwrap()), "alices-code")
            .await
            .expect("finish");
        let tokens = installation.broker.access_tokens(&workspace(), &created.id);

        assert_eq!(
            tokens.fresh().await.unwrap(),
            "access-for-refresh-for-alices-code"
        );
        assert_eq!(
            tokens.fresh().await.unwrap(),
            "access-for-refresh-for-alices-code"
        );
        // One exchange and one refresh, and no second refresh.
        assert_eq!(installation.google.calls.lock().unwrap().len(), 2);
    }

    /// A Connection that holds no token cannot be called: the answer is
    /// `reauth_required`, not a token from somewhere else.
    #[tokio::test]
    async fn a_connection_with_no_token_asks_for_a_new_consent() {
        let installation = installation().await;
        let created = installation
            .connector
            .create(google_connection(&workspace(), "google"))
            .await
            .expect("create");

        let error = installation
            .broker
            .access_tokens(&workspace(), &created.id)
            .fresh()
            .await
            .unwrap_err();

        assert_eq!(error.code, ProviderErrorCode::ReauthRequired);
    }

    #[tokio::test]
    async fn only_a_carrier_has_a_sip_credential() {
        let installation = installation().await;
        let connector = &installation.connector;
        let google = connector
            .create(google_connection(&workspace(), "work"))
            .await
            .unwrap();

        let refused = connector
            .set_sip_credential(
                &workspace(),
                &google.id,
                "robin",
                "secret",
                "sip.telnyx.com",
            )
            .await;

        assert!(matches!(refused, Err(ConnectError::Validation(_))));
    }
}
