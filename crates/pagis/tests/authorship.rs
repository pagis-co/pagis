//! Authorship through the Product API.

use std::sync::Arc;

use futures::StreamExt;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::{Value, json};

async fn get(daemon: &TestDaemon, path: &str) -> Value {
    daemon
        .client()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn post(daemon: &TestDaemon, path: &str, body: Value) -> Value {
    let response = daemon
        .client()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "POST {path}: {status}: {text}");
    if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap()
    }
}

#[tokio::test]
async fn messages_name_the_person_and_the_agent() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Hello Ajay."]));
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    daemon
        .stores()
        .users
        .set_name(&daemon.user_id, "Ajay", pagis_core::now_ms())
        .await
        .unwrap();
    let mut events = daemon.event_socket(daemon.cookie()).await;
    let sent = post(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
        json!({"pending_id": "authorship", "text": "Hello"}),
    )
    .await;
    assert_eq!(
        sent["author"],
        json!({"kind": "person", "id": daemon.user_id, "name": "Ajay"})
    );
    let agent = agent(&daemon).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(
                &daemon,
                &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
            )
            .await;
            let rows = page["messages"]
                .as_array()
                .or_else(|| page["items"].as_array())
                .unwrap();
            if let Some(reply) = rows.iter().find(|m| m["text_content"] == "Hello Ajay.") {
                assert_eq!(
                    reply["author"],
                    json!({"kind": "agent", "id": daemon.agent_id, "name": agent["name"]})
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let requests = brain.requests();
    assert!(
        requests[0]
            .system
            .contains("A message from Ajay starts with `Ajay:`")
    );
    assert!(
        requests[0]
            .messages
            .last()
            .unwrap()
            .text
            .starts_with("Ajay:")
    );
    let mut saw_person = false;
    let mut saw_agent = false;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !(saw_person && saw_agent) {
            let frame = events.next().await.unwrap().unwrap().into_text().unwrap();
            let frame: Value = serde_json::from_str(&frame).unwrap();
            if frame["type"] != "message.completed" {
                continue;
            }
            let author = &frame["payload"]["payload"]["author"];
            if author["kind"] == "person" {
                assert_eq!(*author, person(&daemon));
                saw_person = true;
            }
            if author["kind"] == "agent" {
                assert_eq!(
                    *author,
                    json!({"kind": "agent", "id": daemon.agent_id, "name": agent["name"]})
                );
                saw_agent = true;
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_product_schedule_names_its_person() {
    let daemon = named_daemon(Arc::new(ScriptedBrain::default())).await;
    let person = get(&daemon, "/api/v1/user").await;
    let schedule = post(&daemon, "/api/v1/schedules", json!({
        "agent_id": daemon.agent_id, "name": "Reminder", "instruction": "Check the plan",
        "channel_id": daemon.dm_channel_id, "local_time": "2099-01-01T09:00:00", "timezone": "UTC"
    })).await;
    assert_eq!(schedule["created_by"]["kind"], "person");
    assert_eq!(schedule["created_by"]["id"], json!(daemon.user_id));
    assert_eq!(
        schedule["created_by"]["name"],
        person["name"]
            .as_str()
            .map(Value::from)
            .unwrap_or(person["email"].clone())
    );
}

async fn named_daemon(brain: Arc<ScriptedBrain>) -> TestDaemon {
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain as _,
        ..TestDaemonOptions::default()
    })
    .await;
    daemon
        .stores()
        .users
        .set_name(&daemon.user_id, "Ajay", pagis_core::now_ms())
        .await
        .unwrap();
    daemon
}

async fn agent(daemon: &TestDaemon) -> Value {
    get(daemon, "/api/v1/agents").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == daemon.agent_id)
        .unwrap()
        .clone()
}

fn person(daemon: &TestDaemon) -> Value {
    json!({"kind": "person", "id": daemon.user_id, "name": "Ajay"})
}

async fn wait_for_items(daemon: &TestDaemon, path: &str) -> Vec<Value> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(daemon, path).await;
            let items = page["items"].as_array().unwrap();
            if !items.is_empty() {
                return items.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_request_decision_names_the_person() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(&[], "ask_user", json!({
        "title": "Which name?", "fields": [{"key": "name", "label": "Name", "kind": "text", "required": true}]
    })));
    brain.push(Script::reply(&["Thank you."]));
    let daemon = named_daemon(brain).await;
    post(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
        json!({"pending_id": "decision-author", "text": "Ask for my name"}),
    )
    .await;
    let requests = wait_for_items(&daemon, "/api/v1/requests").await;
    assert!(requests[0]["decided_by"].is_null());
    let path = format!(
        "/api/v1/requests/{}/decision",
        requests[0]["id"].as_str().unwrap()
    );
    post(
        &daemon,
        &path,
        json!({"decision": "approved", "values": {"name": "Ajay"}}),
    )
    .await;
    let decided = get(
        &daemon,
        &format!("/api/v1/requests/{}", requests[0]["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(decided["decided_by"], person(&daemon));
}

async fn connection(daemon: &TestDaemon) -> String {
    let id = pagis_core::ConnectionId::generate();
    daemon
        .plant_google_connection(&pagis_core::Connection {
            id: id.clone(),
            workspace_id: daemon.workspace_id.clone(),
            provider: "google".into(),
            alias: "work".into(),
            display_name: "Work".into(),
            status: pagis_core::Connection::CONNECTED.into(),
            authorized_capabilities: vec![],
            config: json!({"account": "ajay@example.com", "client": "work"}),
            created_at: pagis_core::now_ms(),
        })
        .await;
    id.to_string()
}

#[tokio::test]
async fn product_grants_credentials_and_subscriptions_name_the_person() {
    let daemon = named_daemon(Arc::new(ScriptedBrain::default())).await;
    let connection_id = connection(&daemon).await;
    let grant = post(&daemon, "/api/v1/grants", json!({"agent_id": daemon.agent_id, "connection_id": connection_id, "capabilities": ["gmail_read"]})).await;
    assert_eq!(grant["created_by"], person(&daemon));
    let credential = post(&daemon, "/api/v1/settings/credentials", json!({
        "domain": "example.com", "username": "ajay@example.com", "login_url": "https://example.com/login", "secret": "test secret"
    })).await;
    assert_eq!(credential["created_by"], person(&daemon));
    let subscription = post(&daemon, "/api/v1/event-subscriptions", json!({
        "agent_id": daemon.agent_id, "connection_id": connection_id, "event_kind": "mail.message_received",
        "name": "Work mail", "instruction": "Read new mail", "channel_id": daemon.dm_channel_id, "filter": {"mailbox": "work"}
    })).await;
    assert_eq!(subscription["created_by"], person(&daemon));
    for path in [
        "/api/v1/grants",
        "/api/v1/settings/credentials",
        "/api/v1/event-subscriptions",
    ] {
        assert_eq!(
            get(&daemon, path).await["items"][0]["created_by"],
            person(&daemon)
        );
    }
}

#[tokio::test]
async fn an_agent_schedule_names_the_agent() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(&[], "schedule_create", json!({
        "name": "Reminder", "instruction": "Check the plan", "channel": "user", "kind": "one_shot", "local_time": "2099-01-01T09:00:00", "timezone": "UTC"
    })));
    brain.push(Script::reply(&["Scheduled."]));
    let daemon = named_daemon(brain).await;
    post(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
        json!({"pending_id": "agent-author", "text": "Make the Reminder Schedule"}),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(&daemon, "/api/v1/schedules").await;
            if let Some(schedule) = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == "Reminder")
            {
                let agent = agent(&daemon).await;
                assert_eq!(
                    schedule["created_by"],
                    json!({"kind": "agent", "id": daemon.agent_id, "name": agent["name"]})
                );
                break;
            }
            approve_pending(&daemon).await;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_agent_subscription_names_the_agent() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "event_subscription_create",
        json!({
            "name": "Agent mail rule", "instruction": "Read new mail", "connection": "work",
            "event_kind": "mail.message_received", "channel": "user", "filter": {"mailbox": "work"}
        }),
    ));
    brain.push(Script::reply(&["The rule is ready."]));
    let daemon = named_daemon(brain).await;
    let connection_id = connection(&daemon).await;
    post(&daemon, "/api/v1/grants", json!({"agent_id": daemon.agent_id, "connection_id": connection_id, "capabilities": ["gmail_read"]})).await;
    post(&daemon, &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id), json!({"pending_id": "agent-subscription-author", "text": "Make an Event Subscription for new mail"})).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page = get(&daemon, "/api/v1/event-subscriptions").await;
            if let Some(subscription) = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == "Agent mail rule")
            {
                let agent = agent(&daemon).await;
                assert_eq!(
                    subscription["created_by"],
                    json!({"kind": "agent", "id": daemon.agent_id, "name": agent["name"]})
                );
                break;
            }
            approve_pending(&daemon).await;
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn approve_pending(daemon: &TestDaemon) {
    let page = get(daemon, "/api/v1/requests").await;
    for request in page["items"].as_array().unwrap() {
        post(
            daemon,
            &format!(
                "/api/v1/requests/{}/decision",
                request["id"].as_str().unwrap()
            ),
            json!({"decision": "approved"}),
        )
        .await;
    }
}
