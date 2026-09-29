use crate::auth::Tenant;
use crate::{AppState, error::ApiError};
use axum::{
    Json,
    extract::{Path, State},
};
use pagis_core::knowledge::{SourceKey, SyncConfig, SyncStatus};
use pagis_core::reflection_filter::{Catalogue, FilterPreview, ReflectionFilter};
use pagis_core::{AgentId, AgentStatus, Connection, ConnectionId, Grant, StoreError, now_ms};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureSync {
    pub agent_id: String,
    pub enabled: bool,
    /// Earliest source date to import, as Unix milliseconds. Zero imports all history.
    pub since: i64,
    /// The rules that decide which Subject Pages reflect (ADR-0011).
    pub filter: ReflectionFilter,
}

/// The Signal Catalogue of one resource and the filter it starts from.
#[derive(Serialize, ToSchema)]
pub struct CatalogueView {
    pub catalogue: Catalogue,
    pub default_filter: ReflectionFilter,
}

#[derive(Serialize, ToSchema)]
pub struct SyncView {
    pub status: Option<SyncStatus>,
    pub blocked_reason: Option<String>,
}

fn key(tenant: &Tenant, id: String) -> SourceKey {
    SourceKey {
        workspace_id: tenant.workspace_id.clone(),
        connection_id: ConnectionId::from(id),
        resource: "gmail".into(),
    }
}

async fn view(state: &AppState, key: &SourceKey) -> Result<SyncView, ApiError> {
    let status = state.knowledge.status(key, state.clock.now_ms()).await?;
    let mut blocked_reason = None;
    if let Some(status) = status.as_ref() {
        let connection = state
            .connections
            .get(&key.workspace_id, &key.connection_id)
            .await?;
        let agent = state
            .agent_store
            .get(&key.workspace_id, &status.config.agent_id)
            .await?;
        let grant = state
            .grants
            .live_for_resource(
                &key.workspace_id,
                &status.config.agent_id,
                Grant::CONNECTION_KIND,
                key.connection_id.as_str(),
            )
            .await?;
        if !connection.is_some_and(|c| {
            c.status == Connection::CONNECTED
                && c.authorized_capabilities
                    .contains(&status.config.required_capability)
        }) {
            blocked_reason =
                Some("Reconnect the account with Gmail read access to continue.".into());
        } else if !agent.is_some_and(|a| a.status == AgentStatus::Active)
            || !grant.is_some_and(|g| {
                g.capabilities()
                    .contains(&status.config.required_capability)
            })
        {
            blocked_reason =
                Some("The responsible agent needs active Gmail read access to continue.".into());
        }
    }
    Ok(SyncView {
        status,
        blocked_reason,
    })
}

#[utoipa::path(get, path = "/api/v1/connections/{connection_id}/sync", params(("connection_id" = String, Path)), responses((status = 200, body = SyncView)))]
pub async fn get_sync(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<SyncView>, ApiError> {
    let key = key(&tenant, id);
    state
        .connections
        .get(&tenant.workspace_id, &key.connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection"))?;
    Ok(Json(view(&state, &key).await?))
}

#[utoipa::path(get, path = "/api/v1/connections/{connection_id}/sync/catalogue", params(("connection_id" = String, Path)), responses((status = 200, body = CatalogueView)))]
pub async fn get_catalogue(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<CatalogueView>, ApiError> {
    let key = key(&tenant, id);
    let connection = state
        .connections
        .get(&tenant.workspace_id, &key.connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection"))?;
    if connection.provider != "google" {
        return Err(ApiError::validation("Sync supports Gmail only"));
    }
    Ok(Json(CatalogueView {
        catalogue: state.catalogues.catalogue(&key).await?,
        default_filter: state.catalogues.default_filter(&key),
    }))
}

/// The counts one filter gives over the stored pages, for the filter
/// editor. One rule alone in the body gives the rule editor its line.
#[utoipa::path(post, path = "/api/v1/connections/{connection_id}/sync/filter/preview", params(("connection_id" = String, Path)), request_body = ReflectionFilter, responses((status = 200, body = FilterPreview)))]
pub async fn preview_filter(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(filter): Json<ReflectionFilter>,
) -> Result<Json<FilterPreview>, ApiError> {
    let key = key(&tenant, id);
    let connection = state
        .connections
        .get(&tenant.workspace_id, &key.connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection"))?;
    if connection.provider != "google" {
        return Err(ApiError::validation("Sync supports Gmail only"));
    }
    let catalogue = state.catalogues.catalogue(&key).await?;
    filter.validate(&catalogue).map_err(|error| {
        ApiError::validation(format!("The reflection filter is not valid. {error}"))
    })?;
    Ok(Json(
        state
            .catalogues
            .preview(&key, &filter, state.clock.now_ms())
            .await?,
    ))
}

#[utoipa::path(put, path = "/api/v1/connections/{connection_id}/sync", params(("connection_id" = String, Path)), request_body = ConfigureSync, responses((status = 200, body = SyncView)))]
pub async fn configure_sync(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<ConfigureSync>,
) -> Result<Json<SyncView>, ApiError> {
    let key = key(&tenant, id);
    let connection = state
        .connections
        .get(&tenant.workspace_id, &key.connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection"))?;
    if connection.provider != "google" {
        return Err(ApiError::validation("Sync supports Gmail only"));
    }
    let agent_id = AgentId::from(request.agent_id);
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent"))?;
    if request.enabled {
        if agent.status != AgentStatus::Active
            || connection.status != Connection::CONNECTED
            || !connection
                .authorized_capabilities
                .iter()
                .any(|c| c == "gmail_read")
        {
            return Err(ApiError::validation(
                "Sync needs an active Agent and connected Gmail read access",
            ));
        }
        let grants = state
            .grants
            .list_live_for_agent(&tenant.workspace_id, &agent_id)
            .await?;
        if !grants.iter().any(|g| {
            g.resource_kind == Grant::CONNECTION_KIND
                && g.resource_id.as_deref() == Some(key.connection_id.as_str())
                && g.capabilities().iter().any(|c| c == "gmail_read")
        }) {
            return Err(ApiError::validation(
                "Give the responsible Agent Gmail read access before enabling sync",
            ));
        }
    }
    let catalogue = state.catalogues.catalogue(&key).await?;
    request.filter.validate(&catalogue).map_err(|error| {
        ApiError::validation(format!("The reflection filter is not valid. {error}"))
    })?;
    state
        .knowledge
        .configure(
            SyncConfig {
                workspace_id: key.workspace_id.clone(),
                connection_id: key.connection_id.clone(),
                resource: key.resource.clone(),
                agent_id,
                required_capability: "gmail_read".into(),
                enabled: request.enabled,
                since: request.since,
                filter: request.filter,
            },
            now_ms(),
        )
        .await
        .map_err(map_error)?;
    Ok(Json(view(&state, &key).await?))
}

/// One connection whose sync names the Agent as responsible.
#[derive(Serialize, ToSchema)]
pub struct AgentSyncConnection {
    pub connection_id: String,
    pub display_name: String,
    pub provider: String,
    pub resource: String,
    pub enabled: bool,
    pub caught_up: bool,
    /// The number of rules in the Reflection Filter.
    pub rule_count: usize,
}

#[derive(Serialize, ToSchema)]
pub struct AgentSyncConnectionList {
    pub items: Vec<AgentSyncConnection>,
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{agent_id}/sync-connections",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, body = AgentSyncConnectionList),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn agent_sync_connections(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<AgentSyncConnectionList>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent"))?;
    let now = state.clock.now_ms();
    let mut items = Vec::new();
    // Sync supports Gmail alone, so only a Google connection can hold
    // one. A workspace holds a few connections, so one status read for
    // each costs little.
    for connection in state.connections.list(&tenant.workspace_id).await? {
        if connection.provider != "google" {
            continue;
        }
        let key = key(&tenant, connection.id.to_string());
        let Some(status) = state.knowledge.status(&key, now).await? else {
            continue;
        };
        if status.config.agent_id != agent_id {
            continue;
        }
        items.push(AgentSyncConnection {
            connection_id: connection.id.to_string(),
            display_name: connection.display_name,
            provider: connection.provider,
            resource: key.resource,
            enabled: status.config.enabled,
            caught_up: status.caught_up,
            rule_count: status.config.filter.rules.len(),
        });
    }
    Ok(Json(AgentSyncConnectionList { items }))
}

fn map_error(error: StoreError) -> ApiError {
    match error {
        StoreError::Conflict(message) => ApiError::conflict(message),
        other => other.into(),
    }
}
