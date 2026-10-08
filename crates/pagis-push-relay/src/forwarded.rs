//! The client address of a request.
//!
//! The relay terminates no TLS. A reverse proxy in front of it adds the
//! peer that it saw to `X-Forwarded-For`. A client writes that header too,
//! so the relay reads it only from one configured address, and it takes
//! the **last** entry: every entry before it came inside the request and
//! may be invented. This is the rule of the daemon's own Trusted Proxy.
//!
//! The deployment in `deploy/push-relay/` puts a Cloudflare Tunnel in
//! front of the relay. The Cloudflare edge appends the address that
//! connected to it to `X-Forwarded-For`, or sets the header to that
//! address when the request has none, so the last entry is the same
//! address as `CF-Connecting-IP`
//! (<https://developers.cloudflare.com/fundamentals/reference/http-headers/>).
//! cloudflared sends the request to the relay with the headers of the
//! edge and does not change `X-Forwarded-For`. So the relay reads the
//! same header behind the tunnel as behind another reverse proxy.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

/// `X-Forwarded-For`: the addresses that a chain of proxies saw.
const FORWARDED_FOR: &str = "x-forwarded-for";

/// The one address whose `X-Forwarded-For` the relay believes.
///
/// `None` believes nobody: every request is the peer on the socket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrustedProxy(Option<IpAddr>);

impl TrustedProxy {
    /// Believe the `X-Forwarded-For` of this address alone.
    pub fn at(address: IpAddr) -> Self {
        Self(Some(address))
    }

    /// Believe nobody's `X-Forwarded-For`.
    pub fn none() -> Self {
        Self(None)
    }

    /// The address that the request came from: the last forwarded entry
    /// when the Trusted Proxy sent it, and otherwise the peer on the
    /// socket. The registration limit counts against this address.
    pub fn client_address(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        if self.0 != Some(peer.ip()) {
            return peer.ip();
        }
        last_value(headers, FORWARDED_FOR)
            .and_then(|value| value.parse::<IpAddr>().ok())
            .unwrap_or_else(|| peer.ip())
    }
}

/// The last entry that is not empty across every `name` header.
fn last_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .rfind(|value| !value.is_empty())
}
