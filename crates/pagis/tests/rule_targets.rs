//! The target of a Schedule or an Event Subscription is in the rule's
//! own Workspace (ADR-0006).
//!
//! Person A drives the REST routes with the ids of person B's Agent,
//! Channel and Thread root. Each call answers 404 with the same body as
//! an id that names nothing, so that A cannot find B's ids, and the
//! stored rule does not change. An edit can also move a rule to another
//! Channel, which drops its Thread, and an edit can clear the Thread.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{Connection, ConnectionId, WorkspaceId, now_ms};
use pagis_google::{GogCommand, GogRunner, ProcessFailure, ProcessOutput};
use pagis_testkit::{TestDaemon, TestDaemonOptions, TwoTenants, fixture};
use serde_json::{Value, json};

/// A `gog` with an empty mailbox. The collector of an Event
/// Subscription that a test makes reads it, so no test starts the real
/// binary.
struct EmptyMailbox;

#[async_trait]
impl GogRunner for EmptyMailbox {
    async fn run(&self, _command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        Ok(ProcessOutput {
            status: Some(0),
            stdout: json!({"messages": []}).to_string().into_bytes(),
        })
    }
}

/// An Agent, a Channel and a top-level message of that Channel, all in
/// one Workspace.
struct Targets {
    agent_id: String,
    channel_id: String,
    root_message_id: String,
}

async fn start() -> TwoTenants {
    TwoTenants::on(
        TestDaemon::start_with(TestDaemonOptions {
            gog: Some(Arc::new(EmptyMailbox)),
            ..TestDaemonOptions::default()
        })
        .await,
    )
    .await
}

/// Write a new Agent, a new Channel and a top-level message of that
/// Channel into one Workspace.
async fn targets_in(world: &TwoTenants, workspace_id: &WorkspaceId) -> Targets {
    let stores = world.daemon.stores();
    let agent = fixture::agent(workspace_id);
    stores.agents.create(&agent).await.expect("write the Agent");
    let channel = fixture::channel(workspace_id);
    stores
        .channels
        .create(&channel)
        .await
        .expect("write the Channel");
    let root = fixture::user_message(workspace_id, &channel.id, "a Thread of its own");
    stores
        .messages
        .insert(&root)
        .await
        .expect("write the Thread root");
    Targets {
        agent_id: agent.id.to_string(),
        channel_id: channel.id.to_string(),
        root_message_id: root.id.to_string(),
    }
}

/// POST as person A, with the status and the JSON answer.
async fn post(world: &TwoTenants, path: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}{path}", world.daemon.base_url))
        .header("cookie", &world.a.cookie)
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("POST {path}: {error}"));
    let status = response.status().as_u16();
    (status, response.json().await.expect("a JSON answer"))
}

/// GET as person A.
async fn get(world: &TwoTenants, path: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{}{path}", world.daemon.base_url))
        .header("cookie", &world.a.cookie)
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET {path}: {error}"))
        .error_for_status()
        .unwrap_or_else(|error| panic!("GET {path}: {error}"))
        .json()
        .await
        .expect("a JSON answer")
}

/// The fields of a rule that a refused call must leave as they were.
fn rule(value: &Value) -> Value {
    json!({
        "revision": value["revision"],
        "agent_id": value["agent_id"],
        "name": value["name"],
        "channel_id": value["channel_id"],
        "root_message_id": value["root_message_id"],
        "updated_at": value["updated_at"],
    })
}

/// The ids of the rules one list route answers.
async fn ids(world: &TwoTenants, path: &str) -> Vec<Value> {
    get(world, path).await["items"]
        .as_array()
        .expect("a page of rules")
        .iter()
        .map(|item| item["id"].clone())
        .collect()
}

/// Send `body` with `field` set to an id of person B, then with an id
/// that names nothing. Both answer 404 with the same body.
async fn refused_like_a_missing_id(
    world: &TwoTenants,
    path: &str,
    body: &Value,
    field: &str,
    foreign: &str,
) {
    let mut with_foreign = body.clone();
    with_foreign[field] = json!(foreign);
    let (status, foreign_answer) = post(world, path, with_foreign).await;
    assert_eq!(
        status, 404,
        "POST {path} with person B's {field} answered {status}: {foreign_answer}"
    );

    let mut with_missing = body.clone();
    with_missing[field] = json!("01J00000000000000000000000");
    let (status, missing_answer) = post(world, path, with_missing).await;
    assert_eq!(status, 404, "POST {path} with a missing {field}");
    assert_eq!(
        foreign_answer, missing_answer,
        "person B's {field} must read as an id that names nothing"
    );
}

/// A second Channel of person A, with a top-level message in it.
async fn second_channel_of_a(world: &TwoTenants) -> Targets {
    targets_in(world, &world.a.workspace_id).await
}

#[tokio::test]
async fn a_schedule_create_refuses_the_targets_of_another_workspace() {
    let world = start().await;
    let b = targets_in(&world, &world.b.workspace_id).await;
    let own = json!({
        "agent_id": world.a_id("agent_id"),
        "name": "Morning review",
        "instruction": "Review the inbox",
        "channel_id": world.a_id("channel_id"),
        "root_message_id": world.a_id("root_message_id"),
        "kind": "cron",
        "cron_expression": "0 9 * * *",
        "timezone": "UTC",
    });
    let before = ids(&world, "/api/v1/schedules").await;

    for (field, foreign) in [
        ("agent_id", &b.agent_id),
        ("channel_id", &b.channel_id),
        ("root_message_id", &b.root_message_id),
    ] {
        refused_like_a_missing_id(&world, "/api/v1/schedules", &own, field, foreign).await;
    }
    assert_eq!(
        ids(&world, "/api/v1/schedules").await,
        before,
        "a refused create stores no Schedule"
    );

    let (status, created) = post(&world, "/api/v1/schedules", own).await;
    assert_eq!(
        status, 201,
        "person A's own targets make a Schedule: {created}"
    );
}

#[tokio::test]
async fn a_schedule_edit_refuses_the_targets_of_another_workspace() {
    let world = start().await;
    let b = targets_in(&world, &world.b.workspace_id).await;
    let path = format!("/api/v1/schedules/{}", world.a_id("schedule_id"));
    let before = rule(&get(&world, &path).await);

    for (field, foreign) in [
        ("agent_id", &b.agent_id),
        ("channel_id", &b.channel_id),
        ("root_message_id", &b.root_message_id),
    ] {
        refused_like_a_missing_id(&world, &path, &json!({"action": "edit"}), field, foreign).await;
    }

    assert_eq!(
        rule(&get(&world, &path).await),
        before,
        "a refused edit leaves the Schedule as it was"
    );
}

#[tokio::test]
async fn a_schedule_edit_refuses_a_thread_root_of_another_channel() {
    let world = start().await;
    let other = second_channel_of_a(&world).await;
    let path = format!("/api/v1/schedules/{}", world.a_id("schedule_id"));
    let before = rule(&get(&world, &path).await);

    // The root is person A's own, but it is in another Channel than
    // the one the Schedule posts to.
    let (status, answer) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": other.root_message_id}),
    )
    .await;

    assert_eq!(status, 404, "{answer}");
    assert_eq!(rule(&get(&world, &path).await), before);
}

#[tokio::test]
async fn a_schedule_edit_to_another_channel_drops_the_thread() {
    let world = start().await;
    let other = second_channel_of_a(&world).await;
    let path = format!("/api/v1/schedules/{}", world.a_id("schedule_id"));
    let (status, threaded) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": world.a_id("root_message_id")}),
    )
    .await;
    assert_eq!(status, 200, "{threaded}");
    assert_eq!(threaded["root_message_id"], world.a_id("root_message_id"));

    let (status, moved) = post(
        &world,
        &path,
        json!({"action": "edit", "channel_id": other.channel_id}),
    )
    .await;

    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["channel_id"], other.channel_id);
    assert!(
        moved["root_message_id"].is_null(),
        "a Thread root of the old Channel does not follow the Schedule: {moved}"
    );
}

#[tokio::test]
async fn a_schedule_edit_with_a_null_root_clears_the_thread() {
    let world = start().await;
    let path = format!("/api/v1/schedules/{}", world.a_id("schedule_id"));
    let (status, threaded) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": world.a_id("root_message_id")}),
    )
    .await;
    assert_eq!(status, 200, "{threaded}");

    // An edit that leaves the root out keeps it.
    let (status, renamed) = post(
        &world,
        &path,
        json!({"action": "edit", "name": "Evening plan"}),
    )
    .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["root_message_id"], world.a_id("root_message_id"));

    let (status, cleared) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": null}),
    )
    .await;

    assert_eq!(status, 200, "{cleared}");
    assert!(cleared["root_message_id"].is_null(), "{cleared}");
    assert_eq!(cleared["channel_id"], world.a_id("channel_id"));
}

/// A Google Connection of person A with a mail Grant for A's Agent.
/// It uses the user's own client, so the collector reads the fake `gog`
/// and never asks Google for a token.
async fn mail_connection_of_a(world: &TwoTenants) -> String {
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: world.a.workspace_id.clone(),
        provider: "google".to_string(),
        alias: "home".to_string(),
        display_name: "Home".to_string(),
        status: Connection::CONNECTED.to_string(),
        auth_mode: Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: vec!["gmail_read".to_string()],
        config: json!({"account": "a@example.com", "client": "home"}),
        created_at: now_ms(),
    };
    world
        .daemon
        .stores()
        .connections
        .create(&connection)
        .await
        .expect("write A's Connection");
    let (status, grant) = post(
        world,
        "/api/v1/grants",
        json!({
            "agent_id": world.a_id("agent_id"),
            "connection_id": connection.id.as_str(),
            "capabilities": ["gmail_read"],
        }),
    )
    .await;
    assert_eq!(status, 201, "{grant}");
    connection.id.to_string()
}

#[tokio::test]
async fn an_event_subscription_create_refuses_the_targets_of_another_workspace() {
    let world = start().await;
    let b = targets_in(&world, &world.b.workspace_id).await;
    let connection_id = mail_connection_of_a(&world).await;
    let own = json!({
        "agent_id": world.a_id("agent_id"),
        "connection_id": connection_id,
        "event_kind": "mail.message_received",
        "name": "Invoices",
        "instruction": "File the invoice",
        "channel_id": world.a_id("channel_id"),
        "root_message_id": world.a_id("root_message_id"),
        "filter": {},
    });
    let before = ids(&world, "/api/v1/event-subscriptions").await;

    for (field, foreign) in [
        ("agent_id", &b.agent_id),
        ("channel_id", &b.channel_id),
        ("root_message_id", &b.root_message_id),
    ] {
        refused_like_a_missing_id(&world, "/api/v1/event-subscriptions", &own, field, foreign)
            .await;
    }
    assert_eq!(
        ids(&world, "/api/v1/event-subscriptions").await,
        before,
        "a refused create stores no Event Subscription"
    );

    let (status, created) = post(&world, "/api/v1/event-subscriptions", own).await;
    assert_eq!(
        status, 201,
        "person A's own targets make an Event Subscription: {created}"
    );
}

#[tokio::test]
async fn an_event_subscription_edit_refuses_the_targets_of_another_workspace() {
    let world = start().await;
    let b = targets_in(&world, &world.b.workspace_id).await;
    let path = format!(
        "/api/v1/event-subscriptions/{}",
        world.a_id("subscription_id")
    );
    let before = rule(&get(&world, &path).await);

    // An edit does not name the Agent: the Agent of an Event
    // Subscription does not change.
    for (field, foreign) in [
        ("channel_id", &b.channel_id),
        ("root_message_id", &b.root_message_id),
    ] {
        refused_like_a_missing_id(&world, &path, &json!({"action": "edit"}), field, foreign).await;
    }

    assert_eq!(
        rule(&get(&world, &path).await),
        before,
        "a refused edit leaves the Event Subscription as it was"
    );
}

#[tokio::test]
async fn an_event_subscription_edit_to_another_channel_drops_the_thread() {
    let world = start().await;
    let other = second_channel_of_a(&world).await;
    let path = format!(
        "/api/v1/event-subscriptions/{}",
        world.a_id("subscription_id")
    );
    let (status, threaded) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": world.a_id("root_message_id")}),
    )
    .await;
    assert_eq!(status, 200, "{threaded}");
    assert_eq!(threaded["root_message_id"], world.a_id("root_message_id"));

    let (status, moved) = post(
        &world,
        &path,
        json!({"action": "edit", "channel_id": other.channel_id}),
    )
    .await;

    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["channel_id"], other.channel_id);
    assert!(
        moved["root_message_id"].is_null(),
        "a Thread root of the old Channel does not follow the rule: {moved}"
    );
}

#[tokio::test]
async fn an_event_subscription_edit_with_a_null_root_clears_the_thread() {
    let world = start().await;
    let path = format!(
        "/api/v1/event-subscriptions/{}",
        world.a_id("subscription_id")
    );
    let (status, threaded) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": world.a_id("root_message_id")}),
    )
    .await;
    assert_eq!(status, 200, "{threaded}");

    // An edit that leaves the root out keeps it.
    let (status, renamed) =
        post(&world, &path, json!({"action": "edit", "name": "Receipts"})).await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["root_message_id"], world.a_id("root_message_id"));

    let (status, cleared) = post(
        &world,
        &path,
        json!({"action": "edit", "root_message_id": null}),
    )
    .await;

    assert_eq!(status, 200, "{cleared}");
    assert!(cleared["root_message_id"].is_null(), "{cleared}");
    assert_eq!(cleared["channel_id"], world.a_id("channel_id"));
}
