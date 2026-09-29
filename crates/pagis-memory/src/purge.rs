//! Forgetting rebuilds a fresh repository, so unreachable objects cannot retain prose.
use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};

struct Revision {
    id: Oid,
    parents: Vec<Oid>,
    files: Vec<(String, Oid, Option<usize>)>,
    message: String,
    message_scope: Option<usize>,
    author: (String, String, git2::Time),
}
struct Archive {
    revisions: Vec<Revision>,
    scopes: Vec<FileMetadata>,
    references: Vec<(String, Oid)>,
    head: Option<String>,
    revision: Option<String>,
}

/// Every memory file of one commit with its blob and its scope, and
/// every source stamp by the file it stamps. The scan carries one
/// snapshot along a line of first parents and applies one commit diff
/// to it at a time, so its cost follows the changed files and not the
/// count of commits multiplied by the count of files.
#[derive(Clone, Default)]
struct Snapshot {
    files: BTreeMap<String, (Oid, Option<usize>)>,
    stamps: BTreeMap<String, Oid>,
}

/// The file that the source stamp `name` belongs to, where `name` is a
/// path below `.pagis/exposure/`. This is the inverse of
/// [`metadata_rel`].
fn stamped_file(name: &str) -> Option<String> {
    String::from_utf8(hex::decode(name.strip_suffix(".json")?).ok()?).ok()
}

impl Snapshot {
    /// Drop `path`, which the commit deleted or moved away.
    fn remove(&mut self, path: &str) {
        let Some(name) = path.strip_prefix(".pagis/exposure/") else {
            self.files.remove(path);
            return;
        };
        // A file keeps its place in the history when its stamp goes. It
        // has no scope then, and it stays hidden.
        if let Some(file) = stamped_file(name) {
            self.stamps.remove(&file);
            if let Some((_, scope)) = self.files.get_mut(&file) {
                *scope = None;
            }
        }
    }
}

/// The scopes of one archive, with two memos: one for a stamp blob that
/// a later commit does not change, and one for equal metadata that
/// arrives as more than one object or in a commit message.
#[derive(Default)]
struct Scopes {
    blobs: HashMap<Oid, Option<usize>>,
    records: HashMap<Vec<u8>, usize>,
}

impl Scopes {
    fn of_record(&mut self, record: &[u8], archive: &mut Archive) -> Option<usize> {
        if let Some(index) = self.records.get(record) {
            return Some(*index);
        }
        let metadata = serde_json::from_slice::<FileMetadata>(record).ok()?;
        let index = archive.scopes.len();
        archive.scopes.push(metadata);
        self.records.insert(record.to_vec(), index);
        Some(index)
    }

    fn of_blob(&mut self, repo: &Repository, blob: Oid, archive: &mut Archive) -> Option<usize> {
        if let Some(index) = self.blobs.get(&blob) {
            return *index;
        }
        let index = repo
            .find_blob(blob)
            .ok()
            .and_then(|record| self.of_record(record.content(), archive));
        self.blobs.insert(blob, index);
        index
    }
}

fn is_blob(mode: git2::FileMode) -> bool {
    matches!(
        mode,
        git2::FileMode::Blob
            | git2::FileMode::BlobExecutable
            | git2::FileMode::BlobGroupWritable
            | git2::FileMode::Link
    )
}

/// Put `path` into the snapshot with the blob the commit gave it. A
/// stamp carries the scope of the file it names, a file takes the scope
/// of the stamp the snapshot already holds. Both directions run, so the
/// order of the deltas of one commit does not change the result.
fn record(
    snapshot: &mut Snapshot,
    path: &str,
    blob: Oid,
    repo: &Repository,
    scopes: &mut Scopes,
    archive: &mut Archive,
) {
    let Some(name) = path.strip_prefix(".pagis/exposure/") else {
        let scope = snapshot
            .stamps
            .get(path)
            .copied()
            .and_then(|stamp| scopes.of_blob(repo, stamp, archive));
        snapshot.files.insert(path.to_owned(), (blob, scope));
        return;
    };
    if let Some(file) = stamped_file(name) {
        snapshot.stamps.insert(file.clone(), blob);
        // A stamp of a file that this commit does not hold gives no
        // scope. The scan reads it when the file comes back.
        if snapshot.files.contains_key(&file) {
            let scope = scopes.of_blob(repo, blob, archive);
            if let Some((_, held)) = snapshot.files.get_mut(&file) {
                *held = scope;
            }
        }
    }
}

fn scan(dir: &Path) -> GitResult<Archive> {
    let mut archive = Archive {
        revisions: vec![],
        scopes: vec![],
        references: vec![],
        head: None,
        revision: None,
    };
    let Some(repo) = open_repo(dir)? else {
        return Ok(archive);
    };
    archive.head = repo
        .find_reference("HEAD")
        .ok()
        .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned));
    archive.revision = head_revision(&repo);
    let mut walk = repo.revwalk()?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)?;
    for reference in repo.references()? {
        let reference = reference?;
        if let (Ok(name), Ok(commit)) = (reference.name(), reference.peel_to_commit()) {
            walk.push(commit.id())?;
            archive.references.push((name.to_owned(), commit.id()));
        }
    }
    if let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) {
        walk.push(head.id())?;
    }
    let mut scopes = Scopes::default();
    let order = walk.collect::<Result<Vec<Oid>, _>>()?;
    // A snapshot is only needed by the commits that name its commit as
    // their first parent, so the scan counts them and drops the
    // snapshot after the last one. A line of commits holds one.
    let mut successors = HashMap::<Oid, usize>::new();
    for id in &order {
        if let Some(parent) = repo.find_commit(*id)?.parent_ids().next() {
            *successors.entry(parent).or_default() += 1;
        }
    }
    let mut snapshots = HashMap::<Oid, Snapshot>::new();
    for id in order {
        let commit = repo.find_commit(id)?;
        let parent = commit.parents().next();
        let mut snapshot = match &parent {
            Some(parent) => {
                let count = successors
                    .get_mut(&parent.id())
                    .expect("a first parent is counted");
                *count -= 1;
                match *count {
                    0 => snapshots.remove(&parent.id()),
                    _ => snapshots.get(&parent.id()).cloned(),
                }
                .expect("a walk gives a parent before its child")
            }
            None => Snapshot::default(),
        };
        // A merge is compared with its first parent alone. The diff
        // holds every difference between that tree and this one, so the
        // snapshot becomes the full tree of this commit, which is what a
        // walk of the whole tree gave.
        let parent_tree = parent.as_ref().map(|parent| parent.tree()).transpose()?;
        let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit.tree()?), None)?;
        for delta in diff.deltas() {
            let (old, new) = (delta.old_file(), delta.new_file());
            if !old.id().is_zero()
                && is_blob(old.mode())
                && let Some(path) = old.path().and_then(Path::to_str)
            {
                snapshot.remove(path);
            }
            if !new.id().is_zero()
                && is_blob(new.mode())
                && let Some(path) = new.path().and_then(Path::to_str)
            {
                record(
                    &mut snapshot,
                    path,
                    new.id(),
                    &repo,
                    &mut scopes,
                    &mut archive,
                );
            }
        }
        let files = snapshot
            .files
            .iter()
            .map(|(path, (blob, scope))| (path.clone(), *blob, *scope))
            .collect();
        if successors.get(&id).is_some_and(|count| *count > 0) {
            snapshots.insert(id, snapshot);
        }
        let message = commit.message().unwrap_or_default().to_owned();
        let exposures = message
            .lines()
            .find_map(|line| line.strip_prefix("Exposure: "))
            .and_then(|record| serde_json::from_str::<Vec<MemoryExposure>>(record).ok());
        let message_scope = exposures
            .and_then(|exposures| {
                serde_json::to_vec(&FileMetadata {
                    exposures,
                    kind: MemoryContentKind::DependentView,
                })
                .ok()
            })
            .and_then(|record| scopes.of_record(&record, &mut archive));
        let author = commit.author();
        archive.revisions.push(Revision {
            id: commit.id(),
            parents: commit.parent_ids().collect(),
            files,
            message,
            message_scope,
            author: (
                author.name().unwrap_or("Pagis").into(),
                author.email().unwrap_or("pagis@local").into(),
                author.when(),
            ),
        });
    }
    Ok(archive)
}

impl GitMemoryStore {
    async fn forget_archive(&self, workspace: &WorkspaceId) -> Result<Archive, MemoryError> {
        let dir = self.repo_dir(workspace);
        tokio::task::spawn_blocking(move || scan(&dir))
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .map_err(|error| error.0)
    }
    async fn excluded_scopes(
        &self,
        workspace: &WorkspaceId,
        archive: &Archive,
        target: Option<&pagis_core::ForgetTarget>,
    ) -> Result<HashSet<usize>, MemoryError> {
        let mut excluded = HashSet::new();
        for (index, metadata) in archive.scopes.iter().enumerate() {
            let affected = match target {
                Some(target) => {
                    self.forget
                        .preview_affects(workspace, target, &metadata.exposures)
                        .await
                }
                None => self
                    .forget
                    .permits_provenance(workspace, &metadata.exposures)
                    .await
                    .map(|allowed| !allowed),
            }
            .map_err(|error| MemoryError::Storage(error.to_string()))?;
            if affected {
                excluded.insert(index);
            }
        }
        Ok(excluded)
    }
    pub(crate) async fn forget_preview(
        &self,
        workspace: &WorkspaceId,
        target: &pagis_core::ForgetTarget,
    ) -> Result<pagis_core::ForgetPreview, MemoryError> {
        let mut preview = self
            .forget
            .preview(workspace, target)
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?;
        let archive = self.forget_archive(workspace).await?;
        let excluded = self
            .excluded_scopes(workspace, &archive, Some(target))
            .await?;
        let mut paths = HashSet::new();
        for revision in &archive.revisions {
            let mut changed = revision
                .message_scope
                .is_some_and(|scope| excluded.contains(&scope));
            for (path, _, scope) in &revision.files {
                if scope.is_some_and(|scope| excluded.contains(&scope)) {
                    paths.insert(path);
                    changed = true;
                }
            }
            if changed {
                preview.memory_revisions += 1;
            }
        }
        preview.memory_revision = archive.revision;
        preview.memory_paths = paths.len() as u64;
        Ok(preview)
    }
    pub(crate) async fn purge_forgotten_locked(
        &self,
        workspace: &WorkspaceId,
        operation: &str,
    ) -> Result<(), MemoryError> {
        if operation.is_empty()
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(MemoryError::Invalid("invalid forget operation".into()));
        }
        let operation_state = self
            .forget
            .operation(workspace, operation)
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .ok_or(MemoryError::Unavailable)?;
        if operation_state.phase == pagis_core::ForgetPhase::Blocked {
            return Err(MemoryError::Unavailable);
        }
        let dir = self.repo_dir(workspace);
        let fresh = self
            .root
            .join(format!(".{}-forget-{operation}.new", workspace.as_str()));
        let old = self
            .root
            .join(format!(".{}-forget-{operation}.old", workspace.as_str()));
        let recovery = (dir.clone(), fresh.clone(), old.clone());
        tokio::task::spawn_blocking(move || recover_swap(&recovery.0, &recovery.1, &recovery.2))
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .map_err(|error| error.0)?;
        let archive = self.forget_archive(workspace).await?;
        if archive.revision.is_none() {
            return Ok(());
        }
        let excluded = self.excluded_scopes(workspace, &archive, None).await?;
        let building = (dir.clone(), fresh.clone());
        tokio::task::spawn_blocking(move || rebuild(&building.0, &building.1, archive, &excluded))
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .map_err(|error| error.0)?;
        // Only the exchange is exclusive. A concurrent read waits here and
        // then opens either the old repository or the rebuilt one.
        let gate = self.swap_for(workspace);
        let _exclusive = gate.write().await;
        tokio::task::spawn_blocking(move || swap(&dir, &fresh, &old))
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?
            .map_err(|error| error.0)
    }
}

fn recover_swap(dir: &Path, fresh: &Path, old: &Path) -> GitResult<()> {
    if old.exists() {
        if !dir.exists() {
            if fresh.exists() {
                std::fs::rename(fresh, dir)?;
            } else {
                std::fs::rename(old, dir)?;
            }
        }
        if old.exists() {
            std::fs::remove_dir_all(old)?;
        }
    }
    if fresh.exists() {
        std::fs::remove_dir_all(fresh)?;
    }
    Ok(())
}

/// Write the retained history into `fresh`. The repository at `dir` is
/// untouched until [`swap`] exchanges the two directories.
fn rebuild(dir: &Path, fresh: &Path, archive: Archive, excluded: &HashSet<usize>) -> GitResult<()> {
    let source = Repository::open(dir)?;
    let destination = Repository::init(fresh)?;
    let mut rewritten = HashMap::new();
    let mut prior_paths = Vec::new();
    for revision in &archive.revisions {
        for path in prior_paths.drain(..) {
            let file = fresh.join(path);
            if file.exists() {
                std::fs::remove_file(file)?;
            }
        }
        let mut index = destination.index()?;
        index.clear()?;
        for (path, oid, scope) in &revision.files {
            if scope.is_some_and(|scope| excluded.contains(&scope)) {
                continue;
            }
            let blob = source.find_blob(*oid)?;
            let file = fresh.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file, blob.content())?;
            index.add_path(Path::new(path))?;
            prior_paths.push(path.clone());
            // A file without a daemon stamp has no metadata to copy.
            // It keeps its place in the rebuilt history and stays hidden.
            let Some(scope) = scope else {
                continue;
            };
            let metadata_path = metadata_rel(path);
            let file = fresh.join(&metadata_path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                file,
                serde_json::to_vec(&archive.scopes[*scope])
                    .map_err(|error| MemoryError::Storage(error.to_string()))?,
            )?;
            index.add_path(Path::new(&metadata_path))?;
            prior_paths.push(metadata_path);
        }
        index.write()?;
        let tree = destination.find_tree(index.write_tree()?)?;
        let parents = revision
            .parents
            .iter()
            .filter_map(|parent| rewritten.get(parent))
            .map(|id| destination.find_commit(*id))
            .collect::<Result<Vec<_>, _>>()?;
        let parents = parents.iter().collect::<Vec<_>>();
        let signature = Signature::new(&revision.author.0, &revision.author.1, &revision.author.2)?;
        let message = if revision
            .message_scope
            .is_some_and(|scope| excluded.contains(&scope))
        {
            "Forgotten dependent note history"
        } else {
            &revision.message
        };
        let id = destination.commit(None, &signature, &signature, message, &tree, &parents)?;
        rewritten.insert(revision.id, id);
    }
    for (name, original) in archive.references {
        if let Some(id) = rewritten.get(&original) {
            destination.reference(&name, *id, true, "Rebuild after owner forget")?;
        }
    }
    if let Some(head) = archive.head {
        destination.set_head(&head)?;
    } else if let Some(id) = archive
        .revision
        .and_then(|id| Oid::from_str(&id).ok())
        .and_then(|id| rewritten.get(&id))
    {
        destination.set_head_detached(*id)?;
    }
    destination.checkout_head(Some(
        git2::build::CheckoutBuilder::new()
            .force()
            .remove_untracked(true),
    ))?;
    Ok(())
}

/// Exchange the rebuilt repository for the original one. `recover_swap`
/// completes this sequence again after a crash between the renames.
fn swap(dir: &Path, fresh: &Path, old: &Path) -> GitResult<()> {
    std::fs::rename(dir, old)?;
    std::fs::rename(fresh, dir)?;
    std::fs::remove_dir_all(old)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{ConnectionId, ForgetTarget, GrantId, MemoryStore, StoreError};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// The scan before ADR-0008. It walks the whole tree of every commit
    /// and reads the stamp of every file from that tree. The equivalence
    /// test holds it as the oracle of the diff scan.
    fn scan_by_tree(dir: &Path) -> GitResult<Archive> {
        let mut archive = Archive {
            revisions: vec![],
            scopes: vec![],
            references: vec![],
            head: None,
            revision: None,
        };
        let Some(repo) = open_repo(dir)? else {
            return Ok(archive);
        };
        archive.head = repo
            .find_reference("HEAD")
            .ok()
            .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned));
        archive.revision = head_revision(&repo);
        let mut walk = repo.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)?;
        for reference in repo.references()? {
            let reference = reference?;
            if let (Ok(name), Ok(commit)) = (reference.name(), reference.peel_to_commit()) {
                walk.push(commit.id())?;
                archive.references.push((name.to_owned(), commit.id()));
            }
        }
        if let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) {
            walk.push(head.id())?;
        }
        let mut scopes = HashMap::<Vec<u8>, usize>::new();
        let mut remember = |bytes: &[u8], archive: &mut Archive| -> Option<usize> {
            if let Some(index) = scopes.get(bytes) {
                return Some(*index);
            }
            let metadata = serde_json::from_slice::<FileMetadata>(bytes).ok()?;
            let index = archive.scopes.len();
            archive.scopes.push(metadata);
            scopes.insert(bytes.to_vec(), index);
            Some(index)
        };
        for id in walk {
            let commit = repo.find_commit(id?)?;
            let tree = commit.tree()?;
            let mut entries = Vec::new();
            tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
                if entry.kind() == Some(git2::ObjectType::Blob)
                    && let Ok(name) = entry.name()
                {
                    entries.push((format!("{root}{name}"), entry.id()));
                }
                git2::TreeWalkResult::Ok
            })?;
            let mut files = Vec::new();
            for (path, blob) in entries {
                if path.starts_with(".pagis/exposure/") {
                    continue;
                }
                let scope = tree
                    .get_path(Path::new(&metadata_rel(&path)))
                    .ok()
                    .and_then(|entry| repo.find_blob(entry.id()).ok())
                    .and_then(|metadata| remember(metadata.content(), &mut archive));
                files.push((path, blob, scope));
            }
            let message = commit.message().unwrap_or_default().to_owned();
            let exposures = message
                .lines()
                .find_map(|line| line.strip_prefix("Exposure: "))
                .and_then(|record| serde_json::from_str::<Vec<MemoryExposure>>(record).ok());
            let message_scope = exposures
                .and_then(|exposures| {
                    serde_json::to_vec(&FileMetadata {
                        exposures,
                        kind: MemoryContentKind::DependentView,
                    })
                    .ok()
                })
                .and_then(|record| remember(&record, &mut archive));
            let author = commit.author();
            archive.revisions.push(Revision {
                id: commit.id(),
                parents: commit.parent_ids().collect(),
                files,
                message,
                message_scope,
                author: (
                    author.name().unwrap_or("Pagis").into(),
                    author.email().unwrap_or("pagis@local").into(),
                    author.when(),
                ),
            });
        }
        Ok(archive)
    }

    type ComparableRevision = (
        Oid,
        Vec<Oid>,
        Vec<(String, Oid, Option<String>)>,
        String,
        Option<String>,
    );
    type Comparable = (
        Option<String>,
        Option<String>,
        Vec<(String, Oid)>,
        Vec<ComparableRevision>,
    );

    /// One archive with each scope index replaced by the metadata it
    /// names, and the files of each revision in path order. A scope
    /// index counts the order in which a scan first reads the metadata,
    /// which the two scans do not share, and the file order of a
    /// revision reaches no consumer: the rebuild writes a git index,
    /// which sorts itself, and the preview counts a set of paths.
    fn comparable(archive: &Archive) -> Comparable {
        let metadata = |scope: &Option<usize>| {
            scope.map(|scope| serde_json::to_string(&archive.scopes[scope]).expect("metadata"))
        };
        let revisions = archive
            .revisions
            .iter()
            .map(|revision| {
                let mut files = revision
                    .files
                    .iter()
                    .map(|(path, blob, scope)| (path.clone(), *blob, metadata(scope)))
                    .collect::<Vec<_>>();
                files.sort();
                (
                    revision.id,
                    revision.parents.clone(),
                    files,
                    revision.message.clone(),
                    metadata(&revision.message_scope),
                )
            })
            .collect();
        (
            archive.head.clone(),
            archive.revision.clone(),
            archive.references.clone(),
            revisions,
        )
    }

    struct Fixture {
        _root: tempfile::TempDir,
        dir: PathBuf,
        repo: Repository,
    }

    /// A stamp record with one grant, so two stamps that name different
    /// revisions hold different scopes.
    fn stamp(revision: i64) -> String {
        serde_json::to_string(&FileMetadata {
            exposures: vec![MemoryExposure {
                grant_id: GrantId::generate(),
                revision,
            }],
            kind: MemoryContentKind::Procedure,
        })
        .expect("stamp")
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("temp root");
            let dir = root.path().join("repo");
            let repo = Repository::init(&dir).expect("repository");
            Self {
                _root: root,
                dir,
                repo,
            }
        }

        /// Commit `changes` (a path with content, or a path with `None`
        /// to delete it) on the tree of the first parent.
        fn commit(&self, message: &str, parents: &[Oid], changes: &[(&str, Option<&str>)]) -> Oid {
            let mut index = self.repo.index().expect("index");
            index.clear().expect("clear");
            if let Some(first) = parents.first() {
                let tree = self
                    .repo
                    .find_commit(*first)
                    .expect("parent")
                    .tree()
                    .expect("parent tree");
                index.read_tree(&tree).expect("read tree");
            }
            for (path, content) in changes {
                match content {
                    Some(content) => index
                        .add_frombuffer(&entry(path), content.as_bytes())
                        .expect("add"),
                    None => index.remove_path(Path::new(path)).expect("remove"),
                }
            }
            let tree = self
                .repo
                .find_tree(index.write_tree().expect("tree id"))
                .expect("tree");
            let parents = parents
                .iter()
                .map(|id| self.repo.find_commit(*id).expect("parent"))
                .collect::<Vec<_>>();
            let parents = parents.iter().collect::<Vec<_>>();
            let signature = Signature::new(
                "Sage",
                "sage@agents.pagis.local",
                &git2::Time::new(1_700_000_000, 0),
            )
            .expect("signature");
            self.repo
                .commit(None, &signature, &signature, message, &tree, &parents)
                .expect("commit")
        }

        fn head(&self, id: Oid) {
            self.repo
                .reference("refs/heads/main", id, true, "test")
                .expect("branch");
            self.repo.set_head("refs/heads/main").expect("head");
        }
    }

    fn entry(path: &str) -> git2::IndexEntry {
        git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: Oid::ZERO_SHA1,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        }
    }

    /// A history with a rename, a delete, a stamp that changes while its
    /// file does not, a file with no stamp, and a merge.
    fn history() -> Fixture {
        let fixture = Fixture::new();
        let (first, second) = (stamp(1), stamp(2));
        let one = fixture.commit(
            "Record two notes\n\nExposure: []\n",
            &[],
            &[
                ("shared/a.md", Some("a one\n")),
                (&metadata_rel("shared/a.md"), Some(&first)),
                ("shared/b.md", Some("b one\n")),
                (&metadata_rel("shared/b.md"), Some(&first)),
            ],
        );
        let two = fixture.commit(
            "Rewrite one note",
            &[one],
            &[("shared/a.md", Some("a two\n"))],
        );
        let three = fixture.commit(
            "Restamp a note the commit does not touch",
            &[two],
            &[(&metadata_rel("shared/b.md"), Some(&second))],
        );
        let four = fixture.commit(
            "Rename a note",
            &[three],
            &[
                ("shared/b.md", None),
                (&metadata_rel("shared/b.md"), None),
                ("shared/c.md", Some("b one\n")),
                (&metadata_rel("shared/c.md"), Some(&second)),
            ],
        );
        // A branch off the second commit, so the merge holds a file the
        // first parent never saw, and that note carries no stamp.
        let five = fixture.commit(
            "Record a note on a branch",
            &[two],
            &[("shared/d.md", Some("d one\n"))],
        );
        let six = fixture.commit(
            "Merge the branch",
            &[four, five],
            &[("shared/d.md", Some("d one\n"))],
        );
        let seven = fixture.commit(
            "Forget a note\n\nExposure: [{\"grant_id\":\"01J0\",\"revision\":3}]\n",
            &[six],
            &[("shared/a.md", None), (&metadata_rel("shared/a.md"), None)],
        );
        fixture.head(seven);
        fixture
    }

    #[test]
    fn the_diff_scan_gives_the_archive_the_tree_scan_gives() {
        let fixture = history();
        let diffed = scan(&fixture.dir).unwrap_or_else(|error| panic!("{:?}", error.0));
        let walked = scan_by_tree(&fixture.dir).unwrap_or_else(|error| panic!("{:?}", error.0));

        assert_eq!(comparable(&diffed), comparable(&walked));
        assert_eq!(diffed.revisions.len(), 7);
        let last = diffed.revisions.last().expect("the last revision");
        assert_eq!(
            last.files
                .iter()
                .map(|(path, _, scope)| (path.as_str(), scope.is_some()))
                .collect::<Vec<_>>(),
            vec![("shared/c.md", true), ("shared/d.md", false)]
        );
    }

    #[test]
    fn the_diff_scan_reads_an_absent_directory_as_an_empty_archive() {
        let root = tempfile::tempdir().expect("temp root");
        let archive =
            scan(&root.path().join("absent")).unwrap_or_else(|error| panic!("{:?}", error.0));
        assert!(archive.revisions.is_empty());
        assert!(archive.revision.is_none());
    }

    /// A flat directory of many files over many commits. A tree scan
    /// reads one stamp per file per commit, which is 150000 object reads
    /// here and hours on a repository of thousands of commits and files.
    /// The diff scan reads only what each commit changes.
    #[test]
    fn the_diff_scan_of_a_long_flat_history_is_quick() {
        let fixture = Fixture::new();
        let record = stamp(1);
        let paths = (0..1000)
            .map(|index| format!("shared/gmail/note-{index}.md"))
            .collect::<Vec<_>>();
        // One file in twenty carries a stamp. The tree scan reads the
        // tree for the stamp of every file, and a file with no stamp
        // costs it the same read, so this fixture holds the cost it
        // must show and stays quick to build.
        let stamps = paths
            .iter()
            .step_by(20)
            .map(|path| metadata_rel(path))
            .collect::<Vec<_>>();
        let mut changes = paths
            .iter()
            .map(|path| (path.as_str(), Some("one\n")))
            .collect::<Vec<_>>();
        for stamp in &stamps {
            changes.push((stamp.as_str(), Some(record.as_str())));
        }
        let mut head = fixture.commit("Record the arrivals", &[], &changes);
        for index in 0..149 {
            head = fixture.commit(
                "Append an arrival",
                &[head],
                &[(paths[index % paths.len()].as_str(), Some("two\n"))],
            );
        }
        fixture.head(head);

        let start = Instant::now();
        let archive = scan(&fixture.dir).unwrap_or_else(|error| panic!("{:?}", error.0));
        let elapsed = start.elapsed();

        assert_eq!(archive.revisions.len(), 150);
        assert_eq!(archive.revisions[0].files.len(), 1000);
        assert!(
            elapsed < Duration::from_secs(5),
            "the scan took {elapsed:?}"
        );
    }

    /// A forget store that suppresses nothing and reports an empty
    /// preview, so the memory scan alone decides the result.
    struct NoSuppression;

    #[async_trait]
    impl pagis_core::ForgetStore for NoSuppression {
        async fn preview(
            &self,
            _: &WorkspaceId,
            _: &ForgetTarget,
        ) -> Result<pagis_core::ForgetPreview, StoreError> {
            Ok(pagis_core::ForgetPreview {
                revision: "0".into(),
                source_items: 0,
                memory_revision: None,
                memory_paths: 0,
                memory_revisions: 0,
                raw_account_retrieval_blocked: false,
            })
        }
        async fn preview_affects(
            &self,
            _: &WorkspaceId,
            _: &ForgetTarget,
            _: &[MemoryExposure],
        ) -> Result<bool, StoreError> {
            Ok(false)
        }
        async fn permits_provenance(
            &self,
            _: &WorkspaceId,
            _: &[MemoryExposure],
        ) -> Result<bool, StoreError> {
            Ok(true)
        }
        async fn is_suppressed(
            &self,
            _: &pagis_core::knowledge::SourceKey,
            _: &str,
            _: &dyn pagis_core::ForgetKeys,
        ) -> Result<bool, StoreError> {
            Ok(false)
        }
        async fn run_allowed(&self, _: &WorkspaceId, _: &RunId) -> Result<bool, StoreError> {
            Ok(true)
        }
        async fn raw_connection_allowed(
            &self,
            _: &WorkspaceId,
            _: &ConnectionId,
        ) -> Result<bool, StoreError> {
            Ok(true)
        }
        async fn begin(
            &self,
            _: &WorkspaceId,
            _: &ForgetTarget,
            _: &str,
            _: i64,
            _: &dyn pagis_core::ForgetKeys,
        ) -> Result<pagis_core::ForgetOperation, StoreError> {
            unreachable!()
        }
        async fn operation(
            &self,
            _: &WorkspaceId,
            _: &str,
        ) -> Result<Option<pagis_core::ForgetOperation>, StoreError> {
            Ok(None)
        }
        async fn operations(
            &self,
            _: &WorkspaceId,
        ) -> Result<Vec<pagis_core::ForgetOperation>, StoreError> {
            Ok(vec![])
        }
        async fn unfinished(
            &self,
            _: &WorkspaceId,
        ) -> Result<Vec<pagis_core::ForgetOperation>, StoreError> {
            Ok(vec![])
        }
        async fn purge_structured(&self, _: &WorkspaceId, _: &str) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn memory_purged(&self, _: &WorkspaceId, _: &str) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn complete(&self, _: &WorkspaceId, _: &str) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn record_failure(
            &self,
            _: &WorkspaceId,
            _: &str,
            _: &str,
        ) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn retry(&self, _: &WorkspaceId, _: &str) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn reopt_in(&self, _: &WorkspaceId, _: &str, _: i64) -> Result<(), StoreError> {
            unreachable!()
        }
    }

    /// A preview does not hold the write lock. If it did, every commit
    /// of an arrival would stop for as long as the scan runs.
    #[tokio::test]
    async fn a_preview_runs_while_a_writer_holds_the_workspace_lock() {
        let root = tempfile::tempdir().expect("temp root");
        let store = GitMemoryStore::new(
            root.path(),
            Arc::new(NoSuppression),
            Arc::new(pagis_core::VolatilePageIndex::default()),
        );
        let workspace = WorkspaceId::generate();
        let target = ForgetTarget::Account {
            connection_id: ConnectionId::generate(),
        };

        let lock = store.lock_for(&workspace);
        let (writing, held) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let _serialized = lock.lock().await;
            writing.send(()).expect("the test waits for the lock");
            released.await.expect("the test releases the lock");
        });
        held.await.expect("the writer holds the lock");

        let preview = tokio::time::timeout(
            Duration::from_secs(5),
            store.preview_forget(&workspace, &target),
        )
        .await
        .expect("the preview does not wait for the write lock")
        .expect("preview");

        assert_eq!(preview.memory_revision, None);
        release.send(()).expect("the writer waits");
        writer.await.expect("the writer stops");
    }
}
