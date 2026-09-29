//! Full-daemon memory test: a fact told to Pixie lands as one
//! git commit at run end, `memory.committed` reaches the firehose,
//! and a fresh run's context carries the updated index.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The next frame of one type, skipping others, within a timeout.
async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("frame is JSON");
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// Authenticate and consume `ready`; the firehose needs no subscription.
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

async fn rest_send(daemon: &TestDaemon, pending_id: &str, text: &str) {
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

#[tokio::test]
async fn a_fact_told_to_pixie_commits_and_a_fresh_run_recalls_it() {
    let brain = Arc::new(ScriptedBrain::default());
    // The reply Run records durable evidence before it answers.
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "shared/MEMORY.md",
            "content": "- [User](user.md) — birthday is 3 May\n",
            "expected_memory_revision": "missing",
        }),
    ));
    brain.push(Script::reply(&["Noted!"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;

    rest_send(&daemon, "p1", "my birthday is 3 May").await;

    let committed = next_frame_of(&mut socket, "memory.committed").await;
    let payload = &committed["payload"]["payload"];
    assert_eq!(payload["files"], serde_json::json!(["shared/MEMORY.md"]));
    assert_eq!(payload["titles"], serde_json::json!(["Shared memory"]));
    assert_eq!(payload["message"], "Update memory");
    assert!(payload["sha"].as_str().is_some_and(|sha| !sha.is_empty()));

    // The repository is on disk under the data directory.
    let memory_dir = daemon.booted.home.join("memory");
    assert!(memory_dir.exists(), "memory repos live under ~/.pagis");

    // Consume the first run's terminal event before triggering the next.
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }

    // A fresh run's system prompt embeds the updated index verbatim.
    brain.push(Script::reply(&["It is 3 May."]));
    rest_send(&daemon, "p2", "when is my birthday?").await;
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = brain.requests();
    let fresh = requests.last().unwrap();
    assert!(
        fresh
            .system
            .contains("- [User](user.md) — birthday is 3 May"),
        "{}",
        fresh.system
    );
}

#[tokio::test]
async fn an_unrepairable_foreground_page_drops_only_that_page_update() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "private/subjects/gmail/thread-1.md",
            "content": "this is not a subject page",
            "expected_memory_revision": "missing",
        }),
    ));
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "private/MEMORY.md",
            "content": "- The other memory update landed.\n",
            "expected_memory_revision": "missing",
        }),
    ));
    brain.push(Script::reply(&["I will remember that."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;

    rest_send(&daemon, "bad-page", "remember this").await;

    let committed = next_frame_of(&mut socket, "memory.committed").await;
    assert_eq!(
        committed["payload"]["payload"]["files"],
        serde_json::json!(["private/MEMORY.md"])
    );
    let index = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=MEMORY.md",
            daemon.agent_id
        ),
    )
    .await;
    assert_eq!(index.status(), 200);
    let missing = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/thread-1.md",
            daemon.agent_id
        ),
    )
    .await;
    assert_eq!(missing.status(), 404);
}

async fn api_get(daemon: &TestDaemon, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn api_post(
    daemon: &TestDaemon,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie());
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.unwrap()
}

/// Run one scripted turn that stages a memory write, and return the
/// commit sha from the `memory.committed` firehose event.
async fn run_committing(
    daemon: &TestDaemon,
    brain: &ScriptedBrain,
    socket: &mut Socket,
    pending_id: &str,
    path: &str,
    content: &str,
    _summary: &str,
) -> String {
    let feed: serde_json::Value = api_get(daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    let revision = feed["revision"].as_str().unwrap_or("missing");
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({ "path": path, "content": content, "expected_memory_revision": revision }),
    ));
    brain.push(Script::reply(&["Done."]));
    rest_send(daemon, pending_id, "remember").await;
    let committed = next_frame_of(socket, "memory.committed").await;
    loop {
        let frame = next_frame_of(socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    committed["payload"]["payload"]["sha"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_feed_lists_commits_and_revert_makes_an_inverse_entry() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    let sha = run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "shared/MEMORY.md",
        "- [User](user.md) — likes tea\n",
        "Remembered the tea preference",
    )
    .await;

    // The feed serves the foreground entry with the agent's name and the
    // title of each page. Its message is the foreground fallback, which
    // names no file; the later Reflection sentence cannot rename an
    // earlier foreground write.
    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    let items = feed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "committed");
    assert_eq!(items[0]["sha"], sha.as_str());
    assert_eq!(items[0]["agent_name"], "Pixie");
    assert_eq!(items[0]["message"], "Update memory");
    assert_eq!(items[0]["files"], serde_json::json!(["shared/MEMORY.md"]));
    assert_eq!(items[0]["titles"], serde_json::json!(["Shared memory"]));
    assert!(items[0]["run_id"].as_str().is_some());
    assert!(items[0]["message_id"].as_str().is_some());

    // One-tap revert: inverse commit, firehose event, feed entry.
    let response = api_post(
        &daemon,
        &format!("/api/v1/memory/commits/{sha}/revert"),
        Some(serde_json::json!({"expected_revision": feed["revision"]})),
    )
    .await;
    assert_eq!(response.status(), 200);
    let revert: serde_json::Value = response.json().await.unwrap();
    let revert_sha = revert["sha"].as_str().unwrap();

    let reverted = next_frame_of(&mut socket, "memory.reverted").await;
    assert_eq!(reverted["payload"]["payload"]["reverted_sha"], sha.as_str());
    assert_eq!(reverted["payload"]["payload"]["sha"], revert_sha);

    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    let items = feed["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "reverted");
    assert_eq!(items[0]["sha"], revert_sha);
    assert_eq!(items[0]["reverted_sha"], sha.as_str());
    assert!(items[0]["agent_name"].is_null(), "reverts are the user's");

    // The revert removed the file again.
    let missing = api_get(&daemon, "/api/v1/memory/file?scope=shared&path=MEMORY.md").await;
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn a_conflicting_revert_returns_revert_conflict_with_guidance() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    let first = run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "shared/fact.md",
        "v1\n",
        "first",
    )
    .await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p2",
        "shared/fact.md",
        "v2\n",
        "second",
    )
    .await;

    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();

    let response = api_post(
        &daemon,
        &format!("/api/v1/memory/commits/{first}/revert"),
        Some(serde_json::json!({"expected_revision": feed["revision"]})),
    )
    .await;

    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "revert_conflict");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("later changes touch the same files"),
        "{body}"
    );
}

#[tokio::test]
async fn feed_files_open_read_only_and_bad_scopes_are_rejected() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "private/craft.md",
        "private notes\n",
        "noted",
    )
    .await;

    let file: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=craft.md",
            daemon.agent_id
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(file["content"], "private notes\n");

    let bad_scope = api_get(&daemon, "/api/v1/memory/file?scope=everything&path=x.md").await;
    assert_eq!(bad_scope.status(), 422);

    let escape = api_get(&daemon, "/api/v1/memory/file?scope=shared&path=../secret").await;
    assert_eq!(escape.status(), 422);

    let missing = api_get(&daemon, "/api/v1/memory/file?scope=shared&path=nope.md").await;
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn onboarding_seeds_the_user_name_into_shared_memory() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;

    let (status, key) = daemon
        .set_up_provider(
            "anthropic",
            "key",
            serde_json::json!({ "api_key": "sk-test" }),
        )
        .await;
    assert_eq!(status, 200, "{key}");

    let response = api_post(
        &daemon,
        "/api/v1/settings/onboarding/complete",
        Some(serde_json::json!({ "user_name": "Ada" })),
    )
    .await;
    assert_eq!(response.status(), 204);

    // The seed is a user commit and the feed's first entry.
    let committed = next_frame_of(&mut socket, "memory.committed").await;
    assert_eq!(
        committed["payload"]["payload"]["message"],
        "Onboarding: recorded the user's name"
    );
    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(feed["items"][0]["kind"], "committed");
    assert!(feed["items"][0]["run_id"].is_null());
    assert!(feed["items"][0]["agent_name"].is_null());

    let file: serde_json::Value = api_get(&daemon, "/api/v1/memory/file?scope=shared&path=user.md")
        .await
        .json()
        .await
        .unwrap();
    assert!(file["content"].as_str().unwrap().contains("Ada"));

    // Every agent's next run carries the name via the shared index.
    brain.push(Script::reply(&["Hi Ada!"]));
    rest_send(&daemon, "p1", "hello").await;
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = brain.requests();
    assert!(
        requests.last().unwrap().system.contains("Ada"),
        "{}",
        requests.last().unwrap().system
    );
}

#[tokio::test]
async fn memory_controls_hide_revoked_sources_and_reject_restore() {
    use pagis_core::{
        AgentId, AgentStore, Grant, GrantId, GrantStore, MemoryAccess, MemoryAuthor,
        MemoryChangeset, MemoryExposure, MemoryStore, ScopedPath,
    };
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let workspace_id = pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &agent_id)
        .await
        .unwrap()
        .unwrap()
        .workspace_id;
    let grants = pagis_storage_sqlite::SqliteGrantStore::new(daemon.pool().clone());
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        resource_kind: Grant::HOST_KIND.into(),
        resource_id: None,
        scope: Grant::allow_scope(&["cat".into()]),
        revision: 1,
        created_at: pagis_core::now_ms(),
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    let memory = pagis_memory::GitMemoryStore::new(
        daemon.booted.home.join("memory"),
        std::sync::Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(
            daemon.pool().clone(),
        )),
        std::sync::Arc::new(pagis_storage_sqlite::SqliteMemoryPageIndex::new(
            daemon.pool().clone(),
        )),
    );
    let access = MemoryAccess::agent(
        agent_id.clone(),
        [MemoryExposure {
            grant_id: grant.id.clone(),
            revision: 1,
        }],
    );
    let author = MemoryAuthor {
        name: "Pixie".into(),
        email: "pixie@pagis.local".into(),
    };
    let path = ScopedPath::parse("shared/source.md").unwrap();
    let mut changes = MemoryChangeset::default();
    changes
        .writes
        .insert(path.clone(), "private source text".into());
    let first = memory
        .commit(
            &workspace_id,
            &agent_id,
            &access,
            &author,
            &changes,
            None,
            "source view",
            None,
        )
        .await
        .unwrap();
    use pagis_core::EventLog;
    pagis_storage_sqlite::SqliteEventLog::new(daemon.pool().clone())
        .append(pagis_core::NewEvent {
            workspace_id: workspace_id.clone(),
            event_type: "memory.committed".into(),
            agent_id: Some(agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "sha": first, "source_scoped": true,
                "message": "private source text", "files": ["shared/source.md"],
                "message_id": "reply-1",
                "exposures": [{"grant_id": grant.id.as_str(), "revision": 1}],
            }),
        })
        .await
        .unwrap();
    let url = "/api/v1/memory/file?scope=shared&path=source.md";
    assert_eq!(api_get(&daemon, url).await.status(), 200);
    // While the grant is live, the feed shows the sentence Reflection wrote.
    let live: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(live["items"][0]["message"], "private source text");
    assert_eq!(live["items"][0]["message_id"], "reply-1");
    grants
        .revoke(&grant.workspace_id, &grant.id, pagis_core::now_ms())
        .await
        .unwrap();
    let hidden = api_get(&daemon, url).await;
    assert_eq!(hidden.status(), 404);
    assert!(!hidden.text().await.unwrap().contains("private source text"));
    let restored = api_post(
        &daemon,
        &format!("/api/v1/memory/commits/{first}/revert"),
        Some(serde_json::json!({"expected_revision": first})),
    )
    .await;
    assert_eq!(restored.status(), 404);
    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    assert!(!feed.to_string().contains("private source text"));
    assert_eq!(
        feed["items"][0]["message"],
        "Updated a source-scoped memory view."
    );
    assert_eq!(feed["items"][0]["sha"], first);
    assert_eq!(feed["items"][0]["source_scoped"], true);
    assert_eq!(feed["revision"], first);
}

#[tokio::test]
async fn the_pages_of_a_scope_carry_the_title_kind_and_last_change() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "shared/priya.md",
        "---\ntitle: Priya Sharma\nkind: Person\n---\n\nShe leads the renewal.\n",
        "Remembered Priya",
    )
    .await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p2",
        "private/craft.md",
        "---\ntitle: How I write\nkind: Procedure\n---\n\nSteps.\n",
        "Noted the craft",
    )
    .await;

    // The shared scope lists its own file, newest change first.
    let shared: serde_json::Value = api_get(&daemon, "/api/v1/memory/pages?scope=shared")
        .await
        .json()
        .await
        .unwrap();
    let pages = shared["pages"].as_array().unwrap();
    let priya = pages
        .iter()
        .find(|page| page["path"] == "priya.md")
        .expect("the shared scope lists priya.md");
    assert_eq!(priya["scope"], "shared");
    assert_eq!(priya["title"], "Priya Sharma");
    assert_eq!(priya["kind"], "Person");
    assert_eq!(priya["changed_by_agent_id"], daemon.agent_id.as_str());
    assert!(priya["changed_at"].as_i64().unwrap() > 0);
    assert!(
        !pages.iter().any(|page| page["path"] == "craft.md"),
        "the shared scope hides private files"
    );

    // The private scope reads only through the API, per agent.
    let private: serde_json::Value = api_get(
        &daemon,
        &format!("/api/v1/memory/pages?scope=agent:{}", daemon.agent_id),
    )
    .await
    .json()
    .await
    .unwrap();
    let craft = private["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|page| page["path"] == "craft.md")
        .expect("the private scope lists craft.md");
    assert_eq!(craft["scope"], format!("agent:{}", daemon.agent_id));
    assert_eq!(craft["title"], "How I write");
    assert_eq!(craft["kind"], "Procedure");
    assert!(craft["source_connection_id"].is_null());

    // Another agent's private scope is empty, not another agent's files.
    let other: serde_json::Value = api_get(&daemon, "/api/v1/memory/pages?scope=agent:nobody")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(other["pages"].as_array().unwrap().len(), 0);

    let bad_scope = api_get(&daemon, "/api/v1/memory/pages?scope=everything").await;
    assert_eq!(bad_scope.status(), 422);

    // A list comes in parts. Each part names the full count and the
    // cursor of the next part.
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p3",
        "shared/acme.md",
        "---\ntitle: Acme\nkind: Company\n---\n\nA customer.\n",
        "Remembered Acme",
    )
    .await;
    let first: serde_json::Value = api_get(&daemon, "/api/v1/memory/pages?scope=shared&limit=1")
        .await
        .json()
        .await
        .unwrap();
    let total = first["total"].as_u64().unwrap();
    assert!(total >= 2);
    assert_eq!(first["pages"].as_array().unwrap().len(), 1);
    assert_eq!(
        first["pages"][0]["path"], "acme.md",
        "the newest change is first"
    );
    let cursor = first["next"].as_str().expect("a next part");
    let second: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/pages?scope=shared&limit=1&after={}",
            cursor.replace(':', "%3A").replace('/', "%2F")
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(second["total"], total);
    assert_ne!(second["pages"][0]["path"], "acme.md");

    // The search and the kind narrow the list on the server.
    let found: serde_json::Value = api_get(&daemon, "/api/v1/memory/pages?scope=shared&q=PRIYA")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(found["total"], 1);
    assert_eq!(found["pages"][0]["path"], "priya.md");
    assert!(found["next"].is_null());
    let procedures: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/pages?scope=agent:{}&kind=Procedure",
            daemon.agent_id
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(procedures["total"], 1);
    assert_eq!(procedures["pages"][0]["path"], "craft.md");

    let bad_cursor = api_get(&daemon, "/api/v1/memory/pages?scope=shared&after=nonsense").await;
    assert_eq!(bad_cursor.status(), 422);

    // The counts of a scope, for the Agent profile.
    let counts: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/pages/counts?scope=agent:{}",
            daemon.agent_id
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(counts["pages"].as_u64().unwrap() >= 1);
    assert_eq!(counts["procedures"], 1);
    let shared_counts: serde_json::Value =
        api_get(&daemon, "/api/v1/memory/pages/counts?scope=shared")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(shared_counts["pages"], total);
    let author = shared_counts["authors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|author| author["agent_id"] == daemon.agent_id.as_str())
        .expect("the agent wrote shared pages");
    assert!(author["pages"].as_u64().unwrap() >= 2);

    // The file names its own title and kind, so the page view does not
    // need the list.
    let file: serde_json::Value =
        api_get(&daemon, "/api/v1/memory/file?scope=shared&path=priya.md")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(file["title"], "Priya Sharma");
    assert_eq!(file["kind"], "Person");
}

#[tokio::test]
async fn a_commit_diff_gives_the_lines_before_and_after() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "shared/user.md",
        "The user likes tea.\n",
        "Remembered the tea preference",
    )
    .await;
    let second = run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p2",
        "shared/user.md",
        "The user likes coffee.\n",
        "Corrected the drink",
    )
    .await;

    let response = api_get(&daemon, &format!("/api/v1/memory/commits/{second}/diff")).await;
    assert_eq!(response.status(), 200);
    let diff: serde_json::Value = response.json().await.unwrap();
    assert_eq!(diff["sha"], second.as_str());
    assert_eq!(diff["message"], "Update memory");
    assert!(diff["committed_at"].as_i64().unwrap() > 0);
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["scope"], "shared");
    assert_eq!(files[0]["path"], "user.md");
    let hunks = files[0]["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(
        hunks[0]["old_lines"],
        serde_json::json!(["The user likes tea."])
    );
    assert_eq!(
        hunks[0]["new_lines"],
        serde_json::json!(["The user likes coffee."])
    );
    assert_eq!(hunks[0]["old_start"], 1);
    assert_eq!(hunks[0]["new_start"], 1);

    let missing = api_get(
        &daemon,
        "/api/v1/memory/commits/0000000000000000000000000000000000000000/diff",
    )
    .await;
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn the_feed_filters_by_scope_path_and_source_kind() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "shared/user.md",
        "The user likes tea.\n",
        "Remembered the tea preference",
    )
    .await;
    run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p2",
        "private/craft.md",
        "Private notes.\n",
        "Noted the craft",
    )
    .await;

    let items = |feed: &serde_json::Value| feed["items"].as_array().unwrap().len();
    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(items(&feed), 2);
    // A conversation Run is a `thread` source.
    assert_eq!(feed["items"][0]["source_kind"], "thread");

    let shared: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed?scope=shared")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(items(&shared), 1);
    assert_eq!(
        shared["items"][0]["files"],
        serde_json::json!(["shared/user.md"])
    );

    let private: serde_json::Value = api_get(
        &daemon,
        &format!("/api/v1/memory/feed?scope=agent:{}", daemon.agent_id),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(items(&private), 1);
    assert_eq!(
        private["items"][0]["files"],
        serde_json::json!(["private/craft.md"])
    );

    let by_path: serde_json::Value =
        api_get(&daemon, "/api/v1/memory/feed?scope=shared&path=user.md")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(items(&by_path), 1);

    let other_path: serde_json::Value =
        api_get(&daemon, "/api/v1/memory/feed?scope=shared&path=nope.md")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(items(&other_path), 0);

    let threads: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed?kind=thread")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(items(&threads), 2);

    let reverts: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed?kind=revert")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(items(&reverts), 0);

    let bad_kind = api_get(&daemon, "/api/v1/memory/feed?kind=guessing").await;
    assert_eq!(bad_kind.status(), 422);
}

#[tokio::test]
async fn a_revert_lists_under_the_scope_and_files_of_the_commit_it_undoes() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let mut socket = firehose(&daemon).await;
    let sha = run_committing(
        &daemon,
        &brain,
        &mut socket,
        "p1",
        "private/craft.md",
        "Private notes.\n",
        "Noted the craft",
    )
    .await;
    let feed: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed")
        .await
        .json()
        .await
        .unwrap();
    let response = api_post(
        &daemon,
        &format!("/api/v1/memory/commits/{sha}/revert"),
        Some(serde_json::json!({"expected_revision": feed["revision"]})),
    )
    .await;
    assert_eq!(response.status(), 200);

    // The Changes view of the Agent's scope lists the revert.
    let reverts: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/feed?scope=agent:{}&kind=revert",
            daemon.agent_id
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    let items = reverts["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{reverts}");
    assert_eq!(items[0]["kind"], "reverted");
    assert_eq!(items[0]["reverted_sha"], sha.as_str());
    assert_eq!(items[0]["files"], serde_json::json!(["private/craft.md"]));
    assert!(items[0]["agent_name"].is_null(), "reverts are the user's");

    let by_path: serde_json::Value = api_get(
        &daemon,
        &format!(
            "/api/v1/memory/feed?scope=agent:{}&path=craft.md",
            daemon.agent_id
        ),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(by_path["items"].as_array().unwrap().len(), 2, "{by_path}");

    let shared: serde_json::Value = api_get(&daemon, "/api/v1/memory/feed?scope=shared")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(shared["items"].as_array().unwrap().len(), 0, "{shared}");
}

/// Every tenant's Memory Search is built at boot, not only the first
/// Workspace's.
///
/// The Page Index is derived from the memory repository. When the index
/// holds nothing and the repository holds pages, the boot builds the
/// index for every tenant, so a second person's search finds their pages
/// before they write or list a page.
#[tokio::test]
async fn a_second_workspaces_memory_search_is_populated_after_a_boot() {
    let daemon = TestDaemon::start().await;
    // A second person of the same Org, with a Workspace of their own.
    let org_id = pagis_core::UserStore::get(daemon.stores().users.as_ref(), &daemon.user_id)
        .await
        .expect("read the seeded person")
        .expect("the boot seeds one person")
        .org_id;
    let person_b = pagis_core::User {
        email: Some("bo@example.com".to_string()),
        name: Some("Bo".to_string()),
        ..pagis_core::User::new(org_id, pagis_core::UserRole::Member, pagis_core::now_ms())
    };
    daemon
        .stores()
        .users
        .create(&person_b)
        .await
        .expect("write person B");
    let workspace_b = pagis_server::provisioning::WorkspaceSeed::from(daemon.stores())
        .run(
            &person_b.id,
            "B's Workspace",
            "UTC",
            pagis_server::provisioning::Onboarding::Done,
            pagis_core::now_ms(),
        )
        .await
        .expect("seed B's Workspace")
        .id;
    let agent_b = pagis_core::AgentId::generate();
    {
        let memory = pagis_memory::GitMemoryStore::new(
            daemon.booted.home.join("memory"),
            std::sync::Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(
                daemon.pool().clone(),
            )),
            std::sync::Arc::new(pagis_storage_sqlite::SqliteMemoryPageIndex::new(
                daemon.pool().clone(),
            )),
        );
        use pagis_core::MemoryStore as _;
        let path = pagis_core::ScopedPath::parse("shared/harbour.md").unwrap();
        let mut changes = pagis_core::MemoryChangeset::default();
        changes.writes.insert(
            path,
            "# Harbour\n\nThe harbour master answers on channel 12.\n".into(),
        );
        memory
            .commit(
                &workspace_b,
                &agent_b,
                &pagis_core::MemoryAccess::Owner {
                    agent_id: agent_b.clone(),
                },
                &pagis_core::MemoryAuthor {
                    name: "Bo".into(),
                    email: "bo@pagis.local".into(),
                },
                &changes,
                None,
                "the harbour page",
                None,
            )
            .await
            .expect("B writes a page");

        // The repository holds the page and the index holds nothing.
        for statement in [
            "DELETE FROM memory_page_search",
            "DELETE FROM memory_page_link",
            "DELETE FROM memory_page_index",
            "DELETE FROM memory_page_index_heads",
        ] {
            sqlx::query(statement)
                .execute(daemon.pool())
                .await
                .expect("clear the page index");
        }
        assert!(
            pagis_core::MemoryPageIndex::head(
                &pagis_storage_sqlite::SqliteMemoryPageIndex::new(daemon.pool().clone()),
                &workspace_b,
            )
            .await
            .expect("read the head")
            .is_none()
        );
    }

    // A boot on the same state directory.
    let home = daemon.stop().await;
    let daemon =
        pagis_testkit::TestDaemon::start_on(home, pagis_testkit::TestDaemonOptions::default())
            .await;

    let index = pagis_storage_sqlite::SqliteMemoryPageIndex::new(daemon.pool().clone());
    let mut hits = Vec::new();
    for _ in 0..200 {
        hits = pagis_core::MemoryPageIndex::search(&index, &workspace_b, "shared", "harbour", 10)
            .await
            .expect("search B's pages");
        if !hits.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(
        hits.len(),
        1,
        "the boot did not build the second Workspace's Memory Search"
    );
    assert_eq!(hits[0].page.path, "shared/harbour.md");
}
