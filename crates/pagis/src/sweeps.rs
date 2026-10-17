//! Workspace sweeps and their Event Bus wake-ups.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, future::BoxFuture};
use pagis_core::{
    ConnectionStore, Event, EventBus, EventScope, StoreError, WorkspaceId, WorkspaceStore,
};
use pagis_trigger::Trigger;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The dependencies of the daemon's sweep loop.
pub struct Sweeps {
    pub assignments: Arc<pagis_core::MailAssignments>,
    pub clock: Arc<dyn pagis_core::Clock>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub bus: Arc<dyn EventBus>,
    pub mail: Arc<pagis_mail::MailCollector>,
    pub mail_triage: Arc<crate::mail_triage::MailTriagePoll>,
    pub knowledge: Arc<crate::knowledge::Worker>,
    pub trigger: Arc<Trigger>,
    pub connections: Arc<dyn ConnectionStore>,
    pub google: Arc<crate::connections::GoogleBinder>,
    pub collector_interval: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Job {
    Mail,
    MailPoll,
    Publish,
    Collect,
    Reauth,
}

type Key = (WorkspaceId, Job);
type Jobs = FuturesUnordered<BoxFuture<'static, Key>>;

impl Sweeps {
    /// Run one pass. A failed Workspace does not stop another Workspace.
    pub async fn pass(self: &Arc<Self>) -> Result<(), StoreError> {
        self.assignments.sweep(self.clock.now_ms()).await?;
        let mut jobs = Jobs::new();
        for workspace in self.workspaces.list().await? {
            for job in [Job::Mail, Job::MailPoll, Job::Publish, Job::Reauth] {
                jobs.push(self.job((workspace.id.clone(), job)));
            }
        }
        while let Some((workspace_id, job)) = jobs.next().await {
            if job == Job::Reauth {
                jobs.push(self.job((workspace_id, Job::Collect)));
            }
        }
        Ok(())
    }

    fn job(self: &Arc<Self>, key: Key) -> BoxFuture<'static, Key> {
        let sweeps = self.clone();
        async move {
            let (workspace_id, job) = &key;
            // The Org publishes Installation Connection events, but has no sweeps.
            match sweeps.workspaces.get(workspace_id).await {
                Ok(Some(_)) => {}
                Ok(None) => return key,
                Err(error) => {
                    tracing::warn!(%workspace_id, %error, "loading Workspace for sweep failed");
                    return key;
                }
            }
            match job {
                Job::Mail => {
                    if let Err(error) = sweeps.mail.sweep_workspace(workspace_id).await {
                        tracing::warn!(%workspace_id, %error, "mail sweep failed");
                    }
                }
                Job::MailPoll => {
                    if let Err(error) = sweeps.mail.poll_workspace(workspace_id).await {
                        tracing::warn!(%workspace_id, %error, "mail poll failed");
                    }
                    if let Err(error) = sweeps.mail_triage.poll_workspace(workspace_id).await {
                        tracing::warn!(%workspace_id, %error, "Mail Triage poll failed");
                    }
                }
                Job::Publish => {
                    if let Err(error) = sweeps.knowledge.publish_workspace_changes(workspace_id).await {
                        tracing::warn!(%workspace_id, %error, "knowledge change publication failed");
                    }
                }
                Job::Reauth => {
                    if let Err(error) = crate::collectors::unblock_connected(&sweeps.trigger, sweeps.connections.as_ref(), workspace_id).await {
                        tracing::warn!(%workspace_id, %error, "connection catch-up failed");
                    }
                }
                Job::Collect => {
                    let mut shared = HashSet::new();
                    if let Err(error) = sweeps.knowledge.collect_workspace_pass(workspace_id, &mut shared).await {
                        tracing::warn!(%workspace_id, %error, "knowledge acquisition failed");
                    }
                    if let Err(error) = crate::collectors::collect_workspace_targets(&sweeps.trigger, sweeps.connections.as_ref(), &sweeps.google, workspace_id, &shared, sweeps.collector_interval).await {
                        tracing::warn!(%workspace_id, %error, "target collection failed");
                    }
                }
            }
            key
        }.boxed()
    }

    /// Subscribe before startup returns, then run the one sweep loop.
    /// One slow Workspace does not delay another. Repeated changes join one pending job.
    pub async fn spawn(self: &Arc<Self>, cancel: CancellationToken) -> JoinHandle<()> {
        let mut events = self.bus.subscribe(EventScope::Installation, None).await;
        let sweeps = self.clone();
        tokio::spawn(async move {
            let mut mail = tokio::time::interval(pagis_mail::SWEEP_INTERVAL);
            let mut mail_polls = tokio::time::interval(pagis_mail::POLL_INTERVAL);
            let mut collectors = tokio::time::interval(sweeps.collector_interval);
            mail_polls.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            mail.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            collectors.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut jobs = Jobs::new();
            let mut running: HashMap<Key, bool> = HashMap::new();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    Some(key) = jobs.next(), if !jobs.is_empty() => {
                        if key.1 == Job::Reauth {
                            sweeps.enqueue((key.0.clone(), Job::Collect), &mut jobs, &mut running);
                        }
                        let pending = running.remove(&key).unwrap_or(false);
                        if pending {
                            running.insert(key.clone(), false);
                            jobs.push(sweeps.job(key));
                        }
                    }
                    Some(event) = events.next() => {
                        if let Some(job) = event_job(&event) {
                            sweeps.enqueue((event.workspace_id, job), &mut jobs, &mut running);
                        }
                    }
                    _ = mail.tick() => {
                        if let Err(error) = sweeps.assignments.sweep(sweeps.clock.now_ms()).await {
                            tracing::warn!(%error, "Mail Assignment sweep failed");
                        }
                        sweeps.enqueue_pass(&[Job::Mail], &mut jobs, &mut running).await;
                    },
                    _ = mail_polls.tick() => sweeps.enqueue_pass(&[Job::MailPoll], &mut jobs, &mut running).await,
                    _ = collectors.tick() => sweeps.enqueue_pass(&[Job::Publish, Job::Reauth], &mut jobs, &mut running).await,
                }
            }
            sweeps.mail.stop();
        })
    }

    fn enqueue(self: &Arc<Self>, key: Key, jobs: &mut Jobs, running: &mut HashMap<Key, bool>) {
        if let Some(pending) = running.get_mut(&key) {
            *pending = true;
        } else {
            running.insert(key.clone(), false);
            jobs.push(self.job(key));
        }
    }

    async fn enqueue_pass(
        self: &Arc<Self>,
        kinds: &[Job],
        jobs: &mut Jobs,
        running: &mut HashMap<Key, bool>,
    ) {
        match self.workspaces.list().await {
            Ok(workspaces) => {
                for workspace in workspaces {
                    for kind in kinds {
                        self.enqueue((workspace.id.clone(), *kind), jobs, running);
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "listing Workspaces for sweeps failed"),
        }
    }
}

fn event_job(event: &Event) -> Option<Job> {
    match event.event_type.as_str() {
        "agent_mailbox.active"
        | "agent_mailbox.dormant"
        | "agent_mailbox.deleted"
        | "agent_mailbox.unavailable"
        | "agent_mailbox.password_reset" => Some(Job::Mail),
        "event_subscription.created" | "event_subscription.updated" => Some(Job::Collect),
        "event_subscription.state_changed" if event.payload["state"] == "active" => {
            Some(Job::Collect)
        }
        "connection.changed" if event.payload["status"] == "connected" => Some(Job::Reauth),
        "grant.created" => Some(Job::Reauth),
        "sync.changed" | "grant.revoked" => Some(Job::Collect),
        _ => None,
    }
}
