//! Authentication. Every request that reads or writes a person's
//! data carries a session cookie. The middleware resolves it to a
//! [`Tenant`]: the signed-in person, their role, and the Workspace they
//! own. Handlers take the tenant from the extractor, so no handler reads
//! an ambient one.
//!
//! The cookie is HTTP-only, so a script in the page cannot read it, and
//! the value never appears in a URL. The record holds the SHA-256 of the
//! value alone, so a stolen row signs nobody in.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use pagis_core::{SessionId, UnixMillis, UserId, UserRole, WorkspaceId};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::ApiError;

/// The cookie that carries a Session.
pub const SESSION_COOKIE: &str = "pagis_session";

/// How long a client keeps the cookie. It matches the Session record, so
/// the browser drops a cookie the daemon would refuse anyway.
const COOKIE_MAX_AGE_SECS: i64 = pagis_core::SESSION_LIFETIME_MS / 1_000;

/// How long the daemon leaves `last_used_at` alone. One request must not
/// be one write, and the field only has to show recent use.
const TOUCH_INTERVAL_MS: i64 = 60 * 60 * 1_000;

/// Who the request comes from, and whose data it may touch. The
/// Workspace is the person's own: one Workspace per person.
#[derive(Debug, Clone)]
pub struct Tenant {
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub role: UserRole,
    pub session_id: SessionId,
    /// When the Session expires. A live connection closes then.
    pub session_expires_at: UnixMillis,
}

impl<S: Send + Sync> FromRequestParts<S> for Tenant {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Tenant>()
            .cloned()
            .ok_or_else(ApiError::unauthorized)
    }
}

/// A tenant whose role is [`UserRole::Administrator`]. A System
/// Setting belongs to the installation, so the routes that read or
/// write one take this extractor instead of [`Tenant`]: a Member gets
/// `403` and the handler never runs. It carries every field of the
/// tenant, so a handler that has it needs nothing else.
#[derive(Debug, Clone)]
pub struct Administrator(pub Tenant);

impl std::ops::Deref for Administrator {
    type Target = Tenant;

    fn deref(&self) -> &Tenant {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Administrator {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let tenant = Tenant::from_request_parts(parts, state).await?;
        match tenant.role {
            UserRole::Administrator => Ok(Administrator(tenant)),
            UserRole::Member => Err(ApiError::forbidden(
                "this setting belongs to the installation, and an administrator changes it",
            )),
        }
    }
}

/// The SHA-256 of a cookie or link secret, as the records store it.
pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// One cookie of the request, by name.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().to_string())
}

/// The `Set-Cookie` value that hands a browser its Session.
///
/// It names no `Domain`, so the browser sends it back to this host alone
/// and to no sibling of it. `SameSite=Strict` keeps another site
/// from carrying it into a request of its own. `Secure` goes on when the
/// browser reached the daemon over TLS, which a reverse proxy reports;
/// a plain-HTTP local installation must leave it off, because a browser
/// refuses a `Secure` cookie there and the person would never sign in.
pub fn session_cookie(secret: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={secret}; Path=/; HttpOnly; SameSite=Strict; \
         Max-Age={COOKIE_MAX_AGE_SECS}{}",
        secure_attribute(secure)
    )
}

/// The `Set-Cookie` value that takes the Session away again. It carries
/// the same attributes, so a browser replaces the cookie it holds.
pub fn cleared_session_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        secure_attribute(secure)
    )
}

fn secure_attribute(secure: bool) -> &'static str {
    if secure { "; Secure" } else { "" }
}

/// Resolve the session cookie and put the [`Tenant`] in the request.
/// Every authenticated route runs behind this.
pub async fn authenticate(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let tenant = resolve(&state, request.headers())
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    request.extensions_mut().insert(tenant);
    Ok(next.run(request).await)
}

/// The tenant of a request, or `None` when it carries no live Session.
/// The WebSocket routes use it too: they upgrade first and read the same
/// cookie, so a socket and a REST call authenticate the same way.
pub async fn resolve(state: &AppState, headers: &HeaderMap) -> Result<Option<Tenant>, ApiError> {
    let Some(secret) = cookie(headers, SESSION_COOKIE) else {
        return Ok(None);
    };
    let now = state.clock.now_ms();
    let Some(session) = state.sessions.find_live(&hash_secret(&secret), now).await? else {
        return Ok(None);
    };
    let Some(user) = state.users.get(&session.user_id).await? else {
        return Ok(None);
    };
    // A disabled account reaches nothing. Disabling drops every
    // Session of the person, so this is the belt to that braces: a
    // Session minted in the same millisecond still stops here.
    if user.is_disabled() {
        return Ok(None);
    }
    // A person with no Workspace has nothing to serve. It cannot happen
    // on a seeded installation, and answering "unauthorized" is the safe
    // reading of it.
    let Some(workspace) = state.workspaces.for_user(&user.id).await? else {
        return Ok(None);
    };
    if now - session.last_used_at > TOUCH_INTERVAL_MS {
        state.sessions.touch(&session.id, now).await?;
    }
    Ok(Some(Tenant {
        workspace_id: workspace.id,
        user_id: user.id,
        role: user.role,
        session_id: session.id,
        session_expires_at: session.expires_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_cookie_is_read_out_of_a_header_that_holds_several() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "other=1; pagis_session=abc; third=3".parse().unwrap(),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
    }

    #[test]
    fn the_session_cookie_is_http_only_and_the_cleared_one_expires_at_once() {
        assert!(session_cookie("abc", false).contains("HttpOnly"));
        assert!(session_cookie("abc", false).contains("pagis_session=abc"));
        assert!(cleared_session_cookie(false).contains("Max-Age=0"));
    }

    /// Over TLS the cookie is `Secure`, and over plain HTTP it is not:
    /// a browser drops a `Secure` cookie an insecure page set, and a
    /// local installation is that page.
    #[test]
    fn tls_makes_the_cookie_secure_and_plain_http_does_not() {
        assert!(session_cookie("abc", true).contains("; Secure"));
        assert!(cleared_session_cookie(true).contains("; Secure"));
        assert!(!session_cookie("abc", false).contains("Secure"));
        assert!(!cleared_session_cookie(false).contains("Secure"));
    }

    /// The cookie names no `Domain`, so it is host-only: a sibling host
    /// of the same registrable domain never receives it.
    #[test]
    fn the_cookie_is_host_only() {
        for cookie in [session_cookie("abc", true), cleared_session_cookie(true)] {
            assert!(!cookie.to_lowercase().contains("domain"), "{cookie}");
            assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        }
    }

    /// Run the [`Administrator`] extractor against a request that
    /// carries the tenant of one role, as the middleware leaves it.
    async fn administrator_of(role: UserRole) -> Result<Administrator, ApiError> {
        let tenant = Tenant {
            workspace_id: WorkspaceId::generate(),
            user_id: UserId::generate(),
            role,
            session_id: SessionId::generate(),
            session_expires_at: 0,
        };
        let mut request = Request::new(axum::body::Body::empty());
        request.extensions_mut().insert(tenant);
        let (mut parts, _) = request.into_parts();
        Administrator::from_request_parts(&mut parts, &()).await
    }

    #[tokio::test]
    async fn an_administrator_carries_the_tenant_through() {
        let administrator = administrator_of(UserRole::Administrator)
            .await
            .expect("an administrator passes");

        assert_eq!(administrator.role, UserRole::Administrator);
        assert_eq!(administrator.workspace_id, administrator.0.workspace_id);
    }

    #[tokio::test]
    async fn a_member_is_forbidden() {
        let error = administrator_of(UserRole::Member)
            .await
            .expect_err("a member is refused");

        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.code, "forbidden");
    }

    #[tokio::test]
    async fn a_request_with_no_tenant_is_unauthorized() {
        let (mut parts, _) = Request::new(axum::body::Body::empty()).into_parts();

        let error = Administrator::from_request_parts(&mut parts, &())
            .await
            .expect_err("no session is refused");

        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn the_stored_hash_is_not_the_secret() {
        let hash = hash_secret("abc");
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("abc"));
    }
}
