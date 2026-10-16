//! A normal Session enters each Shared Workspace through its role.
use pagis_testkit::TestDaemon;
use reqwest::Method;
use serde_json::{Value, json};

async fn call(
    daemon: &TestDaemon,
    cookie: &str,
    method: Method,
    path: &str,
    body: Value,
) -> reqwest::Response {
    let base = if path.starts_with("/api/v1/administration/") {
        &daemon.administration_base_url
    } else {
        &daemon.base_url
    };
    daemon
        .client()
        .request(method, format!("{base}{path}"))
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn search_across_workspaces_keeps_private_content_private() {
    let daemon = TestDaemon::start_in_memory().await;
    let person: Value = call(&daemon, daemon.cookie(), Method::POST, "/api/v1/administration/people", json!({
        "name":"Reader", "email":"search-reader@example.com", "password":"a-long-test-password", "role":"member"
    })).await.json().await.unwrap();
    let person_id = person["person"]["id"].as_str().unwrap();
    let cookie = daemon
        .cookie_for(&pagis_core::UserId::from(person_id.to_string()))
        .await;
    let org = daemon.stores().orgs.list().await.unwrap().remove(0);
    let owner = daemon.stores().users.list_by_org(&org.id).await.unwrap();
    let manager = owner.iter().find(|p| p.id.as_str() != person_id).unwrap();
    let shared: Value = call(
        &daemon,
        daemon.cookie(),
        Method::POST,
        "/api/v1/administration/workspaces",
        json!({"name":"Support", "manager_id":manager.id}),
    )
    .await
    .json()
    .await
    .unwrap();
    let id = shared["id"].as_str().unwrap();
    let members = format!("/api/v1/workspaces/{id}/members");
    assert_eq!(
        call(
            &daemon,
            daemon.cookie(),
            Method::POST,
            &members,
            json!({"person_id":person_id,"role":"reader"})
        )
        .await
        .status(),
        204
    );
    for (prefix, title) in [
        ("/api/v1".to_string(), "Launch secret"),
        (format!("/api/v1/workspaces/{id}"), "Launch review"),
    ] {
        assert_eq!(call(&daemon, daemon.cookie(), Method::POST, &format!("{prefix}/calendar/events"), json!({
            "title":title,"description":"Plan","location":"Office","start":"2026-03-08T09:00:00","end":"2026-03-08T10:00:00",
            "timezone":"UTC","all_day":false,"rrule":null,"exdates":[],"attendees":[],"reminders":[],"meeting_url":null
        })).await.status(), 201);
    }
    let found: Value = call(
        &daemon,
        &cookie,
        Method::GET,
        "/api/v1/search?q=launch",
        Value::Null,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["items"][0]["workspace"]["name"], "Support");
    assert_eq!(found["items"][0]["title"], "Launch review");
    assert_eq!(found["incomplete"], json!([]));
    assert_eq!(
        call(
            &daemon,
            daemon.cookie(),
            Method::DELETE,
            &format!("{members}/{person_id}"),
            Value::Null
        )
        .await
        .status(),
        204
    );
    let found: Value = call(
        &daemon,
        &cookie,
        Method::GET,
        "/api/v1/search?q=launch",
        Value::Null,
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(found["items"], json!([]));
}
