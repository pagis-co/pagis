//! MemoryStore conformance tests for each backend.

use pagis_core::{
    AgentId, GrantId, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryError, MemoryExposure,
    MemoryStore, RunId, ScopedPath, WorkspaceId,
};
use std::sync::Arc;

/// One implementation and a second handle to its memory.
pub struct Harness {
    pub store: Arc<dyn MemoryStore>,
    pub reopened: Arc<dyn MemoryStore>,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    resource: Option<Box<dyn Send + Sync>>,
}
impl Harness {
    pub fn with_resource(mut self, resource: impl Send + Sync + 'static) -> Self {
        self.resource = Some(Box::new(resource));
        self
    }
    pub fn new(store: Arc<dyn MemoryStore>, reopened: Arc<dyn MemoryStore>) -> Self {
        Self {
            store,
            reopened,
            workspace_id: WorkspaceId::generate(),
            agent_id: AgentId::generate(),
            resource: None,
        }
    }
}

fn access(agent_id: &AgentId) -> MemoryAccess {
    MemoryAccess::agent(agent_id.clone(), [])
}

fn author() -> MemoryAuthor {
    MemoryAuthor {
        name: "Sage".to_string(),
        email: "sage@agents.pagis.local".to_string(),
    }
}

/// A query for each page of a scope in one part.
fn every_page() -> pagis_core::PageListQuery {
    pagis_core::PageListQuery {
        limit: usize::MAX,
        ..Default::default()
    }
}

fn write(path: &str, content: &str) -> MemoryChangeset {
    let mut changeset = MemoryChangeset::default();
    changeset
        .writes
        .insert(ScopedPath::parse(path).unwrap(), content.to_string());
    changeset
}

impl Harness {
    async fn revision(&self) -> Option<String> {
        self.store
            .load_indexes(&self.workspace_id, &access(&self.agent_id))
            .await
            .unwrap()
            .revision
    }

    fn reboot(&self) -> Arc<dyn MemoryStore> {
        self.reopened.clone()
    }

    async fn commit(&self, changeset: &MemoryChangeset, message: &str) -> String {
        self.commit_on(&self.store, changeset, message).await
    }

    async fn commit_on(
        &self,
        store: &Arc<dyn MemoryStore>,
        changeset: &MemoryChangeset,
        message: &str,
    ) -> String {
        let revision = self.revision().await;
        store
            .commit(
                &self.workspace_id,
                &self.agent_id,
                &access(&self.agent_id),
                &author(),
                changeset,
                revision.as_deref(),
                message,
                Some(&RunId::generate()),
            )
            .await
            .expect("commit")
    }

    /// The shared page list as path, author, and change time.
    async fn shared_changes(&self, store: &Arc<dyn MemoryStore>) -> Vec<(String, String, i64)> {
        store
            .list_pages(
                &self.workspace_id,
                &access(&self.agent_id),
                pagis_core::MemoryScope::Shared,
                &every_page(),
            )
            .await
            .expect("shared pages")
            .pages
            .into_iter()
            .map(|page| (page.path.display(), page.changed_by.name, page.changed_at))
            .collect()
    }

    async fn read(&self, path: &str) -> Result<String, MemoryError> {
        self.store
            .read(
                &self.workspace_id,
                &access(&self.agent_id),
                &ScopedPath::parse(path).unwrap(),
            )
            .await
            .map(|file| file.content)
    }

    async fn search(&self, access: &MemoryAccess, query: &str) -> Vec<pagis_core::MemorySearchHit> {
        self.store
            .search_pages(
                &self.workspace_id,
                access,
                pagis_core::MemoryScope::Shared,
                query,
                10,
            )
            .await
            .expect("search pages")
    }
}

fn subject(truth: &str) -> String {
    pagis_core::subject_page::SubjectPage {
        truth: truth.into(),
        ..Default::default()
    }
    .render()
}

#[derive(Default)]
pub struct NoSuppression {
    pub selected: std::sync::Mutex<Option<GrantId>>,
    pub forgotten: std::sync::Mutex<Option<GrantId>>,
}
#[async_trait::async_trait]
impl pagis_core::ForgetStore for NoSuppression {
    async fn preview_affects(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::ForgetTarget,
        exposures: &[MemoryExposure],
    ) -> Result<bool, pagis_core::StoreError> {
        let selected = self.selected.lock().expect("selected grant");
        Ok(exposures
            .iter()
            .any(|exposure| Some(&exposure.grant_id) == selected.as_ref()))
    }
    async fn run_allowed(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::RunId,
    ) -> Result<bool, pagis_core::StoreError> {
        Ok(true)
    }
    async fn operation(
        &self,
        _: &WorkspaceId,
        id: &str,
    ) -> Result<Option<pagis_core::ForgetOperation>, pagis_core::StoreError> {
        Ok(Some(pagis_core::ForgetOperation {
            id: id.to_string(),
            connection_id: pagis_core::ConnectionId::generate(),
            phase: pagis_core::ForgetPhase::StructuredPurged,
            created_at: 0,
            error: None,
            reopted_at: None,
        }))
    }
    async fn operations(
        &self,
        _: &WorkspaceId,
    ) -> Result<Vec<pagis_core::ForgetOperation>, pagis_core::StoreError> {
        Ok(vec![])
    }
    async fn unfinished(
        &self,
        _: &WorkspaceId,
    ) -> Result<Vec<pagis_core::ForgetOperation>, pagis_core::StoreError> {
        Ok(vec![])
    }
    async fn purge_structured(
        &self,
        _: &WorkspaceId,
        _: &str,
    ) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn memory_purged(&self, _: &WorkspaceId, _: &str) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn complete(&self, _: &WorkspaceId, _: &str) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn record_failure(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: &str,
    ) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn retry(&self, _: &WorkspaceId, _: &str) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn reopt_in(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: i64,
    ) -> Result<(), pagis_core::StoreError> {
        unreachable!()
    }
    async fn preview(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::ForgetTarget,
    ) -> Result<pagis_core::ForgetPreview, pagis_core::StoreError> {
        Ok(pagis_core::ForgetPreview {
            revision: "source-revision".to_string(),
            source_items: 0,
            memory_revision: None,
            memory_paths: 0,
            memory_revisions: 0,
            raw_account_retrieval_blocked: false,
        })
    }
    async fn begin(
        &self,
        _: &WorkspaceId,
        target: &pagis_core::ForgetTarget,
        revision: &str,
        now: i64,
        _: &dyn pagis_core::ForgetKeys,
    ) -> Result<pagis_core::ForgetOperation, pagis_core::StoreError> {
        if revision != "source-revision" {
            return Err(pagis_core::StoreError::Conflict(
                "source revision changed".to_string(),
            ));
        }
        let connection_id = match target {
            pagis_core::ForgetTarget::Account { connection_id } => connection_id.clone(),
            pagis_core::ForgetTarget::Source { source, .. } => source.connection_id.clone(),
        };
        Ok(pagis_core::ForgetOperation {
            id: "forget-1".to_string(),
            connection_id,
            phase: pagis_core::ForgetPhase::StructuredPurged,
            created_at: now,
            error: None,
            reopted_at: None,
        })
    }
    async fn is_suppressed(
        &self,
        _: &pagis_core::knowledge::SourceKey,
        _: &str,
        _: &dyn pagis_core::ForgetKeys,
    ) -> Result<bool, pagis_core::StoreError> {
        Ok(false)
    }
    async fn raw_connection_allowed(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::ConnectionId,
    ) -> Result<bool, pagis_core::StoreError> {
        Ok(true)
    }
    async fn permits_provenance(
        &self,
        _: &WorkspaceId,
        exposures: &[MemoryExposure],
    ) -> Result<bool, pagis_core::StoreError> {
        let forgotten = self.forgotten.lock().expect("forgotten grant");
        Ok(!exposures
            .iter()
            .any(|exposure| Some(&exposure.grant_id) == forgotten.as_ref()))
    }
}

pub async fn search_returns_a_fact_file_for_a_word_in_its_body(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write(
            "shared/vignesh-contact.md",
            "---\ntitle: Vignesh\n---\n\nVignesh works in the heliotrope office.\n",
        ),
        "Learned how to contact Vignesh",
    )
    .await;

    let hits = h.search(&access(&h.agent_id), "heliotrope").await;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "shared/vignesh-contact.md");
    assert_eq!(hits[0].title, "Vignesh");
    assert!(hits[0].snippet.contains("heliotrope"));
}

pub async fn search_finds_a_page_by_an_alias_in_its_front_matter(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write(
            "shared/seating.md",
            "---\ntitle: Sofa\naliases: [[davenport]]\n---\n\nIt cost 900 euro.\n",
        ),
        "Learned what the sofa cost",
    )
    .await;

    let hits = h.search(&access(&h.agent_id), "davenport").await;

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "shared/seating.md");
}

pub async fn unwritten_scopes_serve_the_seed_indexes(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let h = make(Arc::new(NoSuppression::default()));

    let indexes = h
        .store
        .load_indexes(&h.workspace_id, &access(&h.agent_id))
        .await
        .unwrap();

    for index in [&indexes.shared, &indexes.private] {
        assert!(index.contains("MEMORY.md"), "seed states the convention");
        assert!(index.contains("one line per fact file"));
    }
    assert!(indexes.shared.contains("Shared memory"));
    assert!(indexes.private.contains("Private memory"));
}

pub async fn a_commit_persists_writes_and_reads_serve_them(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut changeset = write("shared/MEMORY.md", "- [User](user.md) — likes tea\n");
    changeset.writes.insert(
        ScopedPath::parse("private/craft.md").unwrap(),
        "notes\n".to_string(),
    );

    h.commit(&changeset, "Learned the user's tea preference")
        .await;

    assert_eq!(
        h.read("shared/MEMORY.md").await.unwrap(),
        "- [User](user.md) — likes tea\n"
    );
    assert_eq!(h.read("private/craft.md").await.unwrap(), "notes\n");
    let indexes = h
        .store
        .load_indexes(&h.workspace_id, &access(&h.agent_id))
        .await
        .unwrap();
    assert_eq!(indexes.shared, "- [User](user.md) — likes tea\n");
}

pub async fn brief_entries_rank_the_newest_change_first_with_the_score_and_the_words(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write("private/subjects/gmail/priya.md", &subject("Priya v1.")),
        "first",
    )
    .await;
    h.commit(
        &write(
            "shared/subjects/gmail/acme.md",
            &subject("Acme buys paper."),
        ),
        "second",
    )
    .await;
    h.commit(
        &write("shared/note.md", "# A note\n\nThe note names heliotrope.\n"),
        "a fact file",
    )
    .await;
    h.commit(
        &write("private/subjects/gmail/broken.md", "# No layout\n"),
        "a page with no valid layout, read as a fact file",
    )
    .await;
    h.commit(
        &write("shared/MEMORY.md", "- [A note](note.md) — heliotrope\n"),
        "an index of the scope",
    )
    .await;
    let head = h
        .commit(
            &write(
                "private/subjects/gmail/priya.md",
                &subject("Priya leads the pilot."),
            ),
            "first again",
        )
        .await;

    let brief = h
        .store
        .brief_entries(&h.workspace_id, &access(&h.agent_id), "missing")
        .await
        .unwrap();

    assert_eq!(brief.revision, Some(head));
    let mut entries = brief.entries;
    entries.sort_by_key(|entry| entry.change_rank);
    let rows: Vec<_> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.change_rank, entry.changed))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("private/subjects/gmail/priya.md", 0, true),
            ("private/subjects/gmail/broken.md", 2, true),
            ("shared/note.md", 3, true),
            ("shared/subjects/gmail/acme.md", 4, true),
        ],
        "a fact file is a candidate, an index never is, and a cursor \
         that the history does not hold marks each page as changed"
    );
    let mut words: Vec<_> = entries[0].words.iter().map(String::as_str).collect();
    words.sort();
    assert_eq!(words, vec!["leads", "pilot", "priya", "the"]);
}

pub async fn brief_entries_mark_the_pages_changed_after_the_cursor(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write(
            "private/subjects/gmail/older.md",
            &subject("The older page."),
        ),
        "first",
    )
    .await;
    let cursor = h
        .commit(&write("shared/note.md", "# A note\n"), "the cursor")
        .await;
    let changed = |revision: String| {
        let h = &h;
        async move {
            let mut paths: Vec<_> = h
                .store
                .brief_entries(&h.workspace_id, &access(&h.agent_id), &revision)
                .await
                .unwrap()
                .entries
                .into_iter()
                .filter(|entry| entry.changed)
                .map(|entry| entry.path)
                .collect();
            paths.sort();
            paths
        }
    };
    assert!(
        changed(cursor.clone()).await.is_empty(),
        "no commit is after HEAD"
    );

    h.commit(
        &write(
            "private/subjects/gmail/newer.md",
            &subject("The newer page."),
        ),
        "after the cursor",
    )
    .await;
    h.commit(
        &write("shared/other.md", "# Other\n"),
        "also after the cursor",
    )
    .await;

    assert_eq!(
        changed(cursor).await,
        vec![
            "private/subjects/gmail/newer.md".to_string(),
            "shared/other.md".to_string(),
        ]
    );
}

pub async fn brief_entries_hide_a_source_page_from_an_access_without_its_grant(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let permitted = MemoryAccess::agent(h.agent_id.clone(), [exposure]);
    h.store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &permitted,
            &author(),
            &write(
                "private/subjects/gmail/priya.md",
                &subject("From a source."),
            ),
            None,
            "Reflected on a source",
            Some(&RunId::generate()),
        )
        .await
        .expect("commit");

    let with_grant = h
        .store
        .brief_entries(&h.workspace_id, &permitted, "missing")
        .await
        .unwrap();
    assert_eq!(with_grant.entries.len(), 1);
    let without = h
        .store
        .brief_entries(&h.workspace_id, &access(&h.agent_id), "missing")
        .await
        .unwrap();
    assert!(without.entries.is_empty());
}

pub async fn brief_entries_of_an_unwritten_workspace_are_empty(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let brief = h
        .store
        .brief_entries(&h.workspace_id, &access(&h.agent_id), "missing")
        .await
        .unwrap();
    assert_eq!(brief.revision, None);
    assert!(brief.entries.is_empty());
}

pub async fn private_scopes_are_disjoint_between_agents(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let other = AgentId::generate();
    h.commit(&write("private/MEMORY.md", "mine\n"), "note")
        .await;

    let theirs = h
        .store
        .read(
            &h.workspace_id,
            &access(&other),
            &ScopedPath::parse("private/MEMORY.md").unwrap(),
        )
        .await;

    assert!(matches!(theirs, Err(MemoryError::NotFound(_))));
    let their_indexes = h
        .store
        .load_indexes(&h.workspace_id, &access(&other))
        .await
        .unwrap();
    assert!(their_indexes.private.contains("Private memory"), "seed");
}

pub async fn the_commit_carries_author_message_and_run_trailer(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let run_id = RunId::generate();

    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &author(),
            &write("private/MEMORY.md", "x\n"),
            None,
            "Remembered a fact",
            Some(&run_id),
        )
        .await
        .unwrap();

    let diff = h
        .store
        .commit_diff(&h.workspace_id, &sha, &access(&h.agent_id))
        .await
        .unwrap();
    assert_eq!(diff.author, author());
    assert_eq!(diff.message, "Remembered a fact");
    assert_eq!(
        h.store
            .find_commit_with_trailers(&h.workspace_id, &[("Run-Id", run_id.as_str())])
            .await
            .unwrap(),
        Some(sha)
    );
}

pub async fn recovery_finds_only_the_exact_pending_review_commit(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let run_id = RunId::generate();
    let message = "Reviewed evidence\n\nPagis-Memory-Phase: reflection\nPending-Review-Id: pe_1\nPending-Review-Revision: 7\nSource-After-Exclusive: m1\nSource-Through-Inclusive: m9";
    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &author(),
            &write("private/MEMORY.md", "x\n"),
            None,
            message,
            Some(&run_id),
        )
        .await
        .unwrap();

    let revision = h
        .store
        .find_commit_with_trailers(
            &h.workspace_id,
            &[
                ("Run-Id", run_id.as_str()),
                ("Pagis-Memory-Phase", "reflection"),
                ("Pending-Review-Id", "pe_1"),
                ("Pending-Review-Revision", "7"),
                ("Source-After-Exclusive", "m1"),
                ("Source-Through-Inclusive", "m9"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(revision.as_deref(), Some(sha.as_str()));
    assert!(
        h.store
            .find_commit_with_trailers(
                &h.workspace_id,
                &[
                    ("Pending-Review-Id", "pe_1"),
                    ("Pending-Review-Revision", "8")
                ],
            )
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn a_delete_removes_the_file(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/old.md", "stale\n"), "add").await;

    let mut changeset = MemoryChangeset::default();
    changeset
        .deletes
        .insert(ScopedPath::parse("shared/old.md").unwrap());
    h.commit(&changeset, "drop stale note").await;

    assert!(matches!(
        h.read("shared/old.md").await,
        Err(MemoryError::NotFound(_))
    ));
}

pub async fn an_empty_changeset_refuses_to_commit(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let h = make(Arc::new(NoSuppression::default()));

    let result = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &author(),
            &MemoryChangeset::default(),
            None,
            "nothing",
            Some(&RunId::generate()),
        )
        .await;

    assert!(matches!(result, Err(MemoryError::Invalid(_))));
}

pub async fn later_commits_win_whole_file(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/fact.md", "first\n"), "first").await;
    h.commit(&write("shared/fact.md", "second\n"), "second")
        .await;

    assert_eq!(h.read("shared/fact.md").await.unwrap(), "second\n");
}

pub async fn concurrent_whole_file_commits_reject_the_stale_base(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let by = author();
    let current = h.revision().await;
    let run_a = RunId::generate();
    let run_b = RunId::generate();
    let access = access(&h.agent_id);
    let write_a = write("shared/fact.md", "a\n");
    let write_b = write("shared/fact.md", "b\n");
    let (a, b) = tokio::join!(
        h.store.commit(
            &h.workspace_id,
            &h.agent_id,
            &access,
            &by,
            &write_a,
            current.as_deref(),
            "a",
            Some(&run_a)
        ),
        h.store.commit(
            &h.workspace_id,
            &h.agent_id,
            &access,
            &by,
            &write_b,
            current.as_deref(),
            "b",
            Some(&run_b)
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or_else(|| b.as_ref().err()),
        Some(MemoryError::Conflict)
    ));
}

pub async fn revert_makes_an_inverse_commit_by_the_user(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/fact.md", "keep\n"), "keep").await;
    let sha = h.commit(&write("shared/fact.md", "wrong\n"), "wrong").await;

    let user = MemoryAuthor {
        name: "User".to_string(),
        email: "user@pagis.local".to_string(),
    };
    let revert_sha = h
        .store
        .revert(
            &h.workspace_id,
            &sha,
            &MemoryAccess::Owner {
                agent_id: h.agent_id.clone(),
            },
            &h.revision().await.unwrap(),
            &user,
        )
        .await
        .expect("revert");

    assert_eq!(h.read("shared/fact.md").await.unwrap(), "keep\n");
    let diff = h
        .store
        .commit_diff(&h.workspace_id, &revert_sha, &access(&h.agent_id))
        .await
        .unwrap();
    assert_eq!(diff.author, user);
    assert!(diff.message.starts_with("Revert"));
}

pub async fn revert_aborts_cleanly_when_later_commits_conflict(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let sha = h.commit(&write("shared/fact.md", "v1\n"), "v1").await;
    h.commit(&write("shared/fact.md", "v2\n"), "v2").await;

    let before = h.revision().await;
    let result = h
        .store
        .revert(
            &h.workspace_id,
            &sha,
            &access(&h.agent_id),
            &h.revision().await.unwrap(),
            &author(),
        )
        .await;

    assert!(matches!(result, Err(MemoryError::RevertConflict)));
    // The abort left the working tree at v2 and made no commit.
    assert_eq!(h.read("shared/fact.md").await.unwrap(), "v2\n");
    assert_eq!(
        h.revision().await,
        before,
        "a failed revert makes no commit"
    );
}

pub async fn workspaces_get_disjoint_repositories(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let h = make(Arc::new(NoSuppression::default()));
    let other_workspace = WorkspaceId::generate();
    h.commit(&write("shared/fact.md", "here\n"), "here").await;

    let elsewhere = h
        .store
        .read(
            &other_workspace,
            &access(&h.agent_id),
            &ScopedPath::parse("shared/fact.md").unwrap(),
        )
        .await;

    assert!(matches!(elsewhere, Err(MemoryError::NotFound(_))));
}

pub async fn a_commit_without_a_run_has_no_run_trailer(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));

    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &author(),
            &write("shared/user.md", "# User\n"),
            None,
            "Onboarding: the user's name",
            None,
        )
        .await
        .unwrap();

    let diff = h
        .store
        .commit_diff(&h.workspace_id, &sha, &access(&h.agent_id))
        .await
        .unwrap();
    assert_eq!(diff.message, "Onboarding: the user's name");
    assert!(
        h.store
            .find_commit_with_trailers(&h.workspace_id, &[("Run-Id", "")])
            .await
            .unwrap()
            .is_none()
    );
}

pub async fn source_exposed_files_and_indexes_are_invisible_without_current_access(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 3,
    };
    let permitted = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let mut changes = write(
        "shared/MEMORY.md",
        "- [Account note](account.md) — private source\n",
    );
    changes.writes.insert(
        ScopedPath::parse("shared/account.md").unwrap(),
        "A source-derived view.\n".into(),
    );
    let source_commit = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &permitted,
            &author(),
            &changes,
            None,
            "Reflected on a permitted source",
            Some(&RunId::generate()),
        )
        .await
        .unwrap();

    let allowed = h
        .store
        .load_indexes(&h.workspace_id, &permitted)
        .await
        .unwrap();
    assert!(allowed.shared.contains("Account note"));
    assert_eq!(
        allowed.exposures.as_slice(),
        std::slice::from_ref(&exposure)
    );
    let file = h
        .store
        .read(
            &h.workspace_id,
            &permitted,
            &ScopedPath::parse("shared/account.md").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(file.exposures.as_slice(), std::slice::from_ref(&exposure));
    assert_eq!(file.kind, pagis_core::MemoryContentKind::DependentView);

    for denied in [
        MemoryAccess::agent(AgentId::generate(), []),
        MemoryAccess::agent(h.agent_id.clone(), []),
        MemoryAccess::agent(
            h.agent_id.clone(),
            [MemoryExposure {
                revision: 4,
                ..exposure.clone()
            }],
        ),
    ] {
        let indexes = h
            .store
            .load_indexes(&h.workspace_id, &denied)
            .await
            .unwrap();
        assert!(!indexes.shared.contains("Account note"));
        assert!(indexes.exposures.is_empty());
        assert!(matches!(
            h.store
                .read(
                    &h.workspace_id,
                    &denied,
                    &ScopedPath::parse("shared/account.md").unwrap()
                )
                .await,
            Err(MemoryError::Unavailable)
        ));
    }

    let denied = MemoryAccess::agent(h.agent_id.clone(), []);
    let current = h
        .store
        .load_indexes(&h.workspace_id, &denied)
        .await
        .unwrap()
        .revision;
    assert!(matches!(
        h.store
            .revert(
                &h.workspace_id,
                &source_commit,
                &denied,
                current.as_deref().unwrap(),
                &author(),
            )
            .await,
        Err(MemoryError::Unavailable)
    ));
    assert!(matches!(
        h.store
            .commit(
                &h.workspace_id,
                &h.agent_id,
                &denied,
                &author(),
                &write("shared/account.md", "overwrite\n"),
                current.as_deref(),
                "overwrite",
                Some(&RunId::generate()),
            )
            .await,
        Err(MemoryError::Unavailable)
    ));
}

pub async fn owner_edits_preserve_the_existing_source_scope(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "source detail\n"),
            None,
            "source",
            None,
        )
        .await
        .unwrap();
    h.store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &MemoryAccess::Owner {
                agent_id: h.agent_id.clone(),
            },
            &author(),
            &write("shared/source.md", "edited source detail\n"),
            Some(&sha),
            "edit",
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        h.read("shared/source.md").await,
        Err(MemoryError::Unavailable)
    ));
    let file = h
        .store
        .read(
            &h.workspace_id,
            &allowed,
            &ScopedPath::parse("shared/source.md").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(file.exposures, vec![exposure]);
}

pub async fn reverting_a_revert_cannot_restore_a_revoked_source(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write("shared/procedure.md", "source-free procedure\n"),
        "procedure",
    )
    .await;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure]);
    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "source detail\n"),
            h.revision().await.as_deref(),
            "source",
            None,
        )
        .await
        .unwrap();
    let inverse = h
        .store
        .revert(&h.workspace_id, &sha, &allowed, &sha, &author())
        .await
        .unwrap();
    let result = h
        .store
        .revert(
            &h.workspace_id,
            &inverse,
            &access(&h.agent_id),
            &inverse,
            &author(),
        )
        .await;
    assert!(matches!(result, Err(MemoryError::Unavailable)));
    assert_eq!(h.revision().await.as_deref(), Some(inverse.as_str()));
}

pub async fn concurrent_index_loads_return_one_revision_for_both_scopes(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let changes = |value: &str| {
        let mut changes = write("shared/MEMORY.md", value);
        changes.writes.insert(
            ScopedPath::parse("private/MEMORY.md").unwrap(),
            value.into(),
        );
        changes
    };
    h.commit(&changes("initial\n"), "seed").await;
    let writer = async {
        for i in 0..80 {
            h.commit(&changes(&format!("revision {i}\n")), "update")
                .await;
        }
    };
    let reader = async {
        for _ in 0..160 {
            let indexes = h
                .store
                .load_indexes(&h.workspace_id, &access(&h.agent_id))
                .await
                .unwrap();
            assert_eq!(
                indexes.shared, indexes.private,
                "both indexes must come from the same commit"
            );
        }
    };
    tokio::join!(writer, reader);
}

pub async fn list_pages_serves_one_scope_newest_change_first_with_its_author(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut first = write("shared/older.md", "---\ntitle: Older\n---\n");
    first.writes.insert(
        ScopedPath::parse("shared/newer.md").unwrap(),
        "---\ntitle: Newer v1\n---\n".to_string(),
    );
    first.writes.insert(
        ScopedPath::parse("private/craft.md").unwrap(),
        "---\ntitle: Craft\n---\n".to_string(),
    );
    h.commit(&first, "first").await;
    // Git commit times have one-second resolution.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    h.commit(
        &write("shared/newer.md", "---\ntitle: Newer v2\n---\n"),
        "second",
    )
    .await;

    let shared = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
            &every_page(),
        )
        .await
        .expect("shared pages")
        .pages;
    let paths: Vec<String> = shared.iter().map(|page| page.path.display()).collect();
    assert_eq!(paths, vec!["shared/newer.md", "shared/older.md"]);
    assert_eq!(shared[0].title, "Newer v2");
    assert_eq!(shared[0].changed_by.name, "Sage");
    assert!(
        shared[0].changed_at > shared[1].changed_at,
        "the second commit is later"
    );

    let private = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Private,
            &every_page(),
        )
        .await
        .expect("private pages")
        .pages;
    let paths: Vec<String> = private.iter().map(|page| page.path.display()).collect();
    assert_eq!(paths, vec!["private/craft.md"]);

    let other = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&AgentId::generate()),
            pagis_core::MemoryScope::Private,
            &every_page(),
        )
        .await
        .expect("another agent's pages")
        .pages;
    assert!(other.is_empty(), "a private scope is one agent's alone");
}

pub async fn list_pages_follows_new_commits_after_its_first_answer(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut first = write("shared/alpha.md", "# Alpha\n");
    first.writes.insert(
        ScopedPath::parse("shared/beta.md").unwrap(),
        "# Beta\n".to_string(),
    );
    h.commit(&first, "first").await;
    let before = h.shared_changes(&h.store).await;
    assert_eq!(before[0].0, "shared/alpha.md");
    assert_eq!(before[1].0, "shared/beta.md");

    // Git commit times have one-second resolution.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let clown = MemoryAuthor {
        name: "Clown".to_string(),
        email: "clown@agents.pagis.local".to_string(),
    };
    let revision = h.revision().await;
    h.store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &clown,
            &write("shared/beta.md", "# Beta v2\n"),
            revision.as_deref(),
            "second",
            Some(&RunId::generate()),
        )
        .await
        .expect("commit");
    let mut removal = MemoryChangeset::default();
    removal
        .deletes
        .insert(ScopedPath::parse("shared/alpha.md").unwrap());
    removal.writes.insert(
        ScopedPath::parse("shared/gamma.md").unwrap(),
        "# Gamma\n".to_string(),
    );
    h.commit(&removal, "third").await;

    let after = h.shared_changes(&h.store).await;
    let names: Vec<_> = after
        .iter()
        .map(|(path, by, _)| (path.as_str(), by.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![("shared/gamma.md", "Sage"), ("shared/beta.md", "Clown")]
    );
    assert!(after[1].2 > before[1].2, "beta has its later change");
    assert_eq!(
        after,
        h.shared_changes(&h.reboot()).await,
        "a store with no earlier answer reads the same changes"
    );
}

pub async fn list_pages_gives_the_summary_of_each_page_from_the_index(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let subject = pagis_core::subject_page::SubjectPage {
        truth: "Priya leads the pilot.".into(),
        timeline: vec![pagis_core::subject_page::TimelineEntry {
            source_reference: "gmail:conn-1:m1:1".into(),
            source_time: 0,
            words: "A message.".into(),
        }],
        ..Default::default()
    }
    .render();
    let mut first = write("private/subjects/gmail/priya.md", &subject);
    first.writes.insert(
        ScopedPath::parse("private/claims.md").unwrap(),
        "---\ntitle: File a claim\nkind: Procedure\n---\n".to_string(),
    );
    h.commit(&first, "first").await;

    let pages = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Private,
            &every_page(),
        )
        .await
        .expect("private pages")
        .pages;

    assert_eq!(pages[0].excerpt, "");
    assert_eq!(pages[1].excerpt, "Priya leads the pilot.");

    let rows: Vec<_> = pages
        .iter()
        .map(|page| {
            (
                page.path.display(),
                page.title.as_str(),
                page.kind.as_deref(),
                page.source_connection_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "private/claims.md".to_string(),
                "File a claim",
                Some("Procedure"),
                None
            ),
            (
                "private/subjects/gmail/priya.md".to_string(),
                "priya",
                None,
                Some("conn-1")
            ),
        ]
    );
}

pub async fn list_pages_hides_a_source_page_from_an_access_without_its_grant(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/open.md", "---\ntitle: Open\n---\n"), "first")
        .await;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let permitted = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let revision = h.revision().await;
    h.store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &permitted,
            &author(),
            &write(
                "shared/from-a-source.md",
                "---\ntitle: From a source\n---\n",
            ),
            revision.as_deref(),
            "Reflected on a source",
            Some(&RunId::generate()),
        )
        .await
        .expect("commit");

    let titles = |pages: Vec<pagis_core::MemoryPageEntry>| {
        pages.into_iter().map(|page| page.title).collect::<Vec<_>>()
    };
    let with_grant = h
        .store
        .list_pages(
            &h.workspace_id,
            &permitted,
            pagis_core::MemoryScope::Shared,
            &every_page(),
        )
        .await
        .unwrap()
        .pages;
    assert_eq!(titles(with_grant.clone()), vec!["From a source", "Open"]);
    assert_eq!(with_grant[0].exposures, vec![exposure]);
    let without = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
            &every_page(),
        )
        .await
        .unwrap()
        .pages;
    assert_eq!(titles(without), vec!["Open"]);
}

pub async fn search_hides_pages_that_access_and_forget_rules_refuse(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let forget = std::sync::Arc::new(NoSuppression::default());
    let h = make(forget.clone());
    let unreadable = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let forgotten = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    for (name, exposure) in [("unreadable", &unreadable), ("forgotten", &forgotten)] {
        let permitted = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
        let revision = h.revision().await;
        h.store
            .commit(
                &h.workspace_id,
                &h.agent_id,
                &permitted,
                &author(),
                &write(&format!("shared/{name}.md"), "# Secret\n\nnightjar\n"),
                revision.as_deref(),
                "Learned a source fact",
                Some(&RunId::generate()),
            )
            .await
            .unwrap();
    }
    *forget.forgotten.lock().unwrap() = Some(forgotten.grant_id.clone());
    let access = MemoryAccess::agent(h.agent_id.clone(), [forgotten]);

    let hits = h.search(&access, "nightjar").await;

    assert!(hits.is_empty());
}

pub async fn list_pages_gives_one_part_at_a_time_with_the_total(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    for name in ["a", "b", "c", "d", "e"] {
        h.commit(
            &write(
                &format!("shared/{name}.md"),
                &format!("---\ntitle: Page {name}\n---\n"),
            ),
            "one page",
        )
        .await;
    }
    let part = |after: Option<pagis_core::PageCursor>| {
        let query = pagis_core::PageListQuery {
            after,
            limit: 2,
            ..Default::default()
        };
        let h = &h;
        async move {
            h.store
                .list_pages(
                    &h.workspace_id,
                    &access(&h.agent_id),
                    pagis_core::MemoryScope::Shared,
                    &query,
                )
                .await
                .expect("one part")
        }
    };

    let mut titles = Vec::new();
    let mut after = None;
    let mut parts = 0;
    loop {
        let list = part(after).await;
        assert_eq!(list.total, 5, "each part names the full count");
        titles.extend(list.pages.into_iter().map(|page| page.title));
        parts += 1;
        after = list.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(parts, 3);
    assert_eq!(
        titles,
        vec!["Page e", "Page d", "Page c", "Page b", "Page a"]
    );
}

pub async fn list_pages_narrows_by_kind_and_by_search_text(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut first = write(
        "shared/claims.md",
        "---\ntitle: File a claim\nkind: Procedure\n---\n",
    );
    first.writes.insert(
        ScopedPath::parse("shared/refunds.md").unwrap(),
        "---\ntitle: Refund an order\nkind: Procedure\n---\n".to_string(),
    );
    first.writes.insert(
        ScopedPath::parse("shared/people/priya.md").unwrap(),
        "---\ntitle: Priya Raman\nkind: Person\n---\n".to_string(),
    );
    h.commit(&first, "first").await;
    let titles = |query: pagis_core::PageListQuery| {
        let h = &h;
        async move {
            let list = h
                .store
                .list_pages(
                    &h.workspace_id,
                    &access(&h.agent_id),
                    pagis_core::MemoryScope::Shared,
                    &query,
                )
                .await
                .expect("pages");
            (
                list.pages
                    .into_iter()
                    .map(|page| page.title)
                    .collect::<Vec<_>>(),
                list.total,
            )
        }
    };

    let procedures = titles(pagis_core::PageListQuery {
        kind: Some("Procedure".into()),
        ..every_page()
    })
    .await;
    assert_eq!(
        procedures,
        (
            vec!["File a claim".to_string(), "Refund an order".to_string()],
            2
        )
    );
    let by_title = titles(pagis_core::PageListQuery {
        search: Some("  REFUND ".into()),
        ..every_page()
    })
    .await;
    assert_eq!(by_title, (vec!["Refund an order".to_string()], 1));
    let by_path = titles(pagis_core::PageListQuery {
        search: Some("people/".into()),
        ..every_page()
    })
    .await;
    assert_eq!(by_path, (vec!["Priya Raman".to_string()], 1));
}

pub async fn list_pages_finds_a_subject_page_by_a_word_of_its_truth(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write(
            "shared/subjects/gmail/priya.md",
            &subject("Priya leads the renewal."),
        ),
        "remembered Priya",
    )
    .await;

    let list = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
            &pagis_core::PageListQuery {
                search: Some("RENEWAL".into()),
                ..every_page()
            },
        )
        .await
        .expect("pages");

    assert_eq!(list.total, 1);
    assert_eq!(
        list.pages[0].path.display(),
        "shared/subjects/gmail/priya.md"
    );
}

pub async fn list_pages_searches_title_and_path_when_entity_words_are_null(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut changes = write("shared/by-title.md", "---\ntitle: Needle title\n---\n");
    changes.writes.insert(
        ScopedPath::parse("shared/needle-path.md").unwrap(),
        "---\ntitle: Other title\n---\n".to_string(),
    );
    h.commit(&changes, "remembered ordinary pages").await;

    let list = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
            &pagis_core::PageListQuery {
                search: Some("needle".into()),
                ..every_page()
            },
        )
        .await
        .expect("pages");

    assert_eq!(list.total, 2);
    assert_eq!(list.pages.len(), 2);
}

pub async fn count_pages_counts_the_scope_its_procedures_and_its_authors(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let mut first = write(
        "shared/claims.md",
        "---\ntitle: File a claim\nkind: Procedure\n---\n",
    );
    first.writes.insert(
        ScopedPath::parse("shared/a.md").unwrap(),
        "---\ntitle: A\n---\n".to_string(),
    );
    first.writes.insert(
        ScopedPath::parse("private/craft.md").unwrap(),
        "---\ntitle: Craft\n---\n".to_string(),
    );
    h.commit(&first, "first").await;
    let clown = MemoryAuthor {
        name: "Clown".to_string(),
        email: "clown@agents.pagis.local".to_string(),
    };
    let revision = h.revision().await;
    h.store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &clown,
            &write("shared/b.md", "---\ntitle: B\n---\n"),
            revision.as_deref(),
            "second",
            Some(&RunId::generate()),
        )
        .await
        .expect("commit");

    let counts = h
        .store
        .count_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
        )
        .await
        .expect("counts");

    assert_eq!(counts.pages, 3);
    assert_eq!(counts.procedures, 1);
    assert_eq!(counts.authors, vec![(author(), 2), (clown, 1)]);
}

pub async fn list_pages_of_an_unwritten_workspace_is_empty(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let pages = h
        .store
        .list_pages(
            &h.workspace_id,
            &access(&h.agent_id),
            pagis_core::MemoryScope::Shared,
            &every_page(),
        )
        .await
        .expect("no repository yet")
        .pages;
    assert!(pages.is_empty());
}

pub async fn commit_diff_reports_before_and_after_lines_per_file(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/fact.md", "a\nb\nc\n"), "v1").await;
    let mut second = write("shared/fact.md", "a\nB\nc\n");
    second.writes.insert(
        ScopedPath::parse("private/new.md").unwrap(),
        "fresh\n".to_string(),
    );
    let sha = h.commit(&second, "v2").await;

    let diff = h
        .store
        .commit_diff(&h.workspace_id, &sha, &access(&h.agent_id))
        .await
        .expect("diff");
    assert_eq!(diff.sha, sha);
    assert_eq!(diff.message, "v2");
    assert_eq!(diff.author.name, "Sage");
    assert_eq!(diff.files.len(), 2, "metadata files are not memory files");

    let private = &diff.files[0];
    assert_eq!(private.path.display(), "private/new.md");
    assert_eq!(private.agent_id.as_ref(), Some(&h.agent_id));
    assert_eq!(
        private.hunks,
        vec![pagis_core::MemoryHunk {
            old_start: 0,
            old_lines: vec![],
            new_start: 1,
            new_lines: vec!["fresh".to_string()],
        }]
    );

    let shared = &diff.files[1];
    assert_eq!(shared.path.display(), "shared/fact.md");
    assert_eq!(shared.agent_id, None);
    assert_eq!(shared.hunks.len(), 1);
    assert_eq!(shared.hunks[0].old_start, 2);
    assert_eq!(shared.hunks[0].old_lines, vec!["b".to_string()]);
    assert_eq!(shared.hunks[0].new_start, 2);
    assert_eq!(shared.hunks[0].new_lines, vec!["B".to_string()]);
}

pub async fn commit_diff_of_an_unknown_commit_is_not_found(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/fact.md", "a\n"), "v1").await;
    let result = h
        .store
        .commit_diff(
            &h.workspace_id,
            "0123456789012345678901234567890123456789",
            &access(&h.agent_id),
        )
        .await;
    assert!(
        matches!(result, Err(MemoryError::NotFound(_))),
        "{result:?}"
    );
}

pub async fn a_page_write_puts_its_links_in_the_page_index(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(
        &write(
            "private/subjects/gmail/priya.md",
            &pagis_core::subject_page::SubjectPage {
                front_matter: pagis_core::subject_page::FrontMatter {
                    title: Some("Priya Sharma".into()),
                    kind: Some("Person".into()),
                    aliases: Vec::new(),
                    links: vec!["shared/subjects/gmail/acme.md".into()],
                },
                truth: "Priya leads [[private/subjects/gmail/northwind.md]]. \
                        She met [[Vignesh]] once."
                    .into(),
                ..Default::default()
            }
            .render(),
        ),
        "a page that names two others",
    )
    .await;
    h.commit(
        &write(
            "shared/subjects/gmail/acme.md",
            &subject("Acme buys paper."),
        ),
        "the shared page the link names",
    )
    .await;

    let brief = h
        .store
        .brief_entries(&h.workspace_id, &access(&h.agent_id), "missing")
        .await
        .unwrap();
    let priya = brief
        .entries
        .iter()
        .find(|entry| entry.path == "private/subjects/gmail/priya.md")
        .expect("the page");

    assert_eq!(
        priya.links,
        vec![
            "shared/subjects/gmail/acme.md".to_string(),
            // The target of this link has no page yet. The edge stays:
            // it marks a page worth writing later.
            "private/subjects/gmail/northwind.md".to_string(),
        ],
        "a [[...]] that is no scope-relative path is prose, not an edge"
    );
}

pub async fn forget_preview_counts_history_and_begin_rejects_a_changed_revision(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let forget = Arc::new(NoSuppression::default());
    let h = make(forget.clone());
    h.commit(&write("shared/kept.md", "kept\n"), "keep").await;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let first = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "first\n"),
            h.revision().await.as_deref(),
            "source",
            None,
        )
        .await
        .unwrap();
    let second = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "second\n"),
            Some(&first),
            "source again",
            None,
        )
        .await
        .unwrap();
    *forget.selected.lock().unwrap() = Some(exposure.grant_id);
    let target = pagis_core::ForgetTarget::Account {
        connection_id: pagis_core::ConnectionId::generate(),
    };
    let preview = h
        .store
        .preview_forget(&h.workspace_id, &target)
        .await
        .unwrap();
    assert_eq!(preview.memory_revision.as_deref(), Some(second.as_str()));
    assert_eq!(preview.memory_paths, 1);
    assert_eq!(preview.memory_revisions, 2);
    assert_eq!(h.read("shared/kept.md").await.unwrap(), "kept\n");
    let keys = pagis_core::TenantKeys::new(Arc::new(pagis_core::MemorySecretStore::default()));
    let operation = h
        .store
        .begin_forget(&h.workspace_id, &target, &preview, 17, &keys)
        .await
        .unwrap();
    assert_eq!(operation.created_at, 17);
    h.commit(&write("shared/other.md", "other\n"), "new memory")
        .await;
    assert!(matches!(
        h.store
            .begin_forget(&h.workspace_id, &target, &preview, 18, &keys)
            .await,
        Err(MemoryError::Conflict)
    ));
}

pub async fn forget_purge_removes_each_copy_and_invalidates_the_old_history(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let forget = Arc::new(NoSuppression::default());
    let h = make(forget.clone());
    h.commit(&write("shared/kept.md", "kept\n"), "keep").await;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let old = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "secretword\n"),
            h.revision().await.as_deref(),
            "source sentence",
            None,
        )
        .await
        .unwrap();
    *forget.forgotten.lock().unwrap() = Some(exposure.grant_id);
    h.store
        .purge_forgotten(&h.workspace_id, "forget-1")
        .await
        .unwrap();
    assert!(matches!(
        h.store
            .read(
                &h.workspace_id,
                &allowed,
                &ScopedPath::parse("shared/source.md").unwrap()
            )
            .await,
        Err(MemoryError::NotFound(_))
    ));
    assert_eq!(h.read("shared/kept.md").await.unwrap(), "kept\n");
    assert!(h.search(&allowed, "secretword").await.is_empty());
    assert_eq!(
        h.store
            .count_pages(&h.workspace_id, &allowed, pagis_core::MemoryScope::Shared)
            .await
            .unwrap()
            .pages,
        1
    );
    assert!(matches!(
        h.store.commit_diff(&h.workspace_id, &old, &allowed).await,
        Err(MemoryError::NotFound(_))
    ));
    let head = h.revision().await.unwrap();
    assert_ne!(head, old);
    assert_eq!(
        h.store
            .commit_diff(&h.workspace_id, &head, &allowed)
            .await
            .unwrap()
            .message,
        "Forgotten dependent note history"
    );
    assert!(
        h.store
            .brief_entries(&h.workspace_id, &allowed, &old)
            .await
            .unwrap()
            .entries
            .iter()
            .all(|entry| entry.changed)
    );
}

pub async fn reverting_a_forgotten_revision_is_unavailable(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let forget = Arc::new(NoSuppression::default());
    let h = make(forget.clone());
    h.commit(&write("shared/kept.md", "kept\n"), "keep").await;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "source\n"),
            h.revision().await.as_deref(),
            "source",
            None,
        )
        .await
        .unwrap();
    *forget.forgotten.lock().unwrap() = Some(exposure.grant_id);
    assert!(matches!(
        h.store
            .revert(&h.workspace_id, &sha, &allowed, &sha, &author())
            .await,
        Err(MemoryError::Unavailable)
    ));
    assert_eq!(h.revision().await.as_deref(), Some(sha.as_str()));
}

pub async fn revert_keeps_later_changes_to_other_lines(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    h.commit(&write("shared/fact.md", "a\nb\nc\nd\ne\n"), "first")
        .await;
    let changed = h
        .commit(
            &write("shared/fact.md", "A\nb\nc\nd\ne\n"),
            "change first line",
        )
        .await;
    let latest = h
        .commit(
            &write("shared/fact.md", "A\nb\nc\nd\nE\n"),
            "change last line",
        )
        .await;
    h.store
        .revert(
            &h.workspace_id,
            &changed,
            &access(&h.agent_id),
            &latest,
            &author(),
        )
        .await
        .unwrap();
    assert_eq!(h.read("shared/fact.md").await.unwrap(), "a\nb\nc\nd\nE\n");
}

pub async fn a_failed_write_keeps_all_files_and_the_revision(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let before = h.commit(&write("shared/kept.md", "kept\n"), "keep").await;
    let mut invalid = write("shared/a.md", "must not appear\n");
    invalid.writes.insert(
        ScopedPath::parse("shared/z.md").unwrap(),
        "x".repeat(pagis_core::MAX_MEMORY_FILE_BYTES + 1),
    );
    assert!(matches!(
        h.store
            .commit(
                &h.workspace_id,
                &h.agent_id,
                &access(&h.agent_id),
                &author(),
                &invalid,
                Some(&before),
                "invalid",
                None
            )
            .await,
        Err(MemoryError::Invalid(_))
    ));
    assert_eq!(h.revision().await.as_deref(), Some(before.as_str()));
    assert!(matches!(
        h.read("shared/a.md").await,
        Err(MemoryError::NotFound(_))
    ));
    h.commit(&write("shared/b.md", "valid\n"), "valid").await;
    assert!(matches!(
        h.read("shared/a.md").await,
        Err(MemoryError::NotFound(_))
    ));
}

pub async fn owner_cannot_change_a_forgotten_file(make: &dyn Fn(Arc<NoSuppression>) -> Harness) {
    let forget = Arc::new(NoSuppression::default());
    let h = make(forget.clone());
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 1,
    };
    let allowed = MemoryAccess::agent(h.agent_id.clone(), [exposure.clone()]);
    let sha = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &allowed,
            &author(),
            &write("shared/source.md", "source\n"),
            None,
            "source",
            None,
        )
        .await
        .unwrap();
    *forget.forgotten.lock().unwrap() = Some(exposure.grant_id);
    let owner = MemoryAccess::Owner {
        agent_id: h.agent_id.clone(),
    };
    assert!(matches!(
        h.store
            .commit(
                &h.workspace_id,
                &h.agent_id,
                &owner,
                &author(),
                &write("shared/source.md", "edit\n"),
                Some(&sha),
                "edit",
                None
            )
            .await,
        Err(MemoryError::Unavailable)
    ));
    assert_eq!(h.revision().await.as_deref(), Some(sha.as_str()));
}

pub async fn writing_the_same_content_keeps_the_file_change_and_has_no_content_diff(
    make: &dyn Fn(Arc<NoSuppression>) -> Harness,
) {
    let h = make(Arc::new(NoSuppression::default()));
    let first = h.commit(&write("shared/note.md", "note\n"), "first").await;
    let before = h.shared_changes(&h.store).await;
    let other = MemoryAuthor {
        name: "Clown".to_string(),
        email: "clown@agents.pagis.local".to_string(),
    };
    let second = h
        .store
        .commit(
            &h.workspace_id,
            &h.agent_id,
            &access(&h.agent_id),
            &other,
            &write("shared/note.md", "note\n"),
            Some(&first),
            "same content",
            None,
        )
        .await
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(h.shared_changes(&h.store).await, before);
    assert!(
        h.store
            .commit_diff(&h.workspace_id, &second, &access(&h.agent_id))
            .await
            .unwrap()
            .files
            .is_empty()
    );
}

/// Run all MemoryStore tests with a factory that supplies independent memory.
#[macro_export]
macro_rules! memory_suite {
    ($factory:path) => {
        #[tokio::test]
        async fn writing_the_same_content_keeps_the_file_change_and_has_no_content_diff() { $crate::memory_suite::writing_the_same_content_keeps_the_file_change_and_has_no_content_diff(&$factory).await; }

        #[tokio::test]
        async fn owner_cannot_change_a_forgotten_file() { $crate::memory_suite::owner_cannot_change_a_forgotten_file(&$factory).await; }

        #[tokio::test]
        async fn search_returns_a_fact_file_for_a_word_in_its_body() { $crate::memory_suite::search_returns_a_fact_file_for_a_word_in_its_body(&$factory).await; }
        #[tokio::test]
        async fn search_finds_a_page_by_an_alias_in_its_front_matter() { $crate::memory_suite::search_finds_a_page_by_an_alias_in_its_front_matter(&$factory).await; }
        #[tokio::test]
        async fn unwritten_scopes_serve_the_seed_indexes() { $crate::memory_suite::unwritten_scopes_serve_the_seed_indexes(&$factory).await; }
        #[tokio::test]
        async fn a_commit_persists_writes_and_reads_serve_them() { $crate::memory_suite::a_commit_persists_writes_and_reads_serve_them(&$factory).await; }
        #[tokio::test]
        async fn brief_entries_rank_the_newest_change_first_with_the_score_and_the_words() { $crate::memory_suite::brief_entries_rank_the_newest_change_first_with_the_score_and_the_words(&$factory).await; }
        #[tokio::test]
        async fn brief_entries_mark_the_pages_changed_after_the_cursor() { $crate::memory_suite::brief_entries_mark_the_pages_changed_after_the_cursor(&$factory).await; }
        #[tokio::test]
        async fn brief_entries_hide_a_source_page_from_an_access_without_its_grant() { $crate::memory_suite::brief_entries_hide_a_source_page_from_an_access_without_its_grant(&$factory).await; }
        #[tokio::test]
        async fn brief_entries_of_an_unwritten_workspace_are_empty() { $crate::memory_suite::brief_entries_of_an_unwritten_workspace_are_empty(&$factory).await; }
        #[tokio::test]
        async fn private_scopes_are_disjoint_between_agents() { $crate::memory_suite::private_scopes_are_disjoint_between_agents(&$factory).await; }
        #[tokio::test]
        async fn the_commit_carries_author_message_and_run_trailer() { $crate::memory_suite::the_commit_carries_author_message_and_run_trailer(&$factory).await; }
        #[tokio::test]
        async fn recovery_finds_only_the_exact_pending_review_commit() { $crate::memory_suite::recovery_finds_only_the_exact_pending_review_commit(&$factory).await; }
        #[tokio::test]
        async fn a_delete_removes_the_file() { $crate::memory_suite::a_delete_removes_the_file(&$factory).await; }
        #[tokio::test]
        async fn an_empty_changeset_refuses_to_commit() { $crate::memory_suite::an_empty_changeset_refuses_to_commit(&$factory).await; }
        #[tokio::test]
        async fn later_commits_win_whole_file() { $crate::memory_suite::later_commits_win_whole_file(&$factory).await; }
        #[tokio::test]
        async fn concurrent_whole_file_commits_reject_the_stale_base() { $crate::memory_suite::concurrent_whole_file_commits_reject_the_stale_base(&$factory).await; }
        #[tokio::test]
        async fn revert_makes_an_inverse_commit_by_the_user() { $crate::memory_suite::revert_makes_an_inverse_commit_by_the_user(&$factory).await; }
        #[tokio::test]
        async fn revert_aborts_cleanly_when_later_commits_conflict() { $crate::memory_suite::revert_aborts_cleanly_when_later_commits_conflict(&$factory).await; }
        #[tokio::test]
        async fn workspaces_get_disjoint_repositories() { $crate::memory_suite::workspaces_get_disjoint_repositories(&$factory).await; }
        #[tokio::test]
        async fn a_commit_without_a_run_has_no_run_trailer() { $crate::memory_suite::a_commit_without_a_run_has_no_run_trailer(&$factory).await; }
        #[tokio::test]
        async fn source_exposed_files_and_indexes_are_invisible_without_current_access() { $crate::memory_suite::source_exposed_files_and_indexes_are_invisible_without_current_access(&$factory).await; }
        #[tokio::test]
        async fn owner_edits_preserve_the_existing_source_scope() { $crate::memory_suite::owner_edits_preserve_the_existing_source_scope(&$factory).await; }
        #[tokio::test]
        async fn reverting_a_revert_cannot_restore_a_revoked_source() { $crate::memory_suite::reverting_a_revert_cannot_restore_a_revoked_source(&$factory).await; }
        #[tokio::test]
        async fn concurrent_index_loads_return_one_revision_for_both_scopes() { $crate::memory_suite::concurrent_index_loads_return_one_revision_for_both_scopes(&$factory).await; }
        #[tokio::test]
        async fn list_pages_serves_one_scope_newest_change_first_with_its_author() { $crate::memory_suite::list_pages_serves_one_scope_newest_change_first_with_its_author(&$factory).await; }
        #[tokio::test]
        async fn list_pages_follows_new_commits_after_its_first_answer() { $crate::memory_suite::list_pages_follows_new_commits_after_its_first_answer(&$factory).await; }
        #[tokio::test]
        async fn list_pages_gives_the_summary_of_each_page_from_the_index() { $crate::memory_suite::list_pages_gives_the_summary_of_each_page_from_the_index(&$factory).await; }
        #[tokio::test]
        async fn list_pages_hides_a_source_page_from_an_access_without_its_grant() { $crate::memory_suite::list_pages_hides_a_source_page_from_an_access_without_its_grant(&$factory).await; }
        #[tokio::test]
        async fn search_hides_pages_that_access_and_forget_rules_refuse() { $crate::memory_suite::search_hides_pages_that_access_and_forget_rules_refuse(&$factory).await; }
        #[tokio::test]
        async fn list_pages_gives_one_part_at_a_time_with_the_total() { $crate::memory_suite::list_pages_gives_one_part_at_a_time_with_the_total(&$factory).await; }
        #[tokio::test]
        async fn list_pages_narrows_by_kind_and_by_search_text() { $crate::memory_suite::list_pages_narrows_by_kind_and_by_search_text(&$factory).await; }
        #[tokio::test]
        async fn list_pages_finds_a_subject_page_by_a_word_of_its_truth() { $crate::memory_suite::list_pages_finds_a_subject_page_by_a_word_of_its_truth(&$factory).await; }
        #[tokio::test]
        async fn list_pages_searches_title_and_path_when_entity_words_are_null() { $crate::memory_suite::list_pages_searches_title_and_path_when_entity_words_are_null(&$factory).await; }
        #[tokio::test]
        async fn count_pages_counts_the_scope_its_procedures_and_its_authors() { $crate::memory_suite::count_pages_counts_the_scope_its_procedures_and_its_authors(&$factory).await; }
        #[tokio::test]
        async fn list_pages_of_an_unwritten_workspace_is_empty() { $crate::memory_suite::list_pages_of_an_unwritten_workspace_is_empty(&$factory).await; }
        #[tokio::test]
        async fn commit_diff_reports_before_and_after_lines_per_file() { $crate::memory_suite::commit_diff_reports_before_and_after_lines_per_file(&$factory).await; }
        #[tokio::test]
        async fn commit_diff_of_an_unknown_commit_is_not_found() { $crate::memory_suite::commit_diff_of_an_unknown_commit_is_not_found(&$factory).await; }
        #[tokio::test]
        async fn a_page_write_puts_its_links_in_the_page_index() { $crate::memory_suite::a_page_write_puts_its_links_in_the_page_index(&$factory).await; }
        #[tokio::test]
        async fn forget_preview_counts_history_and_begin_rejects_a_changed_revision() { $crate::memory_suite::forget_preview_counts_history_and_begin_rejects_a_changed_revision(&$factory).await; }
        #[tokio::test]
        async fn forget_purge_removes_each_copy_and_invalidates_the_old_history() { $crate::memory_suite::forget_purge_removes_each_copy_and_invalidates_the_old_history(&$factory).await; }
        #[tokio::test]
        async fn reverting_a_forgotten_revision_is_unavailable() { $crate::memory_suite::reverting_a_forgotten_revision_is_unavailable(&$factory).await; }
        #[tokio::test]
        async fn revert_keeps_later_changes_to_other_lines() { $crate::memory_suite::revert_keeps_later_changes_to_other_lines(&$factory).await; }
        #[tokio::test]
        async fn a_failed_write_keeps_all_files_and_the_revision() { $crate::memory_suite::a_failed_write_keeps_all_files_and_the_revision(&$factory).await; }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_macro_names_each_suite_body() {
        let source = include_str!("memory_suite.rs");
        let (bodies, emitted) = source.split_once("macro_rules! memory_suite").unwrap();
        for line in bodies
            .lines()
            .filter(|line| line.starts_with("pub async fn "))
        {
            let name = line
                .strip_prefix("pub async fn ")
                .unwrap()
                .split('(')
                .next()
                .unwrap();
            assert!(
                emitted.contains(&format!("memory_suite::{name}(&")),
                "{name} is missing from the macro"
            );
        }
    }
}
