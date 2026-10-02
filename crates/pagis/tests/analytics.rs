//! Anonymous analytics (ADR-0026): the System Setting, what the
//! Administration Interface says about it, and what the daemon sends.

use std::time::Duration;

use pagis_analytics::{Blocked, Project};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn settings(daemon: &TestDaemon) -> serde_json::Value {
    client()
        .get(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn switch(daemon: &TestDaemon, cookie: &str, enabled: bool) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/settings/system/analytics",
            daemon.administration_base_url
        ))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await
        .unwrap()
}

/// A fake PostHog that accepts every batch.
async fn posthog() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/batch/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

/// Options that send to `posthog` and check every few milliseconds.
fn sending_to(posthog: &MockServer) -> TestDaemonOptions {
    TestDaemonOptions {
        analytics: pagis::AnalyticsOptions {
            destination: Ok(Project {
                id: "12345".to_string(),
                token: "phc_test".to_string(),
                host: posthog.uri(),
            }),
            first_after: Duration::from_millis(10),
            check_every: Duration::from_millis(20),
        },
        ..TestDaemonOptions::default()
    }
}

async fn batches(posthog: &MockServer) -> Vec<serde_json::Value> {
    posthog
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

async fn first_batch(posthog: &MockServer) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(batch) = batches(posthog).await.into_iter().next() {
            return batch;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the daemon sent nothing"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A build from source holds no PostHog project, so it sends nothing,
/// and the settings say why. The setting itself starts on.
#[tokio::test]
async fn a_build_from_source_says_it_sends_nothing() {
    let daemon = TestDaemon::start().await;

    assert_eq!(
        settings(&daemon).await["analytics"],
        serde_json::json!({ "enabled": true, "blocked": "build" })
    );
}

#[tokio::test]
async fn do_not_track_says_so() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        analytics: pagis::AnalyticsOptions {
            destination: Err(Blocked::DoNotTrack),
            ..pagis::AnalyticsOptions::off()
        },
        ..TestDaemonOptions::default()
    })
    .await;

    assert_eq!(
        settings(&daemon).await["analytics"]["blocked"],
        "do_not_track"
    );
}

/// The switch writes the config file and needs no restart. A save of
/// the other System Settings keeps it.
#[tokio::test]
async fn an_administrator_turns_analytics_off_with_no_restart() {
    let daemon = TestDaemon::start().await;

    let response = switch(&daemon, daemon.cookie(), false).await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(saved["restart_required"], false);
    assert_eq!(saved["settings"]["analytics"]["enabled"], false);
    let config = pagis::Config::read_file(&daemon.booted.home.join("config.toml")).unwrap();
    assert!(!config.analytics);

    let current = settings(&daemon).await;
    let response = client()
        .put(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "port": current["port"],
            "docker_endpoint": null,
            "log_level": "debug",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(settings(&daemon).await["analytics"]["enabled"], false);

    assert_eq!(
        switch(&daemon, daemon.cookie(), true).await.status(),
        StatusCode::OK
    );
    assert_eq!(settings(&daemon).await["analytics"]["enabled"], true);
}

/// A Member in the same Org, with their own Workspace.
async fn member(pool: &sqlx::SqlitePool) -> pagis_core::User {
    use pagis_core::{OrgStore, UserStore, WorkspaceStore, now_ms};

    let now = now_ms();
    let org = pagis_storage_sqlite::SqliteOrgStore::new(pool.clone())
        .list()
        .await
        .expect("list the orgs")
        .into_iter()
        .next()
        .expect("the seeded org");
    let person = pagis_core::User {
        email: Some("member@example.com".to_string()),
        ..pagis_core::User::new(org.id, pagis_core::UserRole::Member, now)
    };
    pagis_storage_sqlite::SqliteUserStore::new(pool.clone())
        .create(&person)
        .await
        .expect("create the person");
    let workspace = pagis_core::Workspace {
        id: pagis_core::WorkspaceId::generate(),
        user_id: person.id.clone(),
        name: "Member".to_string(),
        timezone: "UTC".to_string(),
        created_at: now,
        onboarded_at: Some(now),
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
    };
    pagis_storage_sqlite::SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .expect("create the workspace");
    person
}

#[tokio::test]
async fn a_member_cannot_switch_analytics() {
    let daemon = TestDaemon::start().await;
    let member = member(daemon.pool()).await;
    let cookie = daemon.cookie_for(&member.id).await;

    assert_eq!(
        switch(&daemon, &cookie, false).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        switch(&daemon, "", false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(settings(&daemon).await["analytics"]["enabled"], true);
}

/// A release build sends the created event and the Installation Report
/// in the background, under the Installation ID of its ledger.
#[tokio::test]
async fn a_release_build_sends_the_installation_report() {
    let posthog = posthog().await;
    let daemon = TestDaemon::start_with(sending_to(&posthog)).await;
    assert_eq!(
        settings(&daemon).await["analytics"]["blocked"],
        serde_json::Value::Null
    );

    let batch = first_batch(&posthog).await;

    assert_eq!(batch["api_key"], "phc_test");
    let events = batch["batch"].as_array().unwrap();
    let names: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["installation_created", "installation_report"]);
    let ledger = pagis_analytics::Ledger::read(&daemon.booted.home)
        .unwrap()
        .expect("the ledger");
    assert_eq!(events[1]["distinct_id"], ledger.installation_id.as_str());
    let report = &events[1]["properties"];
    assert_eq!(report["release"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["installation"], "local");
    assert_eq!(report["storage"], "sqlite");
    assert_eq!(report["remote_access"], false);
    // The test daemon finds no Docker.
    assert_eq!(report["computers"], false);
    assert_eq!(report["people"], "1");
    assert_eq!(report["agents"], "1");
    // The seeded Report Schedule is not a Schedule that somebody made.
    assert_eq!(report["uses_schedules"], false);
    assert_eq!(report["uses_plugins"], false);
    assert_eq!(report["$process_person_profile"], false);
}

/// With the setting off, a release build sends nothing. Turning it on
/// takes effect at the next check, with no restart.
#[tokio::test]
async fn with_the_setting_off_a_release_build_sends_nothing_until_it_is_on() {
    let posthog = posthog().await;
    let daemon = TestDaemon::start().await;
    switch(&daemon, daemon.cookie(), false).await;
    let daemon = daemon.restart(sending_to(&posthog)).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(batches(&posthog).await.is_empty());
    assert!(
        !daemon
            .booted
            .home
            .join(pagis_analytics::LEDGER_FILE)
            .exists()
    );

    switch(&daemon, daemon.cookie(), true).await;

    first_batch(&posthog).await;
}
