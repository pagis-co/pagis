//! Group-channel trigger tests over the full daemon: only an
//! @-mention triggers an agent, in-thread follow-ups trigger without a
//! re-mention, and archived agents stop triggering with history intact.

use std::sync::Arc;
use std::time::Duration;

use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
}

async fn create_agent(daemon: &TestDaemon, name: &str) -> String {
    let response = client()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "name": name, "job": "helper" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let agent: serde_json::Value = response.json().await.unwrap();
    agent["id"].as_str().unwrap().to_string()
}

async fn create_group(daemon: &TestDaemon, agent_ids: &[&str]) -> String {
    let response = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "title": "ops", "agent_ids": agent_ids }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let channel: serde_json::Value = response.json().await.unwrap();
    assert_eq!(channel["kind"], "group");
    channel["id"].as_str().unwrap().to_string()
}

async fn send(daemon: &TestDaemon, channel_id: &str, pending_id: &str, text: &str) {
    send_in_thread(daemon, channel_id, pending_id, text, None).await;
}

async fn send_in_thread(
    daemon: &TestDaemon,
    channel_id: &str,
    pending_id: &str,
    text: &str,
    parent_message_id: Option<&str>,
) {
    let response = client()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": pending_id,
            "text": text,
            "parent_message_id": parent_message_id,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn timeline(daemon: &TestDaemon, channel_id: &str) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
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
    page["items"].as_array().unwrap().clone()
}

async fn thread(daemon: &TestDaemon, channel_id: &str, root_id: &str) -> Vec<serde_json::Value> {
    let thread: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/threads/{root_id}",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    thread["replies"].as_array().unwrap().clone()
}

/// The count of runs the daemon created; a message that must not
/// trigger leaves this unchanged.
async fn run_count(daemon: &TestDaemon) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(daemon.pool())
        .await
        .unwrap()
}

/// The agent's own messages: a derived progress row is daemon
/// state, not something the agent said.
fn complete_agent_messages(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter(|m| {
            m["author_kind"] == "agent"
                && m["status"] == "complete"
                && m["blocks"][0]["type"] != "progress"
        })
        .cloned()
        .collect()
}

/// Poll until `probe` yields `count` complete agent messages.
async fn await_agent_messages<F, Fut>(probe: F, count: usize) -> Vec<serde_json::Value>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Vec<serde_json::Value>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let found = complete_agent_messages(&probe().await);
        if found.len() >= count {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent messages did not arrive: {} of {count}",
            found.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn only_a_mention_triggers_an_agent_in_a_group_channel() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let rex = create_agent(&daemon, "Rex").await;
    let group = create_group(&daemon, &[daemon.agent_id.as_str(), rex.as_str()]).await;

    // No mention: no agent runs. The scripted brain would fail the run
    // (no script queued), so a triggered agent would leave a message.
    send(&daemon, &group, "p1", "hello team, kicking things off").await;

    brain.push(Script::reply(&["On it."]));
    send(&daemon, &group, "p2", "@Rex can you take this?").await;

    let replies = await_agent_messages(|| timeline(&daemon, &group), 1).await;
    assert_eq!(replies[0]["author_agent_id"], serde_json::json!(rex));
    // Exactly one run: the un-mentioned message and the un-mentioned
    // agent (Pixie) never triggered.
    assert_eq!(run_count(&daemon).await, 1);
    let failed: Vec<_> = timeline(&daemon, &group)
        .await
        .into_iter()
        .filter(|m| m["status"] == "failed")
        .collect();
    assert_eq!(failed, Vec::<serde_json::Value>::new());
}

#[tokio::test]
async fn an_in_thread_follow_up_triggers_without_a_re_mention() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let rex = create_agent(&daemon, "Rex").await;
    let group = create_group(&daemon, &[rex.as_str()]).await;

    send(&daemon, &group, "root", "planning thread").await;
    let root_id = timeline(&daemon, &group).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The mention pulls Rex into the thread.
    brain.push(Script::reply(&["Digging in."]));
    send_in_thread(&daemon, &group, "r1", "@Rex dig into this", Some(&root_id)).await;
    let first = await_agent_messages(|| thread(&daemon, &group, &root_id), 1).await;
    let first_run = first[0]["run_id"].as_str().expect("reply carries its run");
    // This test requires a new Run for r2.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let complete: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE id=? AND state='completed')",
            )
            .bind(first_run)
            .fetch_one(daemon.pool())
            .await
            .unwrap();
            if complete {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first run completed before follow-up");

    // The follow-up carries no mention; the re-mention rule triggers
    // Rex because Rex already posted in the thread.
    brain.push(Script::reply(&["Found it."]));
    send_in_thread(&daemon, &group, "r2", "any progress?", Some(&root_id)).await;
    let replies = await_agent_messages(|| thread(&daemon, &group, &root_id), 2).await;

    assert!(
        replies
            .iter()
            .all(|m| m["author_agent_id"] == serde_json::json!(rex.as_str()))
    );
    // Two runs: the mention and the follow-up. The root message
    // triggered nothing.
    assert_eq!(run_count(&daemon).await, 2);
}

#[tokio::test]
async fn an_archived_agent_stops_triggering_and_history_stays() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let rex = create_agent(&daemon, "Rex").await;
    let group = create_group(&daemon, &[rex.as_str()]).await;

    brain.push(Script::reply(&["Here."]));
    send(&daemon, &group, "p1", "@Rex hello").await;
    await_agent_messages(|| timeline(&daemon, &group), 1).await;

    let response = client()
        .post(format!("{}/api/v1/agents/{rex}/archive", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    send(&daemon, &group, "p2", "@Rex are you there?").await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    // No new run started, and the history is intact: both user
    // messages, the pre-archive reply, and that run's progress row.
    assert_eq!(run_count(&daemon).await, 1);
    let messages = timeline(&daemon, &group).await;
    assert_eq!(messages.len(), 4);
    assert_eq!(complete_agent_messages(&messages).len(), 1);
}
