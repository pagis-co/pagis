//! The portable sign-in rules of Remote Access.

use crate::{AppState, system::is_local};
use axum::http::HeaderMap;
use serde::Serialize;
use std::net::SocketAddr;
use utoipa::ToSchema;

/// Whether the running daemon serves this machine alone: a Local
/// Installation (ADR-0025) with Remote Access off. It answers only
/// programs of its own machine, and it has one Person who signs in.
///
/// A switch of Remote Access writes `config.toml` and takes effect at the
/// next start, so the answer holds for the life of the process. The guard
/// that refuses other machines and the start route of a Google
/// authorization both read it here. A Server serves its network whatever
/// Remote Access says.
pub fn serves_this_machine_only(state: &AppState) -> bool {
    is_local(state) && !state.remote_access
}

/// How a client at the address that asks signs in (ADR-0028). The
/// sign-in page of the Product App shows the form that it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// An address and a password.
    Password,
    /// A Sign-In Link. In Remote Access the daemon takes no password from
    /// another machine, because the public name faces the internet.
    Link,
}

/// How the client of one request signs in: with a Sign-In Link where
/// Remote Access is on and the request did not come from this machine,
/// and with a password everywhere else.
pub(crate) fn sign_in_method(
    remote_access: bool,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> SignInMethod {
    if remote_access && !crate::forwarded::is_from_this_machine(peer, headers) {
        SignInMethod::Link
    } else {
        SignInMethod::Password
    }
}

#[cfg(test)]
mod sign_in_tests {
    use super::*;

    #[test]
    fn remote_access_sign_in_takes_a_link_without_native_services() {
        let remote = "192.0.2.10:1234".parse().unwrap();
        let local = "127.0.0.1:1234".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("host", "localhost".parse().unwrap());
        assert_eq!(sign_in_method(true, remote, &headers), SignInMethod::Link);
        assert_eq!(
            sign_in_method(true, local, &headers),
            SignInMethod::Password
        );
        headers.insert("x-forwarded-for", "192.0.2.10".parse().unwrap());
        assert_eq!(sign_in_method(true, local, &headers), SignInMethod::Link);
        assert_eq!(
            sign_in_method(false, remote, &headers),
            SignInMethod::Password
        );
    }
}
