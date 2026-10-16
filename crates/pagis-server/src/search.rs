//! Search sends each query to its owning Workspace. Results have no stored copy.
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json,
    extract::{Query, State},
};
use pagis_core::{AgentId, Channel, ChannelId, MemoryScope, WorkspaceId, WorkspaceKind};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, auth::Tenant, error::ApiError, workspace::AccessibleWorkspaceDto};

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchWorkspace {
    pub id: String,
    pub name: String,
    pub kind: WorkspaceKind,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchItem {
    pub workspace: SearchWorkspace,
    pub kind: String,
    pub title: String,
    pub snippet: String,
    pub at: i64,
    pub url: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct IncompleteWorkspace {
    pub workspace_id: String,
    pub workspace_name: String,
    pub reason: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResponse {
    pub items: Vec<SearchItem>,
    pub incomplete: Vec<IncompleteWorkspace>,
}

fn query_url(path: &str, pairs: &[(&str, &str)]) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().copied())
        .finish();
    format!("{path}?{query}")
}
fn newest(items: &mut Vec<SearchItem>, limit: u32) {
    items.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| a.workspace.id.cmp(&b.workspace.id))
            .then_with(|| a.url.cmp(&b.url))
    });
    items.truncate(limit as usize);
}

/// Search the source once, then check and name each distinct Channel.
async fn search_messages(
    state: &AppState,
    person: &Tenant,
    workspace: &SearchWorkspace,
    query: &str,
    limit: u32,
    conversation: bool,
) -> Result<Vec<SearchItem>, ApiError> {
    let source_id = if conversation {
        state.org_workspace_id.clone()
    } else {
        WorkspaceId::from(workspace.id.clone())
    };
    let hits = state
        .conversation_evidence
        .search_workspace_messages(
            &source_id,
            conversation.then_some(&person.workspace_id),
            query,
            limit,
        )
        .await?;
    let mut channels: HashMap<ChannelId, Option<Channel>> = HashMap::new();
    let mut found = Vec::new();
    for hit in hits {
        if let Entry::Vacant(entry) = channels.entry(hit.channel_id.clone()) {
            let permitted = if conversation {
                match crate::conversations::entrance(state, person, &hit.channel_id).await {
                    Ok(_) => true,
                    Err(error) if error.status == axum::http::StatusCode::NOT_FOUND => false,
                    Err(error) => return Err(error),
                }
            } else {
                true
            };
            let channel = if permitted {
                state.channels.get(&source_id, &hit.channel_id).await?
            } else {
                None
            };
            entry.insert(channel);
        }
        let Some(channel) = channels.get(&hit.channel_id).and_then(Option::as_ref) else {
            continue;
        };
        let path = if conversation {
            format!("/conversations/{}", hit.channel_id)
        } else {
            format!("/c/{}/t/{}", hit.channel_id, hit.root_message_id)
        };
        found.push(SearchItem {
            workspace: workspace.clone(),
            kind: "message".into(),
            title: channel
                .title
                .clone()
                .unwrap_or_else(|| "Conversation".into()),
            snippet: hit.snippet,
            at: hit.created_at,
            url: query_url(&path, &[("message", hit.message_id.as_str())]),
        });
    }
    Ok(found)
}

/// Recheck the owning Workspace before any source read.
async fn search_workspace(
    state: &AppState,
    person: &Tenant,
    choice: &AccessibleWorkspaceDto,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchItem>, ApiError> {
    let id = WorkspaceId::from(choice.id.clone());
    let owner = state
        .workspaces
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    let role = crate::workspace::read_role(state, person, &owner)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    let tenant = Tenant {
        workspace_id: id.clone(),
        workspace_role: (owner.kind == WorkspaceKind::Shared).then_some(role),
        ..person.clone()
    };
    let workspace = SearchWorkspace {
        id: choice.id.clone(),
        name: owner.name,
        kind: owner.kind,
    };
    let mut found = search_messages(state, person, &workspace, query, limit, false).await?;
    let mut scopes = vec![(
        AgentId::from(String::new()),
        MemoryScope::Shared,
        "shared".to_string(),
    )];
    for agent in state.agent_store.list_by_workspace(&id).await? {
        scopes.push((
            agent.id.clone(),
            MemoryScope::Private,
            format!("agent:{}", agent.id),
        ));
    }
    for (agent, scope, label) in scopes {
        let access = crate::memory::current_access(state, &tenant, agent).await?;
        for hit in state
            .memory
            .search_pages(&id, &access, scope, query, limit as usize)
            .await
            .map_err(crate::memory::memory_error)?
        {
            let path = hit
                .path
                .strip_prefix("shared/")
                .or_else(|| hit.path.strip_prefix("private/"))
                .unwrap_or(&hit.path);
            found.push(SearchItem {
                workspace: workspace.clone(),
                kind: "memory".into(),
                title: hit.title,
                snippet: hit.snippet,
                at: hit.changed_at,
                url: query_url("/memory", &[("scope", &label), ("path", path)]),
            });
        }
    }
    for event in state.calendar.search(&id, query, limit).await? {
        let at = event.data.bounds()?.starts_at;
        found.push(SearchItem {
            workspace: workspace.clone(),
            kind: "calendar".into(),
            title: event.data.title,
            snippet: format!("{} {}", event.data.description, event.data.location)
                .chars()
                .take(300)
                .collect(),
            at,
            url: query_url(
                "/calendar",
                &[
                    ("event", event.id.as_str()),
                    ("date", &event.data.start[..10]),
                ],
            ),
        });
    }
    if state
        .mailbox_desk
        .workspace_mailbox(&id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?
        .is_some()
    {
        let mut store = state
            .mailbox_desk
            .open_workspace(&id)
            .await
            .map_err(crate::mailboxes::mailbox_error)?;
        let result = store
            .messages(
                "",
                query,
                &pagis_mail::PageRequest::new(None, limit as usize),
            )
            .await;
        let _ = store.close().await;
        for mail in result
            .map_err(|_| ApiError::conflict("Workspace Mailbox search failed"))?
            .items
        {
            found.push(SearchItem {
                workspace: workspace.clone(),
                kind: "mail".into(),
                title: mail.summary.subject,
                snippet: mail.summary.snippet,
                at: mail.summary.date,
                url: query_url("/mail", &[("message", &mail.summary.id.to_string())]),
            });
        }
    }
    if owner.kind == WorkspaceKind::Private {
        found.extend(search_messages(state, person, &workspace, query, limit, true).await?);
    }
    newest(&mut found, limit);
    Ok(found)
}

#[utoipa::path(get, path = "/api/v1/search", params(SearchQuery), responses(
    (status = 200, body = SearchResponse), (status = 401, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)
))]
pub async fn search(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    let text = query.q.trim();
    let limit = query.limit.unwrap_or(50);
    if !(2..=200).contains(&text.chars().count()) || !(1..=100).contains(&limit) {
        return Err(ApiError::validation(
            "Use 2 to 200 characters and a limit from 1 to 100.",
        ));
    }
    let choices = crate::workspace::accessible_workspaces(&state, &tenant).await?;
    let mut response = fan_out(choices, Duration::from_secs(3), |workspace| {
        let state = &state;
        let tenant = &tenant;
        async move { search_workspace(state, tenant, &workspace, text, limit).await }
    })
    .await;
    newest(&mut response.items, limit);
    Ok(Json(response))
}

async fn fan_out<F, Fut>(
    choices: Vec<AccessibleWorkspaceDto>,
    deadline: Duration,
    search: F,
) -> SearchResponse
where
    F: Fn(AccessibleWorkspaceDto) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SearchItem>, ApiError>>,
{
    let results = futures::future::join_all(choices.into_iter().map(|workspace| {
        let id = workspace.id.clone();
        let name = workspace.name.clone();
        let future = search(workspace);
        async move { (id, name, pagis_rt::timeout(deadline, future).await) }
    }))
    .await;
    let mut response = SearchResponse {
        items: Vec::new(),
        incomplete: Vec::new(),
    };
    for (id, name, result) in results {
        match result {
            Ok(Ok(mut items)) => {
                for item in &mut items {
                    let separator = if item.url.contains('?') { '&' } else { '?' };
                    item.url.push_str(&format!(
                        "{separator}workspace={}",
                        url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                    ));
                }
                response.items.extend(items);
            }
            error => response.incomplete.push(IncompleteWorkspace {
                workspace_id: id,
                workspace_name: name,
                reason: if error.is_err() {
                    "timeout"
                } else {
                    "unavailable"
                }
                .into(),
            }),
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_fans_out_and_keeps_results_when_a_workspace_fails_or_times_out() {
        let choices = ["slow", "failed", "healthy"]
            .into_iter()
            .map(|name| AccessibleWorkspaceDto {
                id: name.into(),
                name: name.into(),
                kind: WorkspaceKind::Shared,
                role: pagis_core::WorkspaceRole::Reader,
            })
            .collect();
        let result = fan_out(choices, Duration::from_millis(20), |workspace| async move {
            match workspace.id.as_str() {
                "slow" => futures::future::pending().await,
                "failed" => Err(ApiError::internal()),
                _ => Ok(vec![SearchItem {
                    workspace: SearchWorkspace {
                        id: workspace.id,
                        name: workspace.name,
                        kind: workspace.kind,
                    },
                    kind: "message".into(),
                    title: "Plan".into(),
                    snippet: "Plan".into(),
                    at: 20,
                    url: "/c/plan".into(),
                }]),
            }
        })
        .await;
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].workspace.name, "healthy");
        assert_eq!(result.incomplete.len(), 2);
        assert_eq!(result.incomplete[0].reason, "timeout");
        assert_eq!(result.incomplete[1].reason, "unavailable");
    }

    #[test]
    fn search_merges_newest_first_with_a_global_limit() {
        let mut items = [10, 30, 20]
            .into_iter()
            .map(|at| SearchItem {
                workspace: SearchWorkspace {
                    id: at.to_string(),
                    name: "Team".into(),
                    kind: WorkspaceKind::Shared,
                },
                kind: "calendar".into(),
                title: "Plan".into(),
                snippet: String::new(),
                at,
                url: "/calendar".into(),
            })
            .collect();
        newest(&mut items, 2);
        assert_eq!(
            items.iter().map(|item| item.at).collect::<Vec<_>>(),
            [30, 20]
        );
    }
}
