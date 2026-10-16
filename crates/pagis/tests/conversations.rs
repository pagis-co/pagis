//! Persons share one conversation through their Private Workspaces.
use futures::{SinkExt, StreamExt};
use pagis_core::{ChannelId, User, UserRole, Workspace, WorkspaceId};
use pagis_testkit::TestDaemon;
use serde_json::{Value, json};

async fn person(daemon: &TestDaemon, name: &str) -> (WorkspaceId, String) {
    let stores = daemon.stores();
    let owner = stores.users.get(&daemon.user_id).await.unwrap().unwrap();
    let user = User {
        name: Some(name.into()),
        ..User::new(owner.org_id, UserRole::Member, pagis_core::now_ms())
    };
    stores.users.create(&user).await.unwrap();
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: user.id.clone(),
        name: name.into(),
        timezone: "UTC".into(),
        monthly_spend_cap_usd: None,
        created_at: pagis_core::now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    stores.workspaces.create(&workspace).await.unwrap();
    (workspace.id, daemon.cookie_for(&user.id).await)
}

async fn request(
    daemon: &TestDaemon,
    cookie: &str,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> reqwest::Response {
    daemon
        .client()
        .request(method, format!("{}{path}", daemon.base_url))
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn get(daemon: &TestDaemon, cookie: &str, path: &str) -> Value {
    request(daemon, cookie, reqwest::Method::GET, path, Value::Null)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn post(daemon: &TestDaemon, cookie: &str, path: &str, body: Value) -> Value {
    request(daemon, cookie, reqwest::Method::POST, path, body)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn conversations_share_messages_authorship_presence_and_live_delivery() {
    let daemon = TestDaemon::start_in_memory().await;
    daemon
        .stores()
        .users
        .set_name(&daemon.user_id, "Alice", pagis_core::now_ms())
        .await
        .unwrap();
    let (b, cookie_b) = person(&daemon, "Bob").await;
    let (_, cookie_c) = person(&daemon, "Carol").await;
    let colleagues = get(&daemon, daemon.cookie(), "/api/v1/colleagues").await;
    let bob = colleagues["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Bob")
        .unwrap();
    assert_eq!(bob["workspace_id"], json!(b));
    assert_eq!(bob["presence"], "away");
    let mut live = daemon.event_socket(&cookie_b).await;
    live.send(tokio_tungstenite::tungstenite::Message::text(
        json!({"type":"activity"}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let colleagues = get(&daemon, daemon.cookie(), "/api/v1/colleagues").await;
            if colleagues["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "Bob" && p["presence"] == "active")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let body = json!({"workspace_ids": [b]});
    let conversation = post(
        &daemon,
        daemon.cookie(),
        "/api/v1/conversations",
        body.clone(),
    )
    .await;
    let again = post(
        &daemon,
        &cookie_b,
        "/api/v1/conversations",
        json!({"workspace_ids": [daemon.workspace_id]}),
    )
    .await;
    assert_eq!(conversation["id"], again["id"]);
    let id = conversation["id"].as_str().unwrap();
    let path = format!("/api/v1/conversations/{id}/messages");
    let sent = post(
        &daemon,
        daemon.cookie(),
        &path,
        json!({"text":"Hello Bob", "pending_id":"alice-1"}),
    )
    .await;
    assert_eq!(sent["author"]["name"], "Alice");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let frame: Value =
                serde_json::from_str(&live.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            if frame["type"] == "conversation.message" {
                assert_eq!(frame["payload"]["payload"]["message_id"], sent["id"]);
                assert!(frame["payload"]["payload"]["message"].is_null());
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        get(&daemon, &cookie_b, &path).await["items"][0]["author"]["name"],
        "Alice"
    );
    post(
        &daemon,
        &cookie_b,
        &path,
        json!({"text":"Hello Alice", "pending_id":"bob-1"}),
    )
    .await;
    let page = get(&daemon, daemon.cookie(), &path).await;
    assert_eq!(page["items"][0]["author"]["name"], "Bob");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        assert_eq!(
            request(
                &daemon,
                &cookie_c,
                method,
                &path,
                json!({"text":"No", "pending_id":"c"})
            )
            .await
            .status(),
            404
        );
    }
    assert!(
        get(&daemon, &cookie_c, "/api/v1/conversations").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let owner = daemon
        .stores()
        .orgs
        .list()
        .await
        .unwrap()
        .pop()
        .unwrap()
        .workspace_id;
    let rows = daemon
        .stores()
        .messages
        .list_top_level(&owner, &ChannelId::from(id.to_string()), None, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for workspace in [&daemon.workspace_id, &b] {
        assert!(
            daemon
                .stores()
                .messages
                .list_top_level(workspace, &ChannelId::from(id.to_string()), None, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
