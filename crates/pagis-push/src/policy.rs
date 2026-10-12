//! The endpoint URL policy of a Web Push, before any DNS lookup.

use std::net::IpAddr;

use url::{Host, Url};

/// Which endpoints and addresses a Web Push may go to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Policy {
    /// An `https` endpoint with a DNS name, at a public unicast address.
    #[default]
    Public,
    /// The public endpoints, and also `http` and the loopback addresses,
    /// so a test reaches a push service on its own machine.
    AllowLoopback,
}

impl Policy {
    /// Refuse an endpoint by its URL alone, before any lookup.
    pub(crate) fn check_endpoint(self, endpoint: &Url) -> Result<(), String> {
        let loopback = self == Policy::AllowLoopback;
        if endpoint.scheme() != "https" && !(loopback && endpoint.scheme() == "http") {
            return Err(format!("the endpoint {endpoint} is not https"));
        }
        let literal = match endpoint.host() {
            Some(Host::Domain(_)) => return Ok(()),
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip),
            None => return Err(format!("the endpoint {endpoint} has no host")),
        };
        if loopback && literal.to_canonical().is_loopback() {
            return Ok(());
        }
        Err(format!(
            "the endpoint {endpoint} names an IP address and not a DNS name"
        ))
    }
}
