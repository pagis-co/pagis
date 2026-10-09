//! The memory seam: scoped paths, the run changeset, and the
//! `MemoryStore` trait. The git implementation lives in `pagis-memory`;
//! a hosted backend implements the same trait later.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;

use crate::id::{AgentId, GrantId, RunId, WorkspaceId};
use crate::store::StoreError;

/// The per-file size cap for one memory write.
pub const MAX_MEMORY_FILE_BYTES: usize = 256 * 1024;

/// The index file at each scope root; loaded verbatim into context.
pub const MEMORY_INDEX_FILE: &str = "MEMORY.md";

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// The path or content breaks a guardrail; the message goes back to
    /// the model as the tool result.
    #[error("{0}")]
    Invalid(String),
    #[error("no memory file at {0}")]
    NotFound(String),
    /// A later commit touches the same files; the revert is aborted.
    #[error("cannot auto-revert: later changes touch the same files")]
    RevertConflict,
    #[error("memory changed after the supplied base revision")]
    Conflict,
    #[error("memory content is not available with the current access")]
    Unavailable,
    #[error("memory storage error: {0}")]
    Storage(String),
}

/// The two scopes an agent can address. `Private` maps to the
/// agent's own directory; the store does the mapping, so one agent can
/// never name another agent's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryScope {
    Shared,
    Private,
}

impl MemoryScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryScope::Shared => "shared",
            MemoryScope::Private => "private",
        }
    }
}

/// A validated scope-relative path: `shared/…` or `private/…`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopedPath {
    pub scope: MemoryScope,
    /// The path under the scope root, always non-empty.
    pub rel: String,
}

impl ScopedPath {
    /// Parse and validate one model-facing path. Rejects paths outside
    /// the two scopes and every path that could escape them.
    pub fn parse(path: &str) -> Result<Self, MemoryError> {
        let invalid = |reason: &str| {
            MemoryError::Invalid(format!(
                "invalid memory path {path:?}: {reason}; use shared/<file> or private/<file>"
            ))
        };
        let (scope, rel) = if let Some(rel) = path.strip_prefix("shared/") {
            (MemoryScope::Shared, rel)
        } else if let Some(rel) = path.strip_prefix("private/") {
            (MemoryScope::Private, rel)
        } else {
            return Err(invalid("it names no scope"));
        };
        if rel.is_empty() {
            return Err(invalid("it names no file"));
        }
        if rel.ends_with('/') {
            return Err(invalid("it names a directory"));
        }
        for segment in rel.split('/') {
            if segment.is_empty() {
                return Err(invalid("it contains an empty segment"));
            }
            if segment == "." || segment == ".." {
                return Err(invalid("it escapes the scope"));
            }
        }
        if rel.contains('\\') || rel.contains('\0') {
            return Err(invalid("it contains a forbidden character"));
        }
        Ok(Self {
            scope,
            rel: rel.to_string(),
        })
    }

    /// The model-facing form: `shared/…` or `private/…`.
    pub fn display(&self) -> String {
        format!("{}/{}", self.scope.as_str(), self.rel)
    }

    /// The path in the repository of the Workspace. A shared path stays
    /// `shared/…`, and a private path goes below `agents/<agent id>/`
    /// for `agent_id`. The Page Index keys its rows on this path
    /// (ADR-0008).
    pub fn repo_path(&self, agent_id: &AgentId) -> String {
        match self.scope {
            MemoryScope::Shared => format!("shared/{}", self.rel),
            MemoryScope::Private => format!("agents/{}/{}", agent_id.as_str(), self.rel),
        }
    }
}

/// Validate one memory write's content against the guardrails.
pub fn validate_content(content: &str) -> Result<(), MemoryError> {
    if content.len() > MAX_MEMORY_FILE_BYTES {
        return Err(MemoryError::Invalid(format!(
            "content is {} bytes; the cap is {MAX_MEMORY_FILE_BYTES}",
            content.len()
        )));
    }
    Ok(())
}

/// One run's staged memory changes, committed once at run end.
/// Whole files: a write replaces the file, a delete removes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryChangeset {
    pub writes: BTreeMap<ScopedPath, String>,
    pub deletes: BTreeSet<ScopedPath>,
}

impl MemoryChangeset {
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes.is_empty()
    }

    /// Every path the changeset touches, in model-facing form.
    pub fn files(&self) -> Vec<String> {
        self.writes
            .keys()
            .chain(self.deletes.iter())
            .map(ScopedPath::display)
            .collect()
    }

    /// The name a person reads for each path of [`Self::files`], in
    /// the same order (see [`crate::memory_page::page_title`]).
    pub fn titles(&self) -> Vec<String> {
        self.writes
            .iter()
            .map(|(path, content)| crate::memory_page::page_title(&path.display(), Some(content)))
            .chain(
                self.deletes
                    .iter()
                    .map(|path| crate::memory_page::page_title(&path.display(), None)),
            )
            .collect()
    }

    /// The scopes the changeset touches.
    pub fn scopes(&self) -> Vec<&'static str> {
        let scopes: BTreeSet<MemoryScope> = self
            .writes
            .keys()
            .chain(self.deletes.iter())
            .map(|p| p.scope)
            .collect();
        scopes.into_iter().map(|s| s.as_str()).collect()
    }
}

/// The two indexes a run's context embeds verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryIndexes {
    pub shared: String,
    pub private: String,
    /// Repository HEAD used as the base for a later whole-file write.
    pub revision: Option<String>,
}

/// One source permission that influenced retained prose. The daemon
/// derives this value from live grants; model output cannot create it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryExposure {
    pub grant_id: GrantId,
    pub revision: i64,
}

/// Current access for one memory operation. The owner can inspect all
/// revisions in the memory controls. An Agent must hold each exposure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryAccess {
    Owner {
        agent_id: AgentId,
    },
    Agent {
        agent_id: AgentId,
        exposures: Vec<MemoryExposure>,
    },
}

impl MemoryAccess {
    pub fn agent(agent_id: AgentId, exposures: impl IntoIterator<Item = MemoryExposure>) -> Self {
        Self::Agent {
            agent_id,
            exposures: exposures.into_iter().collect(),
        }
    }

    pub fn agent_id(&self) -> Option<&AgentId> {
        match self {
            Self::Owner { agent_id } | Self::Agent { agent_id, .. } => Some(agent_id),
        }
    }

    pub fn permits(&self, required: &[MemoryExposure]) -> bool {
        match self {
            Self::Owner { .. } => true,
            Self::Agent { exposures, .. } => required.iter().all(|item| exposures.contains(item)),
        }
    }

    pub fn exposures(&self) -> &[MemoryExposure] {
        match self {
            Self::Owner { .. } => &[],
            Self::Agent { exposures, .. } => exposures,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    pub content: String,
    pub revision: String,
    pub exposures: Vec<MemoryExposure>,
    pub kind: MemoryContentKind,
}

/// What the selection of a Brief needs about each Subject Page an
/// access reads, as of one memory revision.
#[derive(Debug, Clone, PartialEq)]
pub struct BriefEntries {
    /// The memory revision of the entries. `None` for a workspace
    /// with no memory.
    pub revision: Option<String>,
    pub entries: Vec<crate::subject_page::BriefEntry>,
}

/// One file of a scope with the commit that last changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPageEntry {
    pub path: ScopedPath,
    pub title: String,
    /// The first paragraph of the readable page's current content.
    pub excerpt: String,
    /// One of the page kinds (ADR-0007), or another word the page
    /// declares. A word outside the vocabulary is kept as it is.
    pub kind: Option<String>,
    /// The connection that wrote most of the Timeline of a Subject
    /// Page. `None` for a page written from conversations.
    pub source_connection_id: Option<String>,
    /// The time of the last commit that changed the file, in Unix milliseconds.
    pub changed_at: i64,
    pub changed_by: MemoryAuthor,
    pub exposures: Vec<MemoryExposure>,
}

/// Which pages of a scope a list asks for. The order is the
/// newest change first and then the path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageListQuery {
    /// Only pages of this kind, for example `Person`.
    pub kind: Option<String>,
    /// Only pages whose title, path, or entity words hold this text.
    /// The case of the letters does not matter.
    pub search: Option<String>,
    /// Only pages after this place in the order.
    pub after: Option<PageCursor>,
    /// No more pages than this.
    pub limit: usize,
}

/// A place in the order of a page list: the page it names is the last
/// one the reader has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub position: u64,
    /// The scope-relative path.
    pub path: String,
}

impl std::fmt::Display for PageCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.position, self.path)
    }
}

impl std::str::FromStr for PageCursor {
    type Err = MemoryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || MemoryError::Invalid("the page cursor is not valid".to_string());
        let (position, path) = value.split_once(':').ok_or_else(invalid)?;
        Ok(Self {
            position: position.parse().map_err(|_| invalid())?,
            path: path.to_string(),
        })
    }
}

/// One part of a page list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPageList {
    pub pages: Vec<MemoryPageEntry>,
    /// The cursor of the next part. `None` at the end of the list.
    pub next: Option<PageCursor>,
    /// How many pages match the query, in all parts.
    pub total: usize,
}

/// How many pages a scope holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryPageCounts {
    pub pages: usize,
    pub procedures: usize,
    /// The pages each author changed last, most pages first.
    pub authors: Vec<(MemoryAuthor, usize)>,
}

/// One memory file as the Page Index holds it (ADR-0008): the
/// last commit that changed it, and what a page list shows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedPage {
    /// The path in the memory repository (`shared/...` or
    /// `agents/<agent id>/...`).
    pub path: String,
    /// The place of the commit in the history. A larger number is a
    /// newer commit. Commit times have a resolution of one second, so
    /// the time alone does not order two commits.
    pub position: u64,
    /// Unix milliseconds.
    pub changed_at: i64,
    pub changed_by: MemoryAuthor,
    pub title: String,
    /// The complete Markdown file. The Page Index uses it only for
    /// full-text search.
    pub body: String,
    pub kind: Option<String>,
    /// The connection that wrote most of the Timeline of a Subject
    /// Page.
    pub source_connection_id: Option<String>,
    /// The exposure stamp of the file. A file with no stamp is
    /// unavailable, as in a read.
    pub exposures: Option<Vec<MemoryExposure>>,
    /// What the selection of a Brief needs. `None` for a file that is
    /// not a Subject Page with a valid layout.
    pub brief: Option<crate::memory_page::PageBrief>,
    /// The pages this page links to, as repository paths. A
    /// target the index does not hold is kept: it marks a page worth
    /// writing later. Whether a target exists is not stored, because
    /// it changes when another page is written or removed; a reader
    /// computes it from the rows of the index.
    pub links: Vec<String>,
}

/// One full-text match from the Page Index.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSearchHit {
    pub page: IndexedPage,
    pub snippet: String,
    /// The FTS rank. A smaller value is a better match.
    pub rank: f64,
}

/// One memory page that a caller can read and that matches a search.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MemorySearchHit {
    pub path: String,
    pub title: String,
    pub kind: Option<String>,
    pub snippet: String,
    /// The rank of the match. A smaller value is a better match. It
    /// puts the hits of two scopes in one order.
    #[serde(skip)]
    pub rank: f64,
}

/// The memory revision a Page Index describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIndexHead {
    pub revision: String,
    /// The position the next commit gets.
    pub next_position: u64,
}

/// One step of a Page Index to a newer memory revision.
#[derive(Debug, Clone, PartialEq)]
pub struct PageIndexUpdate {
    /// True when the history does not contain the revision the index
    /// held (first build, forget rebuild). The update then holds the
    /// full history, and the earlier rows go.
    pub replace: bool,
    pub head: PageIndexHead,
    pub changed: Vec<IndexedPage>,
    /// Files whose exposure stamp changed in a commit that did not
    /// change the file, with the new stamp. The last change stays.
    pub restamped: Vec<(String, Option<Vec<MemoryExposure>>)>,
    /// Paths whose newest change is a deletion.
    pub removed: Vec<String>,
}

/// The Page Index seam (ADR-0008): derived data about the files
/// of a memory repository, kept so that a page list does not read the
/// history. The repository stays the source of truth. An index whose
/// revision is not the HEAD of the repository is brought to it from
/// the history before a read.
#[async_trait]
pub trait MemoryPageIndex: Send + Sync {
    async fn head(&self, workspace_id: &WorkspaceId) -> Result<Option<PageIndexHead>, StoreError>;

    /// Apply one update and move the head, as one transaction.
    async fn apply(
        &self,
        workspace_id: &WorkspaceId,
        update: &PageIndexUpdate,
    ) -> Result<(), StoreError>;

    /// The pages below `root`, the newest change first and then in
    /// path order.
    async fn pages(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
    ) -> Result<Vec<IndexedPage>, StoreError>;

    /// Search the title, path, and body of pages below `root`.
    async fn search(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<PageSearchHit>, StoreError>;
}

/// A Page Index in process memory. A store that has no database (a
/// test, a one-time import) builds it from the history at its first
/// read.
#[derive(Default)]
pub struct VolatilePageIndex {
    workspaces: std::sync::Mutex<BTreeMap<String, VolatileWorkspaceIndex>>,
}

/// The head of one workspace and its changes by path.
type VolatileWorkspaceIndex = (PageIndexHead, BTreeMap<String, IndexedPage>);

#[async_trait]
impl MemoryPageIndex for VolatilePageIndex {
    async fn head(&self, workspace_id: &WorkspaceId) -> Result<Option<PageIndexHead>, StoreError> {
        let workspaces = self.workspaces.lock().expect("page index lock");
        Ok(workspaces
            .get(workspace_id.as_str())
            .map(|(head, _)| head.clone()))
    }

    async fn apply(
        &self,
        workspace_id: &WorkspaceId,
        update: &PageIndexUpdate,
    ) -> Result<(), StoreError> {
        let mut workspaces = self.workspaces.lock().expect("page index lock");
        let (head, changes) = workspaces
            .entry(workspace_id.as_str().to_string())
            .or_insert_with(|| (update.head.clone(), BTreeMap::new()));
        if update.replace {
            changes.clear();
        }
        for path in &update.removed {
            changes.remove(path);
        }
        for page in &update.changed {
            changes.insert(page.path.clone(), page.clone());
        }
        for (path, exposures) in &update.restamped {
            if let Some(page) = changes.get_mut(path) {
                page.exposures = exposures.clone();
            }
        }
        *head = update.head.clone();
        Ok(())
    }

    async fn pages(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
    ) -> Result<Vec<IndexedPage>, StoreError> {
        let workspaces = self.workspaces.lock().expect("page index lock");
        let mut changes: Vec<_> = workspaces
            .get(workspace_id.as_str())
            .into_iter()
            .flat_map(|(_, changes)| changes.values())
            .filter(|change| change.path.starts_with(root))
            .cloned()
            .collect();
        changes.sort_by(|left, right| {
            right
                .position
                .cmp(&left.position)
                .then_with(|| left.path.cmp(&right.path))
        });
        Ok(changes)
    }

    async fn search(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<PageSearchHit>, StoreError> {
        let tokens: Vec<String> = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let workspaces = self.workspaces.lock().expect("page index lock");
        let mut hits: Vec<_> = workspaces
            .get(workspace_id.as_str())
            .into_iter()
            .flat_map(|(_, changes)| changes.values())
            .filter(|page| page.path.starts_with(root))
            .filter_map(|page| {
                let text = format!("{} {} {}", page.path, page.title, page.body).to_lowercase();
                tokens
                    .iter()
                    .any(|token| text.contains(token))
                    .then(|| PageSearchHit {
                        page: page.clone(),
                        snippet: page
                            .body
                            .lines()
                            .find(|line| {
                                tokens
                                    .iter()
                                    .any(|token| line.to_lowercase().contains(token))
                            })
                            .unwrap_or(&page.title)
                            .to_string(),
                        rank: -(tokens
                            .iter()
                            .filter(|token| text.contains(token.as_str()))
                            .count() as f64),
                    })
            })
            .collect();
        hits.sort_by(|left, right| {
            left.rank
                .total_cmp(&right.rank)
                .then_with(|| left.page.path.cmp(&right.page.path))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

/// The change one commit made, per file, as line ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCommitDiff {
    pub sha: String,
    pub message: String,
    pub author: MemoryAuthor,
    /// The commit time, in Unix milliseconds.
    pub committed_at: i64,
    pub files: Vec<MemoryFileDiff>,
}

/// The hunks of one file in one commit. `agent_id` is the owner of a
/// private file; a shared file has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFileDiff {
    pub path: ScopedPath,
    pub agent_id: Option<AgentId>,
    pub hunks: Vec<MemoryHunk>,
}

/// One changed range: the lines before and the lines after, each with
/// its one-based first line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHunk {
    pub old_start: u32,
    pub old_lines: Vec<String>,
    pub new_start: u32,
    pub new_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryContentKind {
    Procedure,
    DependentView,
}

/// The identity one commit is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAuthor {
    pub name: String,
    pub email: String,
}

/// The memory store seam: a per-workspace git repository. Stateless:
/// in-run staging lives in the agent loop.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Find a commit whose full message contains every exact trailer.
    /// Review recovery uses all claim identity and source-range trailers;
    /// a Run id alone is not unique across memory phases.
    async fn find_commit_with_trailers(
        &self,
        _workspace_id: &WorkspaceId,
        _trailers: &[(&str, &str)],
    ) -> Result<Option<String>, MemoryError> {
        Ok(None)
    }
    async fn preview_forget(
        &self,
        workspace: &WorkspaceId,
        target: &crate::ForgetTarget,
    ) -> Result<crate::ForgetPreview, MemoryError>;
    /// Begin a Forget that the owner confirmed. `keys` gives the
    /// suppression key that the Forget writes its rows with.
    async fn begin_forget(
        &self,
        workspace: &WorkspaceId,
        target: &crate::ForgetTarget,
        preview: &crate::ForgetPreview,
        now: i64,
        keys: &dyn crate::ForgetKeys,
    ) -> Result<crate::ForgetOperation, MemoryError>;
    async fn purge_forgotten(
        &self,
        workspace: &WorkspaceId,
        operation_id: &str,
    ) -> Result<(), MemoryError>;
    /// Both index files for one agent's context. A scope without an
    /// index yet gets the seed header that states the convention.
    async fn load_indexes(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
    ) -> Result<MemoryIndexes, MemoryError>;

    /// One fact file's content.
    async fn read(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        path: &ScopedPath,
    ) -> Result<MemoryFile, MemoryError>;

    /// What the selection of a Brief needs about each permitted Subject
    /// Page, from the Page Index. No page content is read. A page is
    /// `changed` when a commit after `since_revision` changed it. A
    /// revision that the history does not hold marks each page as
    /// changed.
    async fn brief_entries(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        since_revision: &str,
    ) -> Result<BriefEntries, MemoryError>;

    /// Every permitted file of one scope, newest change first. `Private`
    /// is the scope of the agent that `access` names.
    async fn list_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &PageListQuery,
    ) -> Result<MemoryPageList, MemoryError>;

    /// Search the permitted files of one scope.
    async fn search_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemorySearchHit>, MemoryError>;

    /// How many pages of one scope the access reads.
    async fn count_pages(
        &self,
        workspace_id: &WorkspaceId,
        access: &MemoryAccess,
        scope: MemoryScope,
    ) -> Result<MemoryPageCounts, MemoryError>;

    /// The change one commit made to the memory files it touched, as
    /// line ranges per file.
    async fn commit_diff(
        &self,
        workspace_id: &WorkspaceId,
        commit_id: &str,
        access: &MemoryAccess,
    ) -> Result<MemoryCommitDiff, MemoryError>;

    /// Apply one changeset as one commit if the expected HEAD matches,
    /// serialized per workspace. Returns the commit
    /// sha. `run_id` links a run's Reflection commit via the `Run-Id:`
    /// trailer; a user-side commit (the onboarding seed) has none.
    #[expect(
        clippy::too_many_arguments,
        reason = "a memory commit keeps its security and audit inputs explicit"
    )]
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
    ) -> Result<String, MemoryError>;

    /// Revert one commit with an inverse commit, history preserved.
    /// Aborts cleanly when later commits touch the same files. Returns
    /// the revert commit sha.
    async fn revert(
        &self,
        workspace_id: &WorkspaceId,
        commit_id: &str,
        access: &MemoryAccess,
        expected_revision: &str,
        author: &MemoryAuthor,
    ) -> Result<String, MemoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_paths_parse_into_scope_and_relative_path() {
        let shared = ScopedPath::parse("shared/preferences.md").unwrap();
        assert_eq!(shared.scope, MemoryScope::Shared);
        assert_eq!(shared.rel, "preferences.md");
        assert_eq!(shared.display(), "shared/preferences.md");

        let private = ScopedPath::parse("private/craft/notes.md").unwrap();
        assert_eq!(private.scope, MemoryScope::Private);
        assert_eq!(private.rel, "craft/notes.md");
    }

    #[test]
    fn a_private_path_is_in_the_directory_of_its_agent_in_the_repository() {
        let agent = AgentId::from("sage".to_string());
        assert_eq!(
            ScopedPath::parse("shared/preferences.md")
                .unwrap()
                .repo_path(&agent),
            "shared/preferences.md"
        );
        assert_eq!(
            ScopedPath::parse("private/craft/notes.md")
                .unwrap()
                .repo_path(&agent),
            "agents/sage/craft/notes.md"
        );
    }

    #[test]
    fn scope_escaping_paths_are_rejected() {
        for path in [
            "shared/../secret",
            "private/../../etc/passwd",
            "shared/./x.md",
            "/etc/passwd",
            "agents/other/MEMORY.md",
            "shared/",
            "shared",
            "private//x.md",
            "shared/dir/",
            "shared/a\\b.md",
            "shared/a\0b",
            "",
        ] {
            assert!(ScopedPath::parse(path).is_err(), "must reject {path:?}");
        }
    }

    #[test]
    fn oversized_content_is_rejected() {
        assert!(validate_content("small").is_ok());
        assert!(validate_content(&"x".repeat(MAX_MEMORY_FILE_BYTES + 1)).is_err());
    }

    #[test]
    fn a_changeset_reports_its_files_and_scopes() {
        let mut changeset = MemoryChangeset::default();
        assert!(changeset.is_empty());

        changeset.writes.insert(
            ScopedPath::parse("private/MEMORY.md").unwrap(),
            "index".to_string(),
        );
        changeset
            .deletes
            .insert(ScopedPath::parse("shared/old.md").unwrap());

        assert!(!changeset.is_empty());
        assert_eq!(
            changeset.files(),
            vec!["private/MEMORY.md", "shared/old.md"]
        );
        assert_eq!(changeset.scopes(), vec!["shared", "private"]);
    }

    #[test]
    fn a_volatile_page_index_searches_without_letter_case() {
        futures::executor::block_on(async {
            let workspace_id = WorkspaceId::generate();
            let index = VolatilePageIndex::default();
            index
                .apply(
                    &workspace_id,
                    &PageIndexUpdate {
                        replace: true,
                        head: PageIndexHead {
                            revision: "r1".to_string(),
                            next_position: 1,
                        },
                        changed: vec![IndexedPage {
                            path: "shared/vignesh.md".to_string(),
                            position: 0,
                            changed_at: 0,
                            changed_by: MemoryAuthor {
                                name: "Sage".to_string(),
                                email: "sage@agents.pagis.local".to_string(),
                            },
                            title: "Vignesh".to_string(),
                            body: "The Heliotrope office.".to_string(),
                            kind: None,
                            source_connection_id: None,
                            exposures: Some(Vec::new()),
                            brief: None,
                            links: Vec::new(),
                        }],
                        restamped: Vec::new(),
                        removed: Vec::new(),
                    },
                )
                .await
                .unwrap();

            let hits = index
                .search(&workspace_id, "shared/", "heliotrope", 10)
                .await
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].page.path, "shared/vignesh.md");
        });
    }
}
