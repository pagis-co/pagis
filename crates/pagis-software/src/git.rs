//! The version store of a Software Package (ADR-0016).
//!
//! Each package has one bare repository that only the daemon writes,
//! at `<data root>/software/<workspace_id>/<package>.git`. A publish
//! writes the version tar as blobs and trees, commits with the
//! previous Version as parent, and marks the commit with an annotated
//! tag whose message is the publish notes. There is no remote and no
//! working tree.
//!
//! `git2` is a blocking C library, so every call runs inside
//! `spawn_blocking`, as `pagis-memory` does.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use git2::{Oid, Repository, Signature};
use pagis_core::WorkspaceId;

/// Why a publish could not write a Version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitStoreError {
    /// The tree of this publish equals the tree of the named Version.
    #[error("no change since {0}")]
    NoChange(String),
    #[error("{0}")]
    Storage(String),
}

impl From<git2::Error> for GitStoreError {
    fn from(error: git2::Error) -> Self {
        GitStoreError::Storage(error.to_string())
    }
}

/// Who publishes a Version. The name is the Agent's own; the address
/// is a local one, because the daemon never talks to a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishAuthor {
    pub name: String,
    pub email: String,
}

impl PublishAuthor {
    /// The author line of one Agent.
    pub fn of_agent(name: &str, agent_id: &pagis_core::AgentId) -> Self {
        Self {
            name: name.to_string(),
            email: format!("{agent_id}@agents.pagis.local"),
        }
    }
}

/// One Version a publish writes.
pub struct NewVersion<'a> {
    pub package: &'a str,
    /// The tag the daemon minted for this Version.
    pub version: &'a str,
    /// The tag of the Version this one follows, if there is one.
    pub previous: Option<&'a str>,
    /// What the author said about the Version. It is the tag message.
    pub notes: &'a str,
    pub author: &'a PublishAuthor,
    /// The package tree, with entries relative to the package root.
    pub tar: Vec<u8>,
}

/// The bare repositories of every Software Package.
pub struct SoftwareGitStore {
    root: PathBuf,
}

impl SoftwareGitStore {
    /// `root` holds one directory per Workspace.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn repo_dir(&self, workspace_id: &WorkspaceId, package: &str) -> PathBuf {
        self.root
            .join(workspace_id.as_str())
            .join(format!("{package}.git"))
    }

    /// Write one Version: the tree of the tar, a commit on top of the
    /// previous Version, and the annotated tag. The commit id comes
    /// back. A tree equal to the previous Version's is
    /// [`GitStoreError::NoChange`], so a publish that changed nothing
    /// mints no tag.
    pub async fn commit_version(
        &self,
        workspace_id: &WorkspaceId,
        new: NewVersion<'_>,
    ) -> Result<String, GitStoreError> {
        let dir = self.repo_dir(workspace_id, new.package);
        let version = new.version.to_string();
        let previous = new.previous.map(str::to_string);
        let notes = new.notes.to_string();
        let author = new.author.clone();
        let version_tar = new.tar;
        tokio::task::spawn_blocking(move || {
            commit_version(
                &dir,
                &version,
                previous.as_deref(),
                &notes,
                &author,
                &version_tar,
            )
        })
        .await
        .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// The tar of one Version, with entries relative to the package
    /// root. It is what a Computer materializes.
    pub async fn version_tar(
        &self,
        workspace_id: &WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, GitStoreError> {
        let dir = self.repo_dir(workspace_id, package);
        let version = version.to_string();
        tokio::task::spawn_blocking(move || version_tar(&dir, &version))
            .await
            .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// The bytes of one file of one Version. It reads the
    /// blob out of the tag's tree, so it opens no working copy: a
    /// Widget page is served on every render, and a whole checkout
    /// per request would not pay.
    pub async fn version_file(
        &self,
        workspace_id: &WorkspaceId,
        package: &str,
        version: &str,
        path: &str,
    ) -> Result<Vec<u8>, GitStoreError> {
        let dir = self.repo_dir(workspace_id, package);
        let version = version.to_string();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || version_file(&dir, &version, &path))
            .await
            .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// The patch from `base` to `head` (ADR-0016). The two
    /// Versions belong to two packages, and each package has its own
    /// repository, so the trees are written again into one scratch
    /// repository and compared there.
    pub async fn patch(
        &self,
        workspace_id: &WorkspaceId,
        base: VersionRef<'_>,
        head: VersionRef<'_>,
    ) -> Result<Patch, GitStoreError> {
        let base_tar = self
            .version_tar(workspace_id, base.package, base.version)
            .await?;
        let head_tar = self
            .version_tar(workspace_id, head.package, head.version)
            .await?;
        tokio::task::spawn_blocking(move || patch(&base_tar, &head_tar))
            .await
            .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }
}

/// Write both trees into one scratch repository and print the diff.
fn patch(base_tar: &[u8], head_tar: &[u8]) -> Result<Patch, GitStoreError> {
    let scratch = tempfile::tempdir().map_err(|error| GitStoreError::Storage(error.to_string()))?;
    let repository = Repository::init_bare(scratch.path())?;
    let base = repository.find_tree(write_tar_tree(&repository, base_tar)?)?;
    let head = repository.find_tree(write_tar_tree(&repository, head_tar)?)?;
    let diff = repository.diff_tree_to_tree(Some(&base), Some(&head), None)?;

    let files = diff
        .deltas()
        .filter_map(|delta| {
            delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|path| path.display().to_string())
        })
        .collect();
    let mut text = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        match line.origin() {
            '+' | '-' | ' ' => text.push(line.origin()),
            _ => {}
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(Patch { text, files })
}

/// The change between two Versions (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    /// The unified diff, as `git diff` prints it.
    pub text: String,
    /// The files the diff touches, by their path in the package.
    pub files: Vec<String>,
}

impl Patch {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The part of the patch that belongs to one file, or `None` when
    /// the patch does not touch it. Every file starts at its own
    /// `diff --git` line.
    pub fn file(&self, path: &str) -> Option<String> {
        let header = format!(" b/{path}");
        let mut section: Option<Vec<&str>> = None;
        for line in self.text.lines() {
            if line.starts_with("diff --git ") {
                if section.is_some() {
                    break;
                }
                if line.ends_with(&header) {
                    section = Some(Vec::new());
                } else {
                    continue;
                }
            }
            if let Some(held) = section.as_mut() {
                held.push(line);
            }
        }
        section.map(|lines| {
            let mut text = lines.join("\n");
            text.push('\n');
            text
        })
    }
}

/// One Version of one package, as a patch names it.
#[derive(Debug, Clone, Copy)]
pub struct VersionRef<'a> {
    pub package: &'a str,
    pub version: &'a str,
}

/// Open the package repository, or create it bare on the first
/// publish.
fn open_or_init(dir: &Path) -> Result<Repository, GitStoreError> {
    match Repository::open_bare(dir) {
        Ok(repository) => Ok(repository),
        Err(_) => {
            std::fs::create_dir_all(dir)
                .map_err(|error| GitStoreError::Storage(error.to_string()))?;
            Ok(Repository::init_bare(dir)?)
        }
    }
}

fn commit_version(
    dir: &Path,
    version: &str,
    previous: Option<&str>,
    notes: &str,
    author: &PublishAuthor,
    version_tar: &[u8],
) -> Result<String, GitStoreError> {
    let repository = open_or_init(dir)?;
    let tree_id = write_tar_tree(&repository, version_tar)?;

    let parent = match previous {
        Some(previous) => Some(
            repository
                .revparse_single(previous)
                .map_err(|error| {
                    GitStoreError::Storage(format!("cannot read {previous}: {error}"))
                })?
                .peel_to_commit()?,
        ),
        None => None,
    };
    if let Some(parent) = &parent
        && parent.tree_id() == tree_id
    {
        return Err(GitStoreError::NoChange(
            previous.unwrap_or_default().to_string(),
        ));
    }

    let signature = Signature::now(&author.name, &author.email)?;
    let tree = repository.find_tree(tree_id)?;
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let commit_id = repository.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &format!("{version}\n\n{notes}"),
        &tree,
        &parents,
    )?;
    let object = repository.find_object(commit_id, None)?;
    // `force` stays false: a Version is never rewritten, so a repeated
    // tag name must fail rather than move.
    repository.tag(version, &object, &signature, notes, false)?;
    Ok(commit_id.to_string())
}

/// Write every tar entry as a blob and build the trees bottom-up. A
/// bare repository has no index, so the tree builder is the only way
/// in.
fn write_tar_tree(repository: &Repository, version_tar: &[u8]) -> Result<Oid, GitStoreError> {
    /// The mode of one regular file, and of one executable file.
    const FILE_MODE: i32 = 0o100_644;
    const EXECUTABLE_MODE: i32 = 0o100_755;
    const TREE_MODE: i32 = 0o040_000;

    let mut blobs: BTreeMap<PathBuf, (Oid, i32)> = BTreeMap::new();
    let mut archive = tar::Archive::new(version_tar);
    for entry in archive
        .entries()
        .map_err(|error| GitStoreError::Storage(format!("cannot read the version tar: {error}")))?
    {
        let mut entry = entry
            .map_err(|error| GitStoreError::Storage(format!("cannot read a tar entry: {error}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| GitStoreError::Storage(format!("a tar entry has no path: {error}")))?
            .into_owned();
        let executable = entry.header().mode().unwrap_or(0o644) & 0o111 != 0;
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| GitStoreError::Storage(format!("cannot read a tar entry: {error}")))?;
        let blob = repository.blob(&data)?;
        blobs.insert(
            path,
            (
                blob,
                if executable {
                    EXECUTABLE_MODE
                } else {
                    FILE_MODE
                },
            ),
        );
    }

    // Group by directory, then write the deepest directories first, so
    // every child tree exists when its parent is built.
    let mut children: BTreeMap<PathBuf, Vec<(String, Oid, i32)>> = BTreeMap::new();
    for (path, (blob, mode)) in blobs {
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
        // Every directory on the way up must exist as a key, even when
        // it holds no file of its own.
        let mut walk = parent.clone();
        loop {
            children.entry(walk.clone()).or_default();
            match walk.parent() {
                Some(above) => walk = above.to_path_buf(),
                None => break,
            }
        }
        children.entry(parent).or_default().push((name, blob, mode));
    }
    children.entry(PathBuf::new()).or_default();

    let mut depths: Vec<PathBuf> = children.keys().cloned().collect();
    depths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut trees: BTreeMap<PathBuf, Oid> = BTreeMap::new();
    for directory in depths {
        let mut builder = repository.treebuilder(None)?;
        for (name, blob, mode) in children.get(&directory).cloned().unwrap_or_default() {
            builder.insert(&name, blob, mode)?;
        }
        for (child, tree) in &trees {
            if child.parent() == Some(directory.as_path())
                && let Some(name) = child.file_name().and_then(|name| name.to_str())
            {
                builder.insert(name, *tree, TREE_MODE)?;
            }
        }
        trees.insert(directory.clone(), builder.write()?);
    }
    Ok(trees
        .get(Path::new(""))
        .copied()
        .expect("the root tree is written"))
}

/// One blob of one Version's tree. A path that leaves the tree, or
/// names something that is not a file, is a `Storage` failure with
/// the same words as a missing file: the caller answers 404 for both.
fn version_file(dir: &Path, version: &str, path: &str) -> Result<Vec<u8>, GitStoreError> {
    if path.is_empty()
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(GitStoreError::Storage(format!("no such file {path}")));
    }
    let repository = Repository::open_bare(dir)
        .map_err(|error| GitStoreError::Storage(format!("no such package: {error}")))?;
    let object = repository
        .revparse_single(version)
        .map_err(|error| GitStoreError::Storage(format!("no such version {version}: {error}")))?;
    let tree = object.peel_to_tree()?;
    let entry = tree
        .get_path(Path::new(path))
        .map_err(|_| GitStoreError::Storage(format!("no such file {path}")))?;
    let blob = repository
        .find_blob(entry.id())
        .map_err(|_| GitStoreError::Storage(format!("no such file {path}")))?;
    Ok(blob.content().to_vec())
}

/// Check the Version out into a scratch directory and tar it. The
/// checkout keeps the file mode, so an entry stays executable.
fn version_tar(dir: &Path, version: &str) -> Result<Vec<u8>, GitStoreError> {
    let repository = Repository::open_bare(dir)
        .map_err(|error| GitStoreError::Storage(format!("no such package: {error}")))?;
    let object = repository
        .revparse_single(version)
        .map_err(|error| GitStoreError::Storage(format!("no such version {version}: {error}")))?;
    let tree = object.peel_to_tree()?;
    let scratch = tempfile::tempdir().map_err(|error| GitStoreError::Storage(error.to_string()))?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout
        .target_dir(scratch.path())
        .update_index(false)
        .force();
    repository.checkout_tree(tree.as_object(), Some(&mut checkout))?;

    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", scratch.path())
        .map_err(|error| {
            GitStoreError::Storage(format!("cannot write the version tar: {error}"))
        })?;
    builder
        .into_inner()
        .map_err(|error| GitStoreError::Storage(format!("cannot close the version tar: {error}")))
}
