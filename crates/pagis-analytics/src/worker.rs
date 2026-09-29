//! The background task that sends the events that are due.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::catalog::{Event, Report};
use crate::ledger::Ledger;
use crate::posthog::PostHog;
use crate::project::Project;

/// How long after the start the first check waits, so the start and the
/// first minutes of work have the machine to themselves.
pub const FIRST_AFTER: Duration = Duration::from_secs(10 * 60);

/// How often the task looks for events that are due. A report is due
/// once a day; a check finds it within this interval.
pub const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// What the task reads of the installation.
#[async_trait]
pub trait Source: Send + Sync {
    /// Whether the Analytics System Setting is on. The task reads it at
    /// each check, so a change takes effect with no restart.
    fn enabled(&self) -> bool;
    /// The Installation Report of now.
    async fn report(&self) -> anyhow::Result<Report>;
}

/// The analytics task of one daemon.
pub struct Analytics {
    posthog: PostHog,
    source: Arc<dyn Source>,
    /// The State Directory, which holds the ledger.
    home: PathBuf,
    release: String,
    first_after: Duration,
    check_every: Duration,
}

impl Analytics {
    pub fn new(
        project: Project,
        source: Arc<dyn Source>,
        home: PathBuf,
        release: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            posthog: PostHog::new(project)?,
            source,
            home,
            release: release.to_string(),
            first_after: FIRST_AFTER,
            check_every: CHECK_EVERY,
        })
    }

    /// Change when the checks run. Tests shorten them.
    pub fn with_timing(mut self, first_after: Duration, check_every: Duration) -> Self {
        self.first_after = first_after;
        self.check_every = check_every;
        self
    }

    /// Run the checks in a background task until `cancel`. Nothing the
    /// daemon does waits on the task.
    pub fn spawn(self, cancel: CancellationToken) {
        tokio::spawn(async move {
            cancel.run_until_cancelled(self.run()).await;
        });
    }

    async fn run(self) {
        tokio::time::sleep(self.first_after).await;
        loop {
            if let Err(error) = self.check(now_ms()).await {
                // An offline machine fails every check, so the failure
                // is a debug line and the next check tries again.
                tracing::debug!(error = format!("{error:#}"), "analytics were not sent");
            }
            tokio::time::sleep(self.check_every).await;
        }
    }

    /// One check at `now`: send the events that are due, and record
    /// them in the ledger once PostHog accepts them. Answer whether a
    /// batch went.
    pub async fn check(&self, now: i64) -> anyhow::Result<bool> {
        if !self.source.enabled() {
            return Ok(false);
        }
        let ledger = match Ledger::read(&self.home)? {
            Some(ledger) => ledger,
            None => {
                // The ID is written before the first send, so a send that
                // fails is sent again with the same ID.
                let ledger = Ledger::new();
                ledger.write(&self.home)?;
                ledger
            }
        };
        let due = ledger.due(&self.release, now);
        if due.is_empty() {
            return Ok(false);
        }
        let mut events: Vec<Event> = due.lifecycle_event().into_iter().collect();
        if due.report {
            events.push(Event::InstallationReport(self.source.report().await?));
        }
        self.posthog
            .capture(&ledger.installation_id, &self.release, &events)
            .await?;
        let mut ledger = ledger;
        ledger.sent(&due, &self.release, now);
        ledger.write(&self.home)?;
        Ok(true)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
