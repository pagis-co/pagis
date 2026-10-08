//! Full-daemon channels/messages REST tests over real HTTP.

use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn create_channel(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "title": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn send(daemon: &TestDaemon, channel_id: &str, body: serde_json::Value) -> reqwest::Response {
    client()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_request_without_a_session_is_rejected_and_a_session_is_accepted() {
    let daemon = TestDaemon::start().await;

    let no_token = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .json(&serde_json::json!({ "title": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = no_token.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");

    let bad_token = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .bearer_auth("wrong")
        .json(&serde_json::json!({ "title": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token.status(), StatusCode::UNAUTHORIZED);

    create_channel(&daemon).await; // a valid Session is accepted
}

#[tokio::test]
async fn channel_list_returns_created_channels() {
    let daemon = TestDaemon::start().await;

    // Boot seeds the assistant's DM channel.
    let seeded: serde_json::Value = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = seeded["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], daemon.dm_channel_id);
    assert_eq!(items[0]["title"], "Pixie");

    let channel = create_channel(&daemon).await;

    let text = "A long readable preview ".repeat(10);
    let response = send(
        &daemon,
        channel["id"].as_str().unwrap(),
        serde_json::json!({ "pending_id": "preview", "text": text }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let listed: serde_json::Value = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], channel["id"]);
    assert_eq!(items[0]["title"], "general");

    assert_eq!(
        items[0]["last_message"]["text_content"],
        text.chars().take(140).collect::<String>()
    );
    assert_eq!(items[0]["last_message"]["author_kind"], "user");
    assert!(items[0]["last_message"]["created_at"].as_i64().unwrap() > 0);

    let no_token = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn send_and_read_back_through_the_timeline() {
    let daemon = TestDaemon::start().await;
    let channel = create_channel(&daemon).await;
    let channel_id = channel["id"].as_str().unwrap();

    let response = send(
        &daemon,
        channel_id,
        serde_json::json!({ "pending_id": "p1", "text": "hello" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let message: serde_json::Value = response.json().await.unwrap();
    assert_eq!(message["author_kind"], "user");
    assert_eq!(message["status"], "complete");
    assert_eq!(message["text_content"], "hello");
    assert_eq!(message["blocks"][0]["type"], "markdown");

    let timeline: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = timeline["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "message");
    assert_eq!(items[0]["id"], message["id"]);
}

#[tokio::test]
async fn timeline_pages_newest_first_with_cursor() {
    let daemon = TestDaemon::start().await;
    let channel = create_channel(&daemon).await;
    let channel_id = channel["id"].as_str().unwrap();

    for i in 0..3 {
        let response = send(
            &daemon,
            channel_id,
            serde_json::json!({ "pending_id": format!("p{i}"), "text": format!("m{i}") }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let first_page: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/messages?limit=2",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = first_page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["text_content"], "m2");
    assert_eq!(items[1]["text_content"], "m1");

    let cursor = items[1]["id"].as_str().unwrap();
    let second_page: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/messages?before={cursor}",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = second_page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text_content"], "m0");
}

#[tokio::test]
async fn duplicate_pending_id_returns_the_original_message() {
    let daemon = TestDaemon::start().await;
    let channel = create_channel(&daemon).await;
    let channel_id = channel["id"].as_str().unwrap();

    let first = send(
        &daemon,
        channel_id,
        serde_json::json!({ "pending_id": "p1", "text": "hello" }),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let original: serde_json::Value = first.json().await.unwrap();

    let retry = send(
        &daemon,
        channel_id,
        serde_json::json!({ "pending_id": "p1", "text": "hello resent" }),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::OK);
    let deduplicated: serde_json::Value = retry.json().await.unwrap();
    assert_eq!(deduplicated, original);
}

#[tokio::test]
async fn send_validation_and_unknown_channel() {
    let daemon = TestDaemon::start().await;
    let channel = create_channel(&daemon).await;
    let channel_id = channel["id"].as_str().unwrap();

    let empty_text = send(
        &daemon,
        channel_id,
        serde_json::json!({ "pending_id": "p1", "text": "  " }),
    )
    .await;
    assert_eq!(empty_text.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = empty_text.json().await.unwrap();
    assert_eq!(body["error"]["code"], "validation");

    let unknown = send(
        &daemon,
        "01UNKNOWNCHANNEL",
        serde_json::json!({ "pending_id": "p1", "text": "hello" }),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn replies_are_one_level_deep() {
    let daemon = TestDaemon::start().await;
    let channel = create_channel(&daemon).await;
    let channel_id = channel["id"].as_str().unwrap();

    let root: serde_json::Value = send(
        &daemon,
        channel_id,
        serde_json::json!({ "pending_id": "root", "text": "root" }),
    )
    .await
    .json()
    .await
    .unwrap();

    let reply_response = send(
        &daemon,
        channel_id,
        serde_json::json!({
            "pending_id": "reply",
            "text": "reply",
            "parent_message_id": root["id"],
        }),
    )
    .await;
    assert_eq!(reply_response.status(), StatusCode::CREATED);
    let reply: serde_json::Value = reply_response.json().await.unwrap();

    let nested = send(
        &daemon,
        channel_id,
        serde_json::json!({
            "pending_id": "nested",
            "text": "nested",
            "parent_message_id": reply["id"],
        }),
    )
    .await;
    assert_eq!(nested.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // The reply stays out of the top-level timeline.
    let timeline: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(timeline["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn sourced_chat_rereads_hide_revoked_text_in_every_projection() {
    use pagis_core::{
        AgentId, AgentStore, AuthorKind, ConnectionStore, GrantStore, MemoryExposure, MessageStore,
        now_ms,
    };
    use pagis_storage_sqlite::{
        SqliteAgentStore, SqliteConnectionStore, SqliteGrantStore, SqliteMessageStore,
    };
    use pagis_testkit::fixture;
    let daemon = TestDaemon::start().await;
    let pool = daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let channel = create_channel(&daemon).await;
    let channel_id = pagis_core::ChannelId::from(channel["id"].as_str().unwrap().to_owned());
    let connection = pagis_core::Connection {
        id: pagis_core::ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "source".into(),
        display_name: "Source".into(),
        status: "connected".into(),
        auth_mode: "byo".into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let grants = SqliteGrantStore::new(pool.clone());
    let grant = pagis_core::Grant {
        id: pagis_core::GrantId::generate(),
        workspace_id: agent.workspace_id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: pagis_core::Grant::CONNECTION_KIND.into(),
        resource_id: Some(connection.id.to_string()),
        scope: pagis_core::Grant::connection_scope(&["gmail_read".into()]),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    let exposures = vec![MemoryExposure {
        grant_id: grant.id.clone(),
        revision: grant.revision,
    }];
    let messages = SqliteMessageStore::new(pool);
    let root = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "source secret root",
    );
    let mut reply = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "source secret reply",
    );
    reply.parent_message_id = Some(root.id.clone());
    for message in [&root, &reply] {
        messages.insert(message).await.unwrap();
        messages
            .set_exposures(&message.workspace_id, &message.id, &exposures)
            .await
            .unwrap();
    }
    let mut owner = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "owner keeps these words",
    );
    owner.author_kind = AuthorKind::User;
    owner.author_agent_id = None;
    messages.insert(&owner).await.unwrap();
    let ordinary = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "ordinary greeting",
    );
    messages.insert(&ordinary).await.unwrap();
    messages
        .set_exposures(&ordinary.workspace_id, &ordinary.id, &[])
        .await
        .unwrap();
    let source_free = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "known source-free reply",
    );
    messages.insert(&source_free).await.unwrap();
    messages
        .set_exposures(&source_free.workspace_id, &source_free.id, &[])
        .await
        .unwrap();
    let unknown = fixture::agent_message(
        &agent.workspace_id,
        &channel_id,
        &agent.id,
        "unknown secret",
    );
    messages.insert(&unknown).await.unwrap();
    let mut run = fixture::queued_run(&agent.workspace_id, &agent.id, &channel_id);
    run.title = pagis_core::run_title(pagis_core::RunTitleSource::Message(&root.text_content));
    run.trigger_ref = Some(root.id.to_string());
    pagis_core::RunStore::create(
        &pagis_storage_sqlite::SqliteRunStore::new(daemon.pool().clone()),
        &run,
    )
    .await
    .unwrap();
    let paths = [
        format!("/api/v1/runs/{}/events", run.id),
        format!("/api/v1/channels/{channel_id}/messages"),
        format!("/api/v1/channels/{channel_id}/messages/{}", root.id),
        format!("/api/v1/channels/{channel_id}/threads/{}", root.id),
        format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
    ];
    for path in &paths {
        let response = client()
            .get(format!("{}{path}", daemon.base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.text().await.unwrap().contains("source secret"),
            "permitted {path}"
        );
    }
    grants
        .revoke(&grant.workspace_id, &grant.id, now_ms())
        .await
        .unwrap();
    let previews: String = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!previews.contains("source secret"));
    assert!(!previews.contains("unknown secret"));
    let runs: String = client()
        .get(format!("{}/api/v1/runs", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!runs.contains("source secret"));
    assert!(runs.contains("This message is unavailable"));
    for path in &paths {
        let response = client()
            .get(format!("{}{path}", daemon.base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.text().await.unwrap();
        assert!(
            !body.contains("source secret"),
            "revoked source leaked through {path}: {body}"
        );
        assert!(
            !body.contains("unknown secret"),
            "unknown source leaked through {path}"
        );
        assert!(
            body.contains("This message is unavailable"),
            "missing unavailable state for {path}"
        );
        if path == &paths[1] {
            assert!(body.contains("owner keeps these words"));
            assert!(body.contains("ordinary greeting"));
            assert!(body.contains("known source-free reply"));
        }
    }
}
