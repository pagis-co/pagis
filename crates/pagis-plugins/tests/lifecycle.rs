//! What one supervisor does when a server will not start
//! (ADR-0017): it waits, it waits longer, and after three failures in a
//! row it stops trying until the user asks.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pagis_core::{PluginId, WorkspaceId};
use pagis_plugins::{
    BACKOFF_CEILING, BACKOFF_FLOOR, MAX_START_FAILURES, PluginLogs, ServerError, ServerIo,
    ServerProcesses, ServerSupervisor, Spawn, ToolListChanged, backoff,
};

/// A Plugin Computer that starts nothing: every server start fails, the
/// way a container that cannot run the command fails.
struct NoComputer;

#[async_trait]
impl ServerProcesses for NoComputer {
    async fn start(&self, _workspace_id: &WorkspaceId, spawn: &Spawn) -> Result<ServerIo, String> {
        Err(format!(
            "no such program in the container: {}",
            spawn.program
        ))
    }
}

/// No server starts here, so no server says that its tools changed.
struct NoListChange;

#[async_trait]
impl ToolListChanged for NoListChange {
    async fn tool_list_changed(&self) {}
}

/// One supervisor over a Plugin Computer that starts nothing.
fn supervisor() -> ServerSupervisor {
    ServerSupervisor::new(
        "weather",
        WorkspaceId::generate(),
        Arc::new(NoComputer),
        PluginLogs::new(std::env::temp_dir().join("plugin-logs")).log(
            &WorkspaceId::generate(),
            &PluginId::from("weather".to_string()),
        ),
        Arc::new(NoListChange),
    )
}

/// A stdio server whose command does not exist.
fn missing() -> pagis_plugins::Spawn {
    pagis_plugins::Spawn {
        program: "/nonexistent/plugin-server".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn the_backoff_grows_from_one_second_to_one_minute() {
    assert_eq!(backoff(1), BACKOFF_FLOOR);
    assert_eq!(backoff(2), Duration::from_secs(2));
    assert_eq!(backoff(3), Duration::from_secs(4));
    assert_eq!(backoff(20), BACKOFF_CEILING);
}

#[tokio::test]
async fn a_start_failure_makes_the_next_call_wait() {
    let supervisor = supervisor();
    let launch = pagis_plugins::Launch::Stdio(missing());
    let now = Instant::now();

    let first = supervisor.client(&launch, "mark", now).await;
    assert!(matches!(first, Err(ServerError::Start(_))), "{first:?}");

    // The second call is inside the backoff, so it does not start the
    // server again: it says how long it waits.
    let inside = supervisor.client(&launch, "mark", now).await;
    assert!(
        matches!(inside, Err(ServerError::Waiting { .. })),
        "{inside:?}"
    );
}

#[tokio::test]
async fn three_start_failures_in_a_row_stop_the_daemon_from_trying() {
    let supervisor = supervisor();
    let launch = pagis_plugins::Launch::Stdio(missing());
    let mut now = Instant::now();

    for _ in 0..MAX_START_FAILURES - 1 {
        let failed = supervisor.client(&launch, "mark", now).await;
        assert!(matches!(failed, Err(ServerError::Start(_))), "{failed:?}");
        now += BACKOFF_CEILING;
    }
    let failed = supervisor.client(&launch, "mark", now).await;
    assert!(matches!(failed, Err(ServerError::Failed(_))), "{failed:?}");

    // Every later call is refused without a start, until the user asks
    // for one.
    now += BACKOFF_CEILING;
    let refused = supervisor.client(&launch, "mark", now).await;
    assert!(
        matches!(refused, Err(ServerError::Failed(_))),
        "{refused:?}"
    );

    supervisor.clear_failures().await;
    let tried = supervisor.client(&launch, "mark", now).await;
    assert!(matches!(tried, Err(ServerError::Start(_))), "{tried:?}");
}
