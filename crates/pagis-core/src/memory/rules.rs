//! Access, Exposure Stamps, and Forget selection over memory values.
use crate::subject_page::{BriefEntry, FrontMatter, SubjectPage};
use crate::{
    BriefEntries, MemoryPageCounts, MemoryPageEntry, MemoryPageList, MemoryScope, PageCursor,
    PageIndexHead, PageListQuery, ScopedPath,
};
use crate::{
    ForgetStore, GrantId, IndexedPage, MemoryAccess, MemoryContentKind, MemoryError,
    MemoryExposure, WorkspaceId,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The Exposure Stamp of one file. Storage keeps the same fields on disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileStamp {
    pub exposures: Vec<MemoryExposure>,
    pub kind: MemoryContentKind,
}

/// A read requires a stamp and access to each exact Grant revision.
pub fn require_readable(
    access: &MemoryAccess,
    stamp: Option<&FileStamp>,
) -> Result<(), MemoryError> {
    require_revision(access, &stamp.ok_or(MemoryError::Unavailable)?.exposures)
}

/// The sentence and both sides of a revision require current access.
pub fn require_revision(
    access: &MemoryAccess,
    exposures: &[MemoryExposure],
) -> Result<(), MemoryError> {
    if access.permits(exposures) {
        Ok(())
    } else {
        Err(MemoryError::Unavailable)
    }
}

/// A Forget can withhold content from each reader, including the owner.
pub async fn require_provenance(
    forget: &dyn ForgetStore,
    workspace: &WorkspaceId,
    exposures: &[MemoryExposure],
) -> Result<(), MemoryError> {
    if forget
        .permits_provenance(workspace, exposures)
        .await
        .map_err(|error| MemoryError::Storage(error.to_string()))?
    {
        Ok(())
    } else {
        Err(MemoryError::Unavailable)
    }
}

/// Each write inherits the access and every stamp the changeset touches.
pub fn changeset_stamp<'a>(
    access: &MemoryAccess,
    previous: impl IntoIterator<Item = Option<&'a FileStamp>>,
) -> Result<FileStamp, MemoryError> {
    let mut exposures = access.exposures().to_vec();
    for stamp in previous {
        require_readable(access, stamp)?;
        let stamp = stamp.expect("a readable file has a stamp");
        for exposure in &stamp.exposures {
            if !exposures.contains(exposure) {
                exposures.push(exposure.clone());
            }
        }
    }
    let kind = if exposures.is_empty() {
        MemoryContentKind::Procedure
    } else {
        MemoryContentKind::DependentView
    };
    Ok(FileStamp { exposures, kind })
}

/// The paths that hold words written with a revoked Grant.
pub fn withheld_paths<'a>(
    grant: &GrantId,
    files: impl IntoIterator<Item = (&'a str, Option<&'a FileStamp>)>,
) -> BTreeSet<String> {
    files
        .into_iter()
        .filter(|(_, stamp)| {
            stamp.is_some_and(|stamp| {
                stamp
                    .exposures
                    .iter()
                    .any(|exposure| &exposure.grant_id == grant)
            })
        })
        .map(|(path, _)| path.to_string())
        .collect()
}

pub async fn filter_readable_pages(
    forget: &dyn ForgetStore,
    workspace: &WorkspaceId,
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
                let allowed = forget
                    .permits_provenance(workspace, &exposures)
                    .await
                    .map_err(|error| MemoryError::Storage(error.to_string()))?;
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

/// One file in one history revision. A scope names its stamp in the history.
pub struct HistoryFile<'a> {
    pub path: &'a str,
    pub scope: Option<usize>,
}
/// One history revision, with the scope of its sentence.
pub struct HistoryRevision<'a> {
    pub revision: &'a str,
    pub files: Vec<HistoryFile<'a>>,
    pub scope: Option<usize>,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RevisionSelection {
    pub files: BTreeSet<String>,
    pub withhold_message: bool,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ForgetSelection {
    pub paths: BTreeSet<String>,
    pub revisions: BTreeMap<String, RevisionSelection>,
}

/// Resolve the stamps a preview affects, or a purge no longer permits.
pub async fn excluded_scopes(
    forget: &dyn ForgetStore,
    workspace: &WorkspaceId,
    stamps: &[FileStamp],
    target: Option<&crate::ForgetTarget>,
) -> Result<HashSet<usize>, MemoryError> {
    let mut excluded = HashSet::new();
    for (index, metadata) in stamps.iter().enumerate() {
        let affected = match target {
            Some(target) => {
                forget
                    .preview_affects(workspace, target, &metadata.exposures)
                    .await
            }
            None => forget
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

/// Select each file and sentence whose stamp a Forget excludes.
/// Unstamped files retain their place in history and remain unavailable.
pub fn select_forget(
    history: &[HistoryRevision<'_>],
    excluded: &HashSet<usize>,
) -> ForgetSelection {
    let mut selection = ForgetSelection::default();
    for revision in history {
        let files: BTreeSet<_> = revision
            .files
            .iter()
            .filter(|file| file.scope.is_some_and(|scope| excluded.contains(&scope)))
            .map(|file| file.path.to_string())
            .collect();
        let withhold_message = revision
            .scope
            .is_some_and(|scope| excluded.contains(&scope));
        if withhold_message || !files.is_empty() {
            selection.paths.extend(files.iter().cloned());
            selection.revisions.insert(
                revision.revision.to_string(),
                RevisionSelection {
                    files,
                    withhold_message,
                },
            );
        }
    }
    selection
}

/// A list previews current truth, never the page's source Timeline.
pub fn page_excerpt(content: &str) -> String {
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
pub fn seed_index(title: &str) -> String {
    format!(
        "# {title}\n\n\
         This index has one line per fact file: `- [Title](path.md) — hook`.\n\
         Keep it curated; it is loaded into context verbatim. Fact files\n\
         are Markdown, read on demand. This file is MEMORY.md at the\n\
         scope root.\n"
    )
}

pub fn list_pages(
    root: &str,
    scope: MemoryScope,
    query: &PageListQuery,
    rows: Vec<(IndexedPage, Vec<MemoryExposure>)>,
) -> Result<MemoryPageList, MemoryError> {
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

pub fn count_pages(
    rows: Vec<(IndexedPage, Vec<MemoryExposure>)>,
) -> Result<MemoryPageCounts, MemoryError> {
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

pub fn brief_entries(
    private_root: &str,
    shared: Vec<(IndexedPage, Vec<MemoryExposure>)>,
    private: Vec<(IndexedPage, Vec<MemoryExposure>)>,
    head: PageIndexHead,
    commits_after: Option<u64>,
) -> Result<BriefEntries, MemoryError> {
    // A cursor that the history does not hold marks each page.
    let first_changed = commits_after.map_or(0, |count| head.next_position.saturating_sub(count));

    let pages: Vec<_> = shared
        .into_iter()
        .chain(private)
        .filter_map(|(row, _)| {
            let brief = row.brief?;
            // The selection speaks the scope-relative form, so a
            // link of a readable page speaks it too.
            let scoped = |path: String| match path.strip_prefix(private_root) {
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

#[cfg(test)]
mod tests;
