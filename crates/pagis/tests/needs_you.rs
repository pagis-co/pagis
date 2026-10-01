//! The Needs-You Queue on Home (ADR-0022): the Person dismisses a
//! failed Run or a missed Call, and the record keeps the time, so the
//! item stays out of the queue after a reload.

use pagis_core::{AgentId, CallStore, ChannelId, RunState, RunStore};
use pagis_storage_sqlite::{SqliteCallStore, SqliteRunStore};
use pagis_testkit::TestDaemon;
use pagis_testkit::fixture::{missed_call, queued_run};

async fn post(daemon: &TestDaemon, path: &str) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .status()
}

async fn first_item(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    let page = reqwest::Client::new()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    page["items"][0].clone()
}

#[tokio::test]
async fn a_dismissed_failure_and_missed_call_keep_the_time_on_their_records() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let mut run = queued_run(
        &daemon.workspace_id,
        &agent_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    run.state = RunState::Failed;
    SqliteRunStore::new(daemon.pool().clone())
        .create(&run)
        .await
        .expect("write the failed Run");
    let call = missed_call(&daemon.workspace_id, &agent_id, &run.id);
    SqliteCallStore::new(daemon.pool().clone())
        .insert(&call)
        .await
        .expect("write the missed Call");

    assert_eq!(
        first_item(&daemon, "/api/v1/runs?state=failed").await["dismissed_at"],
        serde_json::Value::Null
    );
    assert_eq!(
        post(
            &daemon,
            &format!("/api/v1/runs/{}/dismiss", run.id.as_str())
        )
        .await,
        204
    );
    assert_eq!(
        post(
            &daemon,
            &format!("/api/v1/calls/{}/dismiss", call.id.as_str())
        )
        .await,
        204
    );

    assert!(first_item(&daemon, "/api/v1/runs?state=failed").await["dismissed_at"].is_i64());
    assert!(first_item(&daemon, "/api/v1/calls?direction=inbound").await["dismissed_at"].is_i64());
}

#[tokio::test]
async fn a_dismissal_of_no_record_is_not_found() {
    let daemon = TestDaemon::start().await;

    assert_eq!(post(&daemon, "/api/v1/runs/no-such-run/dismiss").await, 404);
    assert_eq!(
        post(&daemon, "/api/v1/calls/no-such-call/dismiss").await,
        404
    );
}
