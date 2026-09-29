//! The installed states of a Plugin (ADR-0017).
//!
//! Each Plugin owns three directories under `<data root>/plugins/
//! <workspace_id>/`: `<plugin>.git` is the bare repository only the
//! daemon writes, `<plugin>` is the checkout the servers and the
//! Skills read as `PLUGIN_ROOT`, and `<plugin>.data` is the writable
//! `PLUGIN_DATA` that survives an update. An install commits the
//! fetched tree and checks it out; an update commits on top of the
//! previous state, so the two states have a diff.
//!
//! `git2` is a blocking C library, so every call runs inside
//! `spawn_blocking`, as `pagis-memory` and `pagis-software` do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git2::{Oid, Repository, Signature};
use pagis_core::WorkspaceId;

/// Why the daemon could not write or read an installed state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitStoreError {
    /// The tree of this install equals the installed state, so there
    /// is nothing to update to.
    #[error("the plugin is unchanged")]
    NoChange,
    #[error("{0}")]
    Storage(String),
}

impl From<git2::Error> for GitStoreError {
    fn from(error: git2::Error) -> Self {
        GitStoreError::Storage(error.to_string())
    }
}

/// The three directories of one Plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPaths {
    /// The bare repository of every installed state.
    pub repository: PathBuf,
    /// The checkout of the installed state: `PLUGIN_ROOT`.
    pub root: PathBuf,
    /// The writable directory of the Plugin: `PLUGIN_DATA`.
    pub data: PathBuf,
}

/// How one file changed between two installed states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
}

/// One file an update changes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
}

/// What an update changes, as the desk shows it before the user
/// accepts (ADR-0017).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiffSummary {
    pub files: Vec<ChangedFile>,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The repositories and checkouts of every Plugin.
pub struct PluginGitStore {
    root: PathBuf,
}

impl PluginGitStore {
    /// `root` holds one directory per Workspace.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where one Plugin's three directories are.
    pub fn paths(&self, workspace_id: &WorkspaceId, plugin: &str) -> PluginPaths {
        let workspace = self.root.join(workspace_id.as_str());
        PluginPaths {
            repository: workspace.join(format!("{plugin}.git")),
            root: workspace.join(plugin),
            data: workspace.join(format!("{plugin}.data")),
        }
    }

    /// Commit the tree at `tree` as the next installed state and check
    /// it out as `PLUGIN_ROOT`. The commit id comes back. A tree equal
    /// to the installed state is [`GitStoreError::NoChange`], so an
    /// update that changed nothing writes nothing.
    pub async fn install_state(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
        tree: &Path,
        message: &str,
    ) -> Result<String, GitStoreError> {
        let paths = self.paths(workspace_id, plugin);
        let tree = tree.to_path_buf();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || install_state(&paths, &tree, &message))
            .await
            .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// Give the checkout of one Plugin the modes that every uid reads,
    /// as [`install_state`](Self::install_state) gives them. The daemon
    /// does this at start, because a restore gives every file the
    /// owner's bits alone. A Plugin with no checkout is no error.
    pub async fn make_checkout_readable(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
    ) -> Result<(), GitStoreError> {
        let root = self.paths(workspace_id, plugin).root;
        tokio::task::spawn_blocking(move || match root.is_dir() {
            true => make_readable(&root),
            false => Ok(()),
        })
        .await
        .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// What changed between two installed states.
    pub async fn diff(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
        base: &str,
        head: &str,
    ) -> Result<DiffSummary, GitStoreError> {
        let paths = self.paths(workspace_id, plugin);
        let base = base.to_string();
        let head = head.to_string();
        tokio::task::spawn_blocking(move || diff(&paths.repository, &base, &head))
            .await
            .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }

    /// Delete every trace of one Plugin: the repository, the checkout
    /// and the data directory (ADR-0017). The records are the caller's
    /// to delete.
    pub async fn remove(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &str,
    ) -> Result<(), GitStoreError> {
        let paths = self.paths(workspace_id, plugin);
        tokio::task::spawn_blocking(move || {
            for directory in [&paths.repository, &paths.root, &paths.data] {
                if directory.exists() {
                    std::fs::remove_dir_all(directory)
                        .map_err(|error| GitStoreError::Storage(error.to_string()))?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|error| GitStoreError::Storage(error.to_string()))?
    }
}

fn install_state(paths: &PluginPaths, tree: &Path, message: &str) -> Result<String, GitStoreError> {
    let repository = open_or_init(&paths.repository)?;
    let tree_id = write_directory_tree(&repository, tree)?;
    let parent = match repository.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None,
    };
    if let Some(parent) = &parent
        && parent.tree_id() == tree_id
    {
        return Err(GitStoreError::NoChange);
    }

    // The daemon is the only writer, so the author line names it and
    // never a remote identity.
    let signature = Signature::now("Pagis", "daemon@pagis.local")?;
    let tree = repository.find_tree(tree_id)?;
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let commit = repository.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;

    checkout(&repository, commit, &paths.root)?;
    std::fs::create_dir_all(&paths.data).map_err(|error| {
        GitStoreError::Storage(format!("cannot make the plugin data directory: {error}"))
    })?;
    Ok(commit.to_string())
}

/// Write the checkout of one commit. The previous checkout goes first,
/// so a file an update dropped does not stay behind.
fn checkout(repository: &Repository, commit: Oid, root: &Path) -> Result<(), GitStoreError> {
    if root.exists() {
        std::fs::remove_dir_all(root).map_err(|error| GitStoreError::Storage(error.to_string()))?;
    }
    std::fs::create_dir_all(root).map_err(|error| GitStoreError::Storage(error.to_string()))?;
    let tree = repository.find_commit(commit)?.tree()?;
    let mut builder = git2::build::CheckoutBuilder::new();
    builder.target_dir(root).update_index(false).force();
    repository.checkout_tree(tree.as_object(), Some(&mut builder))?;
    make_readable(root)
}

/// Give every directory and every executable of a checkout mode 755,
/// and every other file mode 644.
///
/// The Plugin Computer mounts the checkout read-only, and the plugin
/// server in it runs as a uid that does not own the files (ADR-0017).
/// The daemon writes the checkout under a umask that keeps its files
/// from other users, so the checkout gets its modes here. The state
/// directory, which only its owner can enter, keeps the checkout from
/// the other users of the host.
#[cfg(unix)]
fn make_readable(root: &Path) -> Result<(), GitStoreError> {
    use std::os::unix::fs::PermissionsExt;

    let failed = |error: std::io::Error| {
        GitStoreError::Storage(format!(
            "cannot make the checkout {} readable: {error}",
            root.display()
        ))
    };
    let set = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(failed)
    };
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        set(&directory, 0o755)?;
        for entry in std::fs::read_dir(&directory).map_err(failed)? {
            let entry = entry.map_err(failed)?;
            let kind = entry.file_type().map_err(failed)?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if kind.is_file() {
                let owner_executes = entry.metadata().map_err(failed)?.permissions().mode() & 0o100;
                set(
                    &entry.path(),
                    if owner_executes != 0 { 0o755 } else { 0o644 },
                )?;
            }
            // A symbolic link has no mode of its own.
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_readable(_root: &Path) -> Result<(), GitStoreError> {
    Ok(())
}

fn diff(repository: &Path, base: &str, head: &str) -> Result<DiffSummary, GitStoreError> {
    let repository = Repository::open_bare(repository)
        .map_err(|error| GitStoreError::Storage(format!("no such plugin: {error}")))?;
    let base = repository.find_commit(Oid::from_str(base)?)?.tree()?;
    let head = repository.find_commit(Oid::from_str(head)?)?.tree()?;
    let diff = repository.diff_tree_to_tree(Some(&base), Some(&head), None)?;
    let files = diff
        .deltas()
        .filter_map(|delta| {
            let status = match delta.status() {
                git2::Delta::Added | git2::Delta::Copied => ChangeStatus::Added,
                git2::Delta::Deleted => ChangeStatus::Deleted,
                _ => ChangeStatus::Modified,
            };
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())?
                .display()
                .to_string();
            Some(ChangedFile { path, status })
        })
        .collect();
    Ok(DiffSummary { files })
}

/// Open the Plugin's repository, or create it bare on the first
/// install.
fn open_or_init(directory: &Path) -> Result<Repository, GitStoreError> {
    match Repository::open_bare(directory) {
        Ok(repository) => Ok(repository),
        Err(_) => {
            std::fs::create_dir_all(directory)
                .map_err(|error| GitStoreError::Storage(error.to_string()))?;
            Ok(Repository::init_bare(directory)?)
        }
    }
}

/// Write every file of a directory as a blob and build the trees
/// bottom-up. A bare repository has no index, so the tree builder is
/// the only way in.
fn write_directory_tree(repository: &Repository, source: &Path) -> Result<Oid, GitStoreError> {
    const FILE_MODE: i32 = 0o100_644;
    const EXECUTABLE_MODE: i32 = 0o100_755;
    const TREE_MODE: i32 = 0o040_000;

    let mut blobs: BTreeMap<PathBuf, (Oid, i32)> = BTreeMap::new();
    let mut directories: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(relative) = directories.pop() {
        let entries = std::fs::read_dir(source.join(&relative)).map_err(|error| {
            GitStoreError::Storage(format!("cannot read the plugin directory: {error}"))
        })?;
        for entry in entries {
            let entry =
                entry.map_err(|error| GitStoreError::Storage(format!("cannot read: {error}")))?;
            let path = relative.join(entry.file_name());
            // `symlink_metadata` does not follow a link, so a link out
            // of the plugin is dropped rather than copied in.
            let metadata = entry.metadata().map_err(|error| {
                GitStoreError::Storage(format!("cannot read {}: {error}", path.display()))
            })?;
            if metadata.is_dir() {
                directories.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let data = std::fs::read(entry.path()).map_err(|error| {
                GitStoreError::Storage(format!("cannot read {}: {error}", path.display()))
            })?;
            let executable = is_executable(&metadata);
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
    }

    let mut children: BTreeMap<PathBuf, Vec<(String, Oid, i32)>> = BTreeMap::new();
    children.insert(PathBuf::new(), Vec::new());
    for (path, (blob, mode)) in blobs {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut walk = parent.clone();
        loop {
            children.entry(walk.clone()).or_default();
            match walk.parent() {
                Some(above) => walk = above.to_path_buf(),
                None => break,
            }
        }
        children
            .entry(parent)
            .or_default()
            .push((name.to_string(), blob, mode));
    }

    let mut deepest: Vec<PathBuf> = children.keys().cloned().collect();
    deepest.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut trees: BTreeMap<PathBuf, Oid> = BTreeMap::new();
    for directory in deepest {
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

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}
