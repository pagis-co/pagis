//! Trait-seam tests of the git memory store on real
//! temp-directory repositories.

use pagis_core::{
    AgentId, GrantId, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryError, MemoryExposure,
    MemoryStore, RunId, ScopedPath, WorkspaceId,
};
use pagis_memory::GitMemoryStore;
use tempfile::TempDir;

struct Harness {
    store: GitMemoryStore,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    /// The Page Index outlives a store, as the database does.
    pages: std::sync::Arc<pagis_core::VolatilePageIndex>,
    _root: TempDir,
}

fn harness() -> Harness {
    harness_with(std::sync::Arc::new(NoSuppression::default()))
}

fn harness_with(forget: std::sync::Arc<NoSuppression>) -> Harness {
    let root = tempfile::tempdir().expect("temp root");
    let pages = std::sync::Arc::new(pagis_core::VolatilePageIndex::default());
    Harness {
        store: GitMemoryStore::new(root.path(), forget, pages.clone()),
        pages,
        workspace_id: WorkspaceId::generate(),
        agent_id: AgentId::generate(),
        _root: root,
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

    fn repo_dir(&self) -> std::path::PathBuf {
        self._root.path().join(self.workspace_id.as_str())
    }

    /// A second store over the same repositories. It stands for a
    /// restart of the daemon, which counts commits from zero again.
    fn reboot(&self) -> GitMemoryStore {
        GitMemoryStore::new(
            self._root.path(),
            std::sync::Arc::new(NoSuppression::default()),
            self.pages.clone(),
        )
    }

    async fn commit(&self, changeset: &MemoryChangeset, message: &str) -> String {
        self.commit_on(&self.store, changeset, message).await
    }

    async fn commit_on(
        &self,
        store: &GitMemoryStore,
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
    async fn shared_changes(&self, store: &GitMemoryStore) -> Vec<(String, String, i64)> {
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

#[tokio::test]
async fn search_returns_a_fact_file_for_a_word_in_its_body() {
    let h = harness();
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

/// Memory Search reads the whole body of a file, and the front matter
/// block is part of it. An alias is therefore searchable as soon as it
/// is written, and the search index needs no column for it.
#[tokio::test]
async fn search_finds_a_page_by_an_alias_in_its_front_matter() {
    let h = harness();
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

#[tokio::test]
async fn unwritten_scopes_serve_the_seed_indexes() {
    let h = harness();

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

#[tokio::test]
async fn a_commit_persists_writes_and_reads_serve_them() {
    let h = harness();
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

fn subject(truth: &str) -> String {
    pagis_core::subject_page::SubjectPage {
        truth: truth.into(),
        ..Default::default()
    }
    .render()
}

#[tokio::test]
async fn brief_entries_rank_the_newest_change_first_with_the_score_and_the_words() {
    let h = harness();
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

#[tokio::test]
async fn brief_entries_mark_the_pages_changed_after_the_cursor() {
    let h = harness();
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

#[tokio::test]
async fn brief_entries_hide_a_source_page_from_an_access_without_its_grant() {
    let h = harness();
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

#[tokio::test]
async fn brief_entries_of_an_unwritten_workspace_are_empty() {
    let h = harness();
    let brief = h
        .store
        .brief_entries(&h.workspace_id, &access(&h.agent_id), "missing")
        .await
        .unwrap();
    assert_eq!(brief.revision, None);
    assert!(brief.entries.is_empty());
}

#[tokio::test]
async fn private_scopes_are_disjoint_between_agents() {
    let h = harness();
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

#[tokio::test]
async fn the_commit_carries_author_message_and_run_trailer() {
    let h = harness();
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

    let repo = git2::Repository::open(h._root.path().join(h.workspace_id.as_str())).unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(&sha).unwrap())
        .unwrap();
    assert_eq!(commit.author().name().unwrap(), "Sage");
    assert_eq!(commit.author().email().unwrap(), "sage@agents.pagis.local");
    let message = commit.message().unwrap();
    assert!(message.starts_with("Remembered a fact"), "{message}");
    assert!(message.contains(&format!("Run-Id: {run_id}")), "{message}");
}

#[tokio::test]
async fn recovery_finds_only_the_exact_pending_review_commit() {
    let h = harness();
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

#[tokio::test]
async fn a_delete_removes_the_file() {
    let h = harness();
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

#[tokio::test]
async fn an_empty_changeset_refuses_to_commit() {
    let h = harness();

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

#[tokio::test]
async fn later_commits_win_whole_file() {
    let h = harness();
    h.commit(&write("shared/fact.md", "first\n"), "first").await;
    h.commit(&write("shared/fact.md", "second\n"), "second")
        .await;

    assert_eq!(h.read("shared/fact.md").await.unwrap(), "second\n");
}

#[tokio::test]
async fn concurrent_whole_file_commits_reject_the_stale_base() {
    let h = harness();
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
#[tokio::test]
async fn revert_makes_an_inverse_commit_by_the_user() {
    let h = harness();
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
    let repo = git2::Repository::open(h._root.path().join(h.workspace_id.as_str())).unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(&revert_sha).unwrap())
        .unwrap();
    assert_eq!(commit.author().name().unwrap(), "User");
    assert!(commit.message().unwrap().starts_with("Revert"));
}

#[tokio::test]
async fn revert_aborts_cleanly_when_later_commits_conflict() {
    let h = harness();
    let sha = h.commit(&write("shared/fact.md", "v1\n"), "v1").await;
    h.commit(&write("shared/fact.md", "v2\n"), "v2").await;

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
    let repo = git2::Repository::open(h._root.path().join(h.workspace_id.as_str())).unwrap();
    let mut walk = repo.revwalk().unwrap();
    walk.push_head().unwrap();
    assert_eq!(walk.count(), 2);
}

#[tokio::test]
async fn workspaces_get_disjoint_repositories() {
    let h = harness();
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

#[tokio::test]
async fn a_commit_without_a_run_has_no_run_trailer() {
    let h = harness();

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

    let repo = git2::Repository::open(h._root.path().join(h.workspace_id.as_str())).unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(&sha).unwrap())
        .unwrap();
    let message = commit.message().unwrap();
    assert!(message.starts_with("Onboarding: the user's name\n"));
    assert!(!message.contains("Run-Id:"));
}

#[tokio::test]
async fn source_exposed_files_and_indexes_are_invisible_without_current_access() {
    let h = harness();
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

#[tokio::test]
async fn owner_edits_preserve_the_existing_source_scope() {
    let h = harness();
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

#[tokio::test]
async fn reverting_a_revert_cannot_restore_a_revoked_source() {
    let h = harness();
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

#[tokio::test]
async fn concurrent_index_loads_return_one_revision_for_both_scopes() {
    let h = harness();
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

/// The repack is git's own `gc --auto`, so these tests need the git
/// binary. A machine without it skips them with a clear message.
fn git_present() -> bool {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Write `count` loose objects into `objects/17`. git estimates the
/// loose-object count of a repository from that one directory and
/// multiplies what it finds by 256, so a fixture that fills it makes
/// the estimate certain. A fixture of blobs in any directory would
/// leave the test to chance.
fn fill_the_sampled_object_directory(dir: &std::path::Path, count: usize) {
    let repo = git2::Repository::open(dir).expect("repository");
    let mut written = 0;
    let mut index = 0;
    while written < count {
        let content = format!("loose object {index}\n");
        index += 1;
        let oid = git2::Oid::hash_object(git2::ObjectType::Blob, content.as_bytes()).expect("hash");
        if oid.to_string().starts_with("17") {
            repo.blob(content.as_bytes()).expect("blob");
            written += 1;
        }
    }
}

/// A changeset of `count` files. A commit makes each file and its
/// metadata reachable, so a repack can pack them. git keeps an
/// unreachable object loose, so free blobs alone would prove nothing.
fn many_files(count: usize) -> MemoryChangeset {
    let mut changeset = MemoryChangeset::default();
    for index in 0..count {
        changeset.writes.insert(
            ScopedPath::parse(&format!("shared/facts/fact-{index}.md")).unwrap(),
            format!("# Fact {index}\n"),
        );
    }
    changeset
}

/// Lower git's own repack threshold, so a few hundred loose objects
/// are already too many for this repository.
fn lower_the_repack_threshold(dir: &std::path::Path) {
    let repo = git2::Repository::open(dir).expect("repository");
    let mut config = repo.config().expect("config");
    config.set_i32("gc.auto", 50).expect("gc.auto");
}

fn pack_count(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join(".git/objects/pack"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|kind| kind == "pack"))
                .count()
        })
        .unwrap_or(0)
}

fn loose_object_count(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir.join(".git/objects")) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() == 2 && name.chars().all(|letter| letter.is_ascii_hexdigit()) {
            count += std::fs::read_dir(entry.path())
                .map(|files| files.filter_map(Result::ok).count())
                .unwrap_or(0);
        }
    }
    count
}

#[tokio::test]
async fn a_repository_with_loose_objects_gets_a_pack() {
    if !git_present() {
        eprintln!("skipped: `git --version` failed, so the repack cannot run");
        return;
    }
    let h = harness();
    h.commit(&many_files(50), "many facts").await;
    let dir = h.repo_dir();
    fill_the_sampled_object_directory(&dir, 8);
    lower_the_repack_threshold(&dir);
    assert_eq!(pack_count(&dir), 0, "the repository starts without a pack");
    let loose = loose_object_count(&dir);

    // A new store counts from zero, and the first commit after boot
    // repacks.
    let rebooted = h.reboot();
    h.commit_on(&rebooted, &write("shared/fact.md", "two\n"), "two")
        .await;

    assert!(
        pack_count(&dir) > 0,
        "the repack packs the loose objects: {loose} loose objects before the commit"
    );
    assert!(
        loose_object_count(&dir) < loose,
        "the packed objects are no longer loose"
    );
}

#[tokio::test]
async fn a_repository_with_nothing_to_pack_is_untouched() {
    if !git_present() {
        eprintln!("skipped: `git --version` failed, so the repack cannot run");
        return;
    }
    let h = harness();
    h.commit(&write("shared/fact.md", "one\n"), "one").await;
    let dir = h.repo_dir();
    let loose = loose_object_count(&dir);

    // git's own thresholds hold here: a small repository has nothing
    // to pack, so the repack of the first commit after boot leaves it
    // as it is.
    let rebooted = h.reboot();
    h.commit_on(&rebooted, &write("shared/fact.md", "two\n"), "two")
        .await;

    assert_eq!(pack_count(&dir), 0, "an idle repository gets no pack");
    assert!(
        loose_object_count(&dir) > loose,
        "the second commit only added its own objects"
    );
}

#[tokio::test]
async fn the_commit_count_resets_after_a_repack() {
    if !git_present() {
        eprintln!("skipped: `git --version` failed, so the repack cannot run");
        return;
    }
    let h = harness();
    h.commit(&many_files(50), "many facts").await;
    let dir = h.repo_dir();
    fill_the_sampled_object_directory(&dir, 8);
    lower_the_repack_threshold(&dir);
    let rebooted = h.reboot();
    h.commit_on(&rebooted, &write("shared/fact.md", "two\n"), "two")
        .await;
    let packs = pack_count(&dir);
    assert!(packs > 0, "the first commit after boot repacked");

    // The count restarted at that repack, so the next commit is far
    // from the next repack: the objects it writes stay loose.
    fill_the_sampled_object_directory(&dir, 8);
    let loose = loose_object_count(&dir);
    h.commit_on(&rebooted, &many_files(50), "many facts again")
        .await;

    assert_eq!(pack_count(&dir), packs, "the next commit does not repack");
    assert!(
        loose_object_count(&dir) > loose,
        "the objects of the next commit stay loose"
    );
}
// This isolated Git-store fixture stands in for the durable SQLite
// authority. It suppresses nothing until a test forgets a grant.
#[derive(Default)]
struct NoSuppression {
    forgotten: std::sync::Mutex<Option<GrantId>>,
}
#[async_trait::async_trait]
impl pagis_core::ForgetStore for NoSuppression {
    async fn preview_affects(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::ForgetTarget,
        _: &[MemoryExposure],
    ) -> Result<bool, pagis_core::StoreError> {
        Ok(false)
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
        unreachable!()
    }
    async fn begin(
        &self,
        _: &WorkspaceId,
        _: &pagis_core::ForgetTarget,
        _: &str,
        _: i64,
        _: &dyn pagis_core::ForgetKeys,
    ) -> Result<pagis_core::ForgetOperation, pagis_core::StoreError> {
        unreachable!()
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

/// Remove the source stamp of `rel` and commit that removal, which
/// leaves a note without a daemon stamp.
fn unstamp(dir: &std::path::Path, rel: &str) {
    let repo = git2::Repository::open(dir).expect("repository");
    let metadata = format!(".pagis/exposure/{}.json", hex::encode(rel.as_bytes()));
    std::fs::remove_file(dir.join(&metadata)).expect("stamp file");
    let mut index = repo.index().expect("index");
    index
        .remove_path(std::path::Path::new(&metadata))
        .expect("drop stamp");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("tree id"))
        .expect("tree");
    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .expect("head");
    let signature = git2::Signature::now("Owner", "owner@pagis.local").expect("signature");
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Drop the source stamp",
        &tree,
        &[&head],
    )
    .expect("commit");
}

/// Add an empty exposure stamp without changing the memory file.
fn restamp(dir: &std::path::Path, rel: &str) {
    let repo = git2::Repository::open(dir).expect("repository");
    let metadata = format!(".pagis/exposure/{}.json", hex::encode(rel.as_bytes()));
    std::fs::write(
        dir.join(&metadata),
        r#"{"exposures":[],"kind":"procedure"}"#,
    )
    .expect("stamp file");
    let mut index = repo.index().expect("index");
    index
        .add_path(std::path::Path::new(&metadata))
        .expect("add stamp");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("tree id"))
        .expect("tree");
    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .expect("head");
    let signature = git2::Signature::now("Owner", "owner@pagis.local").expect("signature");
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Restore the source stamp",
        &tree,
        &[&head],
    )
    .expect("stamp commit");
}

#[tokio::test]
async fn a_file_without_its_stamp_is_unavailable() {
    let h = harness();
    h.commit(
        &write("shared/MEMORY.md", "- [Habits](habits.md) — notes\n"),
        "Record a current index",
    )
    .await;
    let dir = h._root.path().join(h.workspace_id.as_str());
    unstamp(&dir, "shared/MEMORY.md");

    assert!(matches!(
        h.store
            .read(
                &h.workspace_id,
                &access(&h.agent_id),
                &ScopedPath::parse("shared/MEMORY.md").unwrap(),
            )
            .await,
        Err(MemoryError::Unavailable)
    ));
}

#[tokio::test]
async fn list_pages_serves_one_scope_newest_change_first_with_its_author() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_follows_new_commits_after_its_first_answer() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_reads_a_replaced_history_from_its_start() {
    let h = harness();
    h.commit(&write("shared/old.md", "# Old\n"), "first").await;
    let listed = h
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
    assert_eq!(listed.len(), 1);

    // A forget rebuild replaces the repository, so the earlier HEAD is
    // not in the new history.
    std::fs::remove_dir_all(h.repo_dir()).expect("remove the repository");
    h.commit(&write("shared/new.md", "# New\n"), "rebuilt")
        .await;

    let listed = h
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
    let paths: Vec<String> = listed.iter().map(|page| page.path.display()).collect();
    assert_eq!(paths, vec!["shared/new.md"]);
}

#[tokio::test]
async fn a_commit_brings_the_page_index_to_its_revision() {
    use pagis_core::MemoryPageIndex;
    let h = harness();
    let first = h.commit(&write("shared/a.md", "# A\n"), "first").await;
    let second = h
        .commit(&write("shared/b.md", "---\ntitle: B\n---\n"), "second")
        .await;
    assert_ne!(first, second);

    // The sync after a commit runs in the background.
    let mut head = None;
    for _ in 0..200 {
        head = h.pages.head(&h.workspace_id).await.unwrap();
        if head.as_ref().is_some_and(|head| head.revision == second) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        head,
        Some(pagis_core::PageIndexHead {
            revision: second,
            next_position: 2,
        })
    );
    let paths: Vec<_> = h
        .pages
        .pages(&h.workspace_id, "shared/")
        .await
        .unwrap()
        .into_iter()
        .map(|change| (change.path, change.position))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("shared/b.md".to_string(), 1),
            ("shared/a.md".to_string(), 0)
        ]
    );
}

#[tokio::test]
async fn the_page_index_holds_memory_files_only_and_drops_a_deleted_file() {
    use pagis_core::MemoryPageIndex;
    let h = harness();
    let mut first = write("shared/kept.md", "# Kept\n");
    first.writes.insert(
        ScopedPath::parse("shared/dropped.md").unwrap(),
        "# Dropped\n".to_string(),
    );
    h.commit(&first, "first").await;
    let mut removal = MemoryChangeset::default();
    removal
        .deletes
        .insert(ScopedPath::parse("shared/dropped.md").unwrap());
    h.commit(&removal, "second").await;

    h.shared_changes(&h.store).await;

    let paths: Vec<_> = h
        .pages
        .pages(&h.workspace_id, "")
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(
        paths,
        vec!["shared/kept.md"],
        "no exposure stamp and no deleted file"
    );
}

#[tokio::test]
async fn a_forget_purge_takes_the_forgotten_path_out_of_the_page_index() {
    use pagis_core::MemoryPageIndex;
    let forget = std::sync::Arc::new(NoSuppression::default());
    let h = harness_with(forget.clone());
    h.commit(&write("shared/kept.md", "# Kept\n"), "first")
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
    h.shared_changes(&h.store).await;
    let before = h.pages.pages(&h.workspace_id, "shared/").await.unwrap();
    assert_eq!(before.len(), 2);

    *forget.forgotten.lock().unwrap() = Some(exposure.grant_id.clone());
    h.store
        .purge_forgotten(&h.workspace_id, "forget-1")
        .await
        .expect("purge");

    let paths: Vec<_> = h
        .pages
        .pages(&h.workspace_id, "shared/")
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(paths, vec!["shared/kept.md"]);
    let head = h.pages.head(&h.workspace_id).await.unwrap();
    assert_eq!(head.map(|head| head.revision), h.revision().await);
}

#[tokio::test]
async fn list_pages_gives_the_summary_of_each_page_from_the_index() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_hides_a_source_page_from_an_access_without_its_grant() {
    let h = harness();
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

#[tokio::test]
async fn search_hides_pages_that_access_and_forget_rules_refuse() {
    let forget = std::sync::Arc::new(NoSuppression::default());
    let h = harness_with(forget.clone());
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

#[tokio::test]
async fn a_stamp_change_alone_reaches_the_index_and_keeps_the_last_change() {
    use pagis_core::MemoryPageIndex;
    let h = harness();
    h.commit(&write("shared/note.md", "# Note\n"), "first")
        .await;
    let before = h.shared_changes(&h.store).await;
    assert_eq!(before.len(), 1);

    // The commit removes the stamp and does not change the file.
    unstamp(&h.repo_dir(), "shared/note.md");

    assert!(
        h.shared_changes(&h.store).await.is_empty(),
        "a file with no stamp is not in a list"
    );
    let rows = h.pages.pages(&h.workspace_id, "shared/").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].exposures, None);
    assert_eq!(
        rows[0].position, 0,
        "the stamp commit is not a change of the file"
    );
    assert_eq!(rows[0].changed_by.name, "Sage");
}

#[tokio::test]
async fn search_follows_a_commit_a_delete_and_a_restamp() {
    let h = harness();
    h.commit(&write("shared/note.md", "# Note\n\nfirstword\n"), "first")
        .await;
    assert_eq!(h.search(&access(&h.agent_id), "firstword").await.len(), 1);

    h.commit(&write("shared/note.md", "# Note\n\nsecondword\n"), "change")
        .await;
    assert!(h.search(&access(&h.agent_id), "firstword").await.is_empty());
    assert_eq!(h.search(&access(&h.agent_id), "secondword").await.len(), 1);

    let mut removal = MemoryChangeset::default();
    removal
        .deletes
        .insert(ScopedPath::parse("shared/note.md").unwrap());
    h.commit(&removal, "delete").await;
    assert!(
        h.search(&access(&h.agent_id), "secondword")
            .await
            .is_empty()
    );

    h.commit(
        &write("shared/restamped.md", "# Restamped\n\nstampword\n"),
        "add",
    )
    .await;
    unstamp(&h.repo_dir(), "shared/restamped.md");
    assert!(h.search(&access(&h.agent_id), "stampword").await.is_empty());
    restamp(&h.repo_dir(), "shared/restamped.md");
    assert_eq!(h.search(&access(&h.agent_id), "stampword").await.len(), 1);
}

#[tokio::test]
async fn list_pages_gives_one_part_at_a_time_with_the_total() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_narrows_by_kind_and_by_search_text() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_finds_a_subject_page_by_a_word_of_its_truth() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_searches_title_and_path_when_entity_words_are_null() {
    let h = harness();
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

#[tokio::test]
async fn count_pages_counts_the_scope_its_procedures_and_its_authors() {
    let h = harness();
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

#[tokio::test]
async fn list_pages_of_an_unwritten_workspace_is_empty() {
    let h = harness();
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

#[tokio::test]
async fn commit_diff_reports_before_and_after_lines_per_file() {
    let h = harness();
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

#[tokio::test]
async fn commit_diff_of_an_unknown_commit_is_not_found() {
    let h = harness();
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

/// A page write puts one edge in the Page Index for each link of the
/// page, with no model call. The selection speaks scope-relative
/// paths, so the links of an entry do too.
#[tokio::test]
async fn a_page_write_puts_its_links_in_the_page_index() {
    let h = harness();
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
