//! The settings of the relay, from `PUSH_RELAY_*` variables.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use crate::TrustedProxy;

const PUBLIC_ORIGIN: &str = "PUSH_RELAY_PUBLIC_ORIGIN";
const DATABASE: &str = "PUSH_RELAY_DATABASE";
const BIND: &str = "PUSH_RELAY_BIND";
const TRUSTED_PROXY: &str = "PUSH_RELAY_TRUSTED_PROXY";

const PUBLIC_ORIGIN_FORM: &str = "the https origin of the relay, such as https://push.pagis.co";
const DATABASE_FORM: &str = "the path of the SQLite file of the relay, such as /data/relay.sqlite";
const BIND_FORM: &str = "an IP address and a port, such as 127.0.0.1:8080";
const TRUSTED_PROXY_FORM: &str = "the IP address of the reverse proxy, such as 10.0.0.2";

/// The address and the port that the relay listens on when
/// `PUSH_RELAY_BIND` is not set.
const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8080);

/// The `https` origin of the relay that each endpoint starts with, with
/// no trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin(String);

impl PublicOrigin {
    /// Accept an `https` URL with a host and nothing after the origin. A
    /// daemon posts only to an `https` endpoint (ADR-0030). An `http`
    /// URL on a loopback address is accepted too, as a browser takes it
    /// as a secure context, so a test sender with the policy
    /// `AllowLoopback` reaches the relay on its own machine.
    pub fn parse(value: &str) -> Result<Self, SettingsError> {
        let bad = || SettingsError::Bad {
            name: PUBLIC_ORIGIN,
            value: value.to_string(),
            expected: PUBLIC_ORIGIN_FORM,
        };
        let url = url::Url::parse(value).map_err(|_| bad())?;
        let secure = match url.scheme() {
            "https" => true,
            "http" => is_loopback(&url),
            _ => false,
        };
        let only_origin = secure
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none();
        if !only_origin {
            return Err(bad());
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `true` when the host of `url` is `localhost` or a loopback address.
fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// What the relay reads at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub public_origin: PublicOrigin,
    pub database: PathBuf,
    pub bind: SocketAddr,
    pub trusted_proxy: TrustedProxy,
}

/// A setting that stops the relay at start. The message names the
/// variable and the form that it takes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("{name} is not set; set it to {expected}")]
    Missing {
        name: &'static str,
        expected: &'static str,
    },
    #[error("{name} is {value:?}, which is not {expected}")]
    Bad {
        name: &'static str,
        value: String,
        expected: &'static str,
    },
}

impl Settings {
    /// Read the `PUSH_RELAY_*` variables through `var`, so a test reads
    /// no process environment. An empty value counts as not set.
    pub fn from_var(var: &dyn Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let read = |name: &str| var(name).filter(|value| !value.trim().is_empty());
        let required = |name: &'static str, expected: &'static str| {
            read(name).ok_or(SettingsError::Missing { name, expected })
        };
        let public_origin = PublicOrigin::parse(&required(PUBLIC_ORIGIN, PUBLIC_ORIGIN_FORM)?)?;
        let database = PathBuf::from(required(DATABASE, DATABASE_FORM)?);
        let bind = match read(BIND) {
            Some(value) => parsed(BIND, value, BIND_FORM)?,
            None => DEFAULT_BIND,
        };
        let trusted_proxy = match read(TRUSTED_PROXY) {
            Some(value) => TrustedProxy::at(parsed(TRUSTED_PROXY, value, TRUSTED_PROXY_FORM)?),
            None => TrustedProxy::none(),
        };
        Ok(Self {
            public_origin,
            database,
            bind,
            trusted_proxy,
        })
    }
}

fn parsed<T: std::str::FromStr>(
    name: &'static str,
    value: String,
    expected: &'static str,
) -> Result<T, SettingsError> {
    value.parse().map_err(|_| SettingsError::Bad {
        name,
        value,
        expected,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn read(pairs: &[(&str, &str)]) -> Result<Settings, SettingsError> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        Settings::from_var(&|name| values.get(name).cloned())
    }

    const ORIGIN: (&str, &str) = ("PUSH_RELAY_PUBLIC_ORIGIN", "https://push.pagis.co");
    const DATABASE: (&str, &str) = ("PUSH_RELAY_DATABASE", "/data/relay.sqlite");

    fn message(pairs: &[(&str, &str)]) -> String {
        read(pairs)
            .expect_err("the settings are refused")
            .to_string()
    }

    #[test]
    fn the_required_variables_and_the_default_bind_make_the_settings() {
        let settings = read(&[ORIGIN, DATABASE]).expect("the settings");

        assert_eq!(settings.public_origin.as_str(), "https://push.pagis.co");
        assert_eq!(settings.database, PathBuf::from("/data/relay.sqlite"));
        assert_eq!(settings.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(settings.trusted_proxy, TrustedProxy::none());
    }

    #[test]
    fn every_variable_is_read() {
        let settings = read(&[
            ("PUSH_RELAY_PUBLIC_ORIGIN", "https://push.example.net:8443/"),
            DATABASE,
            ("PUSH_RELAY_BIND", "0.0.0.0:9000"),
            ("PUSH_RELAY_TRUSTED_PROXY", "10.0.0.2"),
        ])
        .expect("the settings");

        assert_eq!(
            settings.public_origin.as_str(),
            "https://push.example.net:8443"
        );
        assert_eq!(settings.bind, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(
            settings.trusted_proxy,
            TrustedProxy::at(IpAddr::from([10, 0, 0, 2]))
        );
    }

    #[test]
    fn a_missing_public_origin_names_the_variable() {
        assert_eq!(
            message(&[DATABASE]),
            "PUSH_RELAY_PUBLIC_ORIGIN is not set; set it to the https origin of \
             the relay, such as https://push.pagis.co"
        );
        assert_eq!(
            message(&[("PUSH_RELAY_PUBLIC_ORIGIN", ""), DATABASE]),
            message(&[DATABASE]),
            "an empty value is not set"
        );
    }

    #[test]
    fn a_bad_public_origin_names_the_variable() {
        for origin in [
            "push.pagis.co",
            "http://push.pagis.co",
            "https://push.pagis.co/relay",
            "https://push.pagis.co/?a=b",
            "https://user@push.pagis.co",
        ] {
            assert_eq!(
                message(&[("PUSH_RELAY_PUBLIC_ORIGIN", origin), DATABASE]),
                format!(
                    "PUSH_RELAY_PUBLIC_ORIGIN is {origin:?}, which is not the https \
                     origin of the relay, such as https://push.pagis.co"
                )
            );
        }
    }

    #[test]
    fn an_http_origin_on_a_loopback_address_is_accepted() {
        for origin in [
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
            "http://localhost:8080",
        ] {
            let settings = read(&[("PUSH_RELAY_PUBLIC_ORIGIN", origin), DATABASE])
                .unwrap_or_else(|error| panic!("{origin} is refused: {error}"));

            assert_eq!(settings.public_origin.as_str(), origin);
        }
        assert!(
            read(&[("PUSH_RELAY_PUBLIC_ORIGIN", "http://10.0.0.2"), DATABASE]).is_err(),
            "a private address is not loopback"
        );
    }

    #[test]
    fn a_missing_database_names_the_variable() {
        assert_eq!(
            message(&[ORIGIN]),
            "PUSH_RELAY_DATABASE is not set; set it to the path of the SQLite file \
             of the relay, such as /data/relay.sqlite"
        );
    }

    #[test]
    fn a_bad_bind_names_the_variable() {
        assert_eq!(
            message(&[ORIGIN, DATABASE, ("PUSH_RELAY_BIND", "localhost")]),
            "PUSH_RELAY_BIND is \"localhost\", which is not an IP address and a \
             port, such as 127.0.0.1:8080"
        );
    }

    #[test]
    fn a_bad_trusted_proxy_names_the_variable() {
        assert_eq!(
            message(&[
                ORIGIN,
                DATABASE,
                ("PUSH_RELAY_TRUSTED_PROXY", "10.0.0.2:443")
            ]),
            "PUSH_RELAY_TRUSTED_PROXY is \"10.0.0.2:443\", which is not the IP \
             address of the reverse proxy, such as 10.0.0.2"
        );
    }
}
