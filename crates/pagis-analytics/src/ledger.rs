//! The Analytics Ledger: the file in the State Directory that holds the
//! Installation ID and what the daemon has already sent.

use std::io::Write;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::catalog::Event;

/// The ledger file in the State Directory.
pub const LEDGER_FILE: &str = "analytics.json";

/// How long one Installation Report covers, in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// A random ID, made at the first send. It comes from no hardware,
    /// no host name and no Public Origin, so it names nothing but the
    /// installation.
    pub installation_id: String,
    /// The release that sent the last events, or `None` before the
    /// first send.
    pub reported_release: Option<String>,
    /// When the last Installation Report went, in Unix milliseconds.
    pub last_report_at: Option<i64>,
}

/// The lifecycle event a start owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifecycle {
    Created,
    Upgraded { from_release: String },
}

/// What the daemon owes PostHog now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub lifecycle: Option<Lifecycle>,
    pub report: bool,
}

impl Due {
    pub fn is_empty(&self) -> bool {
        self.lifecycle.is_none() && !self.report
    }

    /// The lifecycle event, if one is due.
    pub fn lifecycle_event(&self) -> Option<Event> {
        self.lifecycle.as_ref().map(|lifecycle| match lifecycle {
            Lifecycle::Created => Event::InstallationCreated,
            Lifecycle::Upgraded { from_release } => Event::InstallationUpgraded {
                from_release: from_release.clone(),
            },
        })
    }
}

impl Ledger {
    /// A ledger with a new random Installation ID and nothing sent.
    pub fn new() -> Ledger {
        Ledger {
            installation_id: format!("{:032x}", rand::random::<u128>()),
            reported_release: None,
            last_report_at: None,
        }
    }

    /// The ledger in `home`, or `None` where there is none yet.
    pub fn read(home: &Path) -> anyhow::Result<Option<Ledger>> {
        let path = home.join(LEDGER_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(serde_json::from_str(&text).with_context(|| {
                format!("read the analytics ledger {}", path.display())
            })?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("read the analytics ledger {}", path.display()))
            }
        }
    }

    /// Write the ledger to `home`. A write goes to a temporary file
    /// first, so a crash leaves the old ledger or the new one.
    pub fn write(&self, home: &Path) -> anyhow::Result<()> {
        let path = home.join(LEDGER_FILE);
        let temporary = home.join(format!("{LEDGER_FILE}.tmp"));
        let mut file = std::fs::File::create(&temporary)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("write the analytics ledger {}", path.display()))?;
        Ok(())
    }

    /// What is due for `release` at `now`.
    pub fn due(&self, release: &str, now: i64) -> Due {
        let lifecycle = match &self.reported_release {
            None => Some(Lifecycle::Created),
            Some(reported) if reported != release => Some(Lifecycle::Upgraded {
                from_release: reported.clone(),
            }),
            Some(_) => None,
        };
        let report = self
            .last_report_at
            .is_none_or(|last| now.saturating_sub(last) >= DAY_MS);
        Due { lifecycle, report }
    }

    /// Record that the events of `due` went for `release` at `now`.
    pub fn sent(&mut self, due: &Due, release: &str, now: i64) {
        self.reported_release = Some(release.to_string());
        if due.report {
            self.last_report_at = Some(now);
        }
    }
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_new_ledger_owes_the_created_event_and_a_report() {
        let ledger = Ledger::new();
        assert_eq!(ledger.installation_id.len(), 32);
        assert_ne!(ledger.installation_id, Ledger::new().installation_id);
        assert_eq!(
            ledger.due("0.1.0", NOW),
            Due {
                lifecycle: Some(Lifecycle::Created),
                report: true
            }
        );
    }

    #[test]
    fn a_report_is_due_once_a_day() {
        let mut ledger = Ledger::new();
        let due = ledger.due("0.1.0", NOW);
        ledger.sent(&due, "0.1.0", NOW);

        assert!(ledger.due("0.1.0", NOW + DAY_MS - 1).is_empty());
        assert_eq!(
            ledger.due("0.1.0", NOW + DAY_MS),
            Due {
                lifecycle: None,
                report: true
            }
        );
    }

    #[test]
    fn a_new_release_owes_the_upgrade_event_once() {
        let mut ledger = Ledger::new();
        let due = ledger.due("0.1.0", NOW);
        ledger.sent(&due, "0.1.0", NOW);

        let due = ledger.due("0.2.0", NOW + 1);
        assert_eq!(
            due.lifecycle_event(),
            Some(Event::InstallationUpgraded {
                from_release: "0.1.0".to_string()
            })
        );
        assert!(!due.report);
        ledger.sent(&due, "0.2.0", NOW + 1);
        assert!(ledger.due("0.2.0", NOW + 2).is_empty());
        // The upgrade did not move the report clock.
        assert_eq!(ledger.last_report_at, Some(NOW));
    }

    #[test]
    fn the_ledger_reads_back_what_it_wrote() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Ledger::read(home.path()).unwrap(), None);

        let mut ledger = Ledger::new();
        ledger.sent(&ledger.due("0.1.0", NOW), "0.1.0", NOW);
        ledger.write(home.path()).unwrap();

        assert_eq!(Ledger::read(home.path()).unwrap(), Some(ledger));
        assert!(!home.path().join("analytics.json.tmp").exists());
    }
}
