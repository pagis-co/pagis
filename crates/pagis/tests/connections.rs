//! Full-daemon connection-dispatch tests (ADR-0005): one
//! provider instance serves every Agent granted a Connection, a call
//! that may have reached Google is never sent twice, and authority is
//! checked again after the user decides.
//!
//! The calls here are `mail__*` with an alias in the `mailbox`
//! argument. That names one of the user's mail accounts, so the broker
//! resolves the call onto that Connection under its Grant, and the
//! Gmail adapter runs it (ADR-0019).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pagis::connections::{ConnectionProvider, ConnectionProviderFactory, ProviderFailure};
use pagis_broker::{MAIL_GET_MESSAGE, MAIL_GET_THREAD, MAIL_SEARCH, MAIL_SEND};
use pagis_core::{AgentId, Connection, GrantId, WorkspaceId, now_ms};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What one scripted provider does with the call it receives.
#[derive(Clone)]
enum Behavior {
    Ok,
    /// Fail with a code the provider knows never left, then succeed.
    RetryableOnce,
    /// Never answer, so the call runs out of time.
    Hang,
}

#[derive(Default)]
struct Calls {
    tools: Vec<String>,
}

struct FakeProvider {
    behavior: Behavior,
    calls: Arc<Mutex<Calls>>,
    failed: AtomicUsize,
}

#[async_trait]
impl ConnectionProvider for FakeProvider {
    fn is_write(&self, tool: &str) -> bool {
        pagis_google::is_write(tool)
    }

    fn retain_result(&self, tool: &str) -> bool {
        matches!(tool, MAIL_GET_MESSAGE | MAIL_GET_THREAD)
    }

    fn source_reads(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        result: &serde_json::Value,
    ) -> Vec<pagis_core::knowledge::SourceRead> {
        pagis_google::source_reads(tool, arguments, result)
    }

    async fn invoke(
        &self,
        tool: &str,
        _arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderFailure> {
        self.calls.lock().unwrap().tools.push(tool.to_string());
        match self.behavior {
            Behavior::Ok if tool == MAIL_GET_MESSAGE => Ok(serde_json::json!({
                "message_id": "provider-message-1",
                "body": "The violet launch code is 8241."
            })),
            Behavior::Ok => Ok(serde_json::json!({"messages": []})),
            Behavior::RetryableOnce if self.failed.fetch_add(1, Ordering::SeqCst) == 0 => {
                Err(ProviderFailure::new("temporarily_unavailable", true))
            }
            Behavior::RetryableOnce => Ok(serde_json::json!({"messages": []})),
            Behavior::Hang => {
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }
}

struct FakeFactory {
    behavior: Behavior,
    calls: Arc<Mutex<Calls>>,
    builds: Arc<AtomicUsize>,
}

impl ConnectionProviderFactory for FakeFactory {
    fn build(
        &self,
        _connection: &Connection,
    ) -> Result<Arc<dyn ConnectionProvider>, ProviderFailure> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeProvider {
            behavior: self.behavior.clone(),
            calls: Arc::clone(&self.calls),
            failed: AtomicUsize::new(0),
        }))
    }
}

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    calls: Arc<Mutex<Calls>>,
    builds: Arc<AtomicUsize>,
}

impl Harness {
    fn tools(&self) -> Vec<String> {
        self.calls.lock().unwrap().tools.clone()
    }
}

async fn boot(behavior: Behavior, timeout: Duration) -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let calls = Arc::new(Mutex::new(Calls::default()));
    let builds = Arc::new(AtomicUsize::new(0));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        connection_providers: Some(Arc::new(FakeFactory {
            behavior,
            calls: Arc::clone(&calls),
            builds: Arc::clone(&builds),
        })),
        connection_call_timeout: timeout,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        brain,
        calls,
        builds,
    }
}

async fn workspace_id(daemon: &TestDaemon) -> WorkspaceId {
    use pagis_core::AgentStore;
    pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

/// Seed a connected Google Connection. The connect flow that writes
/// this row for real has its own tests (`tests/connect.rs`); the
/// dispatch path under test starts from the row.
async fn connect_google(daemon: &TestDaemon, id: &str, alias: &str) {
    sqlx::query(
        "INSERT INTO connections \
         (id, workspace_id, provider, alias, display_name, status, auth_mode, config, created_at) \
         VALUES (?, ?, 'google', ?, 'Google', 'connected', 'byo', ?, ?)",
    )
    .bind(id)
    .bind(workspace_id(daemon).await.as_str())
    .bind(alias)
    .bind(serde_json::json!({"account": "alice@example.com", "client": alias}).to_string())
    .bind(now_ms())
    .execute(daemon.pool())
    .await
    .unwrap();
}

async fn grant(daemon: &TestDaemon, connection_id: &str, capabilities: &[&str]) -> GrantId {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": connection_id,
            "capabilities": capabilities,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    GrantId::from(body["id"].as_str().unwrap().to_string())
}

async fn set_capabilities(daemon: &TestDaemon, grant_id: &GrantId, capabilities: &[&str]) {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/grants/{}/capabilities",
            daemon.base_url, grant_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"capabilities": capabilities}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("frame is JSON");
        }
    }
}

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("ws send");
    next_frame_of(&mut socket, "ready").await;
    socket
}

async fn frames_until_settled(socket: &mut Socket) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(socket).await;
        let settled = frame["type"] == "run.state_changed"
            && (frame["payload"]["payload"]["to"] == "completed"
                || frame["payload"]["payload"]["to"] == "failed");
        frames.push(frame);
        if settled {
            return frames;
        }
    }
}

async fn send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

fn tool_fact<'a>(frames: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    frames
        .iter()
        .find(|frame| {
            frame["type"] == "tool.completed" && frame["payload"]["payload"]["name"] == name
        })
        .unwrap_or_else(|| panic!("no tool.completed fact for {name}"))
}

#[tokio::test]
async fn one_instance_serves_every_run_and_an_ungranted_agent_never_reaches_it() {
    let h = boot(Behavior::Ok, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;

    // No grant yet: the tool is not in the run's snapshot at all.
    h.brain.push(Script::reply(&["Nothing to do."]));
    send(&h.daemon, "p0", "hello").await;
    frames_until_settled(&mut socket).await;
    assert_eq!(h.builds.load(Ordering::SeqCst), 0);
    assert!(h.tools().is_empty());

    connect_google(&h.daemon, "conn-work", "work").await;
    grant(&h.daemon, "conn-work", &["gmail_read"]).await;

    for (index, pending) in ["p1", "p2"].iter().enumerate() {
        h.brain.push(Script::tool_call(
            &[],
            MAIL_SEARCH,
            serde_json::json!({"mailbox": "work", "query": format!("is:unread {index}")}),
        ));
        h.brain.push(Script::reply(&["Done."]));
        send(&h.daemon, pending, "check my mail").await;
        let frames = frames_until_settled(&mut socket).await;
        let fact = tool_fact(&frames, MAIL_SEARCH);
        assert_eq!(fact["payload"]["payload"]["outcome"], "completed");
        assert_eq!(
            fact["payload"]["payload"]["selected_connection"],
            "conn-work"
        );
    }

    assert_eq!(h.tools().len(), 2);
    // The account is bound once and shared by every run that follows.
    assert_eq!(h.builds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_provider_reads_remain_retrievable_and_follow_forget_and_revocation() {
    use pagis_core::{ConversationEvidenceStore, ConversationScope, GrantStore};

    let h = boot(Behavior::Ok, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    let grant_id = grant(&h.daemon, "conn-work", &["gmail_read"]).await;

    for pending in ["evidence-1", "evidence-2"] {
        h.brain.push(Script::tool_call(
            &[],
            MAIL_GET_MESSAGE,
            serde_json::json!({"mailbox": "work", "message_id": "provider-message-1"}),
        ));
        h.brain.push(Script::reply(&["I found the exact code."]));
        send(&h.daemon, pending, "read the provider message").await;
        frames_until_settled(&mut socket).await;
    }
    assert!(h.brain.requests().iter().any(|request| {
        request.messages.iter().any(|message| {
            message
                .conversation_evidence
                .as_ref()
                .is_some_and(|status| {
                    status["status"] == "retained"
                        && status["reference"]
                            .as_str()
                            .is_some_and(|reference| reference.starts_with("tool:"))
                        && status["complete"] == true
                })
        })
    }));
    assert!(h.brain.requests().iter().any(|request| {
        request.messages.iter().any(|message| {
            message.text.contains("violet launch code is 8241")
                && !message.text.contains("Pagis conversation evidence")
        })
    }));

    let workspace_id = workspace_id(&h.daemon).await;
    let scope = ConversationScope {
        workspace_id,
        agent_id: AgentId::from(h.daemon.agent_id.clone()),
        channel_id: pagis_core::ChannelId::from(h.daemon.dm_channel_id.clone()),
        root_message_id: None,
    };
    let restarted =
        pagis_storage_sqlite::SqliteConversationEvidenceStore::new(h.daemon.pool().clone());
    let hits = restarted
        .search(&scope, "violet 8241", None, 10)
        .await
        .unwrap();
    let tool_hits: Vec<_> = hits.iter().filter(|hit| hit.kind == "tool").collect();
    assert_eq!(tool_hits.len(), 2);
    assert!(tool_hits.iter().all(|hit| hit.complete));
    let retained = restarted
        .read_tool(&scope, &tool_hits[0].reference)
        .await
        .unwrap()
        .unwrap();
    assert!(retained.content.contains("violet launch code is 8241"));

    let requests_before_retrieval = h.brain.requests().len();
    h.brain.push(Script::tool_call(
        &[],
        "conversation_search",
        serde_json::json!({"query": "violet 8241"}),
    ));
    h.brain.push(Script::tool_call(
        &[],
        "conversation_read",
        serde_json::json!({"reference": tool_hits[0].reference}),
    ));
    h.brain.push(Script::reply(&["The exact code is 8241."]));
    send(
        &h.daemon,
        "retrieve-evidence",
        "recover the earlier provider detail",
    )
    .await;
    frames_until_settled(&mut socket).await;
    let requests = h.brain.requests();
    let retrieval = &requests[requests_before_retrieval..];
    assert_eq!(retrieval.len(), 3);
    let search_result = &retrieval[1].messages.last().unwrap().text;
    assert!(search_result.starts_with("[BEGIN UNTRUSTED source=conversation_evidence]"));
    assert!(search_result.contains("tool:"));
    let read_result = &retrieval[2].messages.last().unwrap().text;
    assert!(read_result.starts_with("[BEGIN UNTRUSTED source=conversation_evidence]"));
    assert!(read_result.contains("violet launch code is 8241"));

    sqlx::query("INSERT INTO forgotten_messages(message_id) VALUES (?)")
        .bind(tool_hits[0].message_id.as_str())
        .execute(h.daemon.pool())
        .await
        .unwrap();
    let after_forget = restarted
        .search(&scope, "violet 8241", None, 10)
        .await
        .unwrap();
    assert_eq!(
        after_forget.iter().filter(|hit| hit.kind == "tool").count(),
        1
    );
    assert!(
        restarted
            .read_tool(&scope, &tool_hits[0].reference)
            .await
            .unwrap()
            .is_none()
    );
    let retained_after_forget: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_tool_evidence")
            .fetch_one(h.daemon.pool())
            .await
            .unwrap();
    assert_eq!(retained_after_forget, 1);

    pagis_storage_sqlite::SqliteGrantStore::new(h.daemon.pool().clone())
        .revoke(&h.daemon.workspace_id, &grant_id, now_ms())
        .await
        .unwrap();
    assert!(
        restarted
            .search(&scope, "violet 8241", None, 10)
            .await
            .unwrap()
            .iter()
            .all(|hit| hit.kind != "tool")
    );
    assert!(
        restarted
            .read_tool(&scope, &tool_hits[1].reference)
            .await
            .unwrap()
            .is_none()
    );
    let retained_after_revoke: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_tool_evidence")
            .fetch_one(h.daemon.pool())
            .await
            .unwrap();
    assert_eq!(retained_after_revoke, 0);
}

/// Confirm a Forget of `target` through the owner API and wait until
/// its purges complete.
async fn forget(daemon: &TestDaemon, target: serde_json::Value) {
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1/knowledge/forget", daemon.base_url);
    let preview: serde_json::Value = client
        .post(format!("{base}/preview"))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target": target}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = client
        .post(&base)
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target": target, "preview": preview}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let operation: serde_json::Value = response.json().await.unwrap();
    let operation_url = format!("{base}/{}", operation["id"].as_str().unwrap());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current: serde_json::Value = client
                .get(&operation_url)
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(current["error"].is_null(), "purge failed: {current}");
            if current["phase"] == "complete" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Forget completes");
}

/// A Run that reads a synced message records what it read. A Forget of
/// that message forgets the reply of the Run as a whole and deletes the
/// tool result that the Run kept. After the purge no value of the
/// database holds the words of the message, the record of the read is
/// gone, and the Person reads that the reply is unavailable (ADR-0008,
/// ADR-0009).
#[tokio::test]
async fn a_forget_of_a_synced_message_forgets_the_reply_of_the_run_that_read_it() {
    use pagis_core::knowledge::{KnowledgeStore, SourceKey, SyncConfig};

    let h = boot(Behavior::Ok, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    grant(&h.daemon, "conn-work", &["gmail_read"]).await;
    let workspace_id = workspace_id(&h.daemon).await;
    let pool = h.daemon.pool();
    // The Sync holds the message as a Source Item. It is off, so no
    // collector reads the Connection.
    pagis_storage_sqlite::SqliteKnowledgeStore::new(pool.clone())
        .configure(
            SyncConfig {
                workspace_id: workspace_id.clone(),
                connection_id: "conn-work".to_string().into(),
                resource: pagis_broker::MAIL_SYNC_RESOURCE.into(),
                agent_id: AgentId::from(h.daemon.agent_id.clone()),
                required_capability: "gmail_read".into(),
                enabled: false,
                since: 0,
                filter: pagis_google::gmail_filter::default_filter(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO source_items(workspace_id,connection_id,resource,id,version,record,observed_at) \
         VALUES(?,'conn-work','gmail','provider-message-1','1','{\"parent\":\"provider-thread-1\"}',1)",
    )
    .bind(workspace_id.as_str())
    .execute(pool)
    .await
    .unwrap();

    h.brain.push(Script::tool_call(
        &[],
        MAIL_GET_MESSAGE,
        serde_json::json!({"mailbox": "work", "message_id": "provider-message-1"}),
    ));
    h.brain
        .push(Script::reply(&["The violet launch code is 8241."]));
    send(&h.daemon, "read-message", "read the provider message").await;
    frames_until_settled(&mut socket).await;
    let reads: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind, source_id FROM run_source_reads WHERE workspace_id=? AND connection_id='conn-work'",
    )
    .bind(workspace_id.as_str())
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        reads,
        [("item".to_string(), "provider-message-1".to_string())],
        "the Run did not record the message that it read"
    );
    let reply: String = sqlx::query_scalar(
        "SELECT id FROM messages WHERE workspace_id=? AND author_kind='agent' \
         AND text_content LIKE '%8241%'",
    )
    .bind(workspace_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap();

    let source = SourceKey {
        workspace_id: workspace_id.clone(),
        connection_id: "conn-work".to_string().into(),
        resource: pagis_broker::MAIL_SYNC_RESOURCE.into(),
    };
    forget(
        &h.daemon,
        serde_json::json!({"kind": "source", "source": source, "source_id": "provider-message-1"}),
    )
    .await;

    let forgotten: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM forgotten_messages WHERE message_id=?)")
            .bind(&reply)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        forgotten,
        "the reply that quoted the message is not forgotten"
    );
    let shown: serde_json::Value = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/channels/{}/messages/{reply}",
            h.daemon.base_url, h.daemon.dm_channel_id
        ))
        .header("cookie", h.daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        shown["text_content"],
        "This message is unavailable because its source access cannot be verified.",
        "the Person reads the reply that quoted the forgotten message"
    );
    let held: Vec<String> = pagis_testkit::store_suite::Rows::Sqlite(pool.clone())
        .every_value()
        .await
        .unwrap()
        .into_iter()
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .filter(|value| value.contains("8241"))
        .collect();
    assert_eq!(
        held,
        Vec::<String>::new(),
        "a value of the database holds the words of the forgotten message"
    );
    let reads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM run_source_reads")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        reads, 0,
        "the record of the read names the forgotten message"
    );
}

#[tokio::test]
async fn changing_capabilities_replaces_the_account_tools_on_the_next_run() {
    let h = boot(Behavior::Ok, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    let grant_id = grant(&h.daemon, "conn-work", &["gmail_read"]).await;

    h.brain.push(Script::reply(&["Ready."]));
    send(&h.daemon, "p1", "check my mail").await;
    frames_until_settled(&mut socket).await;

    // Reading the user's account installs the three mail reads, and
    // the `mailbox` argument names the alias the Agent may act in.
    let offered = latest_tools(&h, |name| name.starts_with("mail__"));
    assert_eq!(offered, [MAIL_SEARCH, MAIL_GET_MESSAGE, MAIL_GET_THREAD]);
    let search = h
        .brain
        .requests()
        .iter()
        .rev()
        .find_map(|request| {
            request
                .tools
                .iter()
                .find(|tool| tool.name == MAIL_SEARCH)
                .cloned()
        })
        .unwrap();
    assert_eq!(
        search.parameters["properties"]["mailbox"]["enum"],
        serde_json::json!(["work"])
    );

    set_capabilities(&h.daemon, &grant_id, &["calendar_read"]).await;
    h.brain.push(Script::reply(&["Ready."]));
    send(&h.daemon, "p2", "check Calendar").await;
    frames_until_settled(&mut socket).await;

    assert_eq!(
        latest_tools(&h, |name| name.starts_with("google__")),
        ["google__calendar_events"]
    );
    // The Agent holds no mailbox of its own, so no mail tool is left.
    assert!(latest_tools(&h, |name| name.starts_with("mail__")).is_empty());
}

/// The tools of the newest brain request that carried a run's whole
/// snapshot, filtered. A memory pass offers a few tools of its own, so
/// the run's own turn is the one that offers `send_message`.
fn latest_tools(h: &Harness, keep: impl Fn(&str) -> bool + Copy) -> Vec<String> {
    h.brain
        .requests()
        .iter()
        .rev()
        .find(|request| request.tools.iter().any(|tool| tool.name == "send_message"))
        .map(|request| {
            request
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .filter(|name| keep(name))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_request_that_never_left_is_retried_exactly_once() {
    let h = boot(Behavior::RetryableOnce, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    grant(&h.daemon, "conn-work", &["gmail_read"]).await;

    h.brain.push(Script::tool_call(
        &[],
        MAIL_SEARCH,
        serde_json::json!({"mailbox": "work", "query": "is:unread"}),
    ));
    h.brain.push(Script::reply(&["Done."]));
    send(&h.daemon, "p1", "check my mail").await;
    let frames = frames_until_settled(&mut socket).await;

    assert_eq!(
        tool_fact(&frames, MAIL_SEARCH)["payload"]["payload"]["outcome"],
        "completed"
    );
    assert_eq!(h.tools().len(), 2, "one retry, not a loop");
}

#[tokio::test]
async fn a_write_that_runs_out_of_time_reports_an_unknown_outcome() {
    let h = boot(Behavior::Hang, Duration::from_millis(50)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    grant(&h.daemon, "conn-work", &["gmail_send"]).await;

    h.brain.push(Script::tool_call(
        &[],
        MAIL_SEND,
        serde_json::json!({
            "mailbox": "work", "to": ["bob@example.com"], "body": "hi", "subject": "hi"
        }),
    ));
    h.brain.push(Script::reply(&["Done."]));
    send(&h.daemon, "p1", "send it").await;

    let requested = next_frame_of(&mut socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            h.daemon.base_url
        ))
        .header("cookie", h.daemon.cookie())
        .json(&serde_json::json!({"decision": "approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let frames = frames_until_settled(&mut socket).await;
    let fact = tool_fact(&frames, MAIL_SEND);
    assert_eq!(fact["payload"]["payload"]["error_code"], "outcome_unknown");
    // A send that timed out is never sent again on its own.
    assert_eq!(h.tools().len(), 1);
}

#[tokio::test]
async fn revoking_the_grant_while_the_user_decides_blocks_the_call() {
    let h = boot(Behavior::Ok, Duration::from_secs(5)).await;
    let mut socket = firehose(&h.daemon).await;
    connect_google(&h.daemon, "conn-work", "work").await;
    let grant_id = grant(&h.daemon, "conn-work", &["gmail_send"]).await;

    h.brain.push(Script::tool_call(
        &[],
        MAIL_SEND,
        serde_json::json!({
            "mailbox": "work", "to": ["bob@example.com"], "body": "hi", "subject": "hi"
        }),
    ));
    h.brain.push(Script::reply(&["Done."]));
    send(&h.daemon, "p1", "send it").await;

    let requested = next_frame_of(&mut socket, "request.created").await;
    let request_id = requested["payload"]["payload"]["request_id"]
        .as_str()
        .unwrap()
        .to_string();

    reqwest::Client::new()
        .delete(format!("{}/api/v1/grants/{grant_id}", h.daemon.base_url))
        .header("cookie", h.daemon.cookie())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            h.daemon.base_url
        ))
        .header("cookie", h.daemon.cookie())
        .json(&serde_json::json!({"decision": "approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let frames = frames_until_settled(&mut socket).await;
    let fact = tool_fact(&frames, MAIL_SEND);
    assert_eq!(
        fact["payload"]["payload"]["error_code"],
        "permission_revoked"
    );
    assert!(
        h.tools().is_empty(),
        "an approved call still checks authority"
    );
}
