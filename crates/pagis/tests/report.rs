//! The Report for Home (ADR-0022): every boot makes sure the
//! Workspace has its daily Report Schedule, the user asks for a Report
//! ahead of the cadence, and Home reads the newest written Report.

use pagis::Installation;
use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn get_json(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    let response = client()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn the_seed_gives_the_workspace_a_daily_report_schedule() {
    let daemon = TestDaemon::start().await;

    let workspace = get_json(&daemon, "/api/v1/workspace").await;
    let schedule_id = workspace["report_schedule_id"].as_str().unwrap();

    let schedule = get_json(&daemon, &format!("/api/v1/schedules/{schedule_id}")).await;
    assert_eq!(schedule["kind"], "cron");
    assert_eq!(schedule["cron_expression"], "0 7 * * *");
    assert_eq!(schedule["state"], "active");
    assert_eq!(schedule["name"], "Daily report");
    // The Chief of Staff writes the Report, in its own channel.
    assert_eq!(
        schedule["agent_id"],
        workspace["chief_of_staff_agent_id"].clone()
    );
}

#[tokio::test]
async fn home_reads_no_report_before_the_first_one_is_written() {
    let daemon = TestDaemon::start().await;

    let report = get_json(&daemon, "/api/v1/workspace/report").await;

    assert!(report["schedule_id"].is_string());
    assert!(report["message"].is_null());
    assert!(report["writing_run_id"].is_null());
    assert!(report["next_due_at"].is_i64());
}

#[tokio::test]
async fn a_report_asked_for_now_wakes_the_schedule_ahead_of_its_cadence() {
    let daemon = TestDaemon::start().await;
    let workspace = get_json(&daemon, "/api/v1/workspace").await;
    let schedule_id = workspace["report_schedule_id"].as_str().unwrap();

    let before = get_json(&daemon, &format!("/api/v1/schedules/{schedule_id}")).await;

    let response = client()
        .post(format!(
            "{}/api/v1/schedules/{schedule_id}/run",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let wakeup: serde_json::Value = response.json().await.unwrap();
    assert_eq!(wakeup["source_kind"], "schedule");
    assert_eq!(wakeup["rule_id"], schedule_id);

    // The cadence is untouched: the next Report still comes at 07:00.
    let after = get_json(&daemon, &format!("/api/v1/schedules/{schedule_id}")).await;
    assert_eq!(after["next_due_at"], before["next_due_at"]);
}

#[tokio::test]
async fn an_unknown_schedule_cannot_be_run_now() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .post(format!(
            "{}/api/v1/schedules/sch_missing/run",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A Workspace that already has its Report Schedule keeps the one it
/// has: the boot makes one, never a second.
#[tokio::test]
async fn a_later_boot_does_not_add_a_second_report_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();
    let before: Option<String> = sqlx::query_scalar("SELECT report_schedule_id FROM workspaces")
        .fetch_one(&rows(&first).await)
        .await
        .unwrap();
    drop(first);

    let second = pagis::boot(&home, Installation::Local).await.unwrap();

    let after: Option<String> = sqlx::query_scalar("SELECT report_schedule_id FROM workspaces")
        .fetch_one(&rows(&second).await)
        .await
        .unwrap();
    assert_eq!(after, before);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM schedules WHERE name = 'Daily report'")
            .fetch_one(&rows(&second).await)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

/// A second pool on the database a boot opened, for the assertions that
/// read a column no store method answers. `Booted` names no pool type
/// because the backend is a setting, and WAL carries a second
/// reader beside the daemon's own pool.
async fn rows(booted: &pagis::Booted) -> sqlx::SqlitePool {
    pagis_storage_sqlite::connect(&booted.home.join("pagis.db"))
        .await
        .expect("open the test database")
}
