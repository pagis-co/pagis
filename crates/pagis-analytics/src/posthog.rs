//! The PostHog capture API: one batch of events in one request.

use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};

use crate::catalog::Event;
use crate::project::Project;

/// How long one request may take. The daemon never waits on it, and a
/// batch that does not go is sent again at the next check.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The capture client of one PostHog project.
pub struct PostHog {
    project: Project,
    client: reqwest::Client,
}

impl PostHog {
    pub fn new(project: Project) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("build the analytics client")?;
        Ok(Self { project, client })
    }

    /// Send `events` of the installation `installation_id` in one batch
    /// to the `/batch/` endpoint.
    pub async fn capture(
        &self,
        installation_id: &str,
        release: &str,
        events: &[Event],
    ) -> anyhow::Result<()> {
        let body = json!({
            "api_key": self.project.token,
            "batch": events
                .iter()
                .map(|event| json!({
                    "event": event.name(),
                    "distinct_id": installation_id,
                    "properties": Value::Object(event.properties(release)),
                }))
                .collect::<Vec<_>>(),
        });
        let url = format!("{}/batch/", self.project.host.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("send the analytics batch")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("PostHog answered {status}");
        }
        Ok(())
    }
}
