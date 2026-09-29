//! A Person's clock. The Workspace timezone is the Person's own: their
//! browser or Client App reports it at their first sign-in, and they
//! change it in their Settings. The Daily report follows it, so its
//! morning is the Person's morning and not the server's.
//!
//! Proactive work needs a brain that can think: a Schedule that comes
//! due while no model key exists waits for one.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Timelike};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

const PASSWORD: &str = "correct horse battery";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Sign in with a password, reporting a timezone as the Product App
/// does, and answer the Session cookie.
async fn sign_in(daemon: &TestDaemon, email: &str, timezone: &str) -> String {
    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": email, "password": PASSWORD, "timezone": timezone }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("pagis_session="))
        .and_then(|value| value.split(';').next())
        .expect("the sign-in sets the cookie")
        .to_string()
}

async fn get(daemon: &TestDaemon, cookie: &str, path: &str) -> serde_json::Value {
    let response = client()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    response.json().await.unwrap()
}

/// The Person's Workspace timezone and their Daily report.
async fn clock_of(daemon: &TestDaemon, cookie: &str) -> (String, serde_json::Value) {
    let workspace = get(daemon, cookie, "/api/v1/workspace").await;
    let report_id = workspace["report_schedule_id"].as_str().expect("a Report");
    let report = get(daemon, cookie, &format!("/api/v1/schedules/{report_id}")).await;
    (workspace["timezone"].as_str().unwrap().to_string(), report)
}

/// The local hour at which a Schedule is next due, in `timezone`.
fn local_hour(report: &serde_json::Value, timezone: &str) -> u32 {
    let zone: chrono_tz::Tz = timezone.parse().unwrap();
    let due = report["next_due_at"].as_i64().expect("a next occurrence");
    zone.timestamp_millis_opt(due).unwrap().hour()
}

/// A server whose clock is UTC, with a first Administrator in Tokyo and
/// a Member in Los Angeles. Each Person's Daily report comes due at 7:00
/// on their own clock. A later sign-in from another zone changes
/// nothing, and the Person's Settings move it.
#[tokio::test]
async fn each_person_on_a_server_gets_the_daily_report_at_seven_on_their_own_clock() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions::default()).await
    else {
        return;
    };
    let setup = client()
        .post(format!("{}/api/v1/setup", daemon.administration_base_url))
        .json(&serde_json::json!({
            "email": "ada@example.com",
            "password": PASSWORD,
            "timezone": "Asia/Tokyo",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status(), StatusCode::OK);
    let (timezone, report) = clock_of(&daemon, daemon.cookie()).await;
    assert_eq!(timezone, "Asia/Tokyo");
    assert_eq!(report["timezone"], "Asia/Tokyo");
    assert_eq!(local_hour(&report, "Asia/Tokyo"), 7);

    let created = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(
            &serde_json::json!({ "email": "lin@example.com", "name": "Lin", "password": PASSWORD }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let lin = sign_in(&daemon, "lin@example.com", "America/Los_Angeles").await;
    let (timezone, report) = clock_of(&daemon, &lin).await;
    assert_eq!(timezone, "America/Los_Angeles");
    assert_eq!(report["timezone"], "America/Los_Angeles");
    assert_eq!(local_hour(&report, "America/Los_Angeles"), 7);

    // A second sign-in, from a laptop in another zone, is not the first.
    let lin = sign_in(&daemon, "lin@example.com", "Europe/Berlin").await;
    assert_eq!(clock_of(&daemon, &lin).await.0, "America/Los_Angeles");

    // The Person's Settings move the Report with the Workspace.
    let moved = client()
        .put(format!("{}/api/v1/workspace/timezone", daemon.base_url))
        .header("cookie", &lin)
        .json(&serde_json::json!({ "timezone": "Europe/Berlin" }))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    let (timezone, report) = clock_of(&daemon, &lin).await;
    assert_eq!(timezone, "Europe/Berlin");
    assert_eq!(report["timezone"], "Europe/Berlin");
    assert_eq!(local_hour(&report, "Europe/Berlin"), 7);

    let refused = client()
        .put(format!("{}/api/v1/workspace/timezone", daemon.base_url))
        .header("cookie", &lin)
        .json(&serde_json::json!({ "timezone": "Mars/Olympus" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // Ada's clock is hers alone.
    assert_eq!(clock_of(&daemon, daemon.cookie()).await.0, "Asia/Tokyo");
}

/// A Report whose Agent is archived does not wake again, so it has no
/// morning to move. The Person's timezone change still lands, and the
/// Report stays as it was.
///
/// The Agent is archived in the store and no event wakes the
/// scheduler, so no due pass has blocked the Report yet. A timezone
/// change can meet this state in the moment after an archive.
#[tokio::test]
async fn a_report_of_an_archived_agent_does_not_stop_a_timezone_change() {
    let daemon = TestDaemon::start().await;
    let (_, before) = clock_of(&daemon, daemon.cookie()).await;
    let agents = &daemon.stores().agents;
    let mut agent = agents
        .get(
            &daemon.workspace_id,
            &pagis_core::AgentId::from(
                before["agent_id"]
                    .as_str()
                    .expect("the Report's Agent")
                    .to_string(),
            ),
        )
        .await
        .unwrap()
        .expect("the Report's Agent");
    agent.status = pagis_core::AgentStatus::Archived;
    agents.update(&agent).await.unwrap();

    let moved = client()
        .put(format!("{}/api/v1/workspace/timezone", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "timezone": "Europe/Berlin" }))
        .send()
        .await
        .unwrap();

    assert_eq!(moved.status(), StatusCode::OK);
    let (timezone, report) = clock_of(&daemon, daemon.cookie()).await;
    assert_eq!(timezone, "Europe/Berlin");
    assert_eq!(report["revision"], before["revision"]);
    assert_eq!(report["timezone"], before["timezone"]);
}

/// A Schedule that comes due while no provider holds a key starts no
/// Run, so nothing fails and nothing needs the Person. It waits, and
/// runs once an Administrator stores a key.
#[tokio::test]
async fn a_schedule_that_finds_no_model_key_waits_for_one() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "gpt-newest", "created": 2 }],
            "has_more": false,
        })))
        .mount(&provider)
        .await;
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Good morning.\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-openai"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&provider)
        .await;
    let keys = pagis_testkit::test_provider_keys(Vec::new());
    let models = Arc::new(
        pagis_agent::ModelCatalog::new(Arc::clone(&keys))
            .with_base_url(pagis_core::Provider::OpenAi, provider.uri()),
    );
    let brain = Arc::new(
        pagis_agent::RouterBrain::new(Arc::clone(&keys), models)
            .with_base_url(pagis_core::Provider::OpenAi, provider.uri()),
    );
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain,
        keys,
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    let due = chrono::Utc::now() + chrono::Duration::seconds(2);
    let created = client()
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Morning plan",
            "instruction": "Prepare today's plan",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": null,
            "local_time": due.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "timezone": "UTC",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    tokio::time::sleep(Duration::from_secs(4)).await;
    let runs = get(&daemon, daemon.cookie(), "/api/v1/runs").await;
    assert!(
        !runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["trigger_kind"] == "schedule"),
        "a Run started with no model key: {runs}"
    );

    let (status, stored) = daemon
        .set_up_provider(
            "openai",
            "key",
            serde_json::json!({ "api_key": "sk-openai" }),
        )
        .await;
    assert_eq!(status, 200, "{stored}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let runs = get(&daemon, daemon.cookie(), "/api/v1/runs").await;
        let states: Vec<String> = runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == "schedule")
            .map(|run| run["state"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(!states.iter().any(|state| state == "failed"), "{runs}");
        if states.iter().any(|state| state == "completed") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Schedule did not run after the key: {runs}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
