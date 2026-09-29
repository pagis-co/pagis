//! One commit per delivery pass (ADR-0007). A pass that appends several
//! arrivals commits once, so the memory repository does not get one
//! commit for each arrival.

use pagis_agent::{Brain, BrainError, TurnRequest, TurnStream};
use pagis_core::knowledge::{KnowledgeStore, SourceKey, SyncStatus};
use pagis_core::{
    AgentId, AgentStore, Connection, ConnectionId, ConnectionStore, WorkspaceId, now_ms,
};
use pagis_storage_sqlite::{SqliteAgentStore, SqliteConnectionStore};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::json;
use std::sync::Arc;

/// Four historical messages over two mail threads. One acquisition
/// brings them all, so one delivery pass appends all four.
struct Mailbox {
    at: i64,
}

impl Mailbox {
    fn message(&self, id: &str) -> serde_json::Value {
        let (thread, offset) = match id {
            "school-1" => ("school", 0),
            "school-2" => ("school", 1),
            "sports-1" => ("sports", 2),
            _ => ("sports", 3),
        };
        json!({"id":id, "threadId":thread, "historyId":"40",
            "internalDate":(self.at + offset).to_string(), "labelIds":["INBOX"],
            "payload":{"mimeType":"text/plain",
                "headers":[{"name":"From","value":"school@example.com"},
                           {"name":"Subject","value":"Update"}],
                "body":{"data":"U2Nob29sIHVwZGF0ZQ"}}})
    }
}

#[async_trait::async_trait]
impl pagis_google::GogRunner for Mailbox {
    async fn run(
        &self,
        command: &pagis_google::GogCommand,
    ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
        let args = command.args();
        let params = args
            .iter()
            .position(|item| item == "--params")
            .map(|index| serde_json::from_str::<serde_json::Value>(&args[index + 1]).unwrap())
            .unwrap_or_default();
        let value = if args.iter().any(|item| item == "users.getProfile") {
            json!({"historyId":"40"})
        } else if args.iter().any(|item| item == "users.messages.list") {
            json!({"messages":[
                {"id":"school-1","threadId":"school"},
                {"id":"school-2","threadId":"school"},
                {"id":"sports-1","threadId":"sports"},
                {"id":"sports-2","threadId":"sports"}
            ]})
        } else if args.iter().any(|item| item == "users.messages.get") {
            self.message(params["id"].as_str().unwrap())
        } else if args.iter().any(|item| item == "users.threads.get") {
            match params["id"].as_str().unwrap() {
                "school" => {
                    json!({"messages":[self.message("school-1"), self.message("school-2")]})
                }
                _ => json!({"messages":[self.message("sports-1"), self.message("sports-2")]}),
            }
        } else if args.iter().any(|item| item == "users.labels.list") {
            json!({"labels":[{"id":"INBOX"}]})
        } else if args.iter().any(|item| item == "users.history.list") {
            json!({"historyId":"40"})
        } else {
            json!({"messages":[]})
        };
        Ok(pagis_google::ProcessOutput {
            status: Some(0),
            stdout: value.to_string().into_bytes(),
        })
    }
}

/// A brain that records nothing. The Backfill Reflection of the
/// historical pages must not write to memory here.
struct Quiet;

#[async_trait::async_trait]
impl Brain for Quiet {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let scripted = ScriptedBrain::default();
        scripted.push(Script::reply(&["nothing to record"]));
        scripted.turn(request).await
    }
}

/// The messages of the commits of one memory repository that
/// appended arrivals, newest first.
pub fn arrival_commits(dir: &std::path::Path) -> Vec<String> {
    let Ok(repo) = git2::Repository::open(dir) else {
        return Vec::new();
    };
    let mut walk = repo.revwalk().unwrap();
    walk.push_head().unwrap();
    let mut summaries = Vec::new();
    for oid in walk {
        let commit = repo.find_commit(oid.unwrap()).unwrap();
        let summary = commit
            .summary()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        if summary.starts_with("Append") {
            summaries.push(summary);
        }
    }
    summaries
}

struct Fixture {
    daemon: TestDaemon,
    key: SourceKey,
    store: pagis_storage_sqlite::SqliteKnowledgeStore,
}

/// A daemon with the mailbox above and one configured Gmail resource.
async fn fixture(before_sync: impl FnOnce(&TestDaemon, &WorkspaceId)) -> Fixture {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::new(Quiet),
        gog: Some(Arc::new(Mailbox {
            at: now_ms() - 86_400_000,
        })),
        collector_interval: std::time::Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let pool = daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({"account":"user@example.com","client":"test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/api/v1/grants", daemon.base_url))
            .header("cookie", daemon.cookie())
            .json(
                &json!({"agent_id":daemon.agent_id,"connection_id":connection.id,
                "capabilities":["gmail_read"]})
            )
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    before_sync(&daemon, &agent.workspace_id);
    assert_eq!(
        client
            .put(format!(
                "{}/api/v1/connections/{}/sync",
                daemon.base_url, connection.id
            ))
            .header("cookie", daemon.cookie())
            .json(&json!({"agent_id":daemon.agent_id,"enabled":true,"since":0,
                "filter": pagis_google::gmail_filter::default_filter()}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let key = SourceKey {
        workspace_id: agent.workspace_id,
        connection_id: connection.id,
        resource: "gmail".into(),
    };
    Fixture {
        daemon,
        key,
        store: pagis_storage_sqlite::SqliteKnowledgeStore::new(pool),
    }
}

impl Fixture {
    async fn status(&self) -> SyncStatus {
        self.store
            .status(&self.key, now_ms())
            .await
            .unwrap()
            .unwrap()
    }

    /// Wait until the acquisition status answers the question, or
    /// give up and report the status that never did.
    async fn settle(&self, what: &str, done: impl Fn(&SyncStatus) -> bool) {
        let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if done(&self.status().await) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await;
        assert!(settled.is_ok(), "{what}: {:?}", self.status().await);
    }

    fn memory_dir(&self) -> std::path::PathBuf {
        self.daemon
            .booted
            .home
            .join("memory")
            .join(self.key.workspace_id.as_str())
    }

    fn arrival_commits(&self) -> Vec<String> {
        arrival_commits(&self.memory_dir())
    }

    async fn page(&self, name: &str) -> Vec<String> {
        let content = self.page_content(name).await;
        let parsed = pagis_core::subject_page::SubjectPage::parse(&content);
        assert!(parsed.layout_valid, "{:?}", parsed.warnings);
        parsed
            .page
            .timeline
            .into_iter()
            .map(|entry| entry.words)
            .collect()
    }

    async fn page_content(&self, name: &str) -> String {
        let response = reqwest::Client::new()
            .get(format!(
                "{}/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/{name}.md",
                self.daemon.base_url, self.daemon.agent_id
            ))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "the subject page {name} exists");
        let page: serde_json::Value = response.json().await.unwrap();
        page["content"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn one_pass_of_several_arrivals_makes_one_commit_and_acknowledges_them_all() {
    let f = fixture(|_, _| {}).await;

    f.settle("the four arrivals reach their pages", |status| {
        status.arrival_pending == 0 && status.arrival_processed == 4
    })
    .await;

    let commits = f.arrival_commits();
    assert_eq!(
        commits,
        vec!["Append 4 source arrivals to subject pages"],
        "four arrivals over two threads make one commit"
    );
    let status = f.status().await;
    assert_eq!(status.arrival_skipped, 0);
    assert_eq!(status.arrival_error, None);
    assert_eq!(f.page("school").await.len(), 2);
    assert_eq!(f.page("sports").await.len(), 2);
    let summary = pagis_core::memory_page::PageSummary::read(
        "agents/agent/subjects/gmail/school.md",
        &f.page_content("school").await,
    );
    assert_eq!(summary.title, "Update");
    assert_eq!(summary.kind.as_deref(), Some("Source"));
}

#[tokio::test]
async fn a_commit_that_keeps_failing_acknowledges_nothing_and_shows_the_error() {
    // A file where the repository belongs refuses every commit.
    let f = fixture(|daemon, workspace_id| {
        let memory = daemon.booted.home.join("memory");
        std::fs::create_dir_all(&memory).unwrap();
        let repo = memory.join(workspace_id.as_str());
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::write(&repo, "not a repository\n").unwrap();
    })
    .await;

    f.settle("the pass reports the failed commit", |status| {
        status.arrival_error.is_some()
    })
    .await;

    // Several passes run on the same arrivals. None of them
    // acknowledges an arrival, and none of them counts an attempt, so
    // the arrivals wait for a repository that works again.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let status = f.status().await;
    assert_eq!(status.arrival_processed, 0, "nothing is acknowledged");
    assert_eq!(status.arrival_skipped, 0, "no arrival spends an attempt");
    assert_eq!(status.arrival_pending, 4, "every arrival waits");
    assert!(
        status
            .arrival_error
            .as_deref()
            .is_some_and(|error| error.contains("unavailable")),
        "the sync status shows the failure: {:?}",
        status.arrival_error
    );
    assert!(f.arrival_commits().is_empty());
}
