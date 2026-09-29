//! The Chief of Staff is a Workspace setting (ADR-0022): the
//! seed names the Default Assistant, the user moves the designation,
//! and archiving the Chief of Staff moves it to the next active Agent.

use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn workspace(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .get(format!("{}/api/v1/workspace", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn set_chief(daemon: &TestDaemon, agent_id: &str) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/workspace/chief-of-staff",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "agent_id": agent_id }))
        .send()
        .await
        .unwrap()
}

async fn hire(daemon: &TestDaemon, name: &str) -> serde_json::Value {
    let response = client()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "name": name,
            "job": "researcher",
            "personality": "curious",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn archive(daemon: &TestDaemon, agent_id: &str) {
    let response = client()
        .post(format!(
            "{}/api/v1/agents/{agent_id}/archive",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn roster(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
        .get(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    page["items"].as_array().unwrap().clone()
}

#[tokio::test]
async fn the_seed_names_the_default_assistant_the_chief_of_staff() {
    let daemon = TestDaemon::start().await;

    let workspace = workspace(&daemon).await;
    let pixie = roster(&daemon).await.remove(0);

    assert_eq!(workspace["name"], "Workspace");
    assert!(workspace["timezone"].is_string());
    assert_eq!(workspace["chief_of_staff_agent_id"], pixie["id"]);
}

#[tokio::test]
async fn the_user_moves_the_designation_to_another_active_agent() {
    let daemon = TestDaemon::start().await;
    let clown = hire(&daemon, "Clown").await;

    let response = set_chief(&daemon, clown["id"].as_str().unwrap()).await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(saved["chief_of_staff_agent_id"], clown["id"]);
    assert_eq!(
        workspace(&daemon).await["chief_of_staff_agent_id"],
        clown["id"]
    );
}

#[tokio::test]
async fn an_archived_agent_cannot_be_the_chief_of_staff() {
    let daemon = TestDaemon::start().await;
    let clown = hire(&daemon, "Clown").await;
    archive(&daemon, clown["id"].as_str().unwrap()).await;

    let response = set_chief(&daemon, clown["id"].as_str().unwrap()).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn an_unknown_agent_cannot_be_the_chief_of_staff() {
    let daemon = TestDaemon::start().await;

    let response = set_chief(&daemon, "agent-that-never-was").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archiving_the_chief_of_staff_moves_it_to_the_oldest_active_agent() {
    let daemon = TestDaemon::start().await;
    let pixie = roster(&daemon).await.remove(0);
    let clown = hire(&daemon, "Clown").await;
    hire(&daemon, "Nurse").await;

    archive(&daemon, pixie["id"].as_str().unwrap()).await;

    assert_eq!(
        workspace(&daemon).await["chief_of_staff_agent_id"],
        clown["id"]
    );
}

#[tokio::test]
async fn archiving_an_agent_that_is_not_the_chief_of_staff_keeps_the_designation() {
    let daemon = TestDaemon::start().await;
    let pixie = roster(&daemon).await.remove(0);
    let clown = hire(&daemon, "Clown").await;

    archive(&daemon, clown["id"].as_str().unwrap()).await;

    assert_eq!(
        workspace(&daemon).await["chief_of_staff_agent_id"],
        pixie["id"]
    );
}

#[tokio::test]
async fn a_workspace_with_no_active_agent_has_no_chief_of_staff() {
    let daemon = TestDaemon::start().await;
    let pixie = roster(&daemon).await.remove(0);

    archive(&daemon, pixie["id"].as_str().unwrap()).await;

    assert!(workspace(&daemon).await["chief_of_staff_agent_id"].is_null());
}
