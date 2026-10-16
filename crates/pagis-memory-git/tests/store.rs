//! Git object, Page Index, and repack tests in temporary repositories.

use pagis_core::{
    AgentId, GrantId, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryError, MemoryExposure,
    MemoryStore, RunId, ScopedPath, WorkspaceId,
};
use pagis_memory_git::GitMemoryStore;
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
    let hits = h.search(&access(&h.agent_id), "firstword").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].changed_at, h.shared_changes(&h.store).await[0].2);

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
