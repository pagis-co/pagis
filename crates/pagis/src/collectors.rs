//! Provider collectors (ADR-0006).
//!
//! One collector runs per Connection that has a live Event
//! Subscription, not one per subscription: three rules on one Gmail
//! account cost one poll. The collector acquires, normalizes, and stops
//! at `Trigger::ingest`; it never creates a Run and never reads a
//! subscription's filter.
//!
//! Two Connection states are different and both matter. A revoked Agent
//! grant is one rule's problem, and `ingest` blocks that rule. A
//! Connection-wide `reauth_required` is the account's problem: every
//! rule on it is blocked, the cursor is kept, and reauthorization runs
//! one catch-up collection from where the cursor stopped.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pagis_broker::MAIL_MESSAGE_RECEIVED;
use pagis_core::{
    BlockReason, CollectorTarget, Connection, ConnectionId, ConnectionStore, IngestBatch,
    WorkspaceStore, now_ms,
};
use pagis_google::ProviderErrorCode;
use pagis_trigger::Trigger;
use tokio_util::sync::CancellationToken;

/// How often the daemon polls each active Connection.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// The longest a temporary failure holds one Connection back.
pub const MAX_BACKOFF: Duration = Duration::from_secs(900);

/// One Connection's failure state between passes.
#[derive(Debug, Default, Clone, Copy)]
struct Backoff {
    failures: u32,
    next_attempt_at: i64,
}

impl Backoff {
    /// Exponential with jitter, capped. The jitter comes off the clock
    /// so two Connections that failed together do not retry together.
    /// The step is the poll interval this loop runs at, not the default
    /// one: a loop that polls every 100 ms must not wait two minutes
    /// after one transient failure.
    fn after_failure(self, interval: Duration, now: i64) -> Self {
        let failures = self.failures.saturating_add(1);
        let base = interval
            .as_millis()
            .saturating_mul(1_u128 << failures.min(8))
            .min(MAX_BACKOFF.as_millis()) as i64;
        let jitter = if base > 0 {
            now.rem_euclid(base / 4 + 1)
        } else {
            0
        };
        Self {
            failures,
            next_attempt_at: now + base + jitter,
        }
    }
}

/// Start the collector loop. It stops polling a Connection as soon as
/// no live subscription remains on it, because the work list is the
/// live subscriptions themselves.
pub(crate) fn spawn_collectors(
    trigger: Arc<Trigger>,
    connections: Arc<dyn ConnectionStore>,
    google: Arc<crate::connections::GoogleBinder>,
    knowledge: Arc<crate::knowledge::Worker>,
    interval: Duration,
    cancel: CancellationToken,
) {
    // Cache invalidations must also progress while acquisition or the model waits.
    let notification_cancel = cancel.clone();
    let notifications = Arc::clone(&knowledge);
    tokio::spawn(async move {
        while let Some(result) = notification_cancel
            .run_until_cancelled(notifications.publish_changes())
            .await
        {
            if let Err(error) = result {
                tracing::warn!(%error, "knowledge change publication failed; will retry");
            }
            if notification_cancel
                .run_until_cancelled(tokio::time::sleep(interval))
                .await
                .is_none()
            {
                break;
            }
        }
    });
    let acquisition = Arc::clone(&knowledge);
    tokio::spawn(async move {
        let mut backoff: HashMap<(ConnectionId, String), Backoff> = HashMap::new();
        while !cancel.is_cancelled() {
            let shared = acquisition.collect_pass().await;
            let targets = match trigger.collector_targets().await {
                Ok(targets) => targets,
                Err(error) => {
                    tracing::error!(%error, "loading collector targets failed");
                    Vec::new()
                }
            };
            let live: std::collections::HashSet<(ConnectionId, String)> = targets
                .iter()
                .map(|target| (target.connection_id.clone(), target.event_kind.clone()))
                .collect();
            backoff.retain(|key, _| live.contains(key));
            for target in targets {
                if target.event_kind == MAIL_MESSAGE_RECEIVED
                    && shared.contains(&target.connection_id)
                {
                    continue;
                }
                let key = (target.connection_id.clone(), target.event_kind.clone());
                let state = backoff.get(&key).copied().unwrap_or_default();
                if now_ms() < state.next_attempt_at {
                    continue;
                }
                match collect_once(&trigger, connections.as_ref(), &google, &target).await {
                    Pass::Done => {
                        backoff.remove(&key);
                    }
                    Pass::Skipped => {}
                    Pass::Failed => {
                        backoff.insert(key, state.after_failure(interval, now_ms()));
                    }
                }
            }
            if cancel
                .run_until_cancelled(tokio::time::sleep(interval))
                .await
                .is_none()
            {
                return;
            }
        }
    });
}

enum Pass {
    Done,
    /// The Connection cannot serve this pass right now.
    Skipped,
    Failed,
}

async fn collect_once(
    trigger: &Trigger,
    connections: &dyn ConnectionStore,
    google: &crate::connections::GoogleBinder,
    target: &CollectorTarget,
) -> Pass {
    if target.event_kind != MAIL_MESSAGE_RECEIVED {
        tracing::warn!(kind = %target.event_kind, "no collector for this event kind");
        return Pass::Skipped;
    }
    let connection = match connections
        .get(&target.workspace_id, &target.connection_id)
        .await
    {
        Ok(Some(connection)) => connection,
        Ok(None) => return Pass::Skipped,
        Err(error) => {
            tracing::error!(%error, "loading a collector's connection failed");
            return Pass::Failed;
        }
    };
    // Every Mailbox Provider declares the mail Incoming Event too,
    // and an Agent Mailbox is read by the mail collector. This
    // loop serves the user's Google accounts alone, so a Connection of
    // another provider is another collector's work and must be left
    // alone: a rule this loop blocks wakes nobody.
    if connection.provider != pagis_google::GOOGLE_PROVIDER {
        return Pass::Skipped;
    }
    if connection.status != Connection::CONNECTED {
        block(trigger, &connection, BlockReason::ReauthRequired).await;
        return Pass::Skipped;
    }
    let Some(provider) = google.provider(&connection) else {
        block(trigger, &connection, BlockReason::ReauthRequired).await;
        return Pass::Skipped;
    };
    let cursor = match trigger
        .cursor(&connection.workspace_id, &connection.id, &target.event_kind)
        .await
    {
        Ok(cursor) => cursor,
        Err(error) => {
            tracing::error!(%error, "loading a provider cursor failed");
            return Pass::Failed;
        }
    };
    let collector = pagis_google::GmailCollector::new(provider, &connection.alias);
    let collected = match collector.collect(cursor.as_deref(), now_ms()).await {
        Ok(collected) => collected,
        Err(error) => {
            let code = error.code.as_str();
            if let Err(error) = trigger
                .record_collection_failure(
                    &target.workspace_id,
                    &connection.id,
                    &target.event_kind,
                    code,
                    now_ms(),
                )
                .await
            {
                tracing::error!(%error, "recording a collection failure failed");
            }
            // The account, not one pass, is what a refused
            // authorization means: every rule on it stops until the
            // user reauthorizes, and the cursor is kept for the
            // catch-up that follows.
            if matches!(
                error.code,
                ProviderErrorCode::ReauthRequired | ProviderErrorCode::PermissionRevoked
            ) {
                block(trigger, &connection, BlockReason::ReauthRequired).await;
                return Pass::Skipped;
            }
            return Pass::Failed;
        }
    };
    let batch = IngestBatch {
        workspace_id: target.workspace_id.clone(),
        source: pagis_core::EventSource::connection(connection.id.clone()),
        // A Gmail account is the user's, and every rule on it sees the
        // pass.
        agent_id: None,
        event_kind: target.event_kind.clone(),
        cursor: Some(collected.cursor),
        events: collected.events,
        received_at: now_ms(),
        baseline: collected.baseline,
    };
    match trigger.ingest(batch).await {
        Ok(_) => Pass::Done,
        Err(error) => {
            tracing::error!(%error, "ingesting a collection batch failed");
            Pass::Failed
        }
    }
}

async fn block(trigger: &Trigger, connection: &Connection, reason: BlockReason) {
    match trigger
        .block_connection(&connection.workspace_id, &connection.id, reason, now_ms())
        .await
    {
        Ok(0) => {}
        Ok(count) => {
            let reason = reason.as_str();
            tracing::info!(count, connection = %connection.alias, reason, "event subscriptions blocked")
        }
        Err(error) => tracing::error!(%error, "blocking event subscriptions failed"),
    }
}

/// Reactivate the subscriptions a Connection blocked, once it is
/// connected again. The retained cursor makes the next collection one
/// catch-up rather than a replay of the whole mailbox.
///
/// One pass covers every Workspace: the tenant comes off each Workspace
/// row, as the Schedule sweeper reads it off each Schedule.
pub fn spawn_reauth_catchup(
    trigger: Arc<Trigger>,
    connections: Arc<dyn ConnectionStore>,
    workspaces: Arc<dyn WorkspaceStore>,
    interval: Duration,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        while !cancel.is_cancelled() {
            match workspaces.list().await {
                Ok(rows) => {
                    for workspace in rows {
                        unblock_connected(&trigger, connections.as_ref(), &workspace.id).await;
                    }
                }
                Err(error) => tracing::error!(%error, "listing workspaces failed"),
            }
            if cancel
                .run_until_cancelled(tokio::time::sleep(interval))
                .await
                .is_none()
            {
                return;
            }
        }
    });
}

/// Reactivate the subscriptions of one Workspace's connected accounts.
async fn unblock_connected(
    trigger: &Trigger,
    connections: &dyn ConnectionStore,
    workspace_id: &pagis_core::WorkspaceId,
) {
    match connections.list(workspace_id).await {
        Ok(records) => {
            for connection in records
                .into_iter()
                .filter(|connection| connection.status == Connection::CONNECTED)
            {
                if let Err(error) = trigger
                    .unblock_connection(workspace_id, &connection.id, now_ms())
                    .await
                {
                    tracing::error!(%error, "reactivating event subscriptions failed");
                }
            }
        }
        Err(error) => tracing::error!(%error, "listing connections failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_grows_from_the_interval_the_loop_polls_at() {
        let interval = Duration::from_millis(100);
        let first = Backoff::default().after_failure(interval, 0);
        let second = first.after_failure(interval, 0);

        assert!(first.next_attempt_at >= 200, "{first:?}");
        assert!(first.next_attempt_at < 400, "one failure costs one step");
        assert!(second.next_attempt_at > first.next_attempt_at);
    }

    #[test]
    fn the_backoff_stops_at_the_cap() {
        let mut state = Backoff::default();
        for _ in 0..12 {
            state = state.after_failure(DEFAULT_POLL_INTERVAL, 0);
        }

        let cap = MAX_BACKOFF.as_millis() as i64;
        assert!(state.next_attempt_at <= cap + cap / 4, "{state:?}");
    }
}
