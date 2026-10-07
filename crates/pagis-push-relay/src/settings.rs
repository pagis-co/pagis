//! The settings of the relay, from `PUSH_RELAY_*` variables.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use crate::TrustedProxy;

const PUBLIC_ORIGIN: &str = "PUSH_RELAY_PUBLIC_ORIGIN";
const DATABASE: &str = "PUSH_RELAY_DATABASE";
const BIND: &str = "PUSH_RELAY_BIND";
const TRUSTED_PROXY: &str = "PUSH_RELAY_TRUSTED_PROXY";
const APNS_KEY_PATH: &str = "PUSH_RELAY_APNS_KEY_PATH";
const APNS_KEY_ID: &str = "PUSH_RELAY_APNS_KEY_ID";
const APNS_TEAM_ID: &str = "PUSH_RELAY_APNS_TEAM_ID";
const APNS_TOPIC: &str = "PUSH_RELAY_APNS_TOPIC";
const FCM_CREDENTIALS_PATH: &str = "PUSH_RELAY_FCM_CREDENTIALS_PATH";
const FCM_PROJECT_ID: &str = "PUSH_RELAY_FCM_PROJECT_ID";

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
    /// The APNs key and the app, or `None` when the relay serves no
    /// `ios` registration.
    pub apns: Option<ApnsSettings>,
    /// The service account and the Firebase project, or `None` when the
    /// relay serves no `android` registration.
    pub fcm: Option<FcmSettings>,
}

/// What the relay needs to send to APNs with a token-based connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApnsSettings {
    /// The `.p8` file of the APNs key: a P-256 private key in PKCS#8 PEM.
    pub key_path: PathBuf,
    /// The 10-character id of the APNs key.
    pub key_id: String,
    /// The 10-character id of the Apple developer team.
    pub team_id: String,
    /// The bundle id of the Mobile App, `app.pagis.mobile` (ADR-0032).
    pub topic: String,
}

/// What the relay needs to send to FCM with the HTTP v1 API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FcmSettings {
    /// The JSON key file of the service account that sends.
    pub credentials_path: PathBuf,
    /// The id of the Firebase project of the Mobile App.
    pub project_id: String,
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
    #[error(
        "some {service} variables are set and some are not; set {}, or remove \
         each {service} variable so that the relay serves no {platform} registration",
        names(.missing)
    )]
    Partial {
        service: &'static str,
        platform: &'static str,
        missing: Vec<&'static str>,
    },
}

/// `A`, `A and B`, or `A, B and C`.
fn names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
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
        let apns = all_or_none(
            &read,
            [APNS_KEY_PATH, APNS_KEY_ID, APNS_TEAM_ID, APNS_TOPIC],
            "APNs",
            "ios",
        )?
        .map(|[key_path, key_id, team_id, topic]| ApnsSettings {
            key_path: PathBuf::from(key_path),
            key_id,
            team_id,
            topic,
        });
        let fcm = all_or_none(
            &read,
            [FCM_CREDENTIALS_PATH, FCM_PROJECT_ID],
            "FCM",
            "android",
        )?
        .map(|[credentials_path, project_id]| FcmSettings {
            credentials_path: PathBuf::from(credentials_path),
            project_id,
        });
        Ok(Self {
            public_origin,
            database,
            bind,
            trusted_proxy,
            apns,
            fcm,
        })
    }
}

/// The values of the variables of one push service: all of them, or
/// `None` when none is set. Some but not all is an error that names each
/// missing variable.
fn all_or_none<const N: usize>(
    read: &dyn Fn(&str) -> Option<String>,
    names: [&'static str; N],
    service: &'static str,
    platform: &'static str,
) -> Result<Option<[String; N]>, SettingsError> {
    let values = names.map(read);
    let missing: Vec<&'static str> = names
        .iter()
        .zip(&values)
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| *name)
        .collect();
    if missing.len() == N {
        return Ok(None);
    }
    if !missing.is_empty() {
        return Err(SettingsError::Partial {
            service,
            platform,
            missing,
        });
    }
    Ok(Some(values.map(|value| value.unwrap_or_default())))
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

    const APNS: [(&str, &str); 4] = [
        ("PUSH_RELAY_APNS_KEY_PATH", "/run/secrets/apns.p8"),
        ("PUSH_RELAY_APNS_KEY_ID", "ABC123DEFG"),
        ("PUSH_RELAY_APNS_TEAM_ID", "DEF123GHIJ"),
        ("PUSH_RELAY_APNS_TOPIC", "app.pagis.mobile"),
    ];

    #[test]
    fn no_apns_variable_serves_no_ios() {
        let settings = read(&[ORIGIN, DATABASE]).expect("the settings");

        assert_eq!(settings.apns, None);
    }

    #[test]
    fn the_four_apns_variables_make_the_apns_settings() {
        let settings = read(&[&[ORIGIN, DATABASE][..], &APNS[..]].concat()).expect("the settings");

        assert_eq!(
            settings.apns,
            Some(ApnsSettings {
                key_path: PathBuf::from("/run/secrets/apns.p8"),
                key_id: "ABC123DEFG".to_string(),
                team_id: "DEF123GHIJ".to_string(),
                topic: "app.pagis.mobile".to_string(),
            })
        );
    }

    #[test]
    fn a_partial_apns_setting_names_each_missing_variable() {
        let partial = [ORIGIN, DATABASE, APNS[0], APNS[2]];

        assert_eq!(
            message(&partial),
            "some APNs variables are set and some are not; set \
             PUSH_RELAY_APNS_KEY_ID and PUSH_RELAY_APNS_TOPIC, or remove each \
             APNs variable so that the relay serves no ios registration"
        );
        assert_eq!(
            message(&[ORIGIN, DATABASE, APNS[3]]),
            "some APNs variables are set and some are not; set \
             PUSH_RELAY_APNS_KEY_PATH, PUSH_RELAY_APNS_KEY_ID and \
             PUSH_RELAY_APNS_TEAM_ID, or remove each APNs variable so that the \
             relay serves no ios registration"
        );
        assert_eq!(
            message(&[
                ORIGIN,
                DATABASE,
                APNS[0],
                APNS[1],
                APNS[2],
                ("PUSH_RELAY_APNS_TOPIC", " ")
            ]),
            "some APNs variables are set and some are not; set \
             PUSH_RELAY_APNS_TOPIC, or remove each APNs variable so that the \
             relay serves no ios registration",
            "an empty value is not set"
        );
    }

    const FCM: [(&str, &str); 2] = [
        ("PUSH_RELAY_FCM_CREDENTIALS_PATH", "/run/secrets/fcm.json"),
        ("PUSH_RELAY_FCM_PROJECT_ID", "pagis-mobile"),
    ];

    #[test]
    fn no_fcm_variable_serves_no_android() {
        let settings = read(&[ORIGIN, DATABASE]).expect("the settings");

        assert_eq!(settings.fcm, None);
    }

    #[test]
    fn the_two_fcm_variables_make_the_fcm_settings() {
        let settings = read(&[ORIGIN, DATABASE, FCM[0], FCM[1]]).expect("the settings");

        assert_eq!(
            settings.fcm,
            Some(FcmSettings {
                credentials_path: PathBuf::from("/run/secrets/fcm.json"),
                project_id: "pagis-mobile".to_string(),
            })
        );
    }

    #[test]
    fn a_partial_fcm_setting_names_the_missing_variable() {
        assert_eq!(
            message(&[ORIGIN, DATABASE, FCM[0]]),
            "some FCM variables are set and some are not; set \
             PUSH_RELAY_FCM_PROJECT_ID, or remove each FCM variable so that the \
             relay serves no android registration"
        );
        assert_eq!(
            message(&[ORIGIN, DATABASE, FCM[1]]),
            "some FCM variables are set and some are not; set \
             PUSH_RELAY_FCM_CREDENTIALS_PATH, or remove each FCM variable so \
             that the relay serves no android registration"
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
