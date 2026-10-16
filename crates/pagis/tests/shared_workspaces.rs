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
async fn shared_workspaces_check_roles_and_keep_private_routes() {
    let daemon = TestDaemon::start_in_memory().await;
    let mut persons = Vec::new();
    for name in ["Manager", "Responder", "Reader"] {
        let response = call(
            &daemon,
            daemon.cookie(),
            Method::POST,
            "/api/v1/administration/people",
            json!({
                "name": name, "email": format!("{}@example.com", name.to_lowercase()),
                "password": "a-long-test-password", "role": "member"
            }),
        )
        .await;
        assert!(
            response.status().is_success(),
            "create Person: {}",
            response.text().await.unwrap()
        );
        let person: Value = response.json().await.unwrap();
        let id = person["person"]["id"].as_str().unwrap().to_string();
        let cookie = daemon
            .cookie_for(&pagis_core::UserId::from(id.clone()))
            .await;
        persons.push((id, cookie));
    }
    let response = call(
        &daemon,
        daemon.cookie(),
        Method::POST,
        "/api/v1/administration/workspaces",
        json!({"name":"Support", "manager_id": persons[0].0}),
    )
    .await;
    assert!(
        response.status().is_success(),
        "create Shared Workspace: {}",
        response.text().await.unwrap()
    );
    let shared: Value = response.json().await.unwrap();
    let id = shared["id"].as_str().unwrap();
    let prefix = format!("/api/v1/workspaces/{id}");
    assert_eq!(
        call(
            &daemon,
            &persons[0].1,
            Method::POST,
            &format!("{prefix}/system/restart"),
            Value::Null,
        )
        .await
        .status(),
        404
    );
    for (method, path) in [(Method::GET, "health"), (Method::POST, "sessions")] {
        assert_eq!(
            call(
                &daemon,
                &persons[0].1,
                method,
                &format!("{prefix}/{path}"),
                json!({})
            )
            .await
            .status(),
            404
        );
    }
    for (index, role) in [(1, "responder"), (2, "reader")] {
        let response = call(
            &daemon,
            &persons[0].1,
            Method::POST,
            &format!("{prefix}/members"),
            json!({"person_id":persons[index].0,"role":role}),
        )
        .await;
        assert!(response.status().is_success());
    }
    for (person_id, cookie) in &persons {
        let response = call(
            &daemon,
            cookie,
            Method::GET,
            &format!("{prefix}/channels"),
            Value::Null,
        )
        .await;
        assert_eq!(response.status(), 200);
        let private: Value = call(
            &daemon,
            cookie,
            Method::GET,
            "/api/v1/workspace",
            Value::Null,
        )
        .await
        .json()
        .await
        .unwrap();
        let expected = daemon
            .stores()
            .workspaces
            .for_user(&pagis_core::UserId::from(person_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(private["id"], json!(expected.id));
        assert_ne!(private["id"], shared["id"]);
    }
    for route in pagis_server::ROUTES.iter().filter(|route| route.shared) {
        let path = if route.path.starts_with("/api/v1/workspaces/") {
            route.path.replace("{workspace_id}", id)
        } else {
            format!("{prefix}{}", route.path.strip_prefix("/api/v1").unwrap())
        };
        let path = path
            .split('/')
            .map(|part| {
                if part.starts_with('{') {
                    "absent"
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for method in route.methods {
            let response = call(
                &daemon,
                daemon.cookie(),
                Method::from_bytes(method.to_uppercase().as_bytes()).unwrap(),
                &path,
                json!({}),
            )
            .await;
            assert_eq!(response.status(), 404, "non-member {method} {path}");
        }
    }
    let workspace_id = pagis_core::WorkspaceId::from(id.to_string());
    let stores = daemon.stores();
    let workspace = stores.workspaces.get(&workspace_id).await.unwrap().unwrap();
    let agent_id = workspace.chief_of_staff_agent_id.clone().unwrap();
    let channel = stores
        .channels
        .list_by_workspace(&workspace_id)
        .await
        .unwrap()
        .remove(0);
    let run = pagis_testkit::fixture::queued_run(&workspace_id, &agent_id, &channel.id);
    stores.runs.create(&run).await.unwrap();
    let request =
        pagis_testkit::fixture::pending_request(&workspace_id, &agent_id, &run.id, "echo hello");
    stores.requests.create(&request).await.unwrap();
    let send_path = format!("{prefix}/channels/{}/messages", channel.id);
    let decide_path = format!("{prefix}/requests/{}/decision", request.id);
    for (path, body) in [
        (
            &send_path,
            json!({"pending_id":"reader-message", "text":"Hello"}),
        ),
        (&decide_path, json!({"decision":"denied"})),
    ] {
        let response = call(&daemon, &persons[2].1, Method::POST, path, body).await;
        assert_eq!(response.status(), 403);
    }
    let sent = call(
        &daemon,
        &persons[1].1,
        Method::POST,
        &send_path,
        json!({"pending_id":"responder-message", "text":"Hello"}),
    )
    .await;
    assert_eq!(sent.status(), 201, "{}", sent.text().await.unwrap());
    let decided = call(
        &daemon,
        &persons[1].1,
        Method::POST,
        &decide_path,
        json!({"decision":"denied"}),
    )
    .await;
    assert_eq!(decided.status(), 200, "{}", decided.text().await.unwrap());
    let settings = call(
        &daemon,
        &persons[0].1,
        Method::PUT,
        &format!("{prefix}/workspace/timezone"),
        json!({"timezone":"Europe/Paris"}),
    )
    .await;
    assert_eq!(settings.status(), 200, "{}", settings.text().await.unwrap());
    let response = call(
        &daemon,
        &persons[0].1,
        Method::PATCH,
        &format!("{prefix}/members/{}", persons[2].0),
        json!({"role":"responder"}),
    )
    .await;
    assert_eq!(response.status(), 204);
    let response = call(
        &daemon,
        &persons[0].1,
        Method::DELETE,
        &format!("{prefix}/members/{}", persons[2].0),
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), 204);
    let response = call(
        &daemon,
        &persons[2].1,
        Method::GET,
        &format!("{prefix}/channels"),
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), 404);

    // The global due-Schedule sweep includes the Shared Workspace seed.
    let now = pagis_core::now_ms();
    sqlx::query("UPDATE schedules SET next_due_at = ? WHERE id = ?")
        .bind(now)
        .bind(workspace.report_schedule_id.unwrap().as_str())
        .execute(daemon.pool())
        .await
        .unwrap();
    daemon.state.trigger.process_due(now).await.unwrap();
    let wakeups: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeups WHERE workspace_id = ?")
        .bind(workspace_id.as_str())
        .fetch_one(daemon.pool())
        .await
        .unwrap();
    assert!(wakeups > 0);
    let response = call(
        &daemon,
        &persons[0].1,
        Method::DELETE,
        &format!("{prefix}/members/{}", persons[0].0),
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), 409);
}
