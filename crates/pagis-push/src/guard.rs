//! The address guard: a Web Push goes only to an `https` endpoint with a
//! DNS name, and only to the public addresses of that name at the time of
//! the send (ADR-0030).
//!
//! The guard is the DNS resolver of the HTTP client. It keeps only the
//! addresses that the policy allows, and the client connects to the
//! addresses that it answers, so no second lookup can change them. A
//! name that moves to a private address after the registration therefore
//! gets nothing.

use std::net::{IpAddr, SocketAddr};

use pagis_core::is_public_unicast;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
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
    fn allows(self, ip: IpAddr) -> bool {
        match self {
            Policy::Public => is_public_unicast(ip),
            Policy::AllowLoopback => ip.to_canonical().is_loopback() || is_public_unicast(ip),
        }
    }

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

/// The DNS resolver of the Web Push client.
pub(crate) struct AddressGuard {
    policy: Policy,
}

impl AddressGuard {
    pub(crate) fn new(policy: Policy) -> Self {
        Self { policy }
    }
}

impl Resolve for AddressGuard {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy;
        Box::pin(async move {
            let host = name.as_str();
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
            let allowed: Vec<SocketAddr> = resolved
                .iter()
                .copied()
                .filter(|address| policy.allows(address.ip()))
                .collect();
            if allowed.is_empty() {
                let resolved: Vec<String> = resolved
                    .iter()
                    .map(|address| address.ip().to_string())
                    .collect();
                return Err(format!(
                    "{host} resolves to no public address: [{}]",
                    resolved.join(", ")
                )
                .into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn resolve(policy: Policy, host: &str) -> Result<Vec<IpAddr>, String> {
        let name: Name = host.parse().expect("a DNS name");
        AddressGuard::new(policy)
            .resolve(name)
            .await
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|error| error.to_string())
    }

    /// The public policy refuses a name whose addresses are all loopback
    /// or private. The system resolver answers an address in numeric
    /// form as that address, with no query on the network.
    #[tokio::test]
    async fn the_public_policy_refuses_a_name_of_a_private_address() {
        for host in ["localhost", "127.0.0.1", "10.0.0.1", "192.168.1.20"] {
            let error = resolve(Policy::Public, host)
                .await
                .expect_err("a private address passes");

            assert!(error.contains("resolves to no public address"), "{error}");
        }
    }

    #[tokio::test]
    async fn the_public_policy_answers_a_public_address() {
        let addresses = resolve(Policy::Public, "93.184.215.14").await;

        assert_eq!(addresses, Ok(vec!["93.184.215.14".parse().unwrap()]));
    }

    #[tokio::test]
    async fn the_loopback_policy_answers_a_loopback_address_and_no_private_one() {
        let addresses = resolve(Policy::AllowLoopback, "127.0.0.1").await;
        assert_eq!(addresses, Ok(vec!["127.0.0.1".parse().unwrap()]));

        assert!(resolve(Policy::AllowLoopback, "10.0.0.1").await.is_err());
    }
}
