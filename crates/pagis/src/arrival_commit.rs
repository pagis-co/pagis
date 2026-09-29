//! One memory commit for the appends of one delivery pass
//! (ADR-0007).
//!
//! Acquisition delivers up to 100 arrivals per pass. The pass stages
//! every append into one changeset and commits it once, so the
//! repository gets one commit per pass and not one commit per arrival.

use pagis_core::subject_page::{FrontMatter, SubjectPage, TimelineEntry};
use pagis_core::{
    AgentId, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryError, MemoryStore, ScopedPath,
    WorkspaceId,
};
use pagis_knowledge::Error;

/// How many times the pass stages and commits again after a commit
/// that conflicts (ADR-0011).
const ATTEMPTS: usize = 3;

/// One Timeline append of the pass: the acquisition row it belongs
/// to, the Subject Page it goes on, and the entry.
pub struct PendingAppend {
    pub sequence: i64,
    pub path: ScopedPath,
    /// The source title and kind to add when the page has no front
    /// matter.
    pub front_matter: FrontMatter,
    pub entry: TimelineEntry,
}

/// One arrival whose Subject Page refused the append, with the
/// reason. The arrival fails alone; the other appends of the pass
/// stay in the commit (ADR-0011).
pub struct RefusedAppend {
    pub sequence: i64,
    pub reason: String,
}

/// The single commit of one delivery pass.
pub struct PassCommit<'a> {
    pub memory: &'a dyn MemoryStore,
    pub workspace_id: &'a WorkspaceId,
    pub agent_id: &'a AgentId,
    pub access: &'a MemoryAccess,
    pub author: &'a MemoryAuthor,
}

/// What the staging of one append did to the changeset.
enum Staged {
    /// The changeset holds new content for the page.
    Written,
    /// The page already has the entry, so the pass writes nothing.
    Unchanged,
    /// The page cannot take the entry.
    Refused(String),
}

impl PassCommit<'_> {
    /// Stage every append of the pass and commit them once.
    ///
    /// Two arrivals of one Subject Page append to the staged content,
    /// not to the stored file. A commit that conflicts reads the
    /// pages again and stages the appends onto the current content,
    /// at most three times. A commit that still fails returns
    /// `Unavailable`, and the caller then acknowledges nothing of the
    /// pass.
    pub async fn run(&self, appends: &[PendingAppend]) -> Result<Vec<RefusedAppend>, Error> {
        for _ in 0..ATTEMPTS {
            let indexes = self
                .memory
                .load_indexes(self.workspace_id, self.access)
                .await
                .map_err(memory_unavailable)?;
            let mut changeset = MemoryChangeset::default();
            let mut refused = Vec::new();
            let mut written = 0;
            for append in appends {
                match self.stage(&mut changeset, append).await? {
                    Staged::Written => written += 1,
                    Staged::Unchanged => {}
                    Staged::Refused(reason) => refused.push(RefusedAppend {
                        sequence: append.sequence,
                        reason,
                    }),
                }
            }
            if changeset.is_empty() {
                // Every arrival of the pass is already on its page, or
                // no page took one.
                return Ok(refused);
            }
            match self
                .memory
                .commit(
                    self.workspace_id,
                    self.agent_id,
                    self.access,
                    self.author,
                    &changeset,
                    indexes.revision.as_deref(),
                    &message(written),
                    None,
                )
                .await
            {
                Ok(_) => return Ok(refused),
                Err(MemoryError::Conflict) => continue,
                Err(error) => return Err(memory_unavailable(error)),
            }
        }
        Err(Error::Unavailable)
    }

    /// Put one append on the staged content of its page, or on the
    /// stored page when the pass has not touched it yet.
    async fn stage(
        &self,
        changeset: &mut MemoryChangeset,
        append: &PendingAppend,
    ) -> Result<Staged, Error> {
        let current = match changeset.writes.get(&append.path) {
            Some(staged) => Some(staged.clone()),
            None => match self
                .memory
                .read(self.workspace_id, self.access, &append.path)
                .await
            {
                Ok(file) => Some(file.content),
                Err(MemoryError::NotFound(_)) => None,
                Err(error) => return Err(memory_unavailable(error)),
            },
        };
        let content = match current {
            Some(content) => {
                let Some(appended) =
                    SubjectPage::append_rendered(&content, &append.entry, &append.front_matter)
                else {
                    return Ok(Staged::Refused(
                        "the subject page cannot take the timeline entry".into(),
                    ));
                };
                for warning in &appended.warnings {
                    tracing::warn!(
                        path = %append.path.display(),
                        code = warning.code,
                        message = %warning.message,
                        "subject page warning"
                    );
                }
                if !appended.appended {
                    return Ok(Staged::Unchanged);
                }
                appended.content
            }
            None => {
                let mut page = SubjectPage::new_timeline(append.entry.clone());
                page.front_matter.clone_from(&append.front_matter);
                page.render()
            }
        };
        changeset.writes.insert(append.path.clone(), content);
        Ok(Staged::Written)
    }
}

fn message(count: usize) -> String {
    let noun = if count == 1 { "arrival" } else { "arrivals" };
    format!("Append {count} source {noun} to subject pages")
}

fn memory_unavailable(error: MemoryError) -> Error {
    tracing::warn!(%error, "subject page update failed");
    Error::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use pagis_core::memory_page::PageHeading;
    use pagis_core::{MemoryFile, MemoryIndexes, RunId};
    use std::sync::Mutex;

    /// A memory store of whole files that counts its commits and can
    /// refuse them the way the git store does.
    #[derive(Default)]
    struct FakeMemory {
        files: Mutex<Vec<(String, String)>>,
        commits: Mutex<Vec<(String, MemoryChangeset)>>,
        /// How many of the next commits answer with a conflict.
        conflicts: Mutex<usize>,
        /// Every commit answers as unavailable.
        unavailable: bool,
    }

    impl FakeMemory {
        fn commits(&self) -> usize {
            self.commits.lock().expect("commits").len()
        }
        fn message(&self, index: usize) -> String {
            self.commits.lock().expect("commits")[index].0.clone()
        }
        fn content(&self, path: &str) -> String {
            self.files
                .lock()
                .expect("files")
                .iter()
                .find(|(held, _)| held == path)
                .map(|(_, content)| content.clone())
                .unwrap_or_default()
        }
        fn put(&self, path: &str, content: &str) {
            self.files
                .lock()
                .expect("files")
                .push((path.to_string(), content.to_string()));
        }
    }

    #[async_trait]
    impl MemoryStore for FakeMemory {
        async fn preview_forget(
            &self,
            _: &WorkspaceId,
            _: &pagis_core::ForgetTarget,
        ) -> Result<pagis_core::ForgetPreview, MemoryError> {
            unreachable!("a delivery pass never previews a forget")
        }
        async fn begin_forget(
            &self,
            _: &WorkspaceId,
            _: &pagis_core::ForgetTarget,
            _: &pagis_core::ForgetPreview,
            _: i64,
            _: &dyn pagis_core::ForgetKeys,
        ) -> Result<pagis_core::ForgetOperation, MemoryError> {
            unreachable!("a delivery pass never begins a forget")
        }
        async fn purge_forgotten(&self, _: &WorkspaceId, _: &str) -> Result<(), MemoryError> {
            unreachable!("a delivery pass never purges")
        }
        async fn load_indexes(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
        ) -> Result<MemoryIndexes, MemoryError> {
            Ok(MemoryIndexes {
                shared: String::new(),
                private: String::new(),
                revision: Some(format!("revision-{}", self.commits())),
            })
        }
        async fn read(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
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
            unreachable!("a delivery pass never lists the pages")
        }
        async fn list_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
            _: &pagis_core::PageListQuery,
        ) -> Result<pagis_core::MemoryPageList, MemoryError> {
            unreachable!("a delivery pass never lists the pages")
        }
        async fn search_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
            _: &str,
            _: usize,
        ) -> Result<Vec<pagis_core::MemorySearchHit>, MemoryError> {
            unreachable!("a delivery pass never searches pages")
        }
        async fn count_pages(
            &self,
            _: &WorkspaceId,
            _: &MemoryAccess,
            _: pagis_core::MemoryScope,
        ) -> Result<pagis_core::MemoryPageCounts, MemoryError> {
            unreachable!("a delivery pass never counts the pages")
        }
        async fn commit_diff(
            &self,
            _: &WorkspaceId,
            _: &str,
            _: &MemoryAccess,
        ) -> Result<pagis_core::MemoryCommitDiff, MemoryError> {
            unreachable!("a delivery pass never reads a diff")
        }
        async fn commit(
            &self,
            _: &WorkspaceId,
            _: &AgentId,
            _: &MemoryAccess,
            _: &MemoryAuthor,
            changeset: &MemoryChangeset,
            _: Option<&str>,
            message: &str,
            _: Option<&RunId>,
        ) -> Result<String, MemoryError> {
            if self.unavailable {
                return Err(MemoryError::Unavailable);
            }
            {
                let mut conflicts = self.conflicts.lock().expect("conflicts");
                if *conflicts > 0 {
                    *conflicts -= 1;
                    return Err(MemoryError::Conflict);
                }
            }
            let mut files = self.files.lock().expect("files");
            for (path, content) in &changeset.writes {
                files.retain(|(held, _)| held != &path.display());
                files.push((path.display(), content.clone()));
            }
            self.commits
                .lock()
                .expect("commits")
                .push((message.to_string(), changeset.clone()));
            Ok(format!(
                "sha{}",
                self.commits.lock().expect("commits").len()
            ))
        }
        async fn revert(
            &self,
            _: &WorkspaceId,
            _: &str,
            _: &MemoryAccess,
            _: &str,
            _: &MemoryAuthor,
        ) -> Result<String, MemoryError> {
            unreachable!("a delivery pass never reverts")
        }
    }

    struct Fixture {
        memory: std::sync::Arc<FakeMemory>,
        workspace_id: WorkspaceId,
        agent_id: AgentId,
        access: MemoryAccess,
        author: MemoryAuthor,
    }

    fn fixture() -> Fixture {
        let agent_id = AgentId::generate();
        Fixture {
            memory: std::sync::Arc::new(FakeMemory::default()),
            workspace_id: WorkspaceId::generate(),
            agent_id: agent_id.clone(),
            access: MemoryAccess::agent(agent_id, []),
            author: MemoryAuthor {
                name: "Pagis".into(),
                email: "system@pagis.local".into(),
            },
        }
    }

    impl Fixture {
        fn pass(&self) -> PassCommit<'_> {
            PassCommit {
                memory: self.memory.as_ref(),
                workspace_id: &self.workspace_id,
                agent_id: &self.agent_id,
                access: &self.access,
                author: &self.author,
            }
        }
    }

    fn append(sequence: i64, page: &str, reference: &str, at: i64) -> PendingAppend {
        PendingAppend {
            sequence,
            path: ScopedPath::parse(&format!("private/subjects/gmail/{page}.md")).expect("path"),
            front_matter: FrontMatter {
                title: Some(format!("{page} title")),
                kind: Some("Source".into()),
                aliases: Vec::new(),
                links: Vec::new(),
            },
            entry: TimelineEntry {
                source_reference: reference.into(),
                source_time: at,
                words: format!("The words of {reference}."),
            },
        }
    }

    #[tokio::test]
    async fn a_new_page_uses_the_arrival_front_matter() {
        let f = fixture();

        f.pass()
            .run(&[append(1, "school", "gmail:c:one:1", 1_000)])
            .await
            .expect("the pass commits");

        assert_eq!(
            PageHeading::parse(
                "private/subjects/gmail/school.md",
                &f.memory.content("private/subjects/gmail/school.md")
            ),
            PageHeading {
                kind: Some("Source".into()),
                title: "school title".into(),
            }
        );
    }

    #[tokio::test]
    async fn the_next_arrival_adds_front_matter_to_an_existing_page_without_it() {
        let f = fixture();
        let path = "private/subjects/gmail/school.md";
        f.memory.put(path, &SubjectPage::default().render());

        f.pass()
            .run(&[append(1, "school", "gmail:c:one:1", 1_000)])
            .await
            .expect("the pass commits");

        assert_eq!(
            PageHeading::parse(path, &f.memory.content(path)).title,
            "school title"
        );
    }

    #[tokio::test]
    async fn an_arrival_keeps_existing_front_matter() {
        let f = fixture();
        let path = "private/subjects/gmail/school.md";
        f.memory.put(
            path,
            &SubjectPage {
                front_matter: FrontMatter {
                    title: Some("Original title".into()),
                    kind: Some("Source".into()),
                    aliases: Vec::new(),
                    links: Vec::new(),
                },
                ..SubjectPage::default()
            }
            .render(),
        );

        f.pass()
            .run(&[append(1, "school", "gmail:c:one:1", 1_000)])
            .await
            .expect("the pass commits");

        assert_eq!(
            PageHeading::parse(path, &f.memory.content(path)).title,
            "Original title"
        );
    }

    fn timeline_of(content: &str) -> Vec<String> {
        SubjectPage::parse(content)
            .page
            .timeline
            .into_iter()
            .map(|entry| entry.source_reference)
            .collect()
    }

    #[tokio::test]
    async fn the_appends_of_one_pass_make_one_commit() {
        let f = fixture();
        let appends = vec![
            append(1, "school", "gmail:c:one:1", 1_000),
            append(2, "sports", "gmail:c:two:1", 2_000),
            append(3, "school", "gmail:c:three:1", 3_000),
        ];

        let refused = f.pass().run(&appends).await.expect("the pass commits");

        assert!(refused.is_empty());
        assert_eq!(f.memory.commits(), 1, "the pass makes one commit");
        assert_eq!(
            f.memory.message(0),
            "Append 3 source arrivals to subject pages"
        );
        // Two arrivals of one page append to the staged content, so
        // the page holds both.
        assert_eq!(
            timeline_of(&f.memory.content("private/subjects/gmail/school.md")),
            vec!["gmail:c:one:1", "gmail:c:three:1"]
        );
        assert_eq!(
            timeline_of(&f.memory.content("private/subjects/gmail/sports.md")),
            vec!["gmail:c:two:1"]
        );
    }

    #[tokio::test]
    async fn a_commit_that_conflicts_stages_again_and_still_makes_one_commit() {
        let f = fixture();
        *f.memory.conflicts.lock().expect("conflicts") = 1;
        let appends = vec![
            append(1, "school", "gmail:c:one:1", 1_000),
            append(2, "school", "gmail:c:two:1", 2_000),
        ];

        let refused = f.pass().run(&appends).await.expect("the pass commits");

        assert!(refused.is_empty());
        assert_eq!(f.memory.commits(), 1, "only the second try commits");
        assert_eq!(
            timeline_of(&f.memory.content("private/subjects/gmail/school.md")),
            vec!["gmail:c:one:1", "gmail:c:two:1"]
        );
    }

    #[tokio::test]
    async fn a_page_that_refuses_an_entry_leaves_the_other_appends_in_the_commit() {
        let f = fixture();
        f.memory
            .put("private/subjects/gmail/broken.md", "not a subject page\n");
        let appends = vec![
            append(1, "broken", "gmail:c:one:1", 1_000),
            append(2, "school", "gmail:c:two:1", 2_000),
        ];

        let refused = f.pass().run(&appends).await.expect("the pass commits");

        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].sequence, 1);
        assert_eq!(f.memory.commits(), 1);
        assert_eq!(
            f.memory.message(0),
            "Append 1 source arrival to subject pages"
        );
        assert_eq!(
            timeline_of(&f.memory.content("private/subjects/gmail/school.md")),
            vec!["gmail:c:two:1"]
        );
    }

    #[tokio::test]
    async fn an_arrival_that_is_already_on_its_page_commits_nothing() {
        let f = fixture();
        let appends = vec![append(1, "school", "gmail:c:one:1", 1_000)];
        f.pass().run(&appends).await.expect("the pass commits");

        let refused = f.pass().run(&appends).await.expect("the second pass");

        assert!(refused.is_empty());
        assert_eq!(f.memory.commits(), 1, "the repeat pass commits nothing");
    }

    #[tokio::test]
    async fn a_commit_that_keeps_conflicting_fails_the_pass() {
        let f = fixture();
        *f.memory.conflicts.lock().expect("conflicts") = ATTEMPTS;

        let result = f
            .pass()
            .run(&[append(1, "school", "gmail:c:one:1", 1_000)])
            .await;

        assert!(matches!(result, Err(Error::Unavailable)));
        assert_eq!(f.memory.commits(), 0, "nothing reached the repository");
    }

    #[tokio::test]
    async fn a_commit_the_store_refuses_fails_the_pass_at_once() {
        let f = Fixture {
            memory: std::sync::Arc::new(FakeMemory {
                unavailable: true,
                ..FakeMemory::default()
            }),
            ..fixture()
        };

        let result = f
            .pass()
            .run(&[append(1, "school", "gmail:c:one:1", 1_000)])
            .await;

        assert!(matches!(result, Err(Error::Unavailable)));
        assert_eq!(f.memory.commits(), 0);
    }
}
