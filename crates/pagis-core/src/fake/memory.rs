//! An in-memory MemoryStore with the same rules as each storage backend.
use crate::memory::rules::{self, FileStamp};
use crate::{memory_page::PageSummary, *};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, PartialEq, Eq)]
struct File {
    content: String,
    stamp: FileStamp,
    position: u64,
    changed_at: i64,
    author: MemoryAuthor,
}
#[derive(Clone)]
struct Revision {
    id: String,
    files: BTreeMap<String, File>,
    exposures: Vec<MemoryExposure>,
    message: String,
    author: MemoryAuthor,
    time: i64,
}
type History = Arc<futures::lock::Mutex<Vec<Revision>>>;

/// A store of memory values. A clone opens the same memory through another handle.
#[derive(Clone)]
pub struct FakeMemoryStore {
    histories: Arc<Mutex<HashMap<WorkspaceId, History>>>,
    sequence: Arc<AtomicU64>,
    forget: Arc<dyn ForgetStore>,
}
impl Default for FakeMemoryStore {
    fn default() -> Self {
        Self::new(Arc::new(super::FakeForgetStore::default()))
    }
}
impl FakeMemoryStore {
    pub fn new(forget: Arc<dyn ForgetStore>) -> Self {
        Self {
            histories: Arc::default(),
            sequence: Arc::new(AtomicU64::new(1)),
            forget,
        }
    }
    fn history(&self, workspace: &WorkspaceId) -> History {
        self.histories
            .lock()
            .expect("memory histories")
            .entry(workspace.clone())
            .or_default()
            .clone()
    }
    fn id(&self) -> String {
        format!("{:040x}", self.sequence.fetch_add(1, Ordering::Relaxed))
    }
    async fn head(&self, workspace: &WorkspaceId) -> Option<Revision> {
        self.history(workspace).lock().await.last().cloned()
    }
    async fn permits(
        &self,
        workspace: &WorkspaceId,
        exposures: &[MemoryExposure],
    ) -> Result<(), MemoryError> {
        rules::require_provenance(self.forget.as_ref(), workspace, exposures).await
    }
    async fn readable(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        head: Option<&Revision>,
    ) -> Result<(String, Vec<(IndexedPage, Vec<MemoryExposure>)>), MemoryError> {
        let root = match scope {
            MemoryScope::Shared => "shared/".to_string(),
            MemoryScope::Private => format!(
                "agents/{}/",
                access.agent_id().expect("memory access names an agent")
            ),
        };
        let mut rows: Vec<_> = head
            .into_iter()
            .flat_map(|head| &head.files)
            .filter(|(path, _)| path.starts_with(&root))
            .map(|(path, file)| {
                let summary = PageSummary::read(path, &file.content);
                IndexedPage {
                    path: path.clone(),
                    position: file.position,
                    changed_at: file.changed_at,
                    changed_by: file.author.clone(),
                    title: summary.title,
                    body: file.content.clone(),
                    kind: summary.kind,
                    source_connection_id: summary.source_connection_id,
                    exposures: Some(file.stamp.exposures.clone()),
                    brief: summary.brief,
                    links: summary.links,
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            b.position
                .cmp(&a.position)
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok((
            root,
            rules::filter_readable_pages(self.forget.as_ref(), workspace, access, rows).await?,
        ))
    }
    async fn selection(
        &self,
        workspace: &WorkspaceId,
        history: &[Revision],
        target: Option<&ForgetTarget>,
    ) -> Result<rules::ForgetSelection, MemoryError> {
        let mut scopes = Vec::<FileStamp>::new();
        let mut scope = |stamp: FileStamp| match scopes.iter().position(|held| held == &stamp) {
            Some(index) => index,
            None => {
                scopes.push(stamp);
                scopes.len() - 1
            }
        };
        let values: Vec<_> = history
            .iter()
            .map(|revision| rules::HistoryRevision {
                revision: &revision.id,
                scope: Some(scope(FileStamp {
                    exposures: revision.exposures.clone(),
                    kind: MemoryContentKind::DependentView,
                })),
                files: revision
                    .files
                    .iter()
                    .map(|(path, file)| rules::HistoryFile {
                        path,
                        scope: Some(scope(file.stamp.clone())),
                    })
                    .collect(),
            })
            .collect();
        let excluded =
            rules::excluded_scopes(self.forget.as_ref(), workspace, &scopes, target).await?;
        Ok(rules::select_forget(&values, &excluded))
    }
    async fn preview(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        history: &[Revision],
    ) -> Result<ForgetPreview, MemoryError> {
        let mut preview = self
            .forget
            .preview(workspace, target)
            .await
            .map_err(storage)?;
        let selection = self.selection(workspace, history, Some(target)).await?;
        preview.memory_revision = history.last().map(|head| head.id.clone());
        preview.memory_paths = selection.paths.len() as u64;
        preview.memory_revisions = selection.revisions.len() as u64;
        Ok(preview)
    }
}
fn storage(error: StoreError) -> MemoryError {
    MemoryError::Storage(error.to_string())
}
fn scoped(path: &str) -> (ScopedPath, Option<AgentId>) {
    if path.starts_with("shared/") {
        return (ScopedPath::parse(path).expect("validated path"), None);
    }
    let (agent, rel) = path
        .strip_prefix("agents/")
        .expect("private path")
        .split_once('/')
        .expect("agent and path");
    (
        ScopedPath::parse(&format!("private/{rel}")).expect("validated path"),
        Some(AgentId::from(agent.to_string())),
    )
}
fn hunks(old: &str, new: &str) -> Vec<MemoryHunk> {
    let mut options = diffy::DiffOptions::new();
    options.set_context_len(0);
    options
        .create_patch(old, new)
        .hunks()
        .iter()
        .map(|hunk| MemoryHunk {
            old_start: hunk.old_range().start() as u32,
            new_start: hunk.new_range().start() as u32,
            old_lines: hunk
                .lines()
                .iter()
                .filter_map(|line| match line {
                    diffy::Line::Delete(text) => Some(text.trim_end_matches('\n').to_string()),
                    _ => None,
                })
                .collect(),
            new_lines: hunk
                .lines()
                .iter()
                .filter_map(|line| match line {
                    diffy::Line::Insert(text) => Some(text.trim_end_matches('\n').to_string()),
                    _ => None,
                })
                .collect(),
        })
        .collect()
}

#[async_trait]
impl MemoryStore for FakeMemoryStore {
    async fn find_commit_with_trailers(
        &self,
        workspace: &WorkspaceId,
        trailers: &[(&str, &str)],
    ) -> Result<Option<String>, MemoryError> {
        let history = self.history(workspace);
        let held = history.lock().await;
        Ok(held
            .iter()
            .rev()
            .find(|revision| {
                trailers.iter().all(|(key, value)| {
                    revision
                        .message
                        .lines()
                        .any(|line| line == format!("{key}: {value}"))
                })
            })
            .map(|revision| revision.id.clone()))
    }
    async fn preview_forget(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
    ) -> Result<ForgetPreview, MemoryError> {
        let snapshot = self.history(workspace).lock().await.clone();
        self.preview(workspace, target, &snapshot).await
    }
    async fn begin_forget(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        preview: &ForgetPreview,
        now: i64,
        keys: &dyn ForgetKeys,
    ) -> Result<ForgetOperation, MemoryError> {
        let history = self.history(workspace);
        let held = history.lock().await;
        let current = self.preview(workspace, target, &held).await?;
        if current.revision != preview.revision
            || current.memory_revision != preview.memory_revision
        {
            return Err(MemoryError::Conflict);
        }
        self.forget
            .begin(workspace, target, &preview.revision, now, keys)
            .await
            .map_err(|error| match error {
                StoreError::Conflict(_) => MemoryError::Conflict,
                error => storage(error),
            })
    }
    async fn purge_forgotten(
        &self,
        workspace: &WorkspaceId,
        operation: &str,
    ) -> Result<(), MemoryError> {
        let _hold = crate::LegalHold::lock(workspace).await;
        self.forget.refuse_delete(workspace).await?;
        if operation.is_empty()
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(MemoryError::Invalid("invalid forget operation".to_string()));
        }
        let state = self
            .forget
            .operation(workspace, operation)
            .await
            .map_err(storage)?
            .ok_or(MemoryError::Unavailable)?;
        if state.phase == ForgetPhase::Blocked {
            return Err(MemoryError::Unavailable);
        }
        let history = self.history(workspace);
        let mut held = history.lock().await;
        let selection = self.selection(workspace, &held, None).await?;
        for revision in held.iter_mut() {
            if let Some(selected) = selection.revisions.get(&revision.id) {
                revision
                    .files
                    .retain(|path, _| !selected.files.contains(path));
                if selected.withhold_message {
                    revision.message = "Forgotten dependent note history".to_string();
                    revision.exposures.clear();
                }
            }
            revision.id = self.id();
        }
        Ok(())
    }
    async fn load_indexes(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
    ) -> Result<MemoryIndexes, MemoryError> {
        let head = self.head(workspace).await;
        let mut indexes = MemoryIndexes {
            revision: head.as_ref().map(|head| head.id.clone()),
            exposures: vec![],
            shared: rules::seed_index("Shared memory"),
            private: rules::seed_index("Private memory"),
        };
        for (scope, content) in [
            (MemoryScope::Shared, &mut indexes.shared),
            (MemoryScope::Private, &mut indexes.private),
        ] {
            let path = ScopedPath {
                scope,
                rel: MEMORY_INDEX_FILE.to_string(),
            }
            .repo_path(access.agent_id().expect("memory access names an agent"));
            if let Some(file) = head.as_ref().and_then(|head| head.files.get(&path))
                && rules::require_readable(access, Some(&file.stamp)).is_ok()
                && self
                    .forget
                    .permits_provenance(workspace, &file.stamp.exposures)
                    .await
                    .map_err(storage)?
            {
                *content = file.content.clone();
                for exposure in &file.stamp.exposures {
                    if !indexes.exposures.contains(exposure) {
                        indexes.exposures.push(exposure.clone());
                    }
                }
            }
        }
        Ok(indexes)
    }
    async fn read(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        path: &ScopedPath,
    ) -> Result<MemoryFile, MemoryError> {
        let head = self
            .head(workspace)
            .await
            .ok_or_else(|| MemoryError::NotFound(path.display()))?;
        let file = head
            .files
            .get(&path.repo_path(access.agent_id().expect("memory access names an agent")))
            .ok_or_else(|| MemoryError::NotFound(path.display()))?;
        rules::require_readable(access, Some(&file.stamp))?;
        self.permits(workspace, &file.stamp.exposures).await?;
        Ok(MemoryFile {
            content: file.content.clone(),
            revision: head.id,
            exposures: file.stamp.exposures.clone(),
            kind: file.stamp.kind,
        })
    }
    async fn brief_entries(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        since: &str,
    ) -> Result<BriefEntries, MemoryError> {
        let history = self.history(workspace);
        let held = history.lock().await;
        let Some(head) = held.last() else {
            return Ok(BriefEntries {
                revision: None,
                entries: vec![],
            });
        };
        let (_, shared) = self
            .readable(workspace, access, MemoryScope::Shared, Some(head))
            .await?;
        let (private_root, private) = self
            .readable(workspace, access, MemoryScope::Private, Some(head))
            .await?;
        let after = held
            .iter()
            .position(|revision| revision.id == since)
            .map(|position| (held.len() - position - 1) as u64);
        rules::brief_entries(
            &private_root,
            shared,
            private,
            PageIndexHead {
                revision: head.id.clone(),
                next_position: held.len() as u64,
            },
            after,
        )
    }
    async fn list_pages(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &PageListQuery,
    ) -> Result<MemoryPageList, MemoryError> {
        let head = self.head(workspace).await;
        let (root, rows) = self
            .readable(workspace, access, scope, head.as_ref())
            .await?;
        rules::list_pages(&root, scope, query, rows)
    }
    async fn search_pages(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>, MemoryError> {
        let head = self.head(workspace).await;
        let (root, rows) = self
            .readable(workspace, access, scope, head.as_ref())
            .await?;
        let index = VolatilePageIndex::default();
        index
            .apply(
                workspace,
                &PageIndexUpdate {
                    replace: true,
                    head: PageIndexHead {
                        revision: head
                            .as_ref()
                            .map(|head| head.id.clone())
                            .unwrap_or_default(),
                        next_position: 0,
                    },
                    changed: rows
                        .into_iter()
                        .map(|(mut row, exposures)| {
                            row.exposures = Some(exposures);
                            row
                        })
                        .collect(),
                    restamped: vec![],
                    removed: vec![],
                },
            )
            .await
            .map_err(storage)?;
        Ok(index
            .search(workspace, &root, query, limit)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|hit| {
                let (path, _) = scoped(&hit.page.path);
                MemorySearchHit {
                    changed_at: hit.page.changed_at,
                    path: path.display(),
                    title: hit.page.title,
                    kind: hit.page.kind,
                    snippet: hit.snippet,
                    rank: hit.rank,
                }
            })
            .collect())
    }
    async fn count_pages(
        &self,
        workspace: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
    ) -> Result<MemoryPageCounts, MemoryError> {
        let head = self.head(workspace).await;
        let (_, rows) = self
            .readable(workspace, access, scope, head.as_ref())
            .await?;
        rules::count_pages(rows)
    }
    async fn commit_diff(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        access: &MemoryAccess,
    ) -> Result<MemoryCommitDiff, MemoryError> {
        let history = self.history(workspace);
        let held = history.lock().await;
        let position = held
            .iter()
            .position(|revision| revision.id == id)
            .ok_or_else(|| MemoryError::NotFound(format!("commit {id}")))?;
        let commit = &held[position];
        rules::require_revision(access, &commit.exposures)?;
        let empty = BTreeMap::new();
        let before = position
            .checked_sub(1)
            .map(|position| &held[position].files)
            .unwrap_or(&empty);
        let paths: std::collections::BTreeSet<_> =
            before.keys().chain(commit.files.keys()).collect();
        let mut files = Vec::new();
        let mut required = commit.exposures.clone();
        for path in paths {
            let (old, new) = (before.get(path), commit.files.get(path));
            if old.map(|file| &file.content) == new.map(|file| &file.content) {
                continue;
            }
            for file in [old, new].into_iter().flatten() {
                rules::require_readable(access, Some(&file.stamp))?;
                for exposure in &file.stamp.exposures {
                    if !required.contains(exposure) {
                        required.push(exposure.clone());
                    }
                }
            }
            let (path, agent_id) = scoped(path);
            files.push(MemoryFileDiff {
                path,
                agent_id,
                hunks: hunks(
                    old.map_or("", |file| &file.content),
                    new.map_or("", |file| &file.content),
                ),
            });
        }
        self.permits(workspace, &required).await?;
        Ok(MemoryCommitDiff {
            sha: commit.id.clone(),
            message: commit
                .message
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            author: commit.author.clone(),
            committed_at: commit.time,
            files,
        })
    }
    async fn commit(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
        access: &MemoryAccess,
        author: &MemoryAuthor,
        changeset: &MemoryChangeset,
        expected: Option<&str>,
        message: &str,
        run: Option<&RunId>,
    ) -> Result<String, MemoryError> {
        let _hold = crate::LegalHold::lock(workspace).await;
        if !changeset.deletes.is_empty() {
            self.forget.refuse_delete(workspace).await?;
        }
        if changeset.is_empty() {
            return Err(MemoryError::Invalid(
                "an empty changeset has nothing to commit".to_string(),
            ));
        }
        for content in changeset.writes.values() {
            validate_content(content)?;
        }
        let history = self.history(workspace);
        let mut held = history.lock().await;
        self.permits(workspace, access.exposures()).await?;
        if held.last().map(|head| head.id.as_str()) != expected {
            return Err(MemoryError::Conflict);
        }
        let mut files = held
            .last()
            .map(|head| head.files.clone())
            .unwrap_or_default();
        let mut previous = Vec::new();
        for path in changeset.writes.keys().chain(&changeset.deletes) {
            if let Some(file) = files.get(&path.repo_path(agent)) {
                self.permits(workspace, &file.stamp.exposures).await?;
                previous.push(&file.stamp);
            }
        }
        let stamp = rules::changeset_stamp(access, previous.into_iter().map(Some))?;
        let time = chrono::Utc::now().timestamp_millis();
        for (path, content) in &changeset.writes {
            let path = path.repo_path(agent);
            let file = match files.get(&path) {
                Some(file) if file.content == *content => File {
                    stamp: stamp.clone(),
                    ..file.clone()
                },
                _ => File {
                    content: content.clone(),
                    stamp: stamp.clone(),
                    position: held.len() as u64,
                    changed_at: time,
                    author: author.clone(),
                },
            };
            files.insert(path, file);
        }
        for path in &changeset.deletes {
            files.remove(&path.repo_path(agent));
        }
        let id = self.id();
        let run = run
            .map(|run| format!("Run-Id: {run}\n"))
            .unwrap_or_default();
        let message = format!(
            "{}\n\n{run}Exposure: {}\n",
            message.trim(),
            serde_json::to_string(&stamp.exposures).expect("exposures are serializable")
        );
        held.push(Revision {
            id: id.clone(),
            files,
            exposures: stamp.exposures,
            message,
            author: author.clone(),
            time,
        });
        Ok(id)
    }
    async fn revert(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        access: &MemoryAccess,
        expected: &str,
        author: &MemoryAuthor,
    ) -> Result<String, MemoryError> {
        let _hold = crate::LegalHold::lock(workspace).await;
        self.forget.refuse_delete(workspace).await?;
        let history = self.history(workspace);
        let mut held = history.lock().await;
        let position = held
            .iter()
            .position(|revision| revision.id == id)
            .ok_or_else(|| MemoryError::NotFound(format!("commit {id}")))?;
        let head = held.last().expect("revision has a head");
        if head.id != expected {
            return Err(MemoryError::Conflict);
        }
        let commit = &held[position];
        rules::require_revision(access, &commit.exposures)?;
        let empty = BTreeMap::new();
        let before = position
            .checked_sub(1)
            .map(|position| &held[position].files)
            .unwrap_or(&empty);
        let paths: std::collections::BTreeSet<_> =
            before.keys().chain(commit.files.keys()).collect();
        let mut files = head.files.clone();
        let mut required = commit.exposures.clone();
        let time = chrono::Utc::now().timestamp_millis();
        for path in paths {
            let (old, changed, current) = (
                before.get(path),
                commit.files.get(path),
                head.files.get(path),
            );
            if old == changed {
                continue;
            }
            for file in [old, changed, current].into_iter().flatten() {
                rules::require_readable(access, Some(&file.stamp))?;
                for exposure in &file.stamp.exposures {
                    if !required.contains(exposure) {
                        required.push(exposure.clone());
                    }
                }
            }
            let content = match (old, changed, current) {
                (Some(old), Some(changed), Some(current)) => Some(
                    diffy::merge(&changed.content, &current.content, &old.content)
                        .map_err(|_| MemoryError::RevertConflict)?,
                ),
                (_, changed, current) if changed == current => old.map(|file| file.content.clone()),
                _ => return Err(MemoryError::RevertConflict),
            };
            match content {
                Some(content) => {
                    let mut file = old.expect("restored file").clone();
                    file.content = content;
                    file.position = held.len() as u64;
                    file.changed_at = time;
                    file.author = author.clone();
                    files.insert(path.clone(), file);
                }
                None => {
                    files.remove(path);
                }
            }
        }
        self.permits(workspace, &required).await?;
        let revision = self.id();
        let message = format!(
            "Revert \"{}\"\n\nReverts commit {id}.\nExposure: {}\n",
            commit.message.lines().next().unwrap_or_default(),
            serde_json::to_string(&required).expect("exposures are serializable")
        );
        held.push(Revision {
            id: revision.clone(),
            files,
            exposures: required,
            message,
            author: author.clone(),
            time,
        });
        Ok(revision)
    }
}
