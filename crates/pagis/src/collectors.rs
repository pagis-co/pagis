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

use std::time::Duration;

use pagis_broker::MAIL_MESSAGE_RECEIVED;
use pagis_core::{
    BlockReason, CollectorTarget, Connection, ConnectionId, ConnectionStore, IngestBatch,
    WorkspaceId, now_ms,
};
use pagis_google::ProviderErrorCode;
use pagis_trigger::Trigger;

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

/// Collect the due targets of one Workspace. Shared acquisition reads Gmail once.
pub async fn collect_workspace_targets(
    trigger: &Trigger,
    connections: &dyn ConnectionStore,
    google: &crate::connections::GoogleBinder,
    workspace_id: &WorkspaceId,
    shared: &std::collections::HashSet<ConnectionId>,
    interval: Duration,
) -> Result<(), pagis_trigger::TriggerError> {
    for target in trigger.collector_targets(workspace_id).await? {
        if target.event_kind == MAIL_MESSAGE_RECEIVED && shared.contains(&target.connection_id) {
            continue;
        }
        if now_ms() < target.next_attempt_at {
            continue;
        }
        let state = Backoff {
            failures: target.failures,
            next_attempt_at: target.next_attempt_at,
        };
        match collect_once(trigger, connections, google, &target).await {
            Pass::Done => trigger.set_collector_backoff(&target, 0, 0).await?,
            Pass::Skipped => {}
            Pass::Failed => {
                let next = state.after_failure(interval, now_ms());
                trigger
                    .set_collector_backoff(&target, next.failures, next.next_attempt_at)
                    .await?;
            }
        }
    }
    Ok(())
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
        tracing::warn!(workspace_id = %target.workspace_id, kind = %target.event_kind, "no collector for this event kind");
        return Pass::Skipped;
    }
    let connection = match connections
        .get(&target.workspace_id, &target.connection_id)
        .await
    {
        Ok(Some(connection)) => connection,
        Ok(None) => return Pass::Skipped,
        Err(error) => {
            tracing::error!(%error, workspace_id = %target.workspace_id, "loading a collector's connection failed");
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
            tracing::error!(%error, workspace_id = %target.workspace_id, "loading a provider cursor failed");
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
                tracing::error!(%error, workspace_id = %target.workspace_id, "recording a collection failure failed");
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
            tracing::error!(%error, workspace_id = %target.workspace_id, "ingesting a collection batch failed");
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
            tracing::info!(workspace_id = %connection.workspace_id, count, connection = %connection.alias, reason, "event subscriptions blocked")
        }
        Err(error) => {
            tracing::error!(%error, workspace_id = %connection.workspace_id, "blocking event subscriptions failed")
        }
    }
}

/// Reactivate the subscriptions of one Workspace's connected accounts.
pub async fn unblock_connected(
    trigger: &Trigger,
    connections: &dyn ConnectionStore,
    workspace_id: &WorkspaceId,
) -> Result<(), pagis_trigger::TriggerError> {
    for connection in connections.list(workspace_id).await? {
        if connection.status == Connection::CONNECTED {
            trigger
                .unblock_connection(workspace_id, &connection.id, now_ms())
                .await?;
        }
    }
    Ok(())
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
