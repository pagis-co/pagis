//! The brokered Google half of the connect flow (ADR-0012).
//!
//! On a local installation the person and the browser are on the daemon's
//! own machine, so `gog` runs the loopback flow of RFC 8252 and the
//! Connection is `byo`: the person's own Desktop client, and a token
//! `gog` keeps.
//!
//! On a server neither holds. The Org registers one Web OAuth client.
//! For each authorization the daemon mints a `state` and records the
//! Connection, and the Person and the Session that started it. The
//! authorize request answers the start route on the daemon's own
//! `public_origin`. For a browser with a Session of that Person, the
//! start route sets a transaction cookie that binds the `state` to that
//! browser, and sends the browser to Google (RFC 9700, section 2.1.1). A
//! local installation with the multi-user mode off has one Person, and
//! there the start route asks the browser for no Session.
//! Google redirects the browser to the callback on the same origin. The
//! daemon trades the code only for the browser with that cookie, while
//! the Session is live, and keeps the token only for the Google account
//! of the Connection. The Connection is `brokered`: the installation
//! holds the client, the person holds the consent.
//!
//! The refresh token is the daemon's from then on. It is sealed with the
//! Tenant Data Key of the Workspace that owns the Connection, so it is
//! ciphertext no other tenant's key opens, and `gog` gets a fresh access
//! token per call through `GOG_ACCESS_TOKEN`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    Clock, Connection, ConnectionId, ConnectionStore, GOOGLE_WEB_CLIENT_SECRET, OrgStore,
    SecretStore, SessionId, SessionStore, TenantKeys, UnixMillis, UserId, WorkspaceId, now_ms,
};
use pagis_google::{
    AccessTokens, GoogleCapability, GoogleOAuth, Pkce, ProviderError, ProviderErrorCode, WebClient,
    oauth_scopes, random_token, redirect_uri,
};

use crate::ConnectError;

/// How long a person has to finish at Google before the daemon forgets
/// the authorization it started. A tab nobody came back to must not
/// hold a verifier for the life of the daemon.
pub const AUTHORIZE_WINDOW_MS: i64 = 10 * 60 * 1_000;

/// The start route on the Public Origin. The authorize request answers
/// it with the `state` in the query.
const START_PATH: &str = "/api/v1/connections/google/start";

/// The Google Web OAuth client of this installation.
///
/// The client id is on the Org record, because one installation
/// registers one client and every person consents against it. The client
/// secret is an installation secret in `secrets.enc`, filed like a model
/// provider key, so it is never a column and never in a workspace.
pub struct OrgWebClient {
    orgs: Arc<dyn OrgStore>,
    secrets: Arc<dyn SecretStore>,
}

impl OrgWebClient {
    pub fn new(orgs: Arc<dyn OrgStore>, secrets: Arc<dyn SecretStore>) -> Self {
        Self { orgs, secrets }
    }

    /// The registered client, or `None` when the installation holds
    /// none. Its presence is what decides which flow a new Connection
    /// runs: with a Web client the flow is brokered, without one it is
    /// bring-your-own.
    pub async fn web_client(&self) -> Result<Option<WebClient>, ConnectError> {
        let Some(client_id) = self.registered_client_id().await? else {
            return Ok(None);
        };
        let Some(client_secret) = self
            .secrets
            .get(GOOGLE_WEB_CLIENT_SECRET)
            .map_err(|error| {
                tracing::error!(%error, "reading the Google Web client secret failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?
        else {
            // A client id with no secret cannot sign a token call. The
            // honest answer is that the installation holds no client,
            // and the administrator registers it again.
            tracing::warn!("the Org holds a Google client id and no client secret");
            return Ok(None);
        };
        WebClient::new(&client_id, &client_secret)
            .map(Some)
            .map_err(|_| {
                ConnectError::Validation(
                    "the installation's Google client is not usable".to_string(),
                )
            })
    }

    /// The registered client id, which is not a secret: the settings
    /// page shows it back to the administrator.
    pub async fn registered_client_id(&self) -> Result<Option<String>, ConnectError> {
        Ok(self
            .orgs
            .list()
            .await?
            .into_iter()
            .next()
            .and_then(|org| org.google_client_id))
    }

    /// Register the installation's Web client. The administrator does
    /// this once; the secret lands in `secrets.enc` and the id on the
    /// Org record.
    pub async fn register(&self, client_id: &str, client_secret: &str) -> Result<(), ConnectError> {
        let client = WebClient::new(client_id, client_secret).map_err(|_| {
            ConnectError::Validation(
                "the client id and client secret are both required".to_string(),
            )
        })?;
        let org = self
            .orgs
            .list()
            .await?
            .into_iter()
            .next()
            .ok_or(ConnectError::NotFound)?;
        // The secret lands first: an id with no secret signs nothing,
        // and `web_client` reports the installation holds no client
        // until both are there.
        self.secrets
            .set(GOOGLE_WEB_CLIENT_SECRET, client_secret.trim())
            .map_err(|error| {
                tracing::error!(%error, "storing the Google Web client secret failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?;
        if !self
            .orgs
            .set_google_client_id(&org.id, Some(client.client_id()))
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(())
    }

    /// Forget the installation's Web client. Connections already
    /// `connected` keep working: the daemon holds their refresh tokens,
    /// but it cannot refresh them without the client, so they reach
    /// `reauth_required` at their next call.
    pub async fn forget(&self) -> Result<(), ConnectError> {
        let org = self
            .orgs
            .list()
            .await?
            .into_iter()
            .next()
            .ok_or(ConnectError::NotFound)?;
        self.orgs.set_google_client_id(&org.id, None).await?;
        self.secrets
            .delete(GOOGLE_WEB_CLIENT_SECRET)
            .map_err(|error| {
                tracing::error!(%error, "deleting the Google Web client secret failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?;
        Ok(())
    }
}

/// The Person and the Session that start one authorization.
///
/// The callback cannot read the Session cookie: it is `SameSite=Strict`,
/// so a browser does not send it on the redirect from Google. The
/// pending authorization therefore names the Session, and the callback
/// reads that Session again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initiator {
    pub user_id: UserId,
    pub session_id: SessionId,
}

/// The browser that opens the start route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener<'a> {
    /// A browser with a Session of this Person. It goes on only when
    /// this Person started the authorization.
    SignedIn(&'a UserId),
    /// A browser of a Local Installation with the multi-user mode off.
    /// That installation has one Person and answers only programs of its
    /// own machine, so the browser needs no Session.
    ThisMachine,
}

/// Why the start route sends a browser nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefusal {
    /// No authorization waits under this `state`: this daemon did not
    /// mint it, it is spent, or its window closed.
    NotWaiting,
    /// The Session belongs to a Person other than the one who started
    /// the authorization.
    AnotherPerson,
}

/// One authorization the daemon started and the person has not finished.
struct Pending {
    workspace_id: WorkspaceId,
    connection_id: ConnectionId,
    initiator: Initiator,
    /// The capabilities the Connection asked for. It records the ones
    /// that Google grants.
    requested: Vec<GoogleCapability>,
    pkce: Pkce,
    /// The address at Google that the start route sends the browser to.
    google_url: String,
    started_at: UnixMillis,
}

/// The authorizations waiting for their person to come back from Google.
///
/// The `state` names one pending authorization, so it is 256 bits of
/// randomness and it is spent on first use: a second redirect carrying
/// the same `state` finds nothing. The `state` alone proves nothing
/// about the browser that brings it back: the person who started the
/// authorization holds it, and so does everybody who sees the address at
/// Google. The transaction cookie of the start route binds it to one
/// browser, and the Session that the entry names binds it to one Person.
///
/// The entries are the daemon's own memory and not a table. A verifier
/// is worth nothing after its window, and a daemon that restarts mid
/// flow leaves the person to press the button again, which costs one
/// consent screen and keeps no secret at rest.
#[derive(Default)]
pub struct PendingAuthorizations {
    entries: Mutex<HashMap<String, Pending>>,
}

impl PendingAuthorizations {
    /// Record one authorization under the `state` that names it.
    fn start(&self, state: &str, pending: Pending) {
        let mut entries = self.entries.lock().expect("pending authorization lock");
        entries.retain(|_, entry| pending.started_at - entry.started_at < AUTHORIZE_WINDOW_MS);
        // One Connection has one authorization in flight. Pressing the
        // button twice must not leave a verifier nobody can spend.
        entries.retain(|_, entry| {
            !(entry.workspace_id == pending.workspace_id
                && entry.connection_id == pending.connection_id)
        });
        entries.insert(state.to_string(), pending);
    }

    /// The address at Google of one waiting authorization, for the
    /// browser `opener`. It spends nothing.
    fn google_url(
        &self,
        state: &str,
        opener: Opener<'_>,
        now: UnixMillis,
    ) -> Result<String, StartRefusal> {
        let entries = self.entries.lock().expect("pending authorization lock");
        let pending = entries
            .get(state)
            .filter(|pending| now - pending.started_at < AUTHORIZE_WINDOW_MS)
            .ok_or(StartRefusal::NotWaiting)?;
        match opener {
            Opener::SignedIn(person) if &pending.initiator.user_id != person => {
                Err(StartRefusal::AnotherPerson)
            }
            Opener::SignedIn(_) | Opener::ThisMachine => Ok(pending.google_url.clone()),
        }
    }

    /// Spend one `state`. It answers `None` for a `state` this daemon
    /// did not mint, one already spent, and one past its window.
    fn take(&self, state: &str, now: UnixMillis) -> Option<Pending> {
        let pending = self
            .entries
            .lock()
            .expect("pending authorization lock")
            .remove(state)?;
        (now - pending.started_at < AUTHORIZE_WINDOW_MS).then_some(pending)
    }

    /// How many authorizations are in flight. The settings tests read
    /// it to prove one is forgotten when it is spent.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("pending authorization lock")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Everything the brokered Google flow reaches: the Connection table,
/// the installation's Web client, Google, the Tenant Data Keys that
/// seal what the daemon keeps, and the Sessions of the People who start
/// an authorization.
pub struct GoogleBroker {
    connections: Arc<dyn ConnectionStore>,
    client: Arc<OrgWebClient>,
    oauth: Arc<GoogleOAuth>,
    keys: Arc<TenantKeys>,
    sessions: Arc<dyn SessionStore>,
    /// The clock of the daemon. The window of an authorization and the
    /// expiry of a Session both read it.
    clock: Arc<dyn Clock>,
    pending: PendingAuthorizations,
    /// The origin a browser reaches this installation at. The start
    /// route and the redirect URI hang off it, and Google matches the
    /// redirect URI character for character against the console entry.
    public_origin: String,
}

impl GoogleBroker {
    pub fn new(
        connections: Arc<dyn ConnectionStore>,
        client: Arc<OrgWebClient>,
        oauth: Arc<GoogleOAuth>,
        keys: Arc<TenantKeys>,
        sessions: Arc<dyn SessionStore>,
        clock: Arc<dyn Clock>,
        public_origin: impl Into<String>,
    ) -> Self {
        Self {
            connections,
            client,
            oauth,
            keys,
            sessions,
            clock,
            pending: PendingAuthorizations::default(),
            public_origin: public_origin.into(),
        }
    }

    /// The installation's Web client, when it holds one.
    pub async fn web_client(&self) -> Result<Option<WebClient>, ConnectError> {
        self.client.web_client().await
    }

    /// The registered client id, for the administrator's settings page.
    pub async fn registered_client_id(&self) -> Result<Option<String>, ConnectError> {
        self.client.registered_client_id().await
    }

    /// Register the installation's Web client.
    pub async fn register(&self, client_id: &str, client_secret: &str) -> Result<(), ConnectError> {
        self.client.register(client_id, client_secret).await
    }

    /// Forget the installation's Web client.
    pub async fn forget(&self) -> Result<(), ConnectError> {
        self.client.forget().await
    }

    /// The redirect URI the administrator registers with Google.
    pub fn redirect_uri(&self) -> String {
        redirect_uri(&self.public_origin)
    }

    pub fn pending(&self) -> &PendingAuthorizations {
        &self.pending
    }

    /// Start one person's authorization and answer the start route to
    /// send their browser to. The request returns here: the exchange
    /// happens later, on the callback, so nothing is held open while the
    /// person reads a consent screen.
    pub async fn authorize(
        &self,
        connection: &Connection,
        capabilities: &[GoogleCapability],
        initiator: &Initiator,
    ) -> Result<String, ConnectError> {
        let client = self.web_client().await?.ok_or_else(|| {
            ConnectError::Validation(
                "this installation has no Google client; an administrator registers one in \
                 settings"
                    .to_string(),
            )
        })?;
        let account = connection.config["account"].as_str().unwrap_or_default();
        if account.is_empty() {
            return Err(ConnectError::Validation(
                "this connection has no Google account".to_string(),
            ));
        }
        let pkce = Pkce::generate();
        let state = random_token();
        let google_url = self.oauth.authorization_url(
            &client,
            &self.redirect_uri(),
            account,
            &oauth_scopes(capabilities.iter().copied()),
            &state,
            &pkce,
        );
        self.pending.start(
            &state,
            Pending {
                workspace_id: connection.workspace_id.clone(),
                connection_id: connection.id.clone(),
                initiator: initiator.clone(),
                requested: capabilities.to_vec(),
                pkce,
                google_url,
                started_at: self.clock.now_ms(),
            },
        );
        Ok(format!(
            "{}{START_PATH}?state={state}",
            self.public_origin.trim_end_matches('/')
        ))
    }

    /// The address at Google of one waiting authorization, for the
    /// browser `opener`. The start route sets the transaction cookie and
    /// sends the browser there. It spends nothing, so the Person who
    /// started the authorization can open the start route again.
    pub fn start(&self, state: &str, opener: Opener<'_>) -> Result<String, StartRefusal> {
        self.pending.google_url(state, opener, self.clock.now_ms())
    }

    /// Spend one `state` and connect nothing. The callback does this for
    /// a redirect that the daemon does not finish: the person declined
    /// at Google, or the browser does not hold the transaction cookie of
    /// the `state`. Such a browser can carry the code of somebody that
    /// the forwarded address reached, and no browser may redeem it
    /// later.
    pub async fn abandon(&self, state: &str) -> Result<(), ConnectError> {
        if let Some(pending) = self.pending.take(state, self.clock.now_ms()) {
            self.disconnect(&pending).await?;
        }
        Ok(())
    }

    /// Finish one authorization from the redirect Google sent the
    /// browser. The caller has checked that the browser holds the
    /// transaction cookie of `state`.
    ///
    /// The `state` names the Connection, and the Person and the Session
    /// that started the authorization. The Session must still be live
    /// and belong to that Person. The daemon then trades the code, keeps
    /// the token only for the Google account of the Connection, and
    /// records only the requested capabilities that Google granted. Each
    /// refusal after the `state` is spent stores no token and leaves the
    /// Connection `disconnected` with its binding.
    pub async fn finish(&self, state: &str, code: &str) -> Result<Connection, ConnectError> {
        let now = self.clock.now_ms();
        let pending = self.pending.take(state, now).ok_or_else(|| {
            ConnectError::Validation(
                "that Google sign-in is no longer waiting; start it again".to_string(),
            )
        })?;
        match self.redeem(&pending, code, now).await {
            Ok(connection) => Ok(connection),
            Err(error) => {
                self.disconnect(&pending).await?;
                Err(error)
            }
        }
    }

    async fn redeem(
        &self,
        pending: &Pending,
        code: &str,
        now: UnixMillis,
    ) -> Result<Connection, ConnectError> {
        if code.trim().is_empty() {
            return Err(ConnectError::Validation(
                "Google sent no authorization code".to_string(),
            ));
        }
        let initiator = &pending.initiator;
        let live = self
            .sessions
            .find_live_by_id(&initiator.session_id, now)
            .await?
            .is_some_and(|session| session.user_id == initiator.user_id);
        if !live {
            return Err(ConnectError::Validation(
                "the Session that started this Google sign-in has ended; sign in and start it \
                 again"
                    .to_string(),
            ));
        }
        let connection = self
            .connections
            .get(&pending.workspace_id, &pending.connection_id)
            .await?
            .ok_or(ConnectError::NotFound)?;
        let client = self.web_client().await?.ok_or_else(|| {
            ConnectError::Validation("this installation has no Google client".to_string())
        })?;
        let tokens = self
            .oauth
            .exchange_code(&client, &self.redirect_uri(), code, pending.pkce.verifier())
            .await
            .map_err(|error| ConnectError::Provider(error.code))?;
        // `login_hint` is a hint: the person at the consent screen can
        // pick any account. The message names no address, because it
        // reaches the log.
        let consented = tokens.verified_account(&client).map_err(|error| {
            ConnectError::Validation(format!(
                "Google did not name the account that consented: {error}"
            ))
        })?;
        let account = connection.config["account"].as_str().unwrap_or_default();
        if consented.trim().to_lowercase() != account.trim().to_lowercase() {
            return Err(ConnectError::Validation(
                "the Google account that consented is not the account of this connection"
                    .to_string(),
            ));
        }
        let granted = tokens
            .granted(&pending.requested)
            .iter()
            .map(|capability| capability.as_str().to_string())
            .collect::<Vec<_>>();
        if granted.is_empty() {
            return Err(ConnectError::Validation(
                "Google granted none of the access this connection asked for".to_string(),
            ));
        }
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
            ConnectError::Validation(
                "Google returned no refresh token for this account; remove Pagis from the \
                 account's third-party access and connect again"
                    .to_string(),
            )
        })?;
        self.keep_refresh_token(&connection, refresh_token).await?;
        if !self
            .connections
            .set_authorization(&connection.workspace_id, &connection.id, &granted)
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(Connection {
            status: Connection::CONNECTED.to_string(),
            authorized_capabilities: granted,
            ..connection
        })
    }

    /// Put the Connection of a spent authorization back at
    /// `disconnected`. It keeps its binding and offers no tools.
    async fn disconnect(&self, pending: &Pending) -> Result<(), ConnectError> {
        self.connections
            .set_status(
                &pending.workspace_id,
                &pending.connection_id,
                Connection::DISCONNECTED,
            )
            .await?;
        Ok(())
    }

    async fn keep_refresh_token(
        &self,
        connection: &Connection,
        refresh_token: &str,
    ) -> Result<(), ConnectError> {
        let sealed = self
            .keys
            .of(&connection.workspace_id)
            .and_then(|key| key.seal(refresh_token))
            .map_err(|error| {
                tracing::error!(%error, "sealing the Google refresh token failed");
                ConnectError::Provider(ProviderErrorCode::TemporarilyUnavailable)
            })?;
        if !self
            .connections
            .set_refresh_token(&connection.workspace_id, &connection.id, Some(&sealed))
            .await?
        {
            return Err(ConnectError::NotFound);
        }
        Ok(())
    }

    /// Mint the access token of one `brokered` Connection, for the
    /// `gog` calls of the Agents granted it.
    pub fn access_tokens(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Arc<dyn AccessTokens> {
        Arc::new(BrokeredAccessTokens {
            broker: Arc::clone(self),
            workspace_id: workspace_id.clone(),
            connection_id: connection_id.clone(),
            held: Mutex::new(None),
        })
    }
}

/// How long before a minted access token runs out the daemon stops
/// reusing it. A call that starts inside the window must not find a
/// token that expired while it ran.
const TOKEN_MARGIN_MS: i64 = 60 * 1_000;

/// The access token of one `brokered` Connection.
///
/// The daemon holds the refresh token, so it makes the refresh call and
/// hands `gog` the result. The minted token is kept in memory until it
/// runs out, so a run of tool calls is one refresh and not one per call.
struct BrokeredAccessTokens {
    broker: Arc<GoogleBroker>,
    workspace_id: WorkspaceId,
    connection_id: ConnectionId,
    /// The token in hand and when it stops being usable.
    held: Mutex<Option<(String, UnixMillis)>>,
}

#[async_trait]
impl AccessTokens for BrokeredAccessTokens {
    async fn fresh(&self) -> Result<String, ProviderError> {
        let now = now_ms();
        if let Some((token, expires_at)) = self.held.lock().expect("access token lock").as_ref()
            && now + TOKEN_MARGIN_MS < *expires_at
        {
            return Ok(token.clone());
        }
        let client = self
            .broker
            .web_client()
            .await
            .ok()
            .flatten()
            .ok_or_else(|| reauth("the installation holds no Google client"))?;
        let sealed = self
            .broker
            .connections
            .refresh_token(&self.workspace_id, &self.connection_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "reading the Google refresh token failed");
                unavailable("the connection record cannot be read")
            })?
            .ok_or_else(|| reauth("this connection holds no Google refresh token"))?;
        let refresh_token = self
            .broker
            .keys
            .of(&self.workspace_id)
            .and_then(|key| key.open(&sealed))
            .map_err(|error| {
                tracing::error!(%error, "opening the Google refresh token failed");
                unavailable("the connection's token cannot be opened")
            })?;
        let tokens = self.broker.oauth.refresh(&client, &refresh_token).await?;
        // Google hands back a new refresh token when it rotates one.
        // Keeping it is what stops a rotation from stranding the person.
        if let Some(rotated) = &tokens.refresh_token
            && rotated != &refresh_token
            && let Ok(resealed) = self
                .broker
                .keys
                .of(&self.workspace_id)
                .and_then(|key| key.seal(rotated))
        {
            let _ = self
                .broker
                .connections
                .set_refresh_token(&self.workspace_id, &self.connection_id, Some(&resealed))
                .await;
        }
        let expires_at = now + (tokens.expires_in as i64) * 1_000;
        *self.held.lock().expect("access token lock") =
            Some((tokens.access_token.clone(), expires_at));
        Ok(tokens.access_token)
    }
}

fn reauth(detail: &'static str) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ReauthRequired,
        retryable: false,
        detail,
    }
}

fn unavailable(detail: &'static str) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::TemporarilyUnavailable,
        retryable: false,
        detail,
    }
}
