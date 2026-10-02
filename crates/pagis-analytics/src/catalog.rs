//! Every event the daemon can send, and every property of each one.
//! A property is an enum, a number or a flag. The types allow no text
//! that a person typed, no name, no address and no identifier but the
//! Installation ID.

use serde::Serialize;
use serde_json::{Map, Value, json};

/// One event of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The first start of an installation that sends analytics.
    InstallationCreated,
    /// The first start after the release changed.
    InstallationUpgraded {
        /// The release that sent the events before this one.
        from_release: String,
    },
    /// The daily report.
    InstallationReport(Report),
}

impl Event {
    /// The event name in PostHog.
    pub fn name(&self) -> &'static str {
        match self {
            Event::InstallationCreated => "installation_created",
            Event::InstallationUpgraded { .. } => "installation_upgraded",
            Event::InstallationReport(_) => "installation_report",
        }
    }

    /// The properties of the event, with the ones that every event
    /// carries: the release, the operating system and the architecture,
    /// and the switches that keep PostHog from making a person profile or
    /// looking up a location.
    pub fn properties(&self, release: &str) -> Map<String, Value> {
        let mut properties = match self {
            Event::InstallationCreated => Map::new(),
            Event::InstallationUpgraded { from_release } => {
                let mut properties = Map::new();
                properties.insert("from_release".to_string(), json!(from_release));
                properties
            }
            Event::InstallationReport(report) => match serde_json::to_value(report) {
                Ok(Value::Object(properties)) => properties,
                _ => unreachable!("a report serializes to an object"),
            },
        };
        properties.insert("release".to_string(), json!(release));
        properties.insert("os".to_string(), json!(std::env::consts::OS));
        properties.insert("arch".to_string(), json!(std::env::consts::ARCH));
        properties.insert("$lib".to_string(), json!("pagis"));
        properties.insert("$process_person_profile".to_string(), json!(false));
        properties.insert("$geoip_disable".to_string(), json!(true));
        properties
    }
}

/// The daily report of one installation: what kind it is, how big it
/// is, and which features it uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub installation: InstallationKind,
    pub storage: StorageBackend,
    /// Whether the installation runs in Remote Access (ADR-0028).
    pub remote_access: bool,
    /// Whether the daemon reaches Docker, so the Agents' Computers run.
    pub computers: bool,
    pub people: Bucket,
    pub agents: Bucket,
    #[serde(flatten)]
    pub features: Features,
}

/// Which kind of installation sends the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationKind {
    Local,
    Server,
}

/// Where the installation keeps its records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    Sqlite,
    Postgres,
}

/// A count as a range, so the report never holds an exact size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Bucket {
    #[serde(rename = "0")]
    None,
    #[serde(rename = "1")]
    One,
    #[serde(rename = "2-5")]
    Few,
    #[serde(rename = "6-20")]
    Some,
    #[serde(rename = "21+")]
    Many,
}

impl Bucket {
    pub fn of(count: usize) -> Bucket {
        match count {
            0 => Bucket::None,
            1 => Bucket::One,
            2..=5 => Bucket::Few,
            6..=20 => Bucket::Some,
            _ => Bucket::Many,
        }
    }
}

/// Which features the installation uses: one flag for each, true when
/// any Workspace holds at least one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Features {
    pub uses_connections: bool,
    pub uses_schedules: bool,
    pub uses_event_subscriptions: bool,
    pub uses_plugins: bool,
    pub uses_software: bool,
    pub uses_hosts: bool,
    pub uses_phone_numbers: bool,
    pub uses_agent_mailboxes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            installation: InstallationKind::Local,
            storage: StorageBackend::Sqlite,
            remote_access: false,
            computers: true,
            people: Bucket::One,
            agents: Bucket::Few,
            features: Features {
                uses_schedules: true,
                ..Features::default()
            },
        }
    }

    #[test]
    fn a_count_becomes_a_range() {
        let buckets: Vec<_> = [0, 1, 2, 5, 6, 20, 21, 500]
            .into_iter()
            .map(|count| serde_json::to_value(Bucket::of(count)).unwrap())
            .collect();
        assert_eq!(
            buckets,
            ["0", "1", "2-5", "2-5", "6-20", "6-20", "21+", "21+"]
        );
    }

    #[test]
    fn every_event_says_the_release_and_asks_for_no_profile_and_no_location() {
        for event in [
            Event::InstallationCreated,
            Event::InstallationUpgraded {
                from_release: "0.1.0".to_string(),
            },
            Event::InstallationReport(report()),
        ] {
            let properties = event.properties("0.2.0");
            assert_eq!(properties["release"], "0.2.0", "{}", event.name());
            assert_eq!(properties["os"], std::env::consts::OS);
            assert_eq!(properties["arch"], std::env::consts::ARCH);
            assert_eq!(properties["$process_person_profile"], false);
            assert_eq!(properties["$geoip_disable"], true);
            assert!(!properties.contains_key("$ip"), "{}", event.name());
        }
    }

    #[test]
    fn an_upgrade_names_the_release_before_it() {
        let properties = Event::InstallationUpgraded {
            from_release: "0.1.0".to_string(),
        }
        .properties("0.2.0");
        assert_eq!(properties["from_release"], "0.1.0");
    }

    /// The report holds these properties and no others, each a flag or
    /// a value from a fixed set. A new property is a change to this list.
    #[test]
    fn the_report_holds_the_catalog_properties_and_nothing_else() {
        let properties = Event::InstallationReport(report()).properties("0.2.0");
        let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "$geoip_disable",
                "$lib",
                "$process_person_profile",
                "agents",
                "arch",
                "computers",
                "installation",
                "os",
                "people",
                "release",
                "remote_access",
                "storage",
                "uses_agent_mailboxes",
                "uses_connections",
                "uses_event_subscriptions",
                "uses_hosts",
                "uses_phone_numbers",
                "uses_plugins",
                "uses_schedules",
                "uses_software",
            ]
        );
        assert_eq!(properties["installation"], "local");
        assert_eq!(properties["storage"], "sqlite");
        assert_eq!(properties["remote_access"], false);
        assert_eq!(properties["people"], "1");
        assert_eq!(properties["agents"], "2-5");
        assert_eq!(properties["uses_schedules"], true);
        assert_eq!(properties["uses_plugins"], false);
    }
}
