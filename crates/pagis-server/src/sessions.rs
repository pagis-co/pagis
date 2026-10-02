//! The ways in, and the ways out.
//!
//! **Password**, on a server: `POST /api/v1/sessions` takes an email and
//! a password and answers with a session cookie. A refusal never says
//! whether the address exists, in its answer or in its time: an unknown
//! address, a disabled Person and a Person with no password check the
//! password against a dummy argon2id hash with the parameters of a real
//! one, so each refusal does the work of a wrong password.
//!
//! Sign-in is rate limited per account and per source address, in
//! windows of fifteen minutes. One locked step checks both keys and
//! counts the attempt against both before the first await, so attempts
//! that arrive together cannot pass the check together. Five attempts
//! that do not sign in lock a key until its window ends. A successful
//! sign-in clears the account key and counts against the address key,
//! and twenty sign-ins lock the address until its window ends, so a
//! right password does not lift the bound.
//!
//! The Argon2 check runs on the blocking pool, never on an async worker,
//! and holds a verification permit while it runs. There is one permit
//! for each core, as `std::thread::available_parallelism` counts them.
//! A sign-in that gets no permit within `PERMIT_WAIT`, half a second,
//! gets `429` and does not queue. So a burst of sign-ins holds at most
//! one thread for each core, and every other request still answers.
//!
//! **Sign-In Link of the Public Origin**, on every installation:
//! `POST /api/v1/sessions/link` takes the secret of a link that a
//! signed-in Person, an Administrator or `pagis pair` made
//! ([`crate::sign_in_links`]) and answers with a session cookie, from
//! any machine. It counts each refusal against the source address with
//! the same limits as a password, so a guess at a secret costs what a
//! guess at a password costs.
//!
//! **Client Credential**, on a local installation: the daemon writes
//! `~/.pagis/client-credential` on first run, readable only by the OS
//! user, and the Client App trades it at
//! `POST /api/v1/sessions/client`. The file is never handed to a
//! browser; a browser a person opens by hand gets in through the
//! start link the `pagis` binary prints.
//!
//! The trade and the start link answer a request from this machine
//! alone, never one that came through a proxy, even a proxy on the same
//! machine ([`crate::forwarded::is_from_this_machine`] holds the rule).
//! A local installation that other People reach through the owner's
//! proxy therefore keeps both as the owner's own way in.
//!
//! A server holds no Client Credential, so it refuses both the exchange
//! and the start link, and a credential file is never a way into a
//! server.
//!
//! **The ways out**: a Person signs out of the Session they hold, and
//! reads and removes their own other Sessions in Settings. A browser
//! Session carries the name of its browser and system, from its
//! `User-Agent` ([`crate::client_name`]), so the Person knows which one
//! to remove.
//!
//! Over a network the Session cookie carries `Secure` when the
//! reverse proxy reports TLS, and the rate limit counts against the
//! browser's own address rather than the proxy's.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier as _, SaltString};
use axum::Json;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pagis_core::{
    ClientKind, SESSION_LIFETIME_MS, Session, SessionId, SignInLinkKind, UnixMillis, User, UserId,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::{Tenant, cleared_session_cookie, hash_secret, session_cookie};
use crate::client_name::browser_session_name;
use crate::error::ApiError;
use crate::user::UserDto;

/// How many attempts that do not sign in one account or one address may
/// make inside the window before it is locked. An attempt counts from
/// the moment the limiter lets it through, before its password is
/// checked.
const MAX_ATTEMPTS: u32 = 5;
/// How many sign-ins one address may make inside the window before it
/// is locked. A right password counts too, so it does not lift the bound
/// on the Argon2 work and on the Sessions that one address makes.
const MAX_SIGN_INS: u32 = 20;
/// The window the attempts are counted in, and how long a lock lasts.
const ATTEMPT_WINDOW_MS: i64 = 15 * 60 * 1_000;
/// How long a sign-in waits for a verification permit before it gets
/// `429`. It is a few Argon2 checks long: sign-ins that arrive together
/// take turns, and a flood is turned away and not queued.
const PERMIT_WAIT: Duration = Duration::from_millis(500);

/// The argon2id hash of a password, as the record stores it. The default
/// parameters of the `argon2` crate are argon2id.
pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| {
            tracing::error!(%error, "hashing a password failed");
            ApiError::internal()
        })
}

/// The hash a sign-in checks when the account has no hash of its own to
/// check. It is argon2id with the parameters of a real hash, made from a
/// random secret, so its check does the work of a real one. The first
/// check that needs it makes it.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| hash_password(&random_secret()).expect("hash the dummy password"));

/// Whether `password` is the one the argon2id `hash` holds. An
/// unreadable hash holds no password.
fn argon2_matches(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// What one password check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Matches,
    DoesNotMatch,
    /// No verification permit came free within `PERMIT_WAIT`, so the
    /// password was not checked.
    NoPermit,
}

/// Whether a password is the one a PHC hash string holds, as
/// `check(password, hash)`: argon2id in the daemon.
type PasswordCheck = dyn Fn(&str, &str) -> bool + Send + Sync;

/// The password checks of the daemon. A check runs on the blocking pool
/// and holds one verification permit until it ends, so the checks never
/// occupy an async worker, and never more threads than there are
/// permits.
pub struct PasswordVerifier {
    permits: Arc<Semaphore>,
    check: Arc<PasswordCheck>,
}

impl PasswordVerifier {
    /// Argon2id, with one verification permit for each core.
    pub fn argon2() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        Self::new(cores, argon2_matches)
    }

    /// `permits` checks at a time, and each check is
    /// `check(password, hash)`.
    pub fn new(permits: usize, check: impl Fn(&str, &str) -> bool + Send + Sync + 'static) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits)),
            check: Arc::new(check),
        }
    }

    /// Check `password` against the `stored` hash of the account. An
    /// account with no hash to check has its password checked against
    /// the dummy hash, which matches nothing, so its refusal takes the
    /// time of a wrong password.
    async fn verify(&self, stored: Option<String>, password: String) -> Result<Verdict, ApiError> {
        let permit = match tokio::time::timeout(
            PERMIT_WAIT,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        {
            Ok(permit) => permit.expect("nothing closes the verification permits"),
            Err(_) => return Ok(Verdict::NoPermit),
        };
        let check = Arc::clone(&self.check);
        tokio::task::spawn_blocking(move || {
            // The permit ends with the check, also when the sign-in that
            // asked for it went away.
            let _permit = permit;
            let matches = check(&password, stored.as_deref().unwrap_or(&DUMMY_HASH));
            match (matches, stored) {
                (true, Some(_)) => Verdict::Matches,
                _ => Verdict::DoesNotMatch,
            }
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, "a password check did not end");
            ApiError::internal()
        })
    }
}

/// The hash a sign-in checks for this account, or `None` when the
/// account has none to check: the Person has no password, or their hash
/// is unreadable.
fn stored_hash(user: &User) -> Option<String> {
    let hash = user.password_hash.as_deref()?;
    if PasswordHash::new(hash).is_err() {
        tracing::error!(user = %user.id, "the stored password hash is unreadable");
        return None;
    }
    Some(hash.to_string())
}

/// The sign-in attempts and the sign-ins of each account and each
/// address inside the window. The limiter is in process: a daemon
/// restart forgets the attempts, which is the same bound a
/// single-process server has anyway.
#[derive(Debug, Default)]
pub struct SignInLimits {
    counts: Mutex<HashMap<String, Counts>>,
}

/// What one key counted since `first_at`.
#[derive(Debug, Clone, Copy)]
struct Counts {
    /// Attempts that did not sign in, and attempts not yet answered.
    failures: u32,
    /// Attempts that signed in. Only an address counts them.
    sign_ins: u32,
    first_at: UnixMillis,
}

impl Counts {
    fn new(now: UnixMillis) -> Self {
        Self {
            failures: 0,
            sign_ins: 0,
            first_at: now,
        }
    }

    fn is_locked(&self) -> bool {
        self.failures >= MAX_ATTEMPTS || self.sign_ins >= MAX_SIGN_INS
    }
}

impl SignInLimits {
    /// Check both keys and count the attempt against both, in one locked
    /// step. `None` when either key is locked. The attempt counts as one
    /// that does not sign in until its [`Reservation`] says otherwise.
    fn reserve(&self, keys: LimitKeys, now: UnixMillis) -> Option<Reservation<'_>> {
        let mut counts = self.counts.lock().expect("sign-in limit lock");
        counts.retain(|_, counts| now - counts.first_at < ATTEMPT_WINDOW_MS);
        if keys
            .all()
            .any(|key| counts.get(key).is_some_and(Counts::is_locked))
        {
            return None;
        }
        for key in keys.all() {
            counts
                .entry(key.clone())
                .or_insert_with(|| Counts::new(now))
                .failures += 1;
        }
        Some(Reservation { limits: self, keys })
    }
}

/// What the limiter counts one attempt against: the account the attempt
/// names, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LimitKeys {
    /// `None` for a Sign-In Link, whose secret names no account until it
    /// is spent.
    account: Option<String>,
    address: String,
}

impl LimitKeys {
    fn new(email: &str, address: IpAddr) -> Self {
        Self {
            account: Some(format!("account:{}", email.trim().to_lowercase())),
            address: format!("address:{address}"),
        }
    }

    /// The keys of an attempt that names no account: the address alone.
    fn address(address: IpAddr) -> Self {
        Self {
            account: None,
            address: format!("address:{address}"),
        }
    }

    fn all(&self) -> impl Iterator<Item = &String> {
        self.account.iter().chain(std::iter::once(&self.address))
    }
}

/// One attempt that the limiter counted. Dropped as it is, the attempt
/// stays counted as one that did not sign in: a refusal, an error, or a
/// client that went away.
struct Reservation<'a> {
    limits: &'a SignInLimits,
    keys: LimitKeys,
}

impl Reservation<'_> {
    /// The password was not checked, so the attempt does not count.
    fn release(self) {
        let mut counts = self.limits.counts.lock().expect("sign-in limit lock");
        for key in self.keys.all() {
            if let Some(counts) = counts.get_mut(key) {
                counts.failures = counts.failures.saturating_sub(1);
            }
        }
    }

    /// The attempt signed in. The account starts again from nothing, and
    /// the address counts a sign-in in place of the attempt.
    fn succeed(self, now: UnixMillis) {
        let mut counts = self.limits.counts.lock().expect("sign-in limit lock");
        if let Some(account) = &self.keys.account {
            counts.remove(account);
        }
        let address = counts
            .entry(self.keys.address)
            .or_insert_with(|| Counts::new(now));
        address.failures = address.failures.saturating_sub(1);
        address.sign_ins += 1;
    }
}

/// A locked account, a locked address and a sign-in that got no
/// verification permit answer the same way, and none of them says whether
/// the address belongs to a person.
fn rate_limited() -> ApiError {
    ApiError {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "rate_limited",
        message: "too many sign-in attempts; wait and try again".to_string(),
    }
}

/// A refused password. It is the same answer for an address with no
/// person, a person with no password and a wrong password.
fn refused() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "unauthorized",
        message: "that email address and password do not match".to_string(),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasswordSignInRequest {
    pub email: String,
    pub password: String,
    /// The machine name a Client App sends, which makes the Session a
    /// Client App Session with that name. A browser sends none.
    #[serde(default)]
    pub client_name: Option<String>,
    /// The IANA timezone of the browser or the Client App. At the
    /// Person's first sign-in it becomes their timezone, so their
    /// Schedules run on their clock and not on the server's.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// The longest machine name a Session keeps.
const CLIENT_NAME_MAX: usize = 255;

/// A refused Sign-In Link. It is the same answer for a secret that
/// names no link, a link that is spent or expired, a link of the other
/// kind, and a link of a disabled Person. The `/sign-in` page shows it
/// as it is, so it names the three ways to a new link (ADR-0028).
fn link_refused() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "unauthorized",
        message: "This sign-in link is spent or expired. Make a new link in Settings → \
                  Sessions on a browser or app that is signed in. Or ask an Administrator \
                  for a new invite, or run \"pagis pair\" on the machine of the server."
            .to_string(),
    }
}

/// The kind of client a sign-in came from, and its name. The Client App
/// names its machine; a browser is named for its `User-Agent`.
fn session_client(
    client_name: Option<&str>,
    headers: &HeaderMap,
) -> Result<(ClientKind, Option<String>), ApiError> {
    match client_name.map(str::trim).filter(|name| !name.is_empty()) {
        None => Ok((ClientKind::Browser, browser_session_name(headers))),
        Some(name) if name.chars().count() > CLIENT_NAME_MAX => Err(ApiError::validation(format!(
            "client_name is longer than {CLIENT_NAME_MAX} characters"
        ))),
        Some(name) => Ok((ClientKind::Desktop, Some(name.to_string()))),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions",
    request_body = PasswordSignInRequest,
    responses(
        (status = 200, body = UserDto, description = "Signed in; the session is in an HTTP-only cookie"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 429, body = crate::error::ErrorBody),
    )
)]
pub async fn sign_in_with_password(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PasswordSignInRequest>,
) -> Result<Response, ApiError> {
    let now = state.clock.now_ms();
    let (client_kind, client_name) = session_client(body.client_name.as_deref(), &headers)?;
    // Behind a reverse proxy every request comes from the proxy, so the
    // limiter would count one address for the whole network. It counts
    // the address the trusted proxy forwarded instead.
    let keys = LimitKeys::new(&body.email, state.proxy.client_address(peer, &headers));
    let secure = state.proxy.is_secure(peer, &headers);
    // The attempt counts before the first await, so attempts that arrive
    // together cannot all pass the check.
    let attempt = state
        .sign_in_limits
        .reserve(keys, now)
        .ok_or_else(rate_limited)?;
    // A disabled account answers exactly as a wrong password does, and
    // after the same work: a refusal must not say whether the address
    // belongs to somebody the administrator turned off.
    let user = state
        .users
        .find_by_email(&body.email)
        .await?
        .filter(|user| !user.is_disabled());
    let stored = user.as_ref().and_then(stored_hash);
    let verdict = state
        .password_verifier
        .verify(stored, body.password)
        .await?;
    let user = match (verdict, user) {
        (Verdict::Matches, Some(user)) => user,
        (Verdict::NoPermit, _) => {
            attempt.release();
            return Err(rate_limited());
        }
        // A refusal keeps the attempt counted.
        _ => return Err(refused()),
    };
    attempt.succeed(now);
    if user.last_signed_in_at.is_none() {
        crate::workspace::take_first_timezone(&state, &user.id, body.timezone.as_deref()).await?;
    }
    open_session(&state, &user.id, client_kind, client_name, now, secure).await
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ClientCredentialRequest {
    /// The content of `~/.pagis/client-credential`.
    pub credential: String,
    /// The machine name of the Client App.
    #[serde(default)]
    pub client_name: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/client",
    request_body = ClientCredentialRequest,
    responses(
        (status = 200, body = UserDto, description = "Signed in as the seeded person"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody, description = "The request did not come from this machine, or came through a proxy"),
        (status = 404, body = crate::error::ErrorBody, description = "This installation holds no Client Credential"),
    )
)]
pub async fn sign_in_with_client_credential(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ClientCredentialRequest>,
) -> Result<Response, ApiError> {
    let expected = state
        .client_credential
        .as_deref()
        .ok_or_else(|| ApiError::not_found("a client credential"))?;
    require_this_machine(peer, &headers, "the client credential")?;
    if !constant_time_eq(expected.as_bytes(), body.credential.trim().as_bytes()) {
        return Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "that client credential is not this installation's".to_string(),
        });
    }
    let now = state.clock.now_ms();
    let user = seeded_administrator(&state).await?;
    let secure = state.proxy.is_secure(peer, &headers);
    // Only the Client App holds the credential, so the Session is its own
    // whether or not it names the machine.
    let (_, client_name) = session_client(body.client_name.as_deref(), &headers)?;
    open_session(
        &state,
        &user.id,
        ClientKind::Desktop,
        client_name,
        now,
        secure,
    )
    .await
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkSignInRequest {
    /// The secret of a Sign-In Link of the Public Origin: the fragment of
    /// `<public origin>/sign-in#<secret>`.
    pub secret: String,
    /// The IANA timezone of the browser. At the Person's first sign-in it
    /// becomes their timezone, as with a password.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Trade the secret of a Sign-In Link of the Public Origin for a
/// Session, from any machine.
///
/// The page at `/sign-in` posts the secret, so opening the link spends
/// nothing. Each refusal counts against the source address with the
/// limits of a password sign-in, and every refusal answers the same way.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/link",
    request_body = LinkSignInRequest,
    responses(
        (status = 200, body = UserDto, description = "Signed in; the session is in an HTTP-only cookie"),
        (status = 401, body = crate::error::ErrorBody, description = "The link is spent, expired or not a link of the Public Origin"),
        (status = 429, body = crate::error::ErrorBody),
    )
)]
pub async fn sign_in_with_link(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LinkSignInRequest>,
) -> Result<Response, ApiError> {
    let now = state.clock.now_ms();
    let keys = LimitKeys::address(state.proxy.client_address(peer, &headers));
    let secure = state.proxy.is_secure(peer, &headers);
    // The attempt counts before the first await, as a password does.
    let attempt = state
        .sign_in_limits
        .reserve(keys, now)
        .ok_or_else(rate_limited)?;
    let spent = state
        .sign_in_links
        .consume(
            &hash_secret(body.secret.trim()),
            SignInLinkKind::PublicOrigin,
            now,
        )
        .await?;
    let user = match spent {
        Some(user_id) => state.users.get(&user_id).await?,
        None => None,
    };
    // A disabled Person signs in to nothing; the link is spent all the
    // same, and the refusal keeps the attempt counted.
    let Some(user) = user.filter(|user| !user.is_disabled()) else {
        return Err(link_refused());
    };
    attempt.succeed(now);
    if user.last_signed_in_at.is_none() {
        crate::workspace::take_first_timezone(&state, &user.id, body.timezone.as_deref()).await?;
    }
    open_session(
        &state,
        &user.id,
        ClientKind::Browser,
        browser_session_name(&headers),
        now,
        secure,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions/link/{code}",
    params(("code" = String, Path, description = "The one-time code the `pagis` binary printed")),
    responses(
        (status = 303, description = "Signed in; the browser is sent to the app"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody, description = "The request did not come from this machine, or came through a proxy"),
        (status = 404, body = crate::error::ErrorBody, description = "This installation holds no Client Credential"),
    )
)]
pub async fn sign_in_with_start_link(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Response, ApiError> {
    // The start link belongs to a local installation, as the Client
    // Credential does.
    if state.client_credential.is_none() {
        return Err(ApiError::not_found("a sign-in link"));
    }
    require_this_machine(peer, &headers, "a sign-in link")?;
    let now = state.clock.now_ms();
    let Some(user_id) = state
        .sign_in_links
        .consume(&hash_secret(&code), SignInLinkKind::Start, now)
        .await?
    else {
        return Err(link_refused());
    };
    let secret = mint_session(
        &state,
        &user_id,
        ClientKind::Browser,
        browser_session_name(&headers),
        now,
    )
    .await?;
    let secure = state.proxy.is_secure(peer, &headers);
    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, session_cookie(&secret, secure)),
            (header::LOCATION, "/".to_string()),
        ],
    )
        .into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/current",
    responses(
        (status = 204, description = "Signed out"),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn sign_out(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    tenant: Tenant,
) -> Result<Response, ApiError> {
    // The record goes first and the live connections after, so a
    // connection that opens between the two finds no Session.
    state.sessions.delete(&tenant.session_id).await?;
    state.live_connections.end_session(&tenant.session_id);
    let secure = state.proxy.is_secure(peer, &headers);
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, cleared_session_cookie(secure))],
    )
        .into_response())
}

/// One Session of the signed-in Person, for their Sessions list.
#[derive(Debug, Serialize, ToSchema)]
pub struct MySessionDto {
    pub id: String,
    /// `browser` or `desktop`.
    pub client_kind: String,
    /// The machine name of a Client App, or the browser and the system
    /// of a browser, such as "Safari on macOS". `null` where the client
    /// said neither.
    pub client_name: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
    /// Thirty days after the last use.
    pub expires_at: i64,
    /// True for the Session that asks, so the list says which client is
    /// this one.
    pub current: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MySessionsDto {
    pub items: Vec<MySessionDto>,
}

/// The live Sessions of the signed-in Person, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/settings/sessions",
    responses(
        (status = 200, body = MySessionsDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_my_sessions(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<MySessionsDto>, ApiError> {
    let now = state.clock.now_ms();
    let items = state
        .sessions
        .list_live_for_user(&tenant.user_id, now)
        .await?
        .into_iter()
        .map(|session| MySessionDto {
            current: session.id == tenant.session_id,
            id: session.id.to_string(),
            client_kind: session.client_kind.as_str().to_string(),
            client_name: session.client_name,
            created_at: session.created_at,
            last_used_at: session.last_used_at,
            expires_at: session.expires_at,
        })
        .collect();
    Ok(Json(MySessionsDto { items }))
}

/// End one Session of the signed-in Person: a lost phone, or a browser
/// they no longer use. Each socket of that Session closes. A Session of
/// another Person reads as absent. Ending the Session that asks is a
/// sign-out, and its answer clears the cookie.
#[utoipa::path(
    delete,
    path = "/api/v1/settings/sessions/{session_id}",
    params(("session_id" = String, Path, description = "One Session of the signed-in Person")),
    responses(
        (status = 204, description = "The Session ended"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn end_my_session(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    tenant: Tenant,
    Path(session_id): Path<String>,
) -> Result<Response, ApiError> {
    let now = state.clock.now_ms();
    let session = state
        .sessions
        .find_live_by_id(&SessionId::from(session_id), now)
        .await?
        .filter(|session| session.user_id == tenant.user_id)
        .ok_or_else(|| ApiError::not_found("that session"))?;
    // The record goes first and the live connections after, as at a
    // sign-out.
    state.sessions.delete(&session.id).await?;
    state.live_connections.end_session(&session.id);
    if session.id != tenant.session_id {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let secure = state.proxy.is_secure(peer, &headers);
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, cleared_session_cookie(secure))],
    )
        .into_response())
}

/// Refuse a request that did not come from this machine, or that came
/// through a proxy. `what` names the way in, for the refusal.
pub(crate) fn require_this_machine(
    peer: SocketAddr,
    headers: &HeaderMap,
    what: &str,
) -> Result<(), ApiError> {
    if crate::forwarded::is_from_this_machine(peer, headers) {
        return Ok(());
    }
    Err(ApiError::forbidden(format!(
        "{what} works only from the machine Pagis runs on, not through a proxy"
    )))
}

/// The administrator a local installation seeded. The Client Credential
/// belongs to that person, because a local installation serves one.
pub(crate) async fn seeded_administrator(state: &AppState) -> Result<User, ApiError> {
    let org = state
        .orgs
        .list()
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::internal)?;
    state
        .users
        .list_by_org(&org.id)
        .await?
        .into_iter()
        .find(|user| user.role == pagis_core::UserRole::Administrator)
        .ok_or_else(|| {
            tracing::error!("the installation has no administrator");
            ApiError::internal()
        })
}

/// Write the Session and answer with the person and the cookie.
pub(crate) async fn open_session(
    state: &AppState,
    user_id: &UserId,
    client_kind: ClientKind,
    client_name: Option<String>,
    now: UnixMillis,
    secure: bool,
) -> Result<Response, ApiError> {
    let secret = mint_session(state, user_id, client_kind, client_name, now).await?;
    // The roster shows when each person last signed in.
    state.users.record_sign_in(user_id, now).await?;
    let user = state
        .users
        .get(user_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, session_cookie(&secret, secure))],
        Json(UserDto::new(&user, &state.administration)),
    )
        .into_response())
}

/// One new Session. The answer is the cookie value; the record holds
/// only its hash.
async fn mint_session(
    state: &AppState,
    user_id: &UserId,
    client_kind: ClientKind,
    client_name: Option<String>,
    now: UnixMillis,
) -> Result<String, ApiError> {
    let secret = random_secret();
    state
        .sessions
        .create(&Session {
            id: SessionId::generate(),
            user_id: user_id.clone(),
            token_hash: hash_secret(&secret),
            client_kind,
            client_name,
            created_at: now,
            last_used_at: now,
            expires_at: now + SESSION_LIFETIME_MS,
        })
        .await?;
    Ok(secret)
}

/// A 32-byte secret in lowercase hexadecimal, for a cookie or a link.
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Compare two secrets without an early exit, so the comparison time
/// says nothing about how much of the credential was right.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn keys() -> LimitKeys {
        LimitKeys::new("Ada@example.com", "127.0.0.1".parse().unwrap())
    }

    fn keys_at(email: &str, address: &str) -> LimitKeys {
        LimitKeys::new(email, address.parse().unwrap())
    }

    #[test]
    fn the_account_and_the_address_are_both_counted() {
        assert_eq!(
            keys(),
            LimitKeys {
                account: Some("account:ada@example.com".to_string()),
                address: "address:127.0.0.1".to_string(),
            }
        );
    }

    /// A link names no account, so its refusals count against the
    /// address alone, and the address they lock is locked for a
    /// password too.
    #[test]
    fn link_refusals_lock_the_address_alone() {
        let limits = SignInLimits::default();
        let link = || LimitKeys::address("127.0.0.1".parse().unwrap());
        for attempt in 0..MAX_ATTEMPTS {
            assert!(
                limits.reserve(link(), 0).is_some(),
                "attempt {attempt} is allowed"
            );
        }
        assert!(limits.reserve(link(), 0).is_none());
        assert!(
            limits.reserve(keys(), 0).is_none(),
            "a password from the same address is locked too"
        );
        assert!(
            limits
                .reserve(keys_at("ada@example.com", "192.0.2.1"), 0)
                .is_some(),
            "the account itself is not locked"
        );
    }

    #[test]
    fn a_link_sign_in_counts_against_the_address() {
        let limits = SignInLimits::default();
        let link = || LimitKeys::address("127.0.0.1".parse().unwrap());
        for sign_in in 0..MAX_SIGN_INS {
            limits
                .reserve(link(), 0)
                .unwrap_or_else(|| panic!("sign-in {sign_in} is allowed"))
                .succeed(0);
        }
        assert!(limits.reserve(link(), 0).is_none());
    }

    #[test]
    fn five_refused_attempts_lock_and_the_window_ends_the_lock() {
        let limits = SignInLimits::default();
        for attempt in 0..MAX_ATTEMPTS {
            // Dropped as it is, the attempt stays counted: a refusal.
            let reservation = limits.reserve(keys(), 0);
            assert!(reservation.is_some(), "attempt {attempt} is allowed");
        }
        assert!(limits.reserve(keys(), 0).is_none());
        assert!(limits.reserve(keys(), ATTEMPT_WINDOW_MS).is_some());
    }

    #[test]
    fn an_attempt_counts_before_it_is_answered() {
        let limits = SignInLimits::default();
        let in_flight: Vec<_> = (0..MAX_ATTEMPTS)
            .map(|attempt| {
                limits
                    .reserve(keys(), 0)
                    .unwrap_or_else(|| panic!("attempt {attempt} is allowed"))
            })
            .collect();
        assert!(
            limits.reserve(keys(), 0).is_none(),
            "attempts in flight count against the limit"
        );
        drop(in_flight);
    }

    #[test]
    fn an_attempt_whose_password_was_not_checked_does_not_count() {
        let limits = SignInLimits::default();
        for _ in 0..MAX_ATTEMPTS * 2 {
            limits.reserve(keys(), 0).expect("allowed").release();
        }
        assert!(limits.reserve(keys(), 0).is_some());
    }

    #[test]
    fn a_sign_in_clears_the_account_and_keeps_the_failures_of_the_address() {
        let limits = SignInLimits::default();
        for _ in 0..MAX_ATTEMPTS - 1 {
            drop(limits.reserve(keys_at("ada@example.com", "127.0.0.1"), 0));
        }
        limits
            .reserve(keys_at("ada@example.com", "127.0.0.1"), 0)
            .expect("allowed")
            .succeed(0);

        // The account starts again: from another address it has every
        // attempt.
        for attempt in 0..MAX_ATTEMPTS {
            assert!(
                limits
                    .reserve(keys_at("ada@example.com", "192.0.2.1"), 0)
                    .is_some(),
                "attempt {attempt} on the account is allowed"
            );
        }
        // The address keeps its four failures, so one more locks it.
        drop(limits.reserve(keys_at("grace@example.com", "127.0.0.1"), 0));
        assert!(
            limits
                .reserve(keys_at("grace@example.com", "127.0.0.1"), 0)
                .is_none()
        );
    }

    #[test]
    fn sign_ins_from_one_address_lock_it_after_the_success_limit() {
        let limits = SignInLimits::default();
        for sign_in in 0..MAX_SIGN_INS {
            limits
                .reserve(keys(), 0)
                .unwrap_or_else(|| panic!("sign-in {sign_in} is allowed"))
                .succeed(0);
        }
        assert!(
            limits.reserve(keys(), 0).is_none(),
            "a right password does not lift the bound"
        );
        assert!(
            limits
                .reserve(keys_at("ada@example.com", "192.0.2.1"), 0)
                .is_some(),
            "another address is not locked"
        );
        assert!(
            limits.reserve(keys(), ATTEMPT_WINDOW_MS).is_some(),
            "the window ends the lock"
        );
    }

    #[test]
    fn one_address_locks_out_the_next_account_it_tries() {
        let limits = SignInLimits::default();
        for _ in 0..MAX_ATTEMPTS {
            drop(limits.reserve(keys_at("ada@example.com", "127.0.0.1"), 0));
        }
        assert!(
            limits
                .reserve(keys_at("grace@example.com", "127.0.0.1"), 0)
                .is_none()
        );
    }

    #[test]
    fn a_hashed_password_verifies_and_a_wrong_one_does_not() {
        let hash = hash_password("correct horse").unwrap();
        assert!(argon2_matches("correct horse", &hash));
        assert!(!argon2_matches("correct horses", &hash));
    }

    #[test]
    fn the_dummy_hash_is_argon2id_with_the_parameters_of_a_real_hash() {
        let real = hash_password("correct horse").unwrap();
        let real = PasswordHash::new(&real).unwrap();
        let dummy = PasswordHash::new(&DUMMY_HASH).unwrap();
        assert_eq!(dummy.algorithm, real.algorithm);
        assert_eq!(dummy.algorithm.as_str(), "argon2id");
        assert_eq!(dummy.version, real.version);
        assert_eq!(dummy.params, real.params);
    }

    /// A check that records each hash it is given and matches all of them.
    fn recording() -> (Arc<Mutex<Vec<String>>>, PasswordVerifier) {
        let checked = Arc::new(Mutex::new(Vec::new()));
        let verifier = PasswordVerifier::new(1, {
            let checked = Arc::clone(&checked);
            move |_password: &str, hash: &str| {
                checked.lock().unwrap().push(hash.to_string());
                true
            }
        });
        (checked, verifier)
    }

    #[tokio::test]
    async fn an_account_with_no_hash_checks_the_dummy_hash_and_never_matches() {
        let (checked, verifier) = recording();

        let none = verifier.verify(None, "anything".to_string()).await.unwrap();
        let stored = verifier
            .verify(Some("stored".to_string()), "anything".to_string())
            .await
            .unwrap();

        assert_eq!(none, Verdict::DoesNotMatch);
        assert_eq!(stored, Verdict::Matches);
        assert_eq!(
            *checked.lock().unwrap(),
            vec![DUMMY_HASH.clone(), "stored".to_string()]
        );
    }

    #[tokio::test]
    async fn a_check_runs_on_the_blocking_pool_and_not_on_the_async_worker() {
        let worker = std::thread::current().id();
        let ran_on = Arc::new(Mutex::new(None));
        let verifier = PasswordVerifier::new(1, {
            let ran_on = Arc::clone(&ran_on);
            move |_password: &str, _hash: &str| {
                *ran_on.lock().unwrap() = Some(std::thread::current().id());
                false
            }
        });

        verifier
            .verify(Some("stored".to_string()), "anything".to_string())
            .await
            .unwrap();

        let ran_on = ran_on.lock().unwrap().expect("the check ran");
        assert_ne!(ran_on, worker, "the check ran on the async worker");
    }

    #[tokio::test]
    async fn a_check_with_no_free_permit_is_not_run() {
        let calls = Arc::new(AtomicUsize::new(0));
        let verifier = PasswordVerifier::new(1, {
            let calls = Arc::clone(&calls);
            move |_password: &str, _hash: &str| {
                calls.fetch_add(1, Ordering::SeqCst);
                true
            }
        });
        let held = Arc::clone(&verifier.permits).acquire_owned().await.unwrap();

        let busy = verifier
            .verify(Some("stored".to_string()), "anything".to_string())
            .await
            .unwrap();
        assert_eq!(busy, Verdict::NoPermit);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no check ran");

        drop(held);
        let free = verifier
            .verify(Some("stored".to_string()), "anything".to_string())
            .await
            .unwrap();
        assert_eq!(free, Verdict::Matches);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_secret_compares_without_an_early_exit() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
    }
}
