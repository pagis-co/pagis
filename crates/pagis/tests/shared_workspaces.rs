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
    let trust: Value = call(
        &daemon,
        &persons[0].1,
        Method::GET,
        &format!("{prefix}/settings/trust-list"),
        Value::Null,
    )
    .await
    .json()
    .await
    .unwrap();
    let team = trust["team"].as_array().unwrap();
    assert_eq!(team.len(), 3);
    for member in team {
        assert_eq!(member["tier"], "team");
    }
    assert!(trust["own_addresses"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &daemon,
            &persons[0].1,
            Method::POST,
            &format!("{prefix}/settings/trust-list"),
            json!({"value":"sam@example.com", "tier":"owner"})
        )
        .await
        .status(),
        422
    );
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
    // Each permitted Person reads the same Request through their own queue.
    for (index, (_, cookie)) in persons.iter().enumerate() {
        for path in [
            "/api/v1/needs-you".to_string(),
            format!("{prefix}/needs-you"),
        ] {
            let queue: Value = call(&daemon, cookie, Method::GET, &path, Value::Null)
                .await
                .json()
                .await
                .unwrap();
            let found = queue["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["request_id"] == request.id.as_str());
            assert_eq!(found, index != 2, "role {index}: {path}: {queue}");
        }
    }
    let fields = json!({"name":"Aria", "job":"Support", "personality":"Clear",
        "standing_brief":"Answer support requests"});
    for (_, cookie) in &persons[1..] {
        assert_eq!(
            call(
                &daemon,
                cookie,
                Method::POST,
                &format!("{prefix}/agents"),
                fields.clone()
            )
            .await
            .status(),
            403
        );
        for (method, path, body) in [
            (
                Method::PUT,
                format!("{prefix}/agents/{agent_id}"),
                fields.clone(),
            ),
            (
                Method::POST,
                format!("{prefix}/agents/{agent_id}/archive"),
                Value::Null,
            ),
            (
                Method::PUT,
                format!("{prefix}/workspace/chief-of-staff"),
                json!({"agent_id":agent_id}),
            ),
        ] {
            assert_eq!(
                call(&daemon, cookie, method, &path, body).await.status(),
                403
            );
        }
    }
    let created = call(
        &daemon,
        &persons[0].1,
        Method::POST,
        &format!("{prefix}/agents"),
        fields.clone(),
    )
    .await;
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    let new_id = created["id"].as_str().unwrap();
    let changed = call(
        &daemon,
        &persons[0].1,
        Method::PUT,
        &format!("{prefix}/agents/{new_id}"),
        json!({"name":"Aria", "job":"Team support", "personality":"Clear",
            "standing_brief":"Follow the team rules"}),
    )
    .await;
    assert_eq!(changed.status(), 200);
    let changed: Value = changed.json().await.unwrap();
    assert_eq!(changed["job"], "Team support");
    assert_eq!(changed["standing_brief"], "Follow the team rules");
    assert_eq!(
        call(
            &daemon,
            &persons[0].1,
            Method::PUT,
            &format!("{prefix}/workspace/chief-of-staff"),
            json!({"agent_id":new_id})
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        stores
            .workspaces
            .get(&workspace_id)
            .await
            .unwrap()
            .unwrap()
            .chief_of_staff_agent_id
            .unwrap()
            .as_str(),
        new_id
    );
    assert_eq!(
        call(
            &daemon,
            &persons[0].1,
            Method::POST,
            &format!("{prefix}/agents/{new_id}/archive"),
            Value::Null
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        stores
            .agents
            .get(
                &workspace_id,
                &pagis_core::AgentId::from(new_id.to_string())
            )
            .await
            .unwrap()
            .unwrap()
            .status,
        pagis_core::AgentStatus::Archived
    );
    assert_eq!(
        stores
            .workspaces
            .get(&workspace_id)
            .await
            .unwrap()
            .unwrap()
            .chief_of_staff_agent_id,
        Some(agent_id.clone())
    );
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

#[tokio::test]
async fn shared_workspaces_refuse_personal_resources_and_own_connections() {
    let daemon = TestDaemon::start_in_memory().await;
    let shared: Value = call(
        &daemon,
        daemon.cookie(),
        Method::POST,
        "/api/v1/administration/workspaces",
        json!({"name":"Support", "manager_id":daemon.user_id}),
    )
    .await
    .json()
    .await
    .unwrap();
    let prefix = format!("/api/v1/workspaces/{}", shared["id"].as_str().unwrap());
    for (method, path, body) in [
        (Method::GET, "settings/home-exit", Value::Null),
        (
            Method::PUT,
            "settings/home-exit",
            json!({"host_id":"absent"}),
        ),
        (Method::DELETE, "settings/home-exit", Value::Null),
        (
            Method::PUT,
            "settings/keypad-code",
            json!({"code":"123456"}),
        ),
        (Method::DELETE, "settings/keypad-code", Value::Null),
        (Method::DELETE, "settings/keypad-code/failures", Value::Null),
        (
            Method::POST,
            "settings/connections",
            json!({"provider":"google", "alias":"personal", "display_name":"Personal", "fields":{}}),
        ),
    ] {
        let response = call(
            &daemon,
            daemon.cookie(),
            method,
            &format!("{prefix}/{path}"),
            body,
        )
        .await;
        assert_eq!(
            response.status(),
            409,
            "{path}: {}",
            response.text().await.unwrap()
        );
        let body: Value = response.json().await.unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Private Workspace")
        );
    }
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    for route in ["hosts/absent/sessions", "hosts/absent/exit"] {
        assert!(matches!(
            daemon
                .open_socket(&format!("{prefix}/{route}"), daemon.cookie())
                .await,
            Err(409)
        ));
    }
    let mut socket = daemon
        .open_socket(&format!("{prefix}/ws"), daemon.cookie())
        .await
        .unwrap();
    socket
        .send(Message::text(json!({"type":"auth"}).to_string()))
        .await
        .unwrap();
    socket.next().await.unwrap().unwrap();
    socket.send(Message::text(json!({"type":"register_host", "name":"Air", "platform":"macos", "capabilities":["shell"]}).to_string())).await.unwrap();
    let frame: Value =
        serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["payload"]["code"], "conflict");
    assert!(
        frame["payload"]["message"]
            .as_str()
            .unwrap()
            .contains("Private Workspace")
    );
    let (status, setup) = daemon
        .set_up_provider(
            "google",
            "oauth-client",
            json!({
                "client_id":"test-client", "client_secret":"test-secret",
            }),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    let response = call(
        &daemon, daemon.cookie(), Method::POST,
        &format!("{prefix}/settings/connections"),
        json!({"provider":"google", "alias":"support", "display_name":"Support", "shared_account":true}),
    ).await;
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    let connection: Value = response.json().await.unwrap();
    let workspace_id = pagis_core::WorkspaceId::from(shared["id"].as_str().unwrap().to_string());
    let workspace = daemon
        .stores()
        .workspaces
        .get(&workspace_id)
        .await
        .unwrap()
        .unwrap();
    let connection_id =
        pagis_core::ConnectionId::from(connection["id"].as_str().unwrap().to_string());
    let authorization = call(
        &daemon,
        daemon.cookie(),
        Method::POST,
        &format!("{prefix}/settings/connections/{connection_id}/authorize"),
        json!({"capabilities":["gmail_read"]}),
    )
    .await;
    assert_eq!(
        authorization.status(),
        200,
        "{}",
        authorization.text().await.unwrap()
    );
    let authorization: Value = authorization.json().await.unwrap();
    assert!(
        authorization["authorization_url"]
            .as_str()
            .unwrap()
            .contains("/connections/google/start?state=")
    );
    let stored = daemon
        .stores()
        .connections
        .get(&workspace_id, &connection_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.config["shared_account"], true);
    assert_eq!(stored.workspace_id, workspace_id);
    assert_eq!(connection["installation"], false);
    // Provider consent is tested by the OAuth suite. This test starts from its result.
    daemon
        .stores()
        .connections
        .set_authorization(&workspace_id, &connection_id, &["gmail_read".to_string()])
        .await
        .unwrap();
    let response = call(&daemon, daemon.cookie(), Method::POST, &format!("{prefix}/grants"),
        json!({"agent_id":workspace.chief_of_staff_agent_id.clone().unwrap(), "connection_id":connection["id"], "capabilities":["gmail_read"]}),
    ).await;
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    let agent_id = workspace.chief_of_staff_agent_id.unwrap();
    let channel = daemon
        .stores()
        .channels
        .list_by_workspace(&workspace_id)
        .await
        .unwrap()
        .remove(0);
    let run = pagis_testkit::fixture::queued_run(&workspace_id, &agent_id, &channel.id);
    daemon.stores().runs.create(&run).await.unwrap();
    let snapshot = daemon
        .state
        .broker
        .prepare_run(&workspace_id, &agent_id, &run.id)
        .await
        .unwrap();
    assert!(
        snapshot
            .tools
            .iter()
            .any(|tool| tool.name == "mail__search")
    );
    assert!(
        daemon
            .stores()
            .connections
            .list(&daemon.workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
}
