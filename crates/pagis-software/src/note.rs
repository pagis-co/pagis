//! The software notes an Agent keeps in its own memory.
//!
//! A publish, a fork, a contribution and its close are facts the
//! daemon holds, so the daemon writes them. Each one appends a line to
//! `software.md` in the acting Agent's private memory as a
//! daemon-authored commit, which puts it in the learning feed and in
//! `memory_read` on the next run. What the Agent *judges* about the
//! work comes from reflection, not from here.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use pagis_core::{
    AgentId, EventBus, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryError, MemoryStore,
    NewEvent, RunId, ScopedPath, WorkspaceId, now_ms,
};

/// The file every software note lands in.
pub const SOFTWARE_NOTE_FILE: &str = "private/software.md";

/// The heading a new note file starts with.
const SOFTWARE_NOTE_HEADER: &str = "# Software\n\n";

/// Who a software note is committed as. The daemon writes the fact, so
/// the commit is not the Agent's own words.
fn daemon_author() -> MemoryAuthor {
    MemoryAuthor {
        name: "Pagis".to_string(),
        email: "daemon@pagis.local".to_string(),
    }
}

/// Where a software action is written down. The daemon fills it with
/// [`MemoryNotes`]; a test fills it with a recorder.
#[async_trait]
pub trait SoftwareNotes: Send + Sync {
    /// Append one line to the Agent's software notes. It never fails
    /// the action it describes: a caller logs the problem and goes on.
    ///
    /// The Workspace comes from the caller: a note is written into
    /// the memory of the tenant whose Agent acted.
    async fn record(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        line: &str,
    ) -> Result<(), String>;
}

/// The notes as memory commits. One instance serves every tenant:
/// the Workspace is an argument of the write, so no note can land in
/// another person's memory.
pub struct MemoryNotes {
    memory: Arc<dyn MemoryStore>,
    bus: Arc<dyn EventBus>,
}

impl MemoryNotes {
    pub fn new(memory: Arc<dyn MemoryStore>, bus: Arc<dyn EventBus>) -> Self {
        Self { memory, bus }
    }
}

#[async_trait]
impl SoftwareNotes for MemoryNotes {
    async fn record(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        line: &str,
    ) -> Result<(), String> {
        let path = ScopedPath::parse(SOFTWARE_NOTE_FILE).expect("the note path is valid");
        let access = MemoryAccess::Owner {
            agent_id: agent_id.clone(),
        };
        let (held, revision) = match self.memory.read(workspace_id, &access, &path).await {
            Ok(held) => (held.content, Some(held.revision)),
            Err(MemoryError::NotFound(_)) => (SOFTWARE_NOTE_HEADER.to_string(), None),
            Err(error) => return Err(error.to_string()),
        };
        let dated = dated_line(now_ms(), line);
        let mut changeset = MemoryChangeset::default();
        changeset.writes.insert(path, format!("{held}{dated}\n"));
        let sha = self
            .memory
            .commit(
                workspace_id,
                agent_id,
                &access,
                &daemon_author(),
                &changeset,
                revision.as_deref(),
                &dated,
                Some(run_id),
            )
            .await
            .map_err(|error| error.to_string())?;
        self.bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "memory.committed".to_string(),
                agent_id: Some(agent_id.clone()),
                run_id: Some(run_id.clone()),
                channel_id: None,
                payload: serde_json::json!({
                    "sha": sha,
                    "scopes": changeset.scopes(),
                    "files": changeset.files(),
                    "titles": changeset.titles(),
                    "message": dated,
                    "run_id": run_id.as_str(),
                    "source_scoped": false,
                }),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// One note line with the day it happened in front of it.
fn dated_line(now: i64, line: &str) -> String {
    let day = Utc
        .timestamp_millis_opt(now)
        .single()
        .map(|stamp| stamp.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    format!("{day} {line}")
}

/// What a publish writes down.
pub fn publish_note(package: &str, version: &str, notes: &str) -> String {
    match notes.trim() {
        "" => format!("published {package} {version}"),
        notes => format!("published {package} {version}: {notes}"),
    }
}

/// What a fork writes down.
pub fn fork_note(source: &str, version: &str, new_name: &str) -> String {
    format!("forked {source} as {new_name} from {version}")
}

/// What an opened Contribution writes down.
pub fn contribute_note(id: &str, package: &str, summary: &str) -> String {
    match summary.trim() {
        "" => format!("contribution {id} to {package} opened"),
        summary => format!("contribution {id} to {package} opened: {summary}"),
    }
}

/// What a closed Contribution writes down. A merge names the Version
/// that carries it, because a merge is proved by a publish.
pub fn close_note(id: &str, package: &str, merged_version: Option<&str>, reason: &str) -> String {
    let outcome = match merged_version {
        Some(version) => format!("merged in {version}"),
        None => "declined".to_string(),
    };
    match reason.trim() {
        "" => format!("contribution {id} to {package} {outcome}"),
        reason => format!("contribution {id} to {package} {outcome}: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use pagis_core::{
        Event, EventScope, EventStream, MemoryFile, MemoryIndexes, StoreError, UnixMillis,
    };

    use super::*;

    #[derive(Default)]
    struct FakeMemory {
        files: Mutex<Vec<(String, String)>>,
        commits: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl MemoryStore for FakeMemory {
        async fn preview_forget(
            &self,
            _: &pagis_core::WorkspaceId,
            _: &pagis_core::ForgetTarget,
        ) -> Result<pagis_core::ForgetPreview, pagis_core::MemoryError> {
            unreachable!()
        }
        async fn begin_forget(
            &self,
            _: &pagis_core::WorkspaceId,
            _: &pagis_core::ForgetTarget,
            _: &pagis_core::ForgetPreview,
            _: i64,
            _: &dyn pagis_core::ForgetKeys,
        ) -> Result<pagis_core::ForgetOperation, pagis_core::MemoryError> {
            unreachable!()
        }
        async fn purge_forgotten(
            &self,
            _: &pagis_core::WorkspaceId,
            _: &str,
        ) -> Result<(), pagis_core::MemoryError> {
            unreachable!()
        }
        async fn load_indexes(
            &self,
            _workspace_id: &WorkspaceId,
            _access: &MemoryAccess,
        ) -> Result<MemoryIndexes, MemoryError> {
            Ok(MemoryIndexes {
                shared: String::new(),
                private: String::new(),
                revision: None,
            })
        }

        async fn read(
            &self,
            _workspace_id: &WorkspaceId,
            _access: &MemoryAccess,
            path: &ScopedPath,
        ) -> Result<MemoryFile, MemoryError> {
            self.files
                .lock()
                .expect("files")
                .iter()
                .find(|(held, _)| held == &path.display())
                .map(|(_, content)| MemoryFile {
                    content: content.clone(),
                    revision: "current".into(),
                    exposures: vec![],
                    kind: pagis_core::MemoryContentKind::Procedure,
                })
                .ok_or_else(|| MemoryError::NotFound(path.display()))
        }

        async fn brief_entries(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: &str,
        ) -> Result<pagis_core::BriefEntries, MemoryError> {
            Ok(pagis_core::BriefEntries {
                revision: None,
                entries: vec![],
            })
        }

        async fn list_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
            _: &pagis_core::PageListQuery,
        ) -> Result<pagis_core::MemoryPageList, MemoryError> {
            Ok(pagis_core::MemoryPageList {
                pages: vec![],
                next: None,
                total: 0,
            })
        }
        async fn search_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
            _: &str,
            _: usize,
        ) -> Result<Vec<pagis_core::MemorySearchHit>, MemoryError> {
            Ok(Vec::new())
        }
        async fn count_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
        ) -> Result<pagis_core::MemoryPageCounts, MemoryError> {
            Ok(pagis_core::MemoryPageCounts::default())
        }

        async fn commit_diff(
            &self,
            _: &WorkspaceId,
            commit_id: &str,
            _: &MemoryAccess,
        ) -> Result<pagis_core::MemoryCommitDiff, MemoryError> {
            Err(MemoryError::NotFound(format!("commit {commit_id}")))
        }

        async fn commit(
            &self,
            _workspace_id: &WorkspaceId,
            agent_id: &AgentId,
            _access: &MemoryAccess,
            author: &MemoryAuthor,
            changeset: &MemoryChangeset,
            _expected_revision: Option<&str>,
            message: &str,
            _run_id: Option<&RunId>,
        ) -> Result<String, MemoryError> {
            let mut files = self.files.lock().expect("files");
            for (path, content) in &changeset.writes {
                files.retain(|(held, _)| held != &path.display());
                files.push((path.display(), content.clone()));
            }
            self.commits.lock().expect("commits").push((
                agent_id.to_string(),
                author.name.clone(),
                message.to_string(),
            ));
            Ok(format!(
                "sha{}",
                self.commits.lock().expect("commits").len()
            ))
        }

        async fn revert(
            &self,
            _workspace_id: &WorkspaceId,
            _commit_id: &str,
            _access: &MemoryAccess,
            _expected_revision: &str,
            _author: &MemoryAuthor,
        ) -> Result<String, MemoryError> {
            unimplemented!("a note never reverts")
        }
    }

    #[derive(Default)]
    struct FakeBus {
        published: Mutex<Vec<NewEvent>>,
    }

    #[async_trait]
    impl EventBus for FakeBus {
        async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
            let stored = Event {
                id: pagis_core::EventId::generate(),
                seq: self.published.lock().expect("events").len() as i64 + 1,
                workspace_id: event.workspace_id.clone(),
                event_type: event.event_type.clone(),
                agent_id: event.agent_id.clone(),
                run_id: event.run_id.clone(),
                channel_id: event.channel_id.clone(),
                payload: event.payload.clone(),
                created_at: now_ms() as UnixMillis,
            };
            self.published.lock().expect("events").push(event);
            Ok(stored)
        }

        async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
            unimplemented!("a note never subscribes")
        }
    }

    fn notes() -> (MemoryNotes, Arc<FakeMemory>, Arc<FakeBus>) {
        let memory = Arc::new(FakeMemory::default());
        let bus = Arc::new(FakeBus::default());
        (
            MemoryNotes::new(Arc::clone(&memory) as _, Arc::clone(&bus) as _),
            memory,
            bus,
        )
    }

    /// The tenant these bodies write as. The note takes the Workspace per
    /// call.
    fn workspace() -> WorkspaceId {
        WorkspaceId::from("wsp_1".to_string())
    }

    fn agent() -> AgentId {
        AgentId::from("agt_1".to_string())
    }

    fn run() -> RunId {
        RunId::from("run_1".to_string())
    }

    #[tokio::test]
    async fn the_first_note_starts_the_file_and_commits_as_the_daemon() {
        let (notes, memory, bus) = notes();

        notes
            .record(&workspace(), &agent(), &run(), "published weather v1")
            .await
            .expect("the note lands");

        let files = memory.files.lock().expect("files");
        let (path, content) = files.first().expect("one file");
        assert_eq!(path, SOFTWARE_NOTE_FILE);
        assert!(content.starts_with(SOFTWARE_NOTE_HEADER));
        assert!(content.ends_with("published weather v1\n"), "{content}");
        let commits = memory.commits.lock().expect("commits");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0, "agt_1");
        assert_eq!(commits[0].1, "Pagis", "the daemon writes the fact");
        let published = bus.published.lock().expect("events");
        assert_eq!(published.len(), 1, "the learning feed reads the commit");
        assert_eq!(published[0].event_type, "memory.committed");
        assert_eq!(published[0].payload["files"][0], SOFTWARE_NOTE_FILE);
    }

    #[tokio::test]
    async fn a_later_note_keeps_the_one_before_it() {
        let (notes, memory, _bus) = notes();

        notes
            .record(&workspace(), &agent(), &run(), "published weather v1")
            .await
            .expect("the first note");
        notes
            .record(&workspace(), &agent(), &run(), "published weather v2")
            .await
            .expect("the second note");

        let files = memory.files.lock().expect("files");
        let content = &files.first().expect("one file").1;
        assert!(content.contains("published weather v1\n"));
        assert!(content.contains("published weather v2\n"));
        assert_eq!(files.len(), 1, "one file holds every note");
    }

    #[test]
    fn a_note_line_carries_the_day_it_happened() {
        assert_eq!(
            dated_line(1_756_857_600_000, "published weather v4"),
            "2025-09-03 published weather v4"
        );
    }

    #[test]
    fn each_action_has_its_own_line() {
        assert_eq!(
            publish_note("weather", "v4", "hourly forecast"),
            "published weather v4: hourly forecast"
        );
        assert_eq!(publish_note("weather", "v4", "  "), "published weather v4");
        assert_eq!(
            fork_note("weather", "v3", "weather-ada"),
            "forked weather as weather-ada from v3"
        );
        assert_eq!(
            contribute_note("c_12", "weather", "fix the wind unit"),
            "contribution c_12 to weather opened: fix the wind unit"
        );
        assert_eq!(
            close_note("c_12", "weather", Some("v5"), "good catch"),
            "contribution c_12 to weather merged in v5: good catch"
        );
        assert_eq!(
            close_note("c_12", "weather", None, "out of scope"),
            "contribution c_12 to weather declined: out of scope"
        );
    }
}
