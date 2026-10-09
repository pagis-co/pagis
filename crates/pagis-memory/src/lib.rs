//! The git memory store: one lazily initialized git
//! repository per workspace under the daemon's data directory, with
//! `shared/` and `agents/<agent_id>/` directories. Scope privacy is
//! this API: an agent addresses `shared/…` and `private/…` only, and
//! `private/` maps to its own directory. Commits serialize behind a
//! per-workspace async mutex; each changeset checks its expected HEAD
//! before replacing whole files.

use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use git2::{Oid, Repository, Signature};
use pagis_core::memory_page::PageSummary;
use pagis_core::subject_page::{BriefEntry, FrontMatter, SubjectPage};
use pagis_core::{
    AgentId, BriefEntries, IndexedPage, MEMORY_INDEX_FILE, MemoryAccess, MemoryAuthor,
    MemoryChangeset, MemoryCommitDiff, MemoryContentKind, MemoryError, MemoryExposure, MemoryFile,
    MemoryFileDiff, MemoryHunk, MemoryIndexes, MemoryPageCounts, MemoryPageEntry, MemoryPageIndex,
    MemoryPageList, MemoryScope, MemorySearchHit, MemoryStore, PageCursor, PageIndexHead,
    PageIndexUpdate, PageListQuery, RunId, ScopedPath, WorkspaceId, validate_content,
};

mod purge;

/// A list previews current truth, never the page's source Timeline.
fn page_excerpt(content: &str) -> String {
    let subject = SubjectPage::parse(content);
    let body = if subject.layout_valid {
        subject.page.truth.as_str()
    } else {
        FrontMatter::split(content).1
    };
    body.lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty() || line.starts_with('#'))
        .take_while(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

/// The seed header a scope's index shows before the agent writes one.
/// It states the convention; it is not committed.
fn seed_index(title: &str) -> String {
    format!(
        "# {title}\n\n\
         This index has one line per fact file: `- [Title](path.md) — hook`.\n\
         Keep it curated; it is loaded into context verbatim. Fact files\n\
         are Markdown, read on demand. This file is MEMORY.md at the\n\
         scope root.\n"
    )
}

impl From<git2::Error> for MemoryErrorWrapper {
    fn from(err: git2::Error) -> Self {
        MemoryErrorWrapper(MemoryError::Storage(err.to_string()))
    }
}

impl From<std::io::Error> for MemoryErrorWrapper {
    fn from(err: std::io::Error) -> Self {
        MemoryErrorWrapper(MemoryError::Storage(err.to_string()))
    }
}

impl From<MemoryError> for MemoryErrorWrapper {
    fn from(err: MemoryError) -> Self {
        MemoryErrorWrapper(err)
    }
}

/// Internal `?`-friendly wrapper: git2 and io errors both fold into
/// [`MemoryError::Storage`].
struct MemoryErrorWrapper(MemoryError);

type GitResult<T> = Result<T, MemoryErrorWrapper>;

/// The git implementation of the memory seam.
pub struct GitMemoryStore {
    root: PathBuf,
    forget: Arc<dyn pagis_core::ForgetStore>,
    /// One commit/revert lock per workspace.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One swap gate per workspace. A read holds it shared; only the
    /// directory exchange at the end of a forget rebuild holds it
    /// exclusive, so a read sees the repository before or after the
    /// exchange and never during it.
    swaps: Mutex<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
    /// Commits since the last repack, per workspace. A workspace that
    /// is absent has not committed in this process, and its first
    /// commit repacks (ADR-0007).
    repacks: Mutex<HashMap<String, u32>>,
    /// True after this process reported that git is not on the PATH.
    git_missing: std::sync::atomic::AtomicBool,
    indexer: PageIndexer,
}

/// How many commits one workspace makes between two repacks. The
/// repack itself is `git gc --auto`, which applies git's own
/// thresholds, so an idle repository costs one process start.
const COMMITS_PER_REPACK: u32 = 100;

impl GitMemoryStore {
    /// `root` is the directory that holds one repository per workspace
    /// (`<root>/<workspace_id>/`), created lazily.
    pub fn new(
        root: impl Into<PathBuf>,
        forget: Arc<dyn pagis_core::ForgetStore>,
        pages: Arc<dyn MemoryPageIndex>,
    ) -> Self {
        let root = root.into();
        // libgit2 hashes each object it reads to verify it, which git
        // itself does not do. On a long history walk the check costs
        // about two thirds of the time. The setting is for the process.
        git2::opts::strict_hash_verification(false);
        Self {
            indexer: PageIndexer {
                root: root.clone(),
                pages,
                locks: Arc::default(),
            },
            root,
            forget,
            locks: Mutex::new(HashMap::new()),
            swaps: Mutex::new(HashMap::new()),
            repacks: Mutex::new(HashMap::new()),
            git_missing: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Pack the loose objects of one workspace repository when the
    /// commit count is due. One commit per source arrival made a
    /// repository of 5477 commits and 67957 loose objects with no
    /// pack, because nothing repacked it (ADR-0007).
    ///
    /// `git gc --auto` applies git's own thresholds, so a repository
    /// with nothing to pack costs one process start. The caller holds
    /// the workspace write lock, which the forget rebuild also holds
    /// for its directory exchange, so the repack cannot run beside a
    /// commit or beside that exchange. libgit2 has no repack, so this
    /// is the git binary.
    async fn repack_if_due(&self, workspace_id: &WorkspaceId, dir: &Path) {
        use std::sync::atomic::Ordering;
        let due = {
            let mut repacks = self.repacks.lock().expect("workspace repack map");
            match repacks.entry(workspace_id.as_str().to_string()) {
                Entry::Vacant(slot) => {
                    // The first commit after boot repacks, so a
                    // repository that grew loose objects before this
                    // process started does not wait.
                    slot.insert(0);
                    true
                }
                Entry::Occupied(mut slot) => {
                    let count = slot.get_mut();
                    *count += 1;
                    let due = *count >= COMMITS_PER_REPACK;
                    if due {
                        *count = 0;
                    }
                    due
                }
            }
        };
        if !due || self.git_missing.load(Ordering::Relaxed) {
            return;
        }
        let dir = dir.to_path_buf();
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                // git detaches an automatic repack by default. This
                // one stays in front, so it ends before the write lock
                // does.
                .args(["-c", "gc.autoDetach=false", "gc", "--auto", "--quiet"])
                .status()
        })
        .await;
        match result {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                tracing::warn!(%workspace_id, code = status.code(), "the memory repack failed");
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                if !self.git_missing.swap(true, Ordering::Relaxed) {
                    tracing::warn!("git is not on the PATH, so the memory repositories stay loose");
                }
            }
            Ok(Err(error)) => tracing::warn!(%workspace_id, %error, "the memory repack failed"),
            Err(error) => tracing::warn!(%workspace_id, %error, "the memory repack failed"),
        }
    }

    fn repo_dir(&self, workspace_id: &WorkspaceId) -> PathBuf {
        self.root.join(workspace_id.as_str())
    }

    fn lock_for(&self, workspace_id: &WorkspaceId) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().expect("workspace lock map");
        Arc::clone(
            locks
                .entry(workspace_id.as_str().to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// The index rows of one scope that the access reads, with the
    /// stamp of each, newest change first. The result also names the
    /// repository root of the scope. The list reads the Page Index
    /// only (ADR-0008): it opens no file and no history.
    async fn readable_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
    ) -> Result<(String, Vec<(IndexedPage, Vec<MemoryExposure>)>), MemoryError> {
        let root = match scope {
            MemoryScope::Shared => "shared/".to_string(),
            MemoryScope::Private => format!(
                "agents/{}/",
                access
                    .agent_id()
                    .expect("memory access names an agent")
                    .as_str()
            ),
        };
        {
            let gate = self.swap_for(workspace_id);
            let _stable = gate.read().await;
            self.indexer.sync(workspace_id).await?;
        }
        let rows = self
            .indexer
            .pages
            .pages(workspace_id, &root)
            .await
            .map_err(storage)?;
        let readable = self
            .filter_readable_pages(workspace_id, access, rows)
            .await?;
        Ok((root, readable))
    }

    /// Apply the read permission and Forget provenance rules to Page
    /// Index rows. Lists and searches use this one rule.
    async fn filter_readable_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        rows: Vec<IndexedPage>,
    ) -> Result<Vec<(IndexedPage, Vec<MemoryExposure>)>, MemoryError> {
        // Many pages share one stamp, so each different stamp asks the
        // forget store one time.
        let mut permitted: Vec<(Vec<MemoryExposure>, bool)> = Vec::new();
        let mut readable = Vec::new();
        for mut row in rows {
            let Some(exposures) = row.exposures.take() else {
                continue;
            };
            if !access.permits(&exposures) {
                continue;
            }
            let known = permitted.iter().find(|(stamp, _)| *stamp == exposures);
            let allowed = match known {
                Some((_, allowed)) => *allowed,
                None => {
                    let allowed = self
                        .forget
                        .permits_provenance(workspace_id, &exposures)
                        .await
                        .map_err(storage)?;
                    permitted.push((exposures.clone(), allowed));
                    allowed
                }
            };
            if allowed {
                readable.push((row, exposures));
            }
        }
        Ok(readable)
    }

    /// Bring the Page Index to HEAD in the background. A page list
    /// does its own sync, so this only keeps that sync short. The
    /// daemon calls it at boot, which is also the time of the first
    /// build from the full history.
    pub fn sync_page_index_later(&self, workspace_id: &WorkspaceId) {
        let indexer = self.indexer.clone();
        let gate = self.swap_for(workspace_id);
        let workspace_id = workspace_id.clone();
        tokio::spawn(async move {
            let _stable = gate.read().await;
            if let Err(error) = indexer.sync(&workspace_id).await {
                tracing::warn!(%workspace_id, %error, "the memory Page Index did not sync");
            }
        });
    }

    pub(crate) fn swap_for(&self, workspace_id: &WorkspaceId) -> Arc<tokio::sync::RwLock<()>> {
        let mut swaps = self.swaps.lock().expect("workspace swap map");
        Arc::clone(
            swaps
                .entry(workspace_id.as_str().to_string())
                .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(()))),
        )
    }
}

/// Open the workspace repository, or report every miss as not-found
/// for `path`.
fn open_repo(dir: &Path) -> Result<Option<Repository>, MemoryErrorWrapper> {
    if !dir.exists() {
        return Ok(None);
    }
    Ok(Some(Repository::open(dir)?))
}

/// Read one committed file from HEAD; the working tree is derived
/// state, HEAD is the truth.
fn read_blob(repo: &Repository, blob_id: Oid, display: &str) -> GitResult<String> {
    let blob = repo
        .find_blob(blob_id)
        .map_err(|_| MemoryError::NotFound(display.to_string()))?;
    let content = std::str::from_utf8(blob.content())
        .map_err(|_| MemoryError::Storage(format!("{display} is not UTF-8")))?;
    Ok(content.to_string())
}

fn head_revision(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|head| head.target())
        .map(|oid| oid.to_string())
}

fn metadata_rel(rel: &str) -> String {
    format!(".pagis/exposure/{}", metadata_name(rel))
}

fn metadata_name(rel: &str) -> String {
    format!("{}.json", hex::encode(rel.as_bytes()))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FileMetadata {
    exposures: Vec<MemoryExposure>,
    kind: MemoryContentKind,
}

fn memory_path(path: &str) -> bool {
    path.starts_with("shared/") || path.starts_with("agents/")
}

fn read_file_at_head(
    repo: &Repository,
    rel: &str,
    display: &str,
    access: &MemoryAccess,
) -> GitResult<MemoryFile> {
    let head = repo
        .head()
        .map_err(|_| MemoryError::NotFound(display.into()))?
        .peel_to_commit()?;
    read_file_from_commit(repo, &head, rel, display, access)
}

fn read_file_from_commit(
    repo: &Repository,
    head: &git2::Commit<'_>,
    rel: &str,
    display: &str,
    access: &MemoryAccess,
) -> GitResult<MemoryFile> {
    let tree = head.tree()?;
    let entry = tree
        .get_path(Path::new(rel))
        .map_err(|_| MemoryError::NotFound(display.to_string()))?;
    ExposureDir::open(repo, &tree)?.read_file(repo, head, entry.id(), rel, display, access)
}

/// The exposure directory of one revision, held open for the read of
/// many files. The directory has one entry per memory file, and libgit2
/// does not cache a tree of that size. A path lookup from the root
/// parses the directory again for each file, which made a read of 8391
/// Subject Pages take 150 seconds. A lookup in the open tree is a
/// search by name.
struct ExposureDir<'repo> {
    tree: Option<git2::Tree<'repo>>,
}

impl<'repo> ExposureDir<'repo> {
    fn open(repo: &'repo Repository, root: &git2::Tree<'_>) -> GitResult<Self> {
        let tree = match root.get_path(Path::new(".pagis/exposure")) {
            Ok(entry) => Some(repo.find_tree(entry.id())?),
            Err(_) => None,
        };
        Ok(Self { tree })
    }

    /// The stamp of one file. A file with no stamp has `None`.
    fn metadata(
        &self,
        repo: &Repository,
        rel: &str,
        display: &str,
    ) -> GitResult<Option<FileMetadata>> {
        let name = metadata_name(rel);
        let Some(entry) = self.tree.as_ref().and_then(|tree| tree.get_name(&name)) else {
            return Ok(None);
        };
        serde_json::from_str::<FileMetadata>(&read_blob(repo, entry.id(), display)?)
            .map(Some)
            .map_err(|error| MemoryError::Storage(error.to_string()).into())
    }

    /// One file of the revision `head`, from the blob the caller found
    /// in the tree.
    fn read_file(
        &self,
        repo: &Repository,
        head: &git2::Commit<'_>,
        blob_id: Oid,
        rel: &str,
        display: &str,
        access: &MemoryAccess,
    ) -> GitResult<MemoryFile> {
        let content = read_blob(repo, blob_id, display)?;
        // A file without a daemon stamp cannot prove that source data
        // did not influence it. Hide it rather than silently treating it
        // as unrestricted.
        let Some(metadata) = self.metadata(repo, rel, display)? else {
            return Err(MemoryError::Unavailable.into());
        };
        if !access.permits(&metadata.exposures) {
            return Err(MemoryError::Unavailable.into());
        }
        Ok(MemoryFile {
            content,
            revision: head.id().to_string(),
            exposures: metadata.exposures,
            kind: metadata.kind,
        })
    }
}

fn signature(author: &MemoryAuthor) -> GitResult<Signature<'static>> {
    Ok(Signature::now(&author.name, &author.email)?)
}

fn author_of(commit: &git2::Commit<'_>) -> MemoryAuthor {
    let author = commit.author();
    MemoryAuthor {
        name: author.name().unwrap_or_default().to_string(),
        email: author.email().unwrap_or_default().to_string(),
    }
}

/// The repository paths one commit changed against its first parent.
/// The diff skips each subtree the two commits share, so the cost
/// follows the size of the change and not the size of the tree.
///
/// The flag of a path is true when the commit deleted the file.
fn changed_paths(repo: &Repository, commit: &git2::Commit<'_>) -> GitResult<Vec<(String, bool)>> {
    let tree = commit.tree()?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let mut options = git2::DiffOptions::new();
    options.skip_binary_check(true);
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
    Ok(diff
        .deltas()
        .filter_map(|delta| {
            let path = delta.new_file().path().and_then(Path::to_str)?;
            Some((path.to_string(), delta.status() == git2::Delta::Deleted))
        })
        .collect())
}

/// The commits from `head` back to `until`, newest first. No `until`
/// gives the full history.
fn history(repo: &Repository, head: Oid, until: Option<Oid>) -> GitResult<git2::Revwalk<'_>> {
    let mut revisions = repo.revwalk()?;
    revisions.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    revisions.push(head)?;
    if let Some(until) = until {
        revisions.hide(until)?;
    }
    Ok(revisions)
}

/// The memory file that an exposure stamp path belongs to.
fn stamped_file(path: &str) -> Option<String> {
    let name = path
        .strip_prefix(".pagis/exposure/")?
        .strip_suffix(".json")?;
    String::from_utf8(hex::decode(name).ok()?).ok()
}

/// The update that brings a Page Index from `held` to `head`. It reads
/// only the commits after `held`. A history that does not contain
/// `held` (first build, forget rebuild) is read from its start, and
/// the update replaces the index.
fn page_index_update(
    repo: &Repository,
    head: Oid,
    held: Option<&PageIndexHead>,
) -> GitResult<PageIndexUpdate> {
    let held = held.and_then(|held| {
        let revision = Oid::from_str(&held.revision).ok()?;
        repo.graph_descendant_of(head, revision)
            .unwrap_or(false)
            .then_some((revision, held.next_position))
    });
    let first_position = held.map_or(0, |(_, next_position)| next_position);
    let revisions =
        history(repo, head, held.map(|(revision, _)| revision))?.collect::<Result<Vec<_>, _>>()?;
    let count = revisions.len() as u64;
    // The last change of each file the commits name, and the files
    // whose stamp they name.
    let mut last_changes = Vec::new();
    let mut removed = Vec::new();
    let mut seen = HashSet::new();
    let mut stamped = HashSet::new();
    for (rank, revision) in revisions.into_iter().enumerate() {
        let commit = repo.find_commit(revision)?;
        for (path, deleted) in changed_paths(repo, &commit)? {
            if let Some(file) = stamped_file(&path) {
                stamped.insert(file);
                continue;
            }
            // The walk is newest first, so the first commit that
            // names a path holds its last change.
            if !memory_path(&path) || !seen.insert(path.clone()) {
                continue;
            }
            if deleted {
                removed.push(path);
            } else {
                last_changes.push((
                    path,
                    first_position + count - 1 - rank as u64,
                    commit.time().seconds() * 1000,
                    author_of(&commit),
                ));
            }
        }
    }

    // The summary and the stamp of a file come from HEAD. One walk
    // gives the blob of each file; a lookup by path parses the large
    // directories again for each file.
    let tree = repo.find_commit(head)?.tree()?;
    let mut blobs = HashMap::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |directory, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            let path = format!("{directory}{}", entry.name().unwrap_or_default());
            if memory_path(&path) {
                blobs.insert(path, entry.id());
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    let exposure_dir = ExposureDir::open(repo, &tree)?;
    let exposures_of = |path: &str| -> GitResult<Option<Vec<MemoryExposure>>> {
        Ok(exposure_dir
            .metadata(repo, path, path)?
            .map(|metadata| metadata.exposures))
    };
    let mut changed = Vec::new();
    for (path, position, changed_at, changed_by) in last_changes {
        let Some(blob_id) = blobs.get(&path) else {
            continue;
        };
        let blob = repo.find_blob(*blob_id)?;
        let body = String::from_utf8_lossy(blob.content()).into_owned();
        let summary = PageSummary::read(&path, &body);
        changed.push(IndexedPage {
            exposures: exposures_of(&path)?,
            path,
            position,
            changed_at,
            changed_by,
            title: summary.title,
            body,
            kind: summary.kind,
            source_connection_id: summary.source_connection_id,
            brief: summary.brief,
            links: summary.links,
        });
    }
    let mut restamped = Vec::new();
    for path in stamped {
        if blobs.contains_key(&path) && !seen.contains(&path) {
            restamped.push((path.clone(), exposures_of(&path)?));
        }
    }
    Ok(PageIndexUpdate {
        replace: held.is_none(),
        head: PageIndexHead {
            revision: head.to_string(),
            next_position: first_position + count,
        },
        changed,
        restamped,
        removed,
    })
}

/// Keeps the Page Index of each workspace at the HEAD of its
/// repository (ADR-0008).
#[derive(Clone)]
struct PageIndexer {
    root: PathBuf,
    pages: Arc<dyn MemoryPageIndex>,
    /// One sync per workspace at a time, so two callers do not read
    /// the same commits.
    locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl PageIndexer {
    /// Bring the index to HEAD. A workspace with no repository has an
    /// empty index. The caller holds the swap gate of the workspace.
    async fn sync(&self, workspace_id: &WorkspaceId) -> Result<(), MemoryError> {
        let lock = {
            let mut locks = self.locks.lock().expect("page index lock map");
            Arc::clone(locks.entry(workspace_id.as_str().to_string()).or_default())
        };
        let _one_sync = lock.lock().await;
        let held = self.pages.head(workspace_id).await.map_err(storage)?;
        let dir = self.root.join(workspace_id.as_str());
        let update = tokio::task::spawn_blocking(move || -> GitResult<_> {
            let Some(repo) = open_repo(&dir)? else {
                return Ok(None);
            };
            let Some(head) = repo.head().ok().and_then(|head| head.target()) else {
                return Ok(None);
            };
            if held
                .as_ref()
                .is_some_and(|held| held.revision == head.to_string())
            {
                return Ok(None);
            }
            page_index_update(&repo, head, held.as_ref()).map(Some)
        })
        .await
        .map_err(|error| MemoryError::Storage(error.to_string()))?
        .map_err(|error| error.0)?;
        if let Some(update) = update {
            if update.replace {
                tracing::info!(%workspace_id, "building the memory Page Index from the history");
            }
            self.pages
                .apply(workspace_id, &update)
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
}

fn storage(error: pagis_core::StoreError) -> MemoryError {
    MemoryError::Storage(error.to_string())
}

/// The scoped form of one repository path, with the owner of a
/// private file. A path outside the two scopes is `None`.
fn scoped(rel: &str) -> Option<(ScopedPath, Option<AgentId>)> {
    if let Some(rel) = rel.strip_prefix("shared/") {
        return ScopedPath::parse(&format!("shared/{rel}"))
            .ok()
            .map(|path| (path, None));
    }
    let (agent_id, rel) = rel.strip_prefix("agents/")?.split_once('/')?;
    ScopedPath::parse(&format!("private/{rel}"))
        .ok()
        .map(|path| (path, Some(AgentId::from(agent_id.to_string()))))
}

/// Add the exposures of every memory file a diff touches, on both
/// sides of the change, to `required`. A file with no metadata on a
/// side is unavailable, as in a read.
fn collect_diff_exposures(
    repo: &Repository,
    old_tree: &git2::Tree<'_>,
    new_tree: &git2::Tree<'_>,
    diff: &git2::Diff<'_>,
    access: &MemoryAccess,
    required: &mut Vec<MemoryExposure>,
) -> GitResult<()> {
    for delta in diff.deltas() {
        for (revision, file) in [(old_tree, delta.old_file()), (new_tree, delta.new_file())] {
            let Some(path) = file.path().and_then(Path::to_str) else {
                continue;
            };
            if !memory_path(path) || file.id().is_zero() {
                continue;
            }
            let metadata = revision
                .get_path(Path::new(&metadata_rel(path)))
                .map_err(|_| MemoryError::Unavailable)?;
            let blob = repo.find_blob(metadata.id())?;
            let metadata: FileMetadata = serde_json::from_slice(blob.content())
                .map_err(|error| MemoryError::Storage(error.to_string()))?;
            if !access.permits(&metadata.exposures) {
                return Err(MemoryError::Unavailable.into());
            }
            for exposure in metadata.exposures {
                if !required.contains(&exposure) {
                    required.push(exposure);
                }
            }
        }
    }
    Ok(())
}

/// The `Exposure:` trailer of one commit.
fn commit_exposures(commit: &git2::Commit<'_>) -> GitResult<Vec<MemoryExposure>> {
    Ok(commit
        .message()
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix("Exposure: "))
        .map(serde_json::from_str::<Vec<MemoryExposure>>)
        .transpose()
        .map_err(|error| MemoryError::Storage(error.to_string()))?
        .unwrap_or_default())
}

/// The hunks of one commit against its first parent, memory files
/// only, with zero context lines so each hunk is one changed range.
/// Returns the exposures the caller must still check with the forget
/// store.
fn commit_diff_hunks(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    access: &MemoryAccess,
) -> GitResult<(Vec<MemoryFileDiff>, Vec<MemoryExposure>)> {
    let new_tree = commit.tree()?;
    let old_tree = match commit.parent(0) {
        Ok(parent) => parent.tree()?,
        Err(_) => repo.find_tree(repo.treebuilder(None)?.write()?)?,
    };
    let mut options = git2::DiffOptions::new();
    options.context_lines(0);
    let diff = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(&mut options))?;
    let mut required = commit_exposures(commit)?;
    if !access.permits(&required) {
        return Err(MemoryError::Unavailable.into());
    }
    collect_diff_exposures(repo, &old_tree, &new_tree, &diff, access, &mut required)?;

    // `None` stands for a file outside the two scopes (metadata), so
    // its hunks are dropped. A callback that returns false would stop
    // the whole iteration.
    let files = std::cell::RefCell::new(Vec::<Option<MemoryFileDiff>>::new());
    let text = |line: &git2::DiffLine<'_>| {
        String::from_utf8_lossy(line.content())
            .trim_end_matches('\n')
            .to_string()
    };
    diff.foreach(
        &mut |delta, _| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(Path::to_str)
                .unwrap_or_default();
            files
                .borrow_mut()
                .push(scoped(path).map(|(path, agent_id)| MemoryFileDiff {
                    path,
                    agent_id,
                    hunks: Vec::new(),
                }));
            true
        },
        None,
        Some(&mut |_, hunk| {
            if let Some(Some(file)) = files.borrow_mut().last_mut() {
                file.hunks.push(MemoryHunk {
                    old_start: hunk.old_start(),
                    old_lines: Vec::new(),
                    new_start: hunk.new_start(),
                    new_lines: Vec::new(),
                });
            }
            true
        }),
        Some(&mut |_, _, line| {
            let mut files = files.borrow_mut();
            let Some(Some(file)) = files.last_mut() else {
                return true;
            };
            let Some(hunk) = file.hunks.last_mut() else {
                return true;
            };
            match line.origin() {
                '-' => hunk.old_lines.push(text(&line)),
                '+' => hunk.new_lines.push(text(&line)),
                _ => {}
            }
            true
        }),
    )?;
    let files = files.into_inner().into_iter().flatten().collect();
    Ok((files, required))
}

/// Apply one changeset onto the working tree and index, then commit.
#[expect(
    clippy::too_many_arguments,
    reason = "the commit keeps its security and audit inputs explicit"
)]
fn apply_and_commit(
    dir: &Path,
    agent_id: &AgentId,
    author: &MemoryAuthor,
    changeset: &MemoryChangeset,
    access: &MemoryAccess,
    expected_revision: Option<&str>,
    message: &str,
    run_id: Option<&RunId>,
) -> GitResult<String> {
    std::fs::create_dir_all(dir)?;
    let repo = match Repository::open(dir) {
        Ok(repo) => repo,
        Err(_) => Repository::init(dir)?,
    };
    if head_revision(&repo).as_deref() != expected_revision {
        return Err(MemoryError::Conflict.into());
    }
    let mut index = repo.index()?;

    let mut exposures: Vec<MemoryExposure> = match access {
        MemoryAccess::Owner { .. } => Vec::new(),
        MemoryAccess::Agent { exposures, .. } => exposures.to_vec(),
    };

    for path in changeset.writes.keys().chain(changeset.deletes.iter()) {
        let rel = path.repo_path(agent_id);
        match read_file_at_head(&repo, &rel, &path.display(), access) {
            Ok(file) => {
                for exposure in file.exposures {
                    if !exposures.contains(&exposure) {
                        exposures.push(exposure);
                    }
                }
            }
            Err(MemoryErrorWrapper(MemoryError::NotFound(_))) => {}
            Err(error) => return Err(error),
        }
    }

    for (path, content) in &changeset.writes {
        validate_content(content)?;
        let rel = path.repo_path(agent_id);
        let file = dir.join(&rel);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, content)?;
        index.add_path(Path::new(&rel))?;
        let metadata = metadata_rel(&rel);
        let metadata_file = dir.join(&metadata);
        if let Some(parent) = metadata_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let metadata_record = FileMetadata {
            exposures: exposures.clone(),
            kind: if exposures.is_empty() {
                MemoryContentKind::Procedure
            } else {
                MemoryContentKind::DependentView
            },
        };
        std::fs::write(
            &metadata_file,
            serde_json::to_vec(&metadata_record)
                .map_err(|error| MemoryError::Storage(error.to_string()))?,
        )?;
        index.add_path(Path::new(&metadata))?;
    }
    for path in &changeset.deletes {
        let rel = path.repo_path(agent_id);
        let file = dir.join(&rel);
        if file.exists() {
            std::fs::remove_file(&file)?;
            index.remove_path(Path::new(&rel))?;
        }
        let metadata = metadata_rel(&rel);
        let metadata_file = dir.join(&metadata);
        if metadata_file.exists() {
            std::fs::remove_file(metadata_file)?;
            index.remove_path(Path::new(&metadata))?;
        }
    }
    index.write()?;

    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = signature(author)?;
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let exposure = serde_json::to_string(&exposures)
        .map_err(|error| MemoryError::Storage(error.to_string()))?;
    let run_trailer = run_id
        .map(|id| format!("Run-Id: {id}\n"))
        .unwrap_or_default();
    let full_message = format!(
        "{}\n\n{}Exposure: {}\n",
        message.trim(),
        run_trailer,
        exposure
    );
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &full_message,
        &tree,
        &parents,
    )?;
    Ok(oid.to_string())
}

/// Revert one commit: an inverse commit when it applies cleanly, a
/// clean abort when later commits touch the same lines.
fn revert_commit(
    dir: &Path,
    commit_id: &str,
    access: &MemoryAccess,
    expected_revision: &str,
    author: &MemoryAuthor,
) -> GitResult<String> {
    let repo = Repository::open(dir)?;
    let oid = Oid::from_str(commit_id)
        .map_err(|_| MemoryError::Invalid(format!("bad commit id {commit_id:?}")))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|_| MemoryError::NotFound(format!("commit {commit_id}")))?;
    let head = repo.head()?.peel_to_commit()?;
    if head.id().to_string() != expected_revision {
        return Err(MemoryError::Conflict.into());
    }
    let required = commit_exposures(&commit)?;
    if !access.permits(&required) {
        return Err(MemoryError::Unavailable.into());
    }

    let mut index = repo.revert_commit(&commit, &head, 0, None)?;
    if index.has_conflicts() {
        return Err(MemoryError::RevertConflict.into());
    }
    let tree = repo.find_tree(index.write_tree_to(&repo)?)?;
    // Validate both removed and restored revisions. A revert of a
    // revert has no original run trailer to use as proof of access.
    let head_tree = head.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&head_tree), Some(&tree), None)?;
    let mut required = required;
    collect_diff_exposures(&repo, &head_tree, &tree, &diff, access, &mut required)?;
    let signature = signature(author)?;
    let summary = commit
        .summary()
        .ok()
        .flatten()
        .unwrap_or(commit_id)
        .to_string();
    let exposure = serde_json::to_string(&required)
        .map_err(|error| MemoryError::Storage(error.to_string()))?;
    let message = format!("Revert \"{summary}\"\n\nReverts commit {oid}.\nExposure: {exposure}\n");
    let revert_oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &[&head],
    )?;
    // Keep the working tree and index in step with the new HEAD.
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
    Ok(revert_oid.to_string())
}

#[async_trait]
impl MemoryStore for GitMemoryStore {
    async fn find_commit_with_trailers(
        &self,
        workspace_id: &WorkspaceId,
        trailers: &[(&str, &str)],
    ) -> Result<Option<String>, MemoryError> {
        let gate = self.swap_for(workspace_id);
        let _stable = gate.read().await;
        let dir = self.repo_dir(workspace_id);
        let trailers = trailers
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || -> GitResult<Option<String>> {
            let Some(repo) = open_repo(&dir)? else {
                return Ok(None);
            };
            let mut walk = repo.revwalk()?;
            if walk.push_head().is_err() {
                return Ok(None);
            }
            for oid in walk {
                let commit = repo.find_commit(oid?)?;
                let message = commit.message().unwrap_or_default();
                if trailers
                    .iter()
                    .all(|trailer| message.lines().any(|line| line == trailer))
                {
                    return Ok(Some(commit.id().to_string()));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|error| MemoryError::Storage(error.to_string()))?
        .map_err(|error| error.0)
    }

    async fn preview_forget(
        &self,
        workspace: &WorkspaceId,
        target: &pagis_core::ForgetTarget,
    ) -> Result<pagis_core::ForgetPreview, MemoryError> {
        // A preview reads a snapshot and writes nothing, so it takes no
        // write lock: a scan of a large history must not stop the
        // commits that deliver arrivals. The swap gate keeps the
        // directory in place while the scan reads it, and `begin_forget`
        // refuses a preview that a commit has made stale.
        let gate = self.swap_for(workspace);
        let _stable = gate.read().await;
        self.forget_preview(workspace, target).await
    }
    async fn begin_forget(
        &self,
        workspace: &WorkspaceId,
        target: &pagis_core::ForgetTarget,
        preview: &pagis_core::ForgetPreview,
        now: i64,
        keys: &dyn pagis_core::ForgetKeys,
    ) -> Result<pagis_core::ForgetOperation, MemoryError> {
        let lock = self.lock_for(workspace);
        let _serialized = lock.lock().await;
        let current = self.forget_preview(workspace, target).await?;
        if current.revision != preview.revision
            || current.memory_revision != preview.memory_revision
        {
            return Err(MemoryError::Conflict);
        }
        self.forget
            .begin(workspace, target, &preview.revision, now, keys)
            .await
            .map_err(|error| match error {
                pagis_core::StoreError::Conflict(_) => MemoryError::Conflict,
                error => MemoryError::Storage(error.to_string()),
            })
    }
    async fn purge_forgotten(
        &self,
        workspace: &WorkspaceId,
        operation: &str,
    ) -> Result<(), MemoryError> {
        let lock = self.lock_for(workspace);
        let _serialized = lock.lock().await;
        self.purge_forgotten_locked(workspace, operation).await?;
        // The rebuilt history does not contain the revision the Page
        // Index holds, so this sync replaces the rows. A forgotten
        // path leaves the index before the purge reports its end.
        let gate = self.swap_for(workspace);
        let _stable = gate.read().await;
        self.indexer.sync(workspace).await?;
        Ok(())
    }
    async fn load_indexes(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
    ) -> Result<MemoryIndexes, MemoryError> {
        let gate = self.swap_for(workspace_id);
        let _stable = gate.read().await;
        let dir = self.repo_dir(workspace_id);
        let access = access.clone();
        let (shared, private, revision) = tokio::task::spawn_blocking(move || -> GitResult<_> {
            let repo = open_repo(&dir)?;
            let head = repo
                .as_ref()
                .and_then(|repo| repo.head().ok())
                .map(|head| head.peel_to_commit())
                .transpose()?;
            let index = |scope| -> GitResult<Option<MemoryFile>> {
                let path = ScopedPath {
                    scope,
                    rel: MEMORY_INDEX_FILE.into(),
                };
                let Some((repo, head)) = repo.as_ref().zip(head.as_ref()) else {
                    return Ok(None);
                };
                let rel = path.repo_path(access.agent_id().expect("memory access names an agent"));
                match read_file_from_commit(repo, head, &rel, &path.display(), &access) {
                    Ok(file) => Ok(Some(file)),
                    Err(MemoryErrorWrapper(
                        MemoryError::NotFound(_) | MemoryError::Unavailable,
                    )) => Ok(None),
                    Err(error) => Err(error),
                }
            };
            Ok((
                index(MemoryScope::Shared)?,
                index(MemoryScope::Private)?,
                head.map(|head| head.id().to_string()),
            ))
        })
        .await
        .map_err(|error| MemoryError::Storage(error.to_string()))?
        .map_err(|error| error.0)?;
        let mut contents = Vec::new();
        for (file, title) in [(shared, "Shared memory"), (private, "Private memory")] {
            if let Some(file) = file
                && self
                    .forget
                    .permits_provenance(workspace_id, &file.exposures)
                    .await
                    .map_err(|error| MemoryError::Storage(error.to_string()))?
            {
                contents.push(file.content);
            } else {
                contents.push(seed_index(title));
            }
        }
        let private = contents.pop().expect("private index");
        let shared = contents.pop().expect("shared index");
        Ok(MemoryIndexes {
            shared,
            private,
            revision,
        })
    }

    async fn read(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        path: &ScopedPath,
    ) -> Result<MemoryFile, MemoryError> {
        let gate = self.swap_for(workspace_id);
        let _stable = gate.read().await;
        let dir = self.repo_dir(workspace_id);
        let rel = path.repo_path(access.agent_id().expect("all memory access names an agent"));
        let access = access.clone();
        let display = path.display();
        let file = tokio::task::spawn_blocking(move || -> GitResult<MemoryFile> {
            let Some(repo) = open_repo(&dir)? else {
                return Err(MemoryError::NotFound(display).into());
            };
            read_file_at_head(&repo, &rel, &display, &access)
        })
        .await
        .map_err(|err| MemoryError::Storage(err.to_string()))?
        .map_err(|wrapped| wrapped.0)?;
        if !self
            .forget
            .permits_provenance(workspace_id, &file.exposures)
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
        {
            return Err(MemoryError::Unavailable);
        }
        Ok(file)
    }

    async fn brief_entries(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        since_revision: &str,
    ) -> Result<BriefEntries, MemoryError> {
        let (_, shared) = self
            .readable_pages(workspace_id, access, MemoryScope::Shared)
            .await?;
        let (private_root, private) = self
            .readable_pages(workspace_id, access, MemoryScope::Private)
            .await?;
        let Some(head) = self
            .indexer
            .pages
            .head(workspace_id)
            .await
            .map_err(storage)?
        else {
            return Ok(BriefEntries {
                revision: None,
                entries: Vec::new(),
            });
        };

        // A page is changed when its last change is one of the commits
        // after the cursor. The count of those commits gives the first
        // position of a changed page. It reads no tree.
        let dir = self.repo_dir(workspace_id);
        let gate = self.swap_for(workspace_id);
        let (since, head_revision) = (since_revision.to_string(), head.revision.clone());
        let commits_after = {
            let _stable = gate.read().await;
            tokio::task::spawn_blocking(move || -> GitResult<Option<u64>> {
                let Some(repo) = open_repo(&dir)? else {
                    return Ok(None);
                };
                let (Ok(since), Ok(head)) = (Oid::from_str(&since), Oid::from_str(&head_revision))
                else {
                    return Ok(None);
                };
                if since != head && !repo.graph_descendant_of(head, since).unwrap_or(false) {
                    return Ok(None);
                }
                Ok(Some(history(&repo, head, Some(since))?.count() as u64))
            })
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .map_err(|error| error.0)?
        };
        // A cursor that the history does not hold marks each page.
        let first_changed =
            commits_after.map_or(0, |count| head.next_position.saturating_sub(count));

        let pages: Vec<_> = shared
            .into_iter()
            .chain(private)
            .filter_map(|(row, _)| {
                let brief = row.brief?;
                // The selection speaks the scope-relative form, so a
                // link of a readable page speaks it too.
                let scoped = |path: String| match path.strip_prefix(&private_root) {
                    Some(rel) => format!("private/{rel}"),
                    None => path,
                };
                let links = row.links.into_iter().map(scoped).collect();
                Some((scoped(row.path), row.position, brief, links))
            })
            .collect();
        let newest = pages.iter().map(|(_, position, ..)| *position).max();
        let entries = pages
            .into_iter()
            .map(|(path, position, brief, links)| BriefEntry {
                path,
                change_rank: (newest.unwrap_or(position) - position) as usize,
                changed: position >= first_changed,
                words: brief.words.into_iter().collect(),
                links,
            })
            .collect();
        Ok(BriefEntries {
            revision: Some(head.revision),
            entries,
        })
    }

    async fn list_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &PageListQuery,
    ) -> Result<MemoryPageList, MemoryError> {
        let (root, rows) = self.readable_pages(workspace_id, access, scope).await?;
        let needle = query
            .search
            .as_deref()
            .map(|search| search.trim().to_lowercase())
            .filter(|needle| !needle.is_empty());
        let mut matches = rows.into_iter().filter(|(row, _)| {
            (query.kind.is_none() || row.kind == query.kind)
                && needle.as_ref().is_none_or(|needle| {
                    row.title.to_lowercase().contains(needle)
                        || row.path[root.len()..].to_lowercase().contains(needle)
                        || row.brief.as_ref().is_some_and(|brief| {
                            brief
                                .words
                                .iter()
                                .any(|word| word.to_lowercase().contains(needle))
                        })
                })
        });
        let matches: Vec<_> = matches.by_ref().collect();
        let total = matches.len();
        // The rows come newest change first and then in path order, so
        // a row is after the cursor when it is older, or of the same
        // commit with a later path.
        let mut rest = matches.into_iter().filter(|(row, _)| {
            query.after.as_ref().is_none_or(|after| {
                row.position < after.position
                    || (row.position == after.position && row.path[root.len()..] > *after.path)
            })
        });
        let part: Vec<_> = rest.by_ref().take(query.limit).collect();
        let next = rest.next().and(part.last()).map(|(row, _)| PageCursor {
            position: row.position,
            path: row.path[root.len()..].to_string(),
        });
        let pages = part
            .into_iter()
            .map(|(row, exposures)| MemoryPageEntry {
                path: ScopedPath {
                    scope,
                    rel: row.path[root.len()..].to_string(),
                },
                excerpt: page_excerpt(&row.body),
                title: row.title,
                kind: row.kind,
                source_connection_id: row.source_connection_id,
                changed_at: row.changed_at,
                changed_by: row.changed_by,
                exposures,
            })
            .collect();
        Ok(MemoryPageList { pages, next, total })
    }

    async fn search_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>, MemoryError> {
        let root = match scope {
            MemoryScope::Shared => "shared/".to_string(),
            MemoryScope::Private => format!(
                "agents/{}/",
                access
                    .agent_id()
                    .expect("memory access names an agent")
                    .as_str()
            ),
        };
        {
            let gate = self.swap_for(workspace_id);
            let _stable = gate.read().await;
            self.indexer.sync(workspace_id).await?;
        }
        let indexed = self
            .indexer
            .pages
            .search(workspace_id, &root, query, limit.saturating_mul(4))
            .await
            .map_err(storage)?;
        let details: HashMap<_, _> = indexed
            .iter()
            .map(|hit| (hit.page.path.clone(), (hit.snippet.clone(), hit.rank)))
            .collect();
        let rows = indexed.into_iter().map(|hit| hit.page).collect();
        let readable = self
            .filter_readable_pages(workspace_id, access, rows)
            .await?;
        Ok(readable
            .into_iter()
            .take(limit)
            .map(|(page, _)| {
                let path = match page.path.strip_prefix(&root) {
                    Some(rel) if scope == MemoryScope::Private => format!("private/{rel}"),
                    _ => page.path.clone(),
                };
                let (snippet, rank) = details.get(&page.path).cloned().unwrap_or_default();
                MemorySearchHit {
                    snippet,
                    rank,
                    path,
                    title: page.title,
                    kind: page.kind,
                }
            })
            .collect())
    }

    async fn count_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
    ) -> Result<MemoryPageCounts, MemoryError> {
        let (_, rows) = self.readable_pages(workspace_id, access, scope).await?;
        let mut counts = MemoryPageCounts {
            pages: rows.len(),
            ..Default::default()
        };
        for (row, _) in rows {
            if row.kind.as_deref() == Some("Procedure") {
                counts.procedures += 1;
            }
            match counts
                .authors
                .iter_mut()
                .find(|(author, _)| *author == row.changed_by)
            {
                Some((_, pages)) => *pages += 1,
                None => counts.authors.push((row.changed_by, 1)),
            }
        }
        counts
            .authors
            .sort_by_key(|(_, pages)| std::cmp::Reverse(*pages));
        Ok(counts)
    }

    async fn commit_diff(
        &self,
        workspace_id: &WorkspaceId,
        commit_id: &str,
        access: &MemoryAccess,
    ) -> Result<MemoryCommitDiff, MemoryError> {
        let gate = self.swap_for(workspace_id);
        let _stable = gate.read().await;
        let dir = self.repo_dir(workspace_id);
        let commit_id = commit_id.to_string();
        let access = access.clone();
        let (diff, required) = tokio::task::spawn_blocking(move || -> GitResult<_> {
            let Some(repo) = open_repo(&dir)? else {
                return Err(MemoryError::NotFound(format!("commit {commit_id}")).into());
            };
            let oid = Oid::from_str(&commit_id)
                .map_err(|_| MemoryError::Invalid(format!("bad commit id {commit_id:?}")))?;
            let commit = repo
                .find_commit(oid)
                .map_err(|_| MemoryError::NotFound(format!("commit {commit_id}")))?;
            let (files, required) = commit_diff_hunks(&repo, &commit, &access)?;
            Ok((
                MemoryCommitDiff {
                    sha: commit.id().to_string(),
                    message: commit
                        .summary()
                        .unwrap_or_default()
                        .unwrap_or_default()
                        .to_string(),
                    author: author_of(&commit),
                    committed_at: commit.time().seconds() * 1000,
                    files,
                },
                required,
            ))
        })
        .await
        .map_err(|error| MemoryError::Storage(error.to_string()))?
        .map_err(|error| error.0)?;
        if !self
            .forget
            .permits_provenance(workspace_id, &required)
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
        {
            return Err(MemoryError::Unavailable);
        }
        Ok(diff)
    }

    async fn commit(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        access: &MemoryAccess,
        author: &MemoryAuthor,
        changeset: &MemoryChangeset,
        expected_revision: Option<&str>,
        message: &str,
        run_id: Option<&RunId>,
    ) -> Result<String, MemoryError> {
        if changeset.is_empty() {
            return Err(MemoryError::Invalid(
                "an empty changeset has nothing to commit".to_string(),
            ));
        }
        let lock = self.lock_for(workspace_id);
        let _serialized = lock.lock().await;
        if !self
            .forget
            .permits_provenance(workspace_id, access.exposures())
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
        {
            return Err(MemoryError::Unavailable);
        }
        let dir = self.repo_dir(workspace_id);
        let repo_dir = dir.clone();
        let agent_id = agent_id.clone();
        let author = author.clone();
        let changeset = changeset.clone();
        let access = access.clone();
        let expected_revision = expected_revision.map(str::to_string);
        let message = message.to_string();
        let run_id = run_id.cloned();
        let revision = tokio::task::spawn_blocking(move || {
            apply_and_commit(
                &dir,
                &agent_id,
                &author,
                &changeset,
                &access,
                expected_revision.as_deref(),
                &message,
                run_id.as_ref(),
            )
        })
        .await
        .map_err(|err| MemoryError::Storage(err.to_string()))?
        .map_err(|wrapped| wrapped.0)?;
        // The repack runs under the same write lock as the commit, so
        // no commit and no forget rebuild touches the repository while
        // git packs it (ADR-0007).
        self.repack_if_due(workspace_id, &repo_dir).await;
        self.sync_page_index_later(workspace_id);
        Ok(revision)
    }

    async fn revert(
        &self,
        workspace_id: &WorkspaceId,
        commit_id: &str,
        access: &MemoryAccess,
        expected_revision: &str,
        author: &MemoryAuthor,
    ) -> Result<String, MemoryError> {
        let lock = self.lock_for(workspace_id);
        let _serialized = lock.lock().await;
        let dir = self.repo_dir(workspace_id);
        if !dir.exists() {
            return Err(MemoryError::NotFound(format!("commit {commit_id}")));
        }
        let commit_id = commit_id.to_string();
        let access = access.clone();
        let expected_revision = expected_revision.to_string();
        let author = author.clone();
        tokio::task::spawn_blocking(move || {
            revert_commit(&dir, &commit_id, &access, &expected_revision, &author)
        })
        .await
        .map_err(|err| MemoryError::Storage(err.to_string()))?
        .map_err(|wrapped| wrapped.0)
    }
}
