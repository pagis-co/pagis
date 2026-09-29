//! The installation as the analytics task reads it (ADR-0026): the
//! System Setting, and the Installation Report built from the stores.

use std::sync::Arc;
use std::time::Duration;

use pagis_analytics::{
    Blocked, Bucket, Features, InstallationKind, Project, Report, StorageBackend,
};
use pagis_core::{OrgId, Stores};
use pagis_server::SystemConfigFile;

/// Where the daemon sends analytics, and when it checks.
pub struct AnalyticsOptions {
    /// The PostHog project, or why the daemon sends nothing.
    pub destination: Result<Project, Blocked>,
    pub first_after: Duration,
    pub check_every: Duration,
}

impl AnalyticsOptions {
    /// The project this build holds, unless `DO_NOT_TRACK` is set.
    pub fn production() -> Self {
        Self {
            destination: pagis_analytics::destination(
                Project::of_this_build(),
                std::env::var("DO_NOT_TRACK").ok().as_deref(),
            ),
            first_after: pagis_analytics::FIRST_AFTER,
            check_every: pagis_analytics::CHECK_EVERY,
        }
    }

    /// A daemon that sends nothing: a build from source.
    pub fn off() -> Self {
        Self {
            destination: Err(Blocked::Build),
            first_after: pagis_analytics::FIRST_AFTER,
            check_every: pagis_analytics::CHECK_EVERY,
        }
    }
}

/// The installation behind the analytics task.
pub(crate) struct InstallationSource {
    pub system: Arc<dyn SystemConfigFile>,
    pub stores: Stores,
    pub org_id: OrgId,
    pub installation: InstallationKind,
    pub storage: StorageBackend,
    pub multi_user: bool,
    pub docker_discovery: Arc<pagis_computer::DockerDiscovery>,
}

#[async_trait::async_trait]
impl pagis_analytics::Source for InstallationSource {
    /// A config file that does not read sends nothing.
    fn enabled(&self) -> bool {
        self.system.read().is_ok_and(|config| config.analytics)
    }

    async fn report(&self) -> anyhow::Result<Report> {
        let stores = &self.stores;
        let people = stores
            .users
            .list_by_org(&self.org_id)
            .await?
            .iter()
            .filter(|person| person.disabled_at.is_none())
            .count();
        let mut agents = 0;
        let mut features = Features::default();
        for workspace in stores.workspaces.list().await? {
            let id = &workspace.id;
            agents += stores.agents.list_by_workspace(id).await?.len();
            features.uses_connections |= !stores.connections.list(id).await?.is_empty();
            // Each Workspace starts with its Report Schedule, so only
            // another Schedule says that the feature is in use.
            features.uses_schedules |= stores
                .schedules
                .list(id, None, 2)
                .await?
                .iter()
                .any(|schedule| Some(&schedule.id) != workspace.report_schedule_id.as_ref());
            features.uses_event_subscriptions |= !stores
                .subscriptions
                .list(id, None, None, 1)
                .await?
                .is_empty();
            features.uses_plugins |= !stores.plugins.list(id).await?.is_empty();
            features.uses_software |= !stores.software.list_packages(id).await?.is_empty();
            features.uses_hosts |= !stores.hosts.list(id).await?.is_empty();
            features.uses_phone_numbers |= !stores.phone_numbers.list(id).await?.is_empty();
            features.uses_agent_mailboxes |= !stores.agent_mailboxes.list(id).await?.is_empty();
        }
        Ok(Report {
            installation: self.installation,
            storage: self.storage,
            multi_user: self.multi_user,
            computers: self.docker_discovery.endpoint().await.is_some(),
            people: Bucket::of(people),
            agents: Bucket::of(agents),
            features,
        })
    }
}
