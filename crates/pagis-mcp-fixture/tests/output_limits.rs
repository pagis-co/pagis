//! The output limits of a stdio server (ADR-0017): one stdout message
//! over the frame cap ends the session, three of them in a row mark the
//! Plugin failed, and the stderr log never passes its size.

use crate::support;

use std::time::Duration;

use pagis_broker::ToolResult;
use pagis_core::{Plugin, PluginState};
use pagis_plugins::{LOG_FULL_MARKER, MAX_FRAME_BYTES, MAX_LOG_BYTES, MAX_OUTPUT_OVERRUNS};
use support::{Harness, api_key, stdio_plugin};

/// One call whose server writes one stdout line over the frame cap and
/// never ends it. The deadline of the call is far away, so an answer
/// that comes in time comes from the cap.
async fn overrun(harness: &Harness, plugin: &Plugin) -> ToolResult {
    let call = harness.call(
        plugin,
        "fixture",
        "flood",
        serde_json::json!({"size": MAX_FRAME_BYTES + 1}),
        Duration::from_secs(120),
    );
    tokio::time::timeout(Duration::from_secs(15), harness.host.dispatch(&call))
        .await
        .expect("the call answers when the line passes the cap, and not at its deadline")
}

/// How many server starts the Plugin Computer took.
fn starts(harness: &Harness) -> usize {
    harness
        .computer
        .starts
        .lock()
        .expect("the stand-in lock")
        .len()
}

/// Wait until no server of the Plugin holds a process.
async fn until_stopped(harness: &Harness, plugin: &Plugin) {
    for _ in 0..50 {
        if harness.host.running_servers(&plugin.id).await.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the server session did not end");
}

#[tokio::test]
async fn a_stdout_line_over_the_frame_cap_ends_the_session_and_the_call_is_outcome_unknown() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let result = overrun(&harness, &plugin).await;

    assert!(result.is_error, "{result:?}");
    assert_eq!(result.code.as_deref(), Some("outcome_unknown"));
    until_stopped(&harness, &plugin).await;
    // One overrun does not fail the Plugin: the next call starts the
    // server again.
    assert_eq!(harness.reread(&plugin).await.state, PluginState::Enabled);
    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "again"}))
        .await;
    assert!(!answered.is_error, "{answered:?}");
    assert_eq!(answered.content, "again");
}

#[tokio::test]
async fn three_output_overruns_in_a_row_mark_the_plugin_failed_and_a_manual_start_clears_it() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    // Each overrun ends the session, and the next call starts the
    // server again with success. A successful start does not forget
    // the overruns.
    for _ in 0..MAX_OUTPUT_OVERRUNS {
        assert_eq!(harness.reread(&plugin).await.state, PluginState::Enabled);
        let result = overrun(&harness, &plugin).await;
        assert_eq!(
            result.code.as_deref(),
            Some("outcome_unknown"),
            "{result:?}"
        );
    }

    assert_eq!(harness.reread(&plugin).await.state, PluginState::Failed);
    assert!(harness.events().iter().any(|kind| kind == "plugin.failed"));
    // A failed Plugin answers no call and starts no server.
    let before = starts(&harness);
    let refused = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;
    assert!(refused.is_error, "{refused:?}");
    assert_eq!(starts(&harness), before);

    harness
        .host
        .start(&plugin.id)
        .await
        .expect("the manual start works");

    assert_eq!(harness.reread(&plugin).await.state, PluginState::Enabled);
    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "again"}))
        .await;
    assert_eq!(answered.content, "again");
}

#[tokio::test]
async fn a_call_that_completes_within_the_cap_sets_the_overrun_count_to_zero() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    for _ in 0..MAX_OUTPUT_OVERRUNS - 1 {
        overrun(&harness, &plugin).await;
    }
    let answered = harness
        .dispatch(
            &plugin,
            "echo",
            serde_json::json!({"text": "within the cap"}),
        )
        .await;
    assert_eq!(answered.content, "within the cap");
    for _ in 0..MAX_OUTPUT_OVERRUNS - 1 {
        overrun(&harness, &plugin).await;
    }

    // Four overruns, and never three in a row.
    assert_eq!(harness.reread(&plugin).await.state, PluginState::Enabled);

    // The count started again from zero at the call: one overrun more
    // makes three in a row.
    overrun(&harness, &plugin).await;
    assert_eq!(harness.reread(&plugin).await.state, PluginState::Failed);
}

#[tokio::test]
async fn a_server_that_prints_without_end_keeps_its_log_at_the_limit() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    let log = harness.host.log_path(&plugin.id);

    let answered = harness
        .dispatch(
            &plugin,
            "stderr",
            serde_json::json!({"size": 8 * MAX_LOG_BYTES}),
        )
        .await;
    assert!(!answered.is_error, "{answered:?}");

    // The log writer runs beside the call, so the test waits for the
    // marker. The log never passes its limit on the way.
    let mut text = String::new();
    for _ in 0..200 {
        let size = std::fs::metadata(&log).map(|held| held.len()).unwrap_or(0);
        assert!(size <= MAX_LOG_BYTES, "the log holds {size} bytes");
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains(LOG_FULL_MARKER) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The server is still running, and it writes nothing more to the
    // full log.
    let answered = harness
        .dispatch(&plugin, "stderr", serde_json::json!({"size": 64 * 1024}))
        .await;
    assert!(!answered.is_error, "{answered:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let text_after = std::fs::read_to_string(&log).expect("the log reads");

    assert!(text.len() as u64 <= MAX_LOG_BYTES, "{} bytes", text.len());
    assert_eq!(text.matches(LOG_FULL_MARKER).count(), 1);
    assert!(text.ends_with(LOG_FULL_MARKER));
    assert_eq!(text_after, text);
    // The desk reads the end of the log, which is the marker.
    let tail = harness.host.read_log(&plugin.id, 4096).await;
    assert!(tail.len() <= 4096);
    assert!(tail.ends_with(LOG_FULL_MARKER), "{tail:?}");
}

#[tokio::test]
async fn a_stdout_line_that_is_not_json_rpc_keeps_the_session() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    // The script prints a banner to stdout before it runs the server,
    // as many servers do. The line is under the cap, so only the
    // JSON-RPC reader sees it, and it skips the line.
    let script = format!(
        "#!/bin/sh\necho 'the fixture starts'\nexec {} \"$@\"\n",
        support::fixture_binary()
    );
    std::fs::write(directory.path().join("server"), script).expect("the server script");
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    assert!(!answered.is_error, "{answered:?}");
    assert_eq!(answered.content, "one");
}
