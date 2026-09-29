//! Roster CRUD tests over real HTTP: create an agent with its
//! DM, update the profile, archive, and validation.

use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn create_agent(daemon: &TestDaemon, name: &str) -> serde_json::Value {
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

async fn list(daemon: &TestDaemon, path: &str) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
        .get(format!("{}{path}", daemon.base_url))
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
async fn a_created_agent_joins_the_roster_with_its_dm() {
    let daemon = TestDaemon::start().await;

    let rex = create_agent(&daemon, "Rex").await;
    assert_eq!(rex["name"], "Rex");
    assert_eq!(rex["job"], "researcher");
    assert_eq!(rex["personality"], "curious");
    assert_eq!(rex["status"], "active");

    // Both agents appear in the roster.
    let agents = list(&daemon, "/api/v1/agents").await;
    let names: Vec<&str> = agents.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Pixie"), "roster: {names:?}");
    assert!(names.contains(&"Rex"), "roster: {names:?}");

    // The new agent has its DM: the user plus the agent.
    let channels = list(&daemon, "/api/v1/channels").await;
    let dm = channels
        .iter()
        .find(|c| c["title"] == "Rex")
        .expect("Rex DM channel");
    assert_eq!(dm["kind"], "dm");
    assert_eq!(
        dm["agent_ids"],
        serde_json::json!([rex["id"].as_str().unwrap()])
    );
}

#[tokio::test]
async fn update_rewrites_the_agent_profile() {
    let daemon = TestDaemon::start().await;
    let rex = create_agent(&daemon, "Rex").await;

    let response = client()
        .put(format!(
            "{}/api/v1/agents/{}",
            daemon.base_url,
            rex["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "name": "Rexa",
            "job": "analyst",
            "personality": "direct",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated: serde_json::Value = response.json().await.unwrap();
    assert_eq!(updated["name"], "Rexa");
    assert_eq!(updated["job"], "analyst");
    assert_eq!(updated["personality"], "direct");

    let agents = list(&daemon, "/api/v1/agents").await;
    let rexa = agents.iter().find(|a| a["id"] == rex["id"]).unwrap();
    assert_eq!(rexa["name"], "Rexa");
}

#[tokio::test]
async fn archive_lands_once_and_is_idempotent() {
    let daemon = TestDaemon::start().await;
    let rex = create_agent(&daemon, "Rex").await;
    let archive_url = format!(
        "{}/api/v1/agents/{}/archive",
        daemon.base_url,
        rex["id"].as_str().unwrap()
    );

    for _ in 0..2 {
        let response = client()
            .post(&archive_url)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let archived: serde_json::Value = response.json().await.unwrap();
        assert_eq!(archived["status"], "archived");
    }

    // The roster keeps the archived agent; history stays visible.
    let agents = list(&daemon, "/api/v1/agents").await;
    let rex_row = agents.iter().find(|a| a["id"] == rex["id"]).unwrap();
    assert_eq!(rex_row["status"], "archived");
}

#[tokio::test]
async fn an_empty_agent_name_is_rejected() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "name": "  ", "job": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_group_channel_rejects_archived_and_unknown_agents() {
    let daemon = TestDaemon::start().await;
    let rex = create_agent(&daemon, "Rex").await;
    client()
        .post(format!(
            "{}/api/v1/agents/{}/archive",
            daemon.base_url,
            rex["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    for agent_id in [rex["id"].as_str().unwrap(), "01UNKNOWNAGENT"] {
        let response = client()
            .post(format!("{}/api/v1/channels", daemon.base_url))
            .header("cookie", daemon.cookie())
            .json(&serde_json::json!({ "title": "ops", "agent_ids": [agent_id] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
async fn the_roster_keeps_each_agents_sprite_appearance() {
    let daemon = TestDaemon::start().await;
    let iris = create_agent(&daemon, "Iris").await;
    let rex = create_agent(&daemon, "Rex").await;
    let appearance = serde_json::json!({"sprite":"pixie", "preset":"lavender",
        "colors":{"body":"#123456"}, "accessories":{"satchel":false}});
    let url = format!(
        "{}/api/v1/agents/{}/appearance",
        daemon.base_url,
        iris["id"].as_str().unwrap()
    );
    let response = client()
        .put(&url)
        .header("cookie", daemon.cookie())
        .json(&appearance)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["avatar"],
        appearance
    );
    // An About edit must not overwrite the separate appearance record.
    let response = client()
        .put(format!(
            "{}/api/v1/agents/{}",
            daemon.base_url,
            iris["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"name":"Iris", "job":"Writer", "personality":"Calm"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let agents = list(&daemon, "/api/v1/agents").await;
    assert_eq!(
        agents.iter().find(|a| a["id"] == iris["id"]).unwrap()["avatar"],
        appearance
    );
    assert_eq!(
        agents.iter().find(|a| a["id"] == rex["id"]).unwrap()["avatar"]["preset"],
        "mint"
    );
    for invalid in [
        serde_json::json!({"sprite":"missing", "preset":"mint", "colors":{}, "accessories":{}}),
        serde_json::json!({"sprite":"pixie", "preset":"missing", "colors":{}, "accessories":{}}),
        serde_json::json!({"sprite":"pixie", "preset":"mint", "colors":{"body":"red"}, "accessories":{}}),
        serde_json::json!({"sprite":"pixie", "preset":"mint", "colors":{"unknown":"#123456"}, "accessories":{}}),
        serde_json::json!({"sprite":"pixie", "preset":"mint", "colors":{}, "accessories":{"unknown":true}}),
    ] {
        let response = client()
            .put(&url)
            .header("cookie", daemon.cookie())
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{invalid}"
        );
    }
    let agents = list(&daemon, "/api/v1/agents").await;
    assert_eq!(
        agents.iter().find(|a| a["id"] == iris["id"]).unwrap()["avatar"],
        appearance
    );
    let daemon = daemon
        .restart(pagis_testkit::TestDaemonOptions::default())
        .await;
    let agents = list(&daemon, "/api/v1/agents").await;
    assert_eq!(
        agents.iter().find(|a| a["id"] == iris["id"]).unwrap()["avatar"],
        appearance
    );
}

#[tokio::test]
async fn a_new_agent_introduces_itself_in_its_dm() {
    let daemon = TestDaemon::start().await;
    let agent = create_agent(&daemon, "Rex").await;
    let channels = list(&daemon, "/api/v1/channels").await;
    let dm = channels.iter().find(|item| item["title"] == "Rex").unwrap();
    let messages = list(
        &daemon,
        &format!("/api/v1/channels/{}/messages", dm["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["author_kind"], "agent");
    assert_eq!(messages[0]["author_agent_id"], agent["id"]);
    assert_eq!(messages[0]["status"], "complete");
    assert!(messages[0]["run_id"].is_null());
    assert_eq!(messages[0]["text_content"], "Hi, I'm Rex, your researcher.");
}

#[tokio::test]
async fn the_seeded_assistant_introduces_itself_once() {
    let daemon = TestDaemon::start().await;
    let path = format!("/api/v1/channels/{}/messages", daemon.dm_channel_id);
    let messages = list(&daemon, &path).await;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["text_content"],
        "Hi, I'm Pixie, your general assistant."
    );
    assert_eq!(messages[0]["author_kind"], "agent");
    assert!(messages[0]["run_id"].is_null());
    let daemon = daemon.restart(Default::default()).await;
    assert_eq!(list(&daemon, &path).await.len(), 1);
}
