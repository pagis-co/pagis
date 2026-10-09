//! The memory REST: the feed is the `memory.committed` /
//! `memory.reverted` event stream with scope, path and
//! source-kind filters; the pages of one scope; one commit's diff;
//! one-tap revert makes the inverse commit; fact files open read-only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{
    AgentId, Event, MemoryAccess, MemoryAuthor, MemoryError, MemoryScope, NewEvent, RunId,
    ScopedPath, TriggerKind, WakeupId, WakeupRule,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use pagis_core::memory_page::page_sources;

/// The guidance shown when later commits touch the reverted files.
const REVERT_CONFLICT_MESSAGE: &str =
    "cannot auto-revert: later changes touch the same files; ask the agent to undo it";

/// The email domain of an agent's commit author, from the run commit.
const AGENT_AUTHOR_DOMAIN: &str = "@agents.pagis.local";

/// The commit identity of user-side actions: reverts and the
/// onboarding seed.
pub(crate) fn user_author() -> MemoryAuthor {
    MemoryAuthor {
        name: "User".to_string(),
        email: "user@pagis.local".to_string(),
    }
}

pub(crate) fn memory_error(err: MemoryError) -> ApiError {
    match err {
        MemoryError::NotFound(what) => ApiError::not_found(&what),
        MemoryError::Invalid(message) => ApiError::validation(message),
        MemoryError::RevertConflict => ApiError {
            status: StatusCode::CONFLICT,
            code: "revert_conflict",
            message: REVERT_CONFLICT_MESSAGE.to_string(),
        },
        MemoryError::Conflict => ApiError {
            status: StatusCode::CONFLICT,
            code: "revision_conflict",
            message: "memory changed after this page loaded; reload and try again".into(),
        },
        MemoryError::Unavailable => ApiError::not_found("memory file"),
        MemoryError::Storage(err) => {
            tracing::error!(error = %err, "memory store error");
            ApiError::internal()
        }
    }
}

/// Owner controls can inspect all agent scopes, subject to current source grants.
async fn current_access(
    state: &AppState,
    tenant: &Tenant,
    agent_id: AgentId,
) -> Result<MemoryAccess, ApiError> {
    let grants = state.grants.list_live(&tenant.workspace_id).await?;
    Ok(MemoryAccess::agent(
        agent_id,
        grants.into_iter().map(|grant| pagis_core::MemoryExposure {
            grant_id: grant.id,
            revision: grant.revision,
        }),
    ))
}

/// The user sees every scope: `shared`, or one agent's directory
/// via `agent:<id>`. The store maps `Private` to the given agent id;
/// for `shared` the agent id is unused.
fn parse_scope(scope: &str) -> Result<(AgentId, MemoryScope), ApiError> {
    if scope == "shared" {
        Ok((AgentId::from(String::new()), MemoryScope::Shared))
    } else if let Some(agent_id) = scope.strip_prefix("agent:") {
        Ok((AgentId::from(agent_id.to_string()), MemoryScope::Private))
    } else {
        Err(ApiError::validation(
            "scope must be `shared` or `agent:<agent_id>`",
        ))
    }
}

/// The `agent:<id>` scope string of a private file, or `shared`.
fn scope_string(scope: MemoryScope, agent_id: Option<&AgentId>) -> String {
    match (scope, agent_id) {
        (MemoryScope::Private, Some(agent_id)) => format!("agent:{}", agent_id.as_str()),
        _ => "shared".to_string(),
    }
}

/// The agent id behind a commit author, from the run commit's email.
fn author_agent_id(author: &MemoryAuthor) -> Option<String> {
    author
        .email
        .strip_suffix(AGENT_AUTHOR_DOMAIN)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// One action the learning feed can apply without another model turn.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFeedAction {
    pub kind: String,
    pub schedule_id: String,
}

/// One learning feed entry: a Reflection commit, a revert, or a wake-only Schedule.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFeedItem {
    /// The event ULID; the page cursor.
    pub id: String,
    /// `committed`, `reverted`, or `schedule`.
    pub kind: String,
    /// Where the change came from: `thread` (a conversation Run),
    /// `sync` (an arrival Run), `backfill` (a historical arrival Run),
    /// or `revert`. `None` for a Schedule entry and for a commit with
    /// no Run, such as the onboarding seed.
    pub source_kind: Option<String>,
    /// The commit this entry is about. Empty for a Schedule entry.
    pub sha: String,
    /// The Agent that wrote the entry. A revert has no author Agent; it
    /// names the owner of the private files it touched, if any.
    pub agent_id: Option<String>,
    /// Denormalized so the feed renders standalone; `None` for
    /// user-side entries (reverts, the onboarding seed).
    pub agent_name: Option<String>,
    pub scopes: Vec<String>,
    pub files: Vec<String>,
    /// The name a person reads for each of `files`, in the same order:
    /// the page title, never a path or an id.
    pub titles: Vec<String>,
    pub message: String,
    pub run_id: Option<String>,
    /// The reply the Run settled; the Thread shows the learned line
    /// under it.
    pub message_id: Option<String>,
    pub source_scoped: bool,
    /// For `reverted`: the commit the inverse commit undoes.
    pub reverted_sha: Option<String>,
    pub created_at: i64,
    pub action: Option<MemoryFeedAction>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFeedPage {
    pub items: Vec<MemoryFeedItem>,
    pub revision: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FeedQuery {
    /// Exclusive ULID cursor: entries older than this event id.
    pub before: Option<String>,
    pub limit: Option<u32>,
    /// `shared`, or `agent:<agent_id>`: entries that touch that scope.
    pub scope: Option<String>,
    /// A scope-relative file path: entries that touch that file.
    pub path: Option<String>,
    /// `thread`, `sync`, `backfill`, or `revert`.
    pub kind: Option<String>,
}

/// The feed filters of one request, parsed once.
struct FeedFilter {
    scope: Option<(AgentId, MemoryScope)>,
    /// The model-facing file paths that match `path`: one when the
    /// scope is given, else the shared and the private form.
    files: Vec<String>,
    kind: Option<String>,
}

impl FeedFilter {
    fn parse(query: &FeedQuery) -> Result<Self, ApiError> {
        let scope = query.scope.as_deref().map(parse_scope).transpose()?;
        let files = match (&query.path, &scope) {
            (None, _) => Vec::new(),
            (Some(path), Some((_, scope))) => vec![format!("{}/{path}", scope.as_str())],
            (Some(path), None) => vec![format!("shared/{path}"), format!("private/{path}")],
        };
        let kind = query.kind.clone();
        if let Some(kind) = &kind
            && !["thread", "sync", "backfill", "revert"].contains(&kind.as_str())
        {
            return Err(ApiError::validation(
                "kind must be `thread`, `sync`, `backfill`, or `revert`",
            ));
        }
        Ok(Self { scope, files, kind })
    }

    fn matches(&self, item: &MemoryFeedItem) -> bool {
        let scope_ok = match &self.scope {
            None => true,
            Some((_, MemoryScope::Shared)) => item.scopes.iter().any(|scope| scope == "shared"),
            Some((agent_id, MemoryScope::Private)) => {
                item.scopes.iter().any(|scope| scope == "private")
                    && item.agent_id.as_deref() == Some(agent_id.as_str())
            }
        };
        let path_ok =
            self.files.is_empty() || item.files.iter().any(|file| self.files.contains(file));
        let kind_ok = self.kind.is_none() || self.kind == item.source_kind;
        scope_ok && path_ok && kind_ok
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryRevertResponse {
    /// The inverse commit's sha.
    pub sha: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileQuery {
    /// `shared`, or `agent:<agent_id>` for one agent's private scope.
    pub scope: String,
    /// The scope-relative file path (for example `MEMORY.md`).
    pub path: String,
}

/// One source that wrote Timeline entries of a page.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryPageSourceDto {
    /// The connection the entries came through; `None` for a
    /// reference that names no connection.
    pub connection_id: Option<String>,
    /// The resource of the reference (`gmail`, `calendar`, ...).
    pub resource: Option<String>,
    pub count: usize,
}

/// A fact file. The desk reads it and does not edit it.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFileDto {
    pub scope: String,
    pub path: String,
    pub content: String,
    pub revision: String,
    /// The title from the front matter or the first heading.
    pub title: String,
    /// One of the page kinds (ADR-0007), or another word the page
    /// declares. A word outside the vocabulary is kept as it is.
    pub kind: Option<String>,
    /// `procedure` or `dependent_view`.
    pub content_kind: String,
    pub source_scoped: bool,
    /// The sources of a Subject Page's Timeline, most entries first.
    /// Empty for a page without a Timeline.
    pub sources: Vec<MemoryPageSourceDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PagesQuery {
    /// `shared`, or `agent:<agent_id>` for one agent's private scope.
    pub scope: String,
    /// Only pages of this kind, for example `Person`.
    pub kind: Option<String>,
    /// Only pages whose title, path, or entity words hold this text.
    pub q: Option<String>,
    /// The `next` of the part before this one.
    pub after: Option<String>,
    /// The size of one part: 100 when absent, 500 at most.
    pub limit: Option<usize>,
}

/// The size of one part of a page list.
const PAGE_LIST_LIMIT: usize = 100;
const PAGE_LIST_LIMIT_MAX: usize = 500;

#[derive(Debug, Deserialize, IntoParams)]
pub struct PageCountsQuery {
    /// `shared`, or `agent:<agent_id>` for one agent's private scope.
    pub scope: String,
}

/// The pages one author changed last.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryPageAuthorDto {
    /// The agent of the author. `None` for the user and for Pagis.
    pub agent_id: Option<String>,
    pub name: String,
    pub pages: usize,
}

/// How many pages a scope holds.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryPageCountsDto {
    pub pages: usize,
    pub procedures: usize,
    /// Most pages first.
    pub authors: Vec<MemoryPageAuthorDto>,
}

/// One file of a scope, as the Memory page lists it.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryPageDto {
    pub scope: String,
    /// The scope-relative file path.
    pub path: String,
    pub title: String,
    pub excerpt: String,
    /// One of the page kinds (ADR-0007), or another word the page
    /// declares. A word outside the vocabulary is kept as it is.
    pub kind: Option<String>,
    /// The connection that wrote most of the page's Timeline. `None`
    /// for a page written from conversations.
    pub source_connection_id: Option<String>,
    /// The time of the last commit that changed the file.
    pub changed_at: i64,
    /// The commit author's name: an agent, `User`, or `Pagis`.
    pub changed_by: String,
    pub changed_by_agent_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryPageList {
    pub pages: Vec<MemoryPageDto>,
    /// The `after` of the next part. `None` at the end of the list.
    pub next: Option<String>,
    /// How many pages match the query, in all parts.
    pub total: usize,
}

/// One changed range of one file: the lines before and the lines
/// after, each with its one-based first line number. A range with no
/// lines on one side starts at the line the other side replaces.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryHunkDto {
    pub old_start: u32,
    pub old_lines: Vec<String>,
    pub new_start: u32,
    pub new_lines: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFileDiffDto {
    /// `shared`, or `agent:<agent_id>`.
    pub scope: String,
    /// The scope-relative file path.
    pub path: String,
    pub hunks: Vec<MemoryHunkDto>,
}

/// The change one commit made, per file.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryCommitDiffDto {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub committed_at: i64,
    pub files: Vec<MemoryFileDiffDto>,
}

fn string_vec(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the owner may read the sentence of one commit now: a
/// source-scoped sentence needs every exposure that shaped it live.
fn sentence_readable(event: &Event, access: &MemoryAccess) -> bool {
    serde_json::from_value::<Vec<pagis_core::MemoryExposure>>(event.payload["exposures"].clone())
        .is_ok_and(|exposures| !exposures.is_empty() && access.permits(&exposures))
}

fn feed_item(
    event: &Event,
    agent_name: Option<String>,
    source_kind: Option<String>,
    access: &MemoryAccess,
) -> MemoryFeedItem {
    let schedule = event.event_type == "schedule.created";
    let kind = if schedule {
        "schedule".to_string()
    } else {
        event
            .event_type
            .strip_prefix("memory.")
            .unwrap_or(&event.event_type)
            .to_string()
    };
    let source_scoped = event.payload["source_scoped"].as_bool().unwrap_or(false);
    // A sentence can contain source text whose grant is now revoked.
    let message = if schedule {
        event.payload["instruction"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    } else if source_scoped && !sentence_readable(event, access) {
        "Updated a source-scoped memory view.".to_string()
    } else {
        event.payload["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    MemoryFeedItem {
        id: event.id.to_string(),
        kind,
        source_kind,
        sha: event.payload["sha"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        // A revert has no Agent author; it carries the owner of the
        // private files it touched.
        agent_id: event
            .agent_id
            .as_ref()
            .map(|a| a.to_string())
            .or_else(|| event.payload["scope_agent_id"].as_str().map(str::to_string)),
        agent_name,
        scopes: if schedule {
            vec!["private".into()]
        } else {
            string_vec(&event.payload["scopes"])
        },
        files: if schedule {
            event.payload["subject_page_path"]
                .as_str()
                .map(str::to_string)
                .into_iter()
                .collect()
        } else {
            string_vec(&event.payload["files"])
        },
        titles: if schedule {
            event.payload["subject_page_path"]
                .as_str()
                .map(|path| pagis_core::memory_page::page_title(path, None))
                .into_iter()
                .collect()
        } else {
            string_vec(&event.payload["titles"])
        },
        message,
        run_id: event_run_id(event),
        message_id: event.payload["message_id"].as_str().map(str::to_string),
        source_scoped,
        reverted_sha: event.payload["reverted_sha"].as_str().map(str::to_string),
        created_at: event.created_at,
        action: schedule.then(|| MemoryFeedAction {
            kind: "cancel_schedule".into(),
            schedule_id: event.payload["schedule_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }),
    }
}

/// Whether a feed event names a file, which each entry of the feed
/// links. A Schedule with no Subject Page names none. A Forget leaves
/// none on the entry of a change that it took out of the history
/// (ADR-0008).
fn names_a_file(event: &Event) -> bool {
    if event.event_type == "schedule.created" {
        event.payload["subject_page_path"].is_string()
    } else {
        !string_vec(&event.payload["files"]).is_empty()
    }
}

fn event_run_id(event: &Event) -> Option<String> {
    event
        .run_id
        .as_ref()
        .map(|run| run.to_string())
        .or_else(|| event.payload["run_id"].as_str().map(str::to_string))
}

/// Where a feed entry came from. A commit's Run names the trigger: a
/// conversation, a Schedule or an Event is `thread`; an arrival Run is
/// `sync`, or `backfill` when its Wake-up holds historical pages
/// alone (ADR-0011). Runs are looked up once per feed page.
async fn source_kind(
    state: &AppState,
    tenant: &Tenant,
    event: &Event,
    runs: &mut HashMap<String, Option<String>>,
) -> Result<Option<String>, ApiError> {
    if event.event_type == "memory.reverted" {
        return Ok(Some("revert".to_string()));
    }
    if event.event_type != "memory.committed" {
        return Ok(None);
    }
    let Some(run_id) = event_run_id(event) else {
        return Ok(None);
    };
    if let Some(kind) = runs.get(&run_id) {
        return Ok(kind.clone());
    }
    let run = state
        .runs
        .get(&tenant.workspace_id, &RunId::from(run_id.clone()))
        .await?;
    let kind = match run {
        None => None,
        Some(run) if run.trigger_kind == TriggerKind::Arrival => {
            let wakeup = match run.trigger_ref {
                Some(wakeup_id) => state
                    .trigger
                    .get_wakeup(&tenant.workspace_id, &WakeupId::from(wakeup_id))
                    .await
                    .map_err(|error| {
                        tracing::error!(error = %error, "wake-up lookup failed");
                        ApiError::internal()
                    })?,
                None => None,
            };
            let historical = matches!(
                wakeup.map(|wakeup| wakeup.rule),
                Some(WakeupRule::Arrival {
                    historical: true,
                    ..
                })
            );
            Some(if historical { "backfill" } else { "sync" }.to_string())
        }
        Some(_) => Some("thread".to_string()),
    };
    runs.insert(run_id, kind.clone());
    Ok(kind)
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/feed",
    params(FeedQuery),
    responses(
        (status = 200, body = MemoryFeedPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn feed(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<FeedQuery>,
) -> Result<Json<MemoryFeedPage>, ApiError> {
    let filter = FeedFilter::parse(&query)?;
    let mut before = query.before.clone().map(pagis_core::EventId::from);
    let limit = query.limit.unwrap_or(50).min(200);
    let names: HashMap<String, String> = state
        .agent_store
        .list_by_workspace(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(|agent| (agent.id.to_string(), agent.name))
        .collect();
    let access = current_access(&state, &tenant, AgentId::from(String::new())).await?;
    let mut runs = HashMap::new();
    let mut items = Vec::new();
    // A filter can drop every event of one fetch, so the feed reads
    // older events until the page is full or the log ends.
    'pages: loop {
        let events = state
            .events
            .list_by_types(
                &tenant.workspace_id,
                &["memory.committed", "memory.reverted", "schedule.created"],
                before.as_ref(),
                limit,
            )
            .await?;
        let Some(last) = events.last() else {
            break;
        };
        before = Some(last.id.clone());
        for event in &events {
            if !names_a_file(event) {
                continue;
            }
            let name = event
                .agent_id
                .as_ref()
                .and_then(|id| names.get(id.as_str()).cloned());
            let kind = source_kind(&state, &tenant, event, &mut runs).await?;
            let item = feed_item(event, name, kind, &access);
            if !filter.matches(&item) {
                continue;
            }
            items.push(item);
            if items.len() as u32 == limit {
                break 'pages;
            }
        }
        if (events.len() as u32) < limit {
            break;
        }
    }
    let revision = state
        .memory
        .load_indexes(&tenant.workspace_id, &access)
        .await
        .map_err(memory_error)?
        .revision;
    Ok(Json(MemoryFeedPage { items, revision }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MemoryRevertRequest {
    pub expected_revision: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/memory/commits/{sha}/revert",
    params(("sha" = String, Path, description = "The commit to revert")),
    request_body = MemoryRevertRequest,
    responses(
        (status = 200, body = MemoryRevertResponse),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "Later commits touch the same files", body = crate::error::ErrorBody),
    )
)]
pub async fn revert_commit(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(sha): Path<String>,
    Json(request): Json<MemoryRevertRequest>,
) -> Result<Json<MemoryRevertResponse>, ApiError> {
    let access = current_access(&state, &tenant, AgentId::from(String::new())).await?;
    let revert_sha = state
        .memory
        .revert(
            &tenant.workspace_id,
            &sha,
            &access,
            &request.expected_revision,
            &user_author(),
        )
        .await
        .map_err(memory_error)?;
    // The revert event names the files and the scope owner of the
    // inverse commit, so the scope and path filters of the feed find it.
    let diff = state
        .memory
        .commit_diff(&tenant.workspace_id, &revert_sha, &access)
        .await
        .map_err(memory_error)?;
    let mut scopes: Vec<&str> = diff
        .files
        .iter()
        .map(|file| file.path.scope.as_str())
        .collect();
    scopes.sort_unstable();
    scopes.dedup();
    let files: Vec<String> = diff.files.iter().map(|file| file.path.display()).collect();
    let titles: Vec<String> = files
        .iter()
        .map(|file| pagis_core::memory_page::page_title(file, None))
        .collect();
    let owner = diff
        .files
        .iter()
        .find_map(|file| file.agent_id.as_ref().map(AgentId::as_str));
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "memory.reverted".to_string(),
            // The user reverts; the event has no Agent author.
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "sha": revert_sha,
                "reverted_sha": sha,
                "scopes": scopes,
                "files": files,
                "titles": titles,
                "scope_agent_id": owner,
            }),
        })
        .await?;
    Ok(Json(MemoryRevertResponse { sha: revert_sha }))
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/commits/{sha}/diff",
    params(("sha" = String, Path, description = "The commit to show")),
    responses(
        (status = 200, body = MemoryCommitDiffDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn commit_diff(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(sha): Path<String>,
) -> Result<Json<MemoryCommitDiffDto>, ApiError> {
    let diff = state
        .memory
        .commit_diff(
            &tenant.workspace_id,
            &sha,
            &current_access(&state, &tenant, AgentId::from(String::new())).await?,
        )
        .await
        .map_err(memory_error)?;
    Ok(Json(MemoryCommitDiffDto {
        sha: diff.sha,
        message: diff.message,
        author: diff.author.name,
        committed_at: diff.committed_at,
        files: diff
            .files
            .into_iter()
            .map(|file| MemoryFileDiffDto {
                scope: scope_string(file.path.scope, file.agent_id.as_ref()),
                path: file.path.rel,
                hunks: file
                    .hunks
                    .into_iter()
                    .map(|hunk| MemoryHunkDto {
                        old_start: hunk.old_start,
                        old_lines: hunk.old_lines,
                        new_start: hunk.new_start,
                        new_lines: hunk.new_lines,
                    })
                    .collect(),
            })
            .collect(),
    }))
}

/// True for a file the daemon writes as a Subject Page (ADR-0007).
fn is_subject_page(scope: MemoryScope, path: &str) -> bool {
    scope == MemoryScope::Private && path.starts_with("subjects/") && path.ends_with(".md")
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/pages",
    params(PagesQuery),
    responses(
        (status = 200, body = MemoryPageList),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn list_pages(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<PagesQuery>,
) -> Result<Json<MemoryPageList>, ApiError> {
    let (agent_id, scope) = parse_scope(&query.scope)?;
    let list = state
        .memory
        .list_pages(
            &tenant.workspace_id,
            &current_access(&state, &tenant, agent_id).await?,
            scope,
            &pagis_core::PageListQuery {
                kind: query.kind,
                search: query.q,
                after: query
                    .after
                    .as_deref()
                    .map(str::parse)
                    .transpose()
                    .map_err(memory_error)?,
                limit: query
                    .limit
                    .unwrap_or(PAGE_LIST_LIMIT)
                    .clamp(1, PAGE_LIST_LIMIT_MAX),
            },
        )
        .await
        .map_err(memory_error)?;
    let pages = list
        .pages
        .into_iter()
        .map(|entry| MemoryPageDto {
            scope: query.scope.clone(),
            path: entry.path.rel,
            title: entry.title,
            excerpt: entry.excerpt,
            kind: entry.kind,
            source_connection_id: entry.source_connection_id,
            changed_at: entry.changed_at,
            changed_by_agent_id: author_agent_id(&entry.changed_by),
            changed_by: entry.changed_by.name,
        })
        .collect();
    Ok(Json(MemoryPageList {
        pages,
        next: list.next.map(|cursor| cursor.to_string()),
        total: list.total,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/pages/counts",
    params(PageCountsQuery),
    responses(
        (status = 200, body = MemoryPageCountsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn count_pages(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<PageCountsQuery>,
) -> Result<Json<MemoryPageCountsDto>, ApiError> {
    let (agent_id, scope) = parse_scope(&query.scope)?;
    let counts = state
        .memory
        .count_pages(
            &tenant.workspace_id,
            &current_access(&state, &tenant, agent_id).await?,
            scope,
        )
        .await
        .map_err(memory_error)?;
    Ok(Json(MemoryPageCountsDto {
        pages: counts.pages,
        procedures: counts.procedures,
        authors: counts
            .authors
            .into_iter()
            .map(|(author, pages)| MemoryPageAuthorDto {
                agent_id: author_agent_id(&author),
                name: author.name,
                pages,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/file",
    params(FileQuery),
    responses(
        (status = 200, body = MemoryFileDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn get_file(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<FileQuery>,
) -> Result<Json<MemoryFileDto>, ApiError> {
    let (agent_id, scope) = parse_scope(&query.scope)?;
    // Re-validate through the model-facing parser for the guardrails.
    let path =
        ScopedPath::parse(&format!("{}/{}", scope.as_str(), query.path)).map_err(memory_error)?;
    let content = state
        .memory
        .read(
            &tenant.workspace_id,
            &current_access(&state, &tenant, agent_id.clone()).await?,
            &path,
        )
        .await
        .map_err(memory_error)?;
    let mut rendered_content = content.content.clone();
    let mut sources = Vec::new();
    if is_subject_page(scope, &query.path) {
        let mut page = pagis_core::subject_page::SubjectPage::parse(&rendered_content);
        if page.layout_valid {
            let display_path = path.display();
            page.page.schedules = state
                .trigger
                .list_open_for_subject_page(&tenant.workspace_id, &agent_id, &display_path)
                .await
                .map_err(|error| ApiError::validation(error.to_string()))?;
            rendered_content = page.page.render();
            sources = page_sources(&page.page.timeline)
                .into_iter()
                .map(|source| MemoryPageSourceDto {
                    connection_id: source.connection_id,
                    resource: source.resource,
                    count: source.count,
                })
                .collect();
        }
    }
    let content_kind = match content.kind {
        pagis_core::MemoryContentKind::Procedure => "procedure",
        pagis_core::MemoryContentKind::DependentView => "dependent_view",
    };
    let heading = pagis_core::memory_page::PageHeading::parse(&query.path, &content.content);
    Ok(Json(MemoryFileDto {
        title: heading.title,
        kind: heading.kind,
        scope: query.scope,
        path: query.path,
        source_scoped: !content.exposures.is_empty(),
        revision: content.revision,
        content_kind: content_kind.into(),
        content: rendered_content,
        sources,
    }))
}
