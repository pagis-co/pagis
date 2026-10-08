//! Agent-to-agent delegation over the full daemon: the
//! `send_message` tool opens an agent DM the user can watch, the reply
//! comes back as a trigger, pointer entries surface the traffic in the
//! user's DM, and the hop cap stops a delegation loop.

use std::sync::Arc;
use std::time::Duration;

use pagis_agent::AgentLoopConfig;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// One run at a time per agent: each agent's scripts play in run
/// order, so multi-run delegation chains stay deterministic.
fn options(brain: &Arc<ScriptedBrain>, max_hops: u32) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        agents: AgentLoopConfig {
            max_concurrent_runs: 1,
            max_hops,
            ..AgentLoopConfig::default()
        },
        ..TestDaemonOptions::default()
    }
}

async fn create_agent(daemon: &TestDaemon, name: &str) -> String {
    create_agent_for(daemon, name, "").await
}

/// Hire one agent with its own line on what to ask it for.
async fn create_agent_for(daemon: &TestDaemon, name: &str, description: &str) -> String {
    let response = client()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "name": name,
            "job": "researcher",
            "description": description,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let agent: serde_json::Value = response.json().await.unwrap();
    agent["id"].as_str().unwrap().to_string()
}

async fn send(daemon: &TestDaemon, channel_id: &str, pending_id: &str, text: &str) {
    let response = client()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn send_in_thread(
    daemon: &TestDaemon,
    channel_id: &str,
    pending_id: &str,
    text: &str,
    parent_message_id: &str,
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

/// The replies of one thread.
async fn thread(daemon: &TestDaemon, channel_id: &str, root_id: &str) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
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
    page["replies"].as_array().unwrap().clone()
}

async fn timeline(daemon: &TestDaemon, channel_id: &str) -> Vec<serde_json::Value> {
    timeline_page(daemon, channel_id, None, None).await
}

async fn timeline_page(
    daemon: &TestDaemon,
    channel_id: &str,
    before: Option<&str>,
    limit: Option<u32>,
) -> Vec<serde_json::Value> {
    let mut request = client()
        .get(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie());
    if let Some(before) = before {
        request = request.query(&[("before", before)]);
    }
    if let Some(limit) = limit {
        request = request.query(&[("limit", limit.to_string())]);
    }
    let page: serde_json::Value = request.send().await.unwrap().json().await.unwrap();
    page["items"].as_array().unwrap().clone()
}

async fn channels(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    page["items"].as_array().unwrap().clone()
}

/// Poll until `probe` yields a thread reply whose text contains
/// `needle`. Thread replies are plain messages: they carry no timeline
/// `kind`.
async fn await_reply<F, Fut>(probe: F, needle: &str) -> serde_json::Value
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Vec<serde_json::Value>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let found = probe().await.into_iter().find(|m| {
            m["status"] == "complete" && m["text_content"].as_str().unwrap_or("").contains(needle)
        });
        if let Some(message) = found {
            return message;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "thread reply containing {needle:?} did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll until `probe` yields a message whose text contains `needle`.
/// Wait until the user's DM derives `count` pointers.
async fn await_pointers(daemon: &TestDaemon, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let found = timeline(daemon, &daemon.dm_channel_id)
            .await
            .iter()
            .filter(|item| item["kind"] == "pointer")
            .count();
        if found >= count {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{count} pointers did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn await_message<F, Fut>(probe: F, needle: &str) -> serde_json::Value
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Vec<serde_json::Value>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let found = probe().await.into_iter().find(|m| {
            m["kind"] == "message"
                && m["status"] == "complete"
                && m["text_content"].as_str().unwrap_or("").contains(needle)
        });
        if let Some(message) = found {
            return message;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "message containing {needle:?} did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The agent DM channel that `send_message` created, when it exists.
async fn agent_dm(daemon: &TestDaemon, a: &str, b: &str) -> Option<serde_json::Value> {
    channels(daemon).await.into_iter().find(|c| {
        let ids = c["agent_ids"].as_array().unwrap();
        c["kind"] == "dm"
            && ids.len() == 2
            && ids.contains(&serde_json::json!(a))
            && ids.contains(&serde_json::json!(b))
    })
}

fn hop_counts(runs: &[serde_json::Value]) -> Vec<i64> {
    let mut hops: Vec<i64> = runs
        .iter()
        .map(|r| r["hop_count"].as_i64().unwrap())
        .collect();
    hops.sort_unstable();
    hops
}

async fn runs(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let page: serde_json::Value = client()
        .get(format!("{}/api/v1/runs", daemon.base_url))
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
async fn a_delegated_answer_relays_into_the_conversation_that_waits_for_it() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    let scout = create_agent(&daemon, "Scout").await;

    // Run 1 (user DM): Pixie delegates, then tells the user. Run 2
    // (agent DM): Scout answers. Run 3 (agent DM, triggered by the
    // answer): Pixie carries the origin of the chain, so its reply goes
    // to the user's DM without a second `send_message`.
    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));
    brain.push_for(
        "Pixie",
        Script::reply(&["Scout says the launch date is May 4."]),
    );

    send(
        &daemon,
        &daemon.dm_channel_id,
        "p1",
        "Ask Scout for the launch date",
    )
    .await;

    // The answer reaches the user's DM with Pixie, unprompted.
    let relay = await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "Scout says the launch date is May 4.",
    )
    .await;
    assert_eq!(relay["author_agent_id"], serde_json::json!(daemon.agent_id));

    // The user can watch the agent DM directly: it lists with both
    // agents, and its timeline shows the exchange.
    let dm = agent_dm(&daemon, &daemon.agent_id, &scout)
        .await
        .expect("agent DM channel exists");
    assert_eq!(dm["title"], "Pixie ↔ Scout");
    let dm_id = dm["id"].as_str().unwrap();
    await_message(|| timeline(&daemon, dm_id), "What is the launch date?").await;
    let answer = await_message(|| timeline(&daemon, dm_id), "The launch date is May 4.").await;
    assert_eq!(answer["author_agent_id"], serde_json::json!(scout));

    // The relay left the agent DM, so it triggered Scout no further:
    // the chain is user trigger 0, Scout 1, the relay 2, and it stops.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let texts: Vec<String> = timeline(&daemon, dm_id)
        .await
        .iter()
        .map(|m| m["text_content"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("Scout says the launch date is May 4.")),
        "the relay must not post back into the agent DM: {texts:?}"
    );
    assert_eq!(hop_counts(&runs(&daemon).await), vec![0, 1, 2]);
}

#[tokio::test]
async fn an_agent_that_answers_in_the_channel_cannot_also_send_a_message() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    let scout = create_agent(&daemon, "Scout").await;

    // Run 2 (agent DM) is Scout answering. It sends a message back to
    // Pixie and replies as well.
    // The send is refused, because the reply already goes to that
    // channel, so Scout speaks once and Pixie answers the user once.
    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for(
        "Scout",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Pixie", "text": "Sent you the launch date." }),
        ),
    );
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));
    brain.push_for(
        "Pixie",
        Script::reply(&["Scout says the launch date is May 4."]),
    );

    send(
        &daemon,
        &daemon.dm_channel_id,
        "p1",
        "Ask Scout for the launch date",
    )
    .await;
    await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "Scout says the launch date is May 4.",
    )
    .await;

    let dm = agent_dm(&daemon, &daemon.agent_id, &scout)
        .await
        .expect("agent DM channel exists");
    let dm_id = dm["id"].as_str().unwrap();
    await_message(|| timeline(&daemon, dm_id), "The launch date is May 4.").await;

    tokio::time::sleep(Duration::from_millis(400)).await;
    let texts: Vec<String> = timeline(&daemon, dm_id)
        .await
        .iter()
        .map(|m| m["text_content"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("Sent you the launch date.")),
        "the refused send must post nothing in the channel: {texts:?}"
    );
    // One Scout message, so one relay: user trigger 0, Scout 1, the
    // relay 2, and no second relay behind a second Scout message.
    assert_eq!(hop_counts(&runs(&daemon).await), vec![0, 1, 2]);
}

#[tokio::test]
async fn a_relay_lands_in_the_thread_the_request_came_from() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    create_agent(&daemon, "Scout").await;

    // The root message triggers a run of its own before the thread
    // starts; the delegation runs follow it.
    brain.push_for("Pixie", Script::reply(&["Noted."]));
    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));
    brain.push_for("Pixie", Script::reply(&["Scout says May 4."]));

    // The user asks inside a thread, so the answer belongs there too.
    send(&daemon, &daemon.dm_channel_id, "p1", "A thread root").await;
    let root = await_message(|| timeline(&daemon, &daemon.dm_channel_id), "A thread root").await;
    let root_id = root["id"].as_str().unwrap().to_string();
    send_in_thread(
        &daemon,
        &daemon.dm_channel_id,
        "p2",
        "Ask Scout for the launch date",
        &root_id,
    )
    .await;

    // The relay is a reply in the thread, not a new top-level message.
    let relay = await_reply(
        || thread(&daemon, &daemon.dm_channel_id, &root_id),
        "Scout says May 4.",
    )
    .await;
    assert_eq!(relay["parent_message_id"].as_str().unwrap(), root_id);
}

#[tokio::test]
async fn the_briefing_names_the_channel_and_the_waiting_conversation() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    create_agent(&daemon, "Scout").await;

    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));
    brain.push_for("Pixie", Script::reply(&["Scout says May 4."]));

    send(
        &daemon,
        &daemon.dm_channel_id,
        "p1",
        "Ask Scout for the launch date",
    )
    .await;
    await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "Scout says May 4.",
    )
    .await;

    let systems: Vec<String> = brain
        .requests()
        .into_iter()
        .map(|request| request.system)
        .collect();

    // Pixie's first run: the user's own DM.
    let first = systems
        .iter()
        .find(|s| s.starts_with("You are Pixie,"))
        .expect("Pixie ran");
    assert!(
        first.contains("your direct channel with the user"),
        "the user DM briefing must name the user: {first}"
    );

    // Scout answers in a channel the user is not in, on Pixie's behalf.
    let scout = systems
        .iter()
        .find(|s| s.starts_with("You are Scout,"))
        .expect("Scout ran");
    assert!(
        scout.contains("The user is not here"),
        "the agent DM briefing must not claim the user is present: {scout}"
    );
    assert!(
        scout.contains("Pixie asked you here"),
        "the briefing must name the requester: {scout}"
    );

    // Pixie's relay run: its reply belongs to the waiting conversation.
    let relay = systems
        .iter()
        .filter(|s| s.starts_with("You are Pixie,"))
        .find(|s| s.contains("delivered to"))
        .expect("the relay run is briefed");
    assert!(
        relay.contains("your direct channel with the user"),
        "the relay briefing must name the waiting conversation: {relay}"
    );
}

#[tokio::test]
async fn pointer_entries_appear_in_the_dm_and_link_through() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    let scout = create_agent(&daemon, "Scout").await;

    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "Check the logs" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["Delegated."]));
    brain.push_for("Scout", Script::reply(&["Logs are clean."]));
    // Scout's answer triggers Pixie again. That run thanks Scout in the
    // agent DM and reports to the user, so Pixie has two messages in
    // the agent DM, and so two pointers. Scout's empty reply then ends
    // the exchange.
    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "Thanks." }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["Scout says the logs are clean."]));
    brain.push_for("Scout", Script::reply(&[]));

    send(
        &daemon,
        &daemon.dm_channel_id,
        "p1",
        "Have Scout check the logs",
    )
    .await;
    await_message(|| timeline(&daemon, &daemon.dm_channel_id), "Delegated.").await;
    let dm = agent_dm(&daemon, &daemon.agent_id, &scout).await.unwrap();
    let dm_id = dm["id"].as_str().unwrap().to_string();
    await_message(|| timeline(&daemon, &dm_id), "Check the logs").await;
    // The follow-up run's own message lands after Scout answers, and
    // it is a second message of Pixie's in the agent DM. Wait for the
    // timeline to stop growing, so the snapshot and the cursor walk
    // below read the same state.
    await_pointers(&daemon, 2).await;
    // The follow-up run's report to the user is the last message of the
    // exchange. It lands after the second pointer, so the snapshot waits
    // for it too.
    await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "Scout says the logs are clean.",
    )
    .await;

    // Pixie's DM timeline derives one pointer per message of Pixie's in
    // the agent DM, each linking through to that channel, newest first.
    let items = timeline(&daemon, &daemon.dm_channel_id).await;
    let pointers: Vec<_> = items.iter().filter(|i| i["kind"] == "pointer").collect();
    assert!(
        pointers.iter().all(
            |pointer| pointer["channel_id"].as_str() == Some(dm_id.as_str())
                && pointer["channel_title"] == "Pixie ↔ Scout"
                && pointer["agent_id"] == serde_json::json!(daemon.agent_id)
        ),
        "every pointer links through to the agent DM: {}",
        serde_json::to_string(&pointers).unwrap()
    );
    let delegated = pointers
        .iter()
        .find(|pointer| pointer["preview"] == "Check the logs")
        .expect("a pointer to the delegated message");
    assert_eq!(delegated["channel_id"].as_str().unwrap(), dm_id);

    // The agent DM's own timeline derives no pointers, and the page
    // cursor stays uniform across kinds: walking with `before` visits
    // every item exactly once.
    assert!(
        timeline(&daemon, &dm_id)
            .await
            .iter()
            .all(|i| i["kind"] == "message")
    );
    let mut walked = Vec::new();
    let mut before: Option<String> = None;
    loop {
        let page = timeline_page(&daemon, &daemon.dm_channel_id, before.as_deref(), Some(1)).await;
        let Some(item) = page.first() else { break };
        let id = item["id"].as_str().unwrap().to_string();
        walked.push((item["kind"].as_str().unwrap().to_string(), id.clone()));
        before = Some(id);
    }
    let mut expected: Vec<(String, String)> = items
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_string(),
                i["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    expected.sort_by(|a, b| b.1.cmp(&a.1));
    assert_eq!(walked, expected);
}

#[tokio::test]
async fn a_delegation_loop_stops_at_the_hop_cap() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 2)).await;
    create_agent(&daemon, "Scout").await;

    // Each run puts one message in the agent DM and ends, so only
    // those messages propagate: Pixie (hop 0) → Scout (hop 1) → Pixie
    // (hop 2) → the hop-capped message triggers no run. Pixie sends,
    // because its own reply goes to the user's DM. Scout answers with
    // its reply, which is where a run in this channel speaks. Scout's
    // second script must never play.
    for text in ["ping", "ping again"] {
        brain.push_for(
            "Pixie",
            Script::tool_call(
                &[],
                "send_message",
                serde_json::json!({ "to": "Scout", "text": text }),
            ),
        );
        brain.push_for("Pixie", Script::reply(&[]));
    }
    for text in ["pong", "pong again"] {
        brain.push_for("Scout", Script::reply(&[text]));
    }

    send(&daemon, &daemon.dm_channel_id, "p1", "start the loop").await;
    // Wait for the sends themselves, not for the Runs that make them: a
    // Run that has started has not yet called `send_message`, so a Run
    // count of three says nothing about the third message.
    let texts = exchange_of(&daemon, 3).await;
    assert_eq!(texts, ["ping", "pong", "ping again"]);

    // Quiet period: the capped message spawns nothing further. The
    // exchange and the Runs stay where the cap left them.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(exchange_of(&daemon, 3).await, texts, "the hop cap holds");
    assert_eq!(hop_counts(&runs(&daemon).await), vec![0, 1, 2]);
}

/// The Pixie ↔ Scout exchange, once it holds at least `count` messages.
async fn exchange_of(daemon: &TestDaemon, count: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let dm = channels(daemon)
            .await
            .into_iter()
            .find(|c| c["title"] == "Pixie ↔ Scout");
        if let Some(dm) = dm {
            let texts: Vec<String> = timeline(daemon, dm["id"].as_str().unwrap())
                .await
                .iter()
                .rev()
                // The runs' progress rows are daemon state, not
                // the exchange.
                .filter(|m| m["blocks"][0]["type"] != "progress")
                .map(|m| m["text_content"].as_str().unwrap().to_string())
                .collect();
            if texts.len() >= count {
                return texts;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the exchange reached {count} messages"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// An agent-to-agent channel is read-only for the user (ADR-0003): the
/// list says the user is not a member, and a send into it is refused,
/// so no user message can start an agent-to-agent loop.
#[tokio::test]
async fn the_user_reads_an_agent_channel_and_cannot_write_in_it() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    let scout = create_agent(&daemon, "Scout").await;

    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));

    send(&daemon, &daemon.dm_channel_id, "p1", "Ask Scout").await;
    await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "I asked Scout.",
    )
    .await;

    let dm = agent_dm(&daemon, &daemon.agent_id, &scout)
        .await
        .expect("agent DM channel exists");
    assert_eq!(dm["user_member"], serde_json::json!(false));

    // The user's own DM stays writable.
    let own = channels(&daemon)
        .await
        .into_iter()
        .find(|c| c["id"] == serde_json::json!(daemon.dm_channel_id))
        .expect("the user's DM lists");
    assert_eq!(own["user_member"], serde_json::json!(true));

    let refused = client()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url,
            dm["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "p2", "text": "Keep talking" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "forbidden");
}

/// Give one agent a live Connection Grant, and answer with its id.
async fn connect(daemon: &TestDaemon, agent_id: &str) -> pagis_core::GrantId {
    use pagis_core::{ConnectionStore, GrantStore, now_ms};
    let pool = daemon.pool().clone();
    let agent = pagis_core::AgentStore::get(
        &pagis_storage_sqlite::SqliteAgentStore::new(pool.clone()),
        &daemon.workspace_id,
        &pagis_core::AgentId::from(agent_id.to_owned()),
    )
    .await
    .unwrap()
    .unwrap();
    let connection = pagis_core::Connection {
        id: pagis_core::ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "work".into(),
        display_name: "Work mail".into(),
        status: "connected".into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    pagis_storage_sqlite::SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
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
    pagis_storage_sqlite::SqliteGrantStore::new(pool)
        .create(&grant)
        .await
        .unwrap();
    grant.id
}

/// The text of every turn the brain saw for one agent.
fn turns_of(brain: &ScriptedBrain, agent: &str) -> Vec<String> {
    brain
        .requests()
        .into_iter()
        .filter(|request| request.system.starts_with(&format!("You are {agent},")))
        .flat_map(|request| {
            request
                .messages
                .into_iter()
                .map(|message| message.text)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The stamp of a message names the Grants of its author. A reader
/// holds its own Grants and never the author's, so a stamp read
/// against the reader hides the request from the agent that is asked.
#[tokio::test]
async fn a_sender_with_a_connection_is_still_read_by_the_agent_it_asks() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    create_agent(&daemon, "Scout").await;
    connect(&daemon, &daemon.agent_id).await;

    brain.push_for(
        "Pixie",
        Script::tool_call(
            &[],
            "send_message",
            serde_json::json!({ "to": "Scout", "text": "What is the launch date?" }),
        ),
    );
    brain.push_for("Pixie", Script::reply(&["I asked Scout."]));
    brain.push_for("Scout", Script::reply(&["The launch date is May 4."]));
    brain.push_for("Pixie", Script::reply(&["Scout says May 4."]));

    send(
        &daemon,
        &daemon.dm_channel_id,
        "p1",
        "Ask Scout for the launch date",
    )
    .await;
    await_message(
        || timeline(&daemon, &daemon.dm_channel_id),
        "Scout says May 4.",
    )
    .await;

    let scout = turns_of(&brain, "Scout");
    assert!(
        scout
            .iter()
            .any(|text| text.contains("What is the launch date?")),
        "Scout must read the question it was asked: {scout:?}"
    );
    let pixie = turns_of(&brain, "Pixie");
    assert!(
        pixie
            .iter()
            .any(|text| text.contains("The launch date is May 4.")),
        "Pixie must read the answer it relays: {pixie:?}"
    );
}

/// An agent that does not know a colleague exists answers in its
/// place. Every run names the user's other sprites and the one call that
/// reaches them.
#[tokio::test]
async fn the_briefing_names_the_other_sprites_in_the_user_dm() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain, 8)).await;
    let clown = create_agent_for(&daemon, "Clown", "Ask Clown for a joke.").await;
    connect(&daemon, &clown).await;

    brain.push_for("Pixie", Script::reply(&["Noted."]));
    send(&daemon, &daemon.dm_channel_id, "p1", "ask Clown for a joke").await;
    await_message(|| timeline(&daemon, &daemon.dm_channel_id), "Noted.").await;

    let system = brain
        .requests()
        .into_iter()
        .map(|request| request.system)
        .find(|system| system.starts_with("You are Pixie,"))
        .expect("Pixie ran");
    assert!(
        system.contains("- Clown (researcher): Ask Clown for a joke."),
        "the sprite line must carry the colleague's own description: {system}"
    );
    assert!(
        system.contains("Connections: work."),
        "the sprite line must name what the colleague can reach: {system}"
    );
    assert!(
        system.contains("delegate it: call send_message"),
        "the sprite line must ask for delegation: {system}"
    );
    assert!(
        system.contains("never write an answer and give it another agent's name"),
        "the prompt must refuse an answer written in another agent's name: {system}"
    );
}
