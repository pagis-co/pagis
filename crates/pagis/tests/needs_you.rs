//! The Needs-You Queue (ADR-0022, ADR-0030): the daemon derives it on
//! each read, and the Person dismisses a failed Run or a missed Call, and
//! the record keeps the time, so the item stays out of the queue after a
//! reload.

use pagis_core::{AgentId, CallStore, ChannelId, RequestStore, RunState, RunStore};
use pagis_storage_sqlite::{SqliteCallStore, SqliteRequestStore, SqliteRunStore};
use pagis_testkit::fixture::{missed_call, pending_request, queued_run};
use pagis_testkit::{TestDaemon, TwoTenants};

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

/// The Needs-You Queue that `cookie` reads.
async fn needs_you(base_url: &str, cookie: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("{base_url}/api/v1/needs-you"))
        .header("cookie", cookie)
        .send()
        .await
        .expect("read the Needs-You Queue");
    assert_eq!(response.status(), 200);
    response.json().await.expect("the queue")
}

/// Each item of a queue as `<kind> <id>`.
fn rows(queue: &serde_json::Value) -> Vec<String> {
    queue["items"]
        .as_array()
        .expect("the items")
        .iter()
        .map(|item| format!("{} {}", item["kind"], item["id"]).replace('"', ""))
        .collect()
}

#[tokio::test]
async fn the_queue_holds_a_pending_request_and_a_failed_run() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    let runs = SqliteRunStore::new(daemon.pool().clone());
    let parked = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
    runs.create(&parked).await.expect("write the parked Run");
    let request = pending_request(&daemon.workspace_id, &agent_id, &parked.id, "echo hi");
    SqliteRequestStore::new(daemon.pool().clone())
        .create(&request)
        .await
        .expect("write the pending Request");
    let mut failed = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
    failed.state = RunState::Failed;
    runs.create(&failed).await.expect("write the failed Run");

    let queue = needs_you(&daemon.base_url, daemon.cookie()).await;

    assert_eq!(queue["count"], 2);
    assert_eq!(
        rows(&queue),
        [
            format!("approval request:{}", request.id),
            format!("failed run:{}", failed.id),
        ]
    );
    assert_eq!(
        queue["items"][0]["url"],
        format!("/c/{}", daemon.dm_channel_id)
    );
    assert_eq!(queue["items"][1]["url"], format!("/runs/{}", failed.id));
}

/// The reader reads the failed Runs of today page by page, so a day
/// with more failures than one page holds them all.
#[tokio::test]
async fn the_queue_holds_every_failure_of_a_busy_day() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    let runs = SqliteRunStore::new(daemon.pool().clone());
    for _ in 0..101 {
        let mut failed = queued_run(&daemon.workspace_id, &agent_id, &channel_id);
        failed.state = RunState::Failed;
        runs.create(&failed).await.expect("write a failed Run");
    }

    let queue = needs_you(&daemon.base_url, daemon.cookie()).await;

    assert_eq!(queue["count"], 101);
}

#[tokio::test]
async fn person_b_reads_nothing_of_person_as_queue() {
    let world = TwoTenants::start().await;
    let a_request = format!("approval request:{}", world.a_id("request_id"));

    let a_queue = needs_you(&world.daemon.base_url, &world.a.cookie).await;
    let b_queue = needs_you(&world.daemon.base_url, &world.b.cookie).await;

    assert!(rows(&a_queue).contains(&a_request), "A reads {a_queue}");
    assert_eq!(b_queue["items"], serde_json::json!([]));
    assert_eq!(b_queue["count"], 0);
}
