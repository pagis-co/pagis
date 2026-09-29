//! The Plugins REST (ADR-0022, ADR-0017).
//!
//! Installing a Plugin is the user's act alone, so every path here is
//! a desk path and no Agent tool reaches it. The install and update
//! answers carry the whole install card: every `env` and `headers`
//! value, because those are package data the user must see, and the
//! effect class of every tool the plugin declared.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use object_store::ObjectStoreExt as _;
use object_store::path::Path as BlobPath;
use pagis_core::{
    AgentId, ArtifactId, ConnectionId, Plugin, PluginBindingValue, PluginId, PluginSource,
    PluginTools,
};
use pagis_plugin::{
    BindingInput, BindingValueInput, ChangeStatus, Installed, McpServer, PluginError, SourceInput,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::{Administrator, Tenant};
use crate::error::ApiError;

/// Where the files of a Plugin come from. An upload arrives as an
/// Artifact: the desk uploads the tar of the directory first and names
/// it here.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginSourceRequest {
    Git {
        url: String,
        /// The branch, tag or commit. Absent takes the default branch.
        #[serde(rename = "ref")]
        reference: Option<String>,
    },
    Upload {
        /// The Artifact that holds the plugin directory as one tar.
        artifact_id: String,
    },
}

/// What one declared config field is bound to.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingValueRequest {
    Connection {
        connection_id: String,
    },
    /// The secret itself. It goes to the daemon's secret store and is
    /// never read back.
    Secret {
        secret: String,
    },
    Value {
        value: serde_json::Value,
    },
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BindingRequest {
    pub field: String,
    #[serde(flatten)]
    pub value: BindingValueRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InstallPluginRequest {
    pub source: PluginSourceRequest,
    #[serde(default)]
    pub bindings: Vec<BindingRequest>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdatePluginRequest {
    /// The new upload, for a Plugin that was uploaded. A Plugin
    /// installed from a repository reads that repository again.
    #[serde(default)]
    pub artifact_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GrantPluginRequest {
    pub agent_id: String,
}

/// One row of the Plugins list.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginRowDto {
    pub id: String,
    pub name: String,
    /// `enabled`, `disabled` or `failed`.
    pub state: String,
    /// `git` or `upload`.
    pub source_kind: String,
    pub source_url: Option<String>,
    pub source_ref: Option<String>,
    pub installed_commit: String,
    pub manifest_version: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PluginPage {
    pub items: Vec<PluginRowDto>,
}

/// One `env` entry or one header, as the package writes it. The value
/// is shown whole: it is package data, and a reference to a bound
/// field is what the user must be able to read (ADR-0017).
#[derive(Debug, Serialize, ToSchema)]
pub struct NamedValueDto {
    pub name: String,
    pub value: String,
}

/// One MCP server the Plugin declares.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginServerDto {
    pub name: String,
    /// `stdio`, `streamable-http` or `sse`.
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub env: Vec<NamedValueDto>,
    pub headers: Vec<NamedValueDto>,
    /// Whether the server holds a process now. A server starts
    /// at the first call and stops when it is idle, so a stopped
    /// server is the usual state and not a fault.
    pub running: bool,
}

/// One config field the Plugin declares.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginFieldDto {
    pub name: String,
    /// `secret`, `connection`, `string`, `number` or `boolean`.
    pub kind: String,
    pub title: String,
    pub description: String,
    pub required: bool,
    pub provider: Option<String>,
    pub capabilities: Vec<String>,
}

/// What one field is bound to. A secret binding carries no value.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginBindingDto {
    pub field: String,
    /// `connection`, `secret` or `value`.
    pub kind: String,
    pub connection_id: Option<String>,
    pub capabilities: Vec<String>,
    pub value: Option<serde_json::Value>,
}

/// The effect class one tool takes at dispatch: what the Plugin
/// declared, or `host` for every tool it did not (ADR-0017).
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginToolEffectDto {
    pub tool: String,
    pub effect: String,
}

/// One tool of the frozen manifest (ADR-0017): what a server
/// offered at install, and what the model calls it.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginFrozenToolDto {
    /// The `mcp.json` name of the server that offers it.
    pub server: String,
    /// The name the model calls: `<plugin>__<tool>`.
    pub name: String,
    pub description: String,
    pub effect: String,
}

/// The end of one Plugin's server log in the asking Person's Workspace.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginLogDto {
    pub text: String,
}

/// One file an update changed.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChangedFileDto {
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: String,
}

/// One Skill the Plugin ships (ADR-0017). The plugin page shows
/// the list read-only: a Skill is the Plugin's, and only an update
/// changes it.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginSkillDto {
    pub name: String,
    pub description: String,
}

/// One Plugin in full: the record, what it declares, and what it is
/// bound to. It is the install card.
#[derive(Debug, Serialize, ToSchema)]
pub struct PluginDto {
    #[serde(flatten)]
    pub record: PluginRowDto,
    pub servers: Vec<PluginServerDto>,
    pub fields: Vec<PluginFieldDto>,
    pub bindings: Vec<PluginBindingDto>,
    pub tools: Vec<PluginToolEffectDto>,
    /// The frozen manifest of the installed state. It is empty until
    /// the Plugin is enabled and its servers have been read once.
    pub frozen_tools: Vec<PluginFrozenToolDto>,
    /// Whether a server has offered a different tool list than the
    /// frozen one. An update accepts the difference (ADR-0017).
    pub tools_changed: bool,
    pub skills: Vec<PluginSkillDto>,
    /// What an update changed. It is empty for every other answer.
    pub changed: Vec<ChangedFileDto>,
}

#[utoipa::path(get, path = "/api/v1/plugins", responses(
    (status = 200, body = PluginPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
) -> Result<Json<PluginPage>, ApiError> {
    // The Org owns which plugins are available (ADR-0017), so
    // everybody in it reads the same list.
    let items = state
        .plugins
        .list(&state.org_workspace_id)
        .await
        .map_err(plugin_error)?
        .iter()
        .map(row_dto)
        .collect();
    Ok(Json(PluginPage { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins",
    request_body = InstallPluginRequest,
    responses(
        (status = 201, body = PluginDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn install_plugin(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Json(request): Json<InstallPluginRequest>,
) -> Result<(StatusCode, Json<PluginDto>), ApiError> {
    let source = read_source(&state, &tenant, request.source).await?;
    let bindings = read_bindings(request.bindings);
    // The install is the Org's (ADR-0017): the rows, the Bindings
    // and the checkout live in the Org's Workspace, and every tenant
    // runs the Plugin in its own Plugin Computer.
    let installed = state
        .plugins
        .install(&state.org_workspace_id, &source, &bindings)
        .await
        .map_err(plugin_error)?;
    Ok((
        StatusCode::CREATED,
        Json(detail_dto(
            &installed,
            catalog(&state, &tenant, &installed).await,
            &running(&state, &tenant, &installed).await,
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/plugins/{plugin_id}",
    params(("plugin_id" = String, Path,)),
    responses(
        (status = 200, body = PluginDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_plugin(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(plugin_id): Path<String>,
) -> Result<Json<PluginDto>, ApiError> {
    let installed = state
        .plugins
        .installed(&state.org_workspace_id, &PluginId::from(plugin_id))
        .await
        .map_err(plugin_error)?;
    Ok(Json(detail_dto(
        &installed,
        catalog(&state, &tenant, &installed).await,
        &running(&state, &tenant, &installed).await,
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/update",
    params(("plugin_id" = String, Path,)),
    request_body = UpdatePluginRequest,
    responses(
        (status = 200, description = "The new state and what it changed", body = PluginDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn update_plugin(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Path(plugin_id): Path<String>,
    Json(request): Json<UpdatePluginRequest>,
) -> Result<Json<PluginDto>, ApiError> {
    let upload = match request.artifact_id {
        Some(artifact_id) => Some(read_artifact(&state, &tenant, &artifact_id).await?),
        None => None,
    };
    let installed = state
        .plugins
        .update(&state.org_workspace_id, &PluginId::from(plugin_id), upload)
        .await
        .map_err(plugin_error)?;
    Ok(Json(detail_dto(
        &installed,
        catalog(&state, &tenant, &installed).await,
        &running(&state, &tenant, &installed).await,
    )))
}

#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{plugin_id}",
    params(("plugin_id" = String, Path,)),
    responses(
        (status = 204, description = "The plugin is uninstalled"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn uninstall_plugin(
    State(state): State<Arc<AppState>>,
    Administrator(_tenant): Administrator,
    Path(plugin_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .plugins
        .uninstall(&state.org_workspace_id, &PluginId::from(plugin_id))
        .await
        .map_err(plugin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/plugins/{plugin_id}/bindings/{field}",
    params(("plugin_id" = String, Path,), ("field" = String, Path,)),
    request_body = BindingValueRequest,
    responses(
        (status = 200, body = PluginDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn bind_plugin_field(
    State(state): State<Arc<AppState>>,
    // A Binding belongs to the Org's install, so only an administrator
    // writes one (ADR-0017). A person's own account reaches a bound
    // Connection through its alias, so a member binds nothing.
    Administrator(tenant): Administrator,
    Path((plugin_id, field)): Path<(String, String)>,
    Json(request): Json<BindingValueRequest>,
) -> Result<Json<PluginDto>, ApiError> {
    let input = BindingInput {
        field,
        value: binding_value(request),
    };
    let installed = state
        .plugins
        .bind(&state.org_workspace_id, &PluginId::from(plugin_id), &input)
        .await
        .map_err(plugin_error)?;
    Ok(Json(detail_dto(
        &installed,
        catalog(&state, &tenant, &installed).await,
        &running(&state, &tenant, &installed).await,
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/grants",
    params(("plugin_id" = String, Path,)),
    request_body = GrantPluginRequest,
    responses(
        (status = 201, body = crate::grants::GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn grant_plugin(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(plugin_id): Path<String>,
    Json(request): Json<GrantPluginRequest>,
) -> Result<(StatusCode, Json<crate::grants::GrantDto>), ApiError> {
    let agent_id = AgentId::from(request.agent_id);
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    let grant = state
        .plugins
        .grant(
            &state.org_workspace_id,
            &tenant.workspace_id,
            &PluginId::from(plugin_id),
            &agent_id,
        )
        .await
        .map_err(plugin_error)?;
    crate::grants::publish_grant_event(&state, &grant, "grant.created").await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::grants::grant_dto(&grant, &agent.name)),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/plugins/{plugin_id}/start",
    params(("plugin_id" = String, Path,)),
    responses(
        (status = 200, description = "Every server started", body = PluginDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, description = "A server did not start", body = crate::error::ErrorBody),
    )
)]
pub async fn start_plugin(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Path(plugin_id): Path<String>,
) -> Result<Json<PluginDto>, ApiError> {
    let plugin_id = PluginId::from(plugin_id);
    // The servers start in the asking person's own Plugin Computer:
    // an administrator who starts a Plugin again starts theirs.
    state
        .plugin_hosts
        .get(&tenant.workspace_id)
        .start(&plugin_id)
        .await
        .map_err(host_error)?;
    let installed = state
        .plugins
        .installed(&state.org_workspace_id, &plugin_id)
        .await
        .map_err(plugin_error)?;
    Ok(Json(detail_dto(
        &installed,
        catalog(&state, &tenant, &installed).await,
        &running(&state, &tenant, &installed).await,
    )))
}

/// The end of the server log of one Plugin in the asking Person's own
/// Workspace.
///
/// Each Workspace runs the Org's Plugin in its own Plugin Computer and
/// keeps its own log of the Plugin's stderr. A server can write Person
/// data or a bound value to stderr, so a Person never reads the log of
/// another Workspace (ADR-0023). A Plugin whose servers have not written
/// to stderr in this Workspace answers an empty text.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{plugin_id}/log",
    params(("plugin_id" = String, Path,)),
    responses(
        (status = 200, description = "The end of the server log in the asking Person's Workspace", body = PluginLogDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn plugin_log(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(plugin_id): Path<String>,
) -> Result<Json<PluginLogDto>, ApiError> {
    let plugin_id = PluginId::from(plugin_id);
    // The Plugin must be installed: the log of a Plugin that is not
    // there is not readable through its id.
    state
        .plugins
        .installed(&state.org_workspace_id, &plugin_id)
        .await
        .map_err(plugin_error)?;
    // The host of the asking Person's Workspace reads that Workspace's
    // log and no other.
    let text = state
        .plugin_hosts
        .get(&tenant.workspace_id)
        .read_log(&plugin_id, MAX_LOG_BYTES)
        .await;
    Ok(Json(PluginLogDto { text }))
}

/// How much of one Plugin's server log the desk reads at once.
const MAX_LOG_BYTES: usize = 64 * 1024;

/// The servers of the Plugin that hold a process in the asking person's
/// own Plugin Computer now.
async fn running(state: &AppState, tenant: &Tenant, installed: &Installed) -> BTreeSet<String> {
    state
        .plugin_hosts
        .get(&tenant.workspace_id)
        .running_servers(&installed.plugin.id)
        .await
}

/// The frozen manifest of the installed state, or `None` when the
/// Plugin has none yet. The catalogue is the Org's, so every tenant
/// reads the same one.
async fn catalog(state: &AppState, tenant: &Tenant, installed: &Installed) -> Option<PluginTools> {
    state
        .plugin_hosts
        .get(&tenant.workspace_id)
        .catalog(&installed.plugin)
        .await
        .unwrap_or_default()
}

/// The upload the request names, read out of its Artifact.
async fn read_source(
    state: &AppState,
    tenant: &Tenant,
    source: PluginSourceRequest,
) -> Result<SourceInput, ApiError> {
    Ok(match source {
        PluginSourceRequest::Git { url, reference } => SourceInput::Git { url, reference },
        PluginSourceRequest::Upload { artifact_id } => SourceInput::Upload {
            tar: read_artifact(state, tenant, &artifact_id).await?,
        },
    })
}

async fn read_artifact(
    state: &AppState,
    tenant: &Tenant,
    artifact_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let artifact = state
        .artifacts
        .get(
            &tenant.workspace_id,
            &ArtifactId::from(artifact_id.to_string()),
        )
        .await?
        .ok_or_else(|| ApiError::not_found("artifact"))?;
    let bytes = state
        .blobs
        .get(&BlobPath::from(artifact.storage_key.clone()))
        .await
        .map_err(|_| ApiError::internal())?
        .bytes()
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(bytes.to_vec())
}

fn read_bindings(requests: Vec<BindingRequest>) -> Vec<BindingInput> {
    requests
        .into_iter()
        .map(|request| BindingInput {
            field: request.field,
            value: binding_value(request.value),
        })
        .collect()
}

fn binding_value(request: BindingValueRequest) -> BindingValueInput {
    match request {
        BindingValueRequest::Connection { connection_id } => BindingValueInput::Connection {
            connection_id: ConnectionId::from(connection_id),
        },
        BindingValueRequest::Secret { secret } => BindingValueInput::Secret { secret },
        BindingValueRequest::Value { value } => BindingValueInput::Value { value },
    }
}

fn row_dto(plugin: &Plugin) -> PluginRowDto {
    let (source_kind, source_url, source_ref) = match &plugin.source {
        PluginSource::Git { url, reference } => ("git", Some(url.clone()), reference.clone()),
        PluginSource::Upload => ("upload", None, None),
    };
    PluginRowDto {
        id: plugin.id.to_string(),
        name: plugin.name.clone(),
        state: plugin.state.as_str().to_string(),
        source_kind: source_kind.to_string(),
        source_url,
        source_ref,
        installed_commit: plugin.installed_commit.clone(),
        manifest_version: plugin.manifest_version.clone(),
        created_at: plugin.created_at,
        updated_at: plugin.updated_at,
    }
}

fn detail_dto(
    installed: &Installed,
    frozen: Option<PluginTools>,
    running: &BTreeSet<String>,
) -> PluginDto {
    let package = &installed.package;
    let servers = package
        .mcp
        .iter()
        .flat_map(|mcp| mcp.servers.iter())
        .map(|(name, server)| server_dto(name, server, running.contains(name)))
        .collect();
    let fields = package
        .pagis
        .config
        .iter()
        .map(|(name, field)| PluginFieldDto {
            name: name.clone(),
            kind: field.kind.as_str().to_string(),
            title: field.title.clone(),
            description: field.description.clone(),
            required: field.required,
            provider: field.provider.clone(),
            capabilities: field.capabilities.clone(),
        })
        .collect();
    let bindings = installed
        .bindings
        .iter()
        .map(|binding| match &binding.value {
            PluginBindingValue::Connection {
                connection_id,
                capabilities,
            } => PluginBindingDto {
                field: binding.field.clone(),
                kind: "connection".to_string(),
                connection_id: Some(connection_id.to_string()),
                capabilities: capabilities.clone(),
                value: None,
            },
            // The name of the secret is daemon state, not something
            // the desk shows, and the value never leaves the store.
            PluginBindingValue::Secret { .. } => PluginBindingDto {
                field: binding.field.clone(),
                kind: "secret".to_string(),
                connection_id: None,
                capabilities: Vec::new(),
                value: None,
            },
            PluginBindingValue::Value { value } => PluginBindingDto {
                field: binding.field.clone(),
                kind: "value".to_string(),
                connection_id: None,
                capabilities: Vec::new(),
                value: Some(value.clone()),
            },
        })
        .collect();
    let tools = package
        .pagis
        .tools
        .keys()
        .map(|tool| PluginToolEffectDto {
            tool: tool.clone(),
            effect: effect_name(package.effect(tool)),
        })
        .collect();
    let changed = installed
        .changed
        .files
        .iter()
        .map(|file| ChangedFileDto {
            path: file.path.clone(),
            status: match file.status {
                ChangeStatus::Added => "added",
                ChangeStatus::Modified => "modified",
                ChangeStatus::Deleted => "deleted",
            }
            .to_string(),
        })
        .collect();
    let frozen_tools = frozen
        .iter()
        .flat_map(|frozen| frozen.tools.iter())
        .map(|tool| PluginFrozenToolDto {
            server: tool.server.clone(),
            name: format!("{}__{}", installed.plugin.name, tool.name),
            description: tool.description.clone(),
            effect: effect_name(package.effect(&tool.name)),
        })
        .collect();
    PluginDto {
        record: row_dto(&installed.plugin),
        servers,
        fields,
        bindings,
        tools,
        frozen_tools,
        tools_changed: frozen.is_some_and(|frozen| frozen.tools_changed),
        skills: package
            .skills
            .iter()
            .map(|skill| PluginSkillDto {
                name: skill.name.clone(),
                description: skill.description.clone(),
            })
            .collect(),
        changed,
    }
}

fn server_dto(name: &str, server: &McpServer, running: bool) -> PluginServerDto {
    let (transport, command, args, cwd, url) = match server {
        McpServer::Stdio {
            command, args, cwd, ..
        } => (
            "stdio",
            Some(command.clone()),
            args.clone(),
            cwd.clone(),
            None,
        ),
        McpServer::StreamableHttp { url, .. } => {
            ("streamable-http", None, Vec::new(), None, Some(url.clone()))
        }
        McpServer::Sse { url, .. } => ("sse", None, Vec::new(), None, Some(url.clone())),
    };
    PluginServerDto {
        name: name.to_string(),
        transport: transport.to_string(),
        command,
        args,
        cwd,
        url,
        env: named(server.env()),
        headers: named(server.headers()),
        running,
    }
}

fn named(values: &std::collections::BTreeMap<String, String>) -> Vec<NamedValueDto> {
    values
        .iter()
        .map(|(name, value)| NamedValueDto {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

fn effect_name(effect: pagis_broker::EffectClass) -> String {
    serde_json::to_value(effect)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "host".to_string())
}

fn host_error(error: pagis_plugins::HostError) -> ApiError {
    match error {
        pagis_plugins::HostError::NotFound => ApiError::not_found("plugin"),
        pagis_plugins::HostError::Refused(message) => ApiError::validation(message),
        pagis_plugins::HostError::Storage(message) => {
            tracing::error!(%message, "the plugin host failed");
            ApiError::internal()
        }
    }
}

fn plugin_error(error: PluginError) -> ApiError {
    match error {
        PluginError::NotFound => ApiError::not_found("plugin"),
        PluginError::Conflict(message) => ApiError::conflict(message),
        PluginError::Refused(message) => ApiError::validation(message),
        PluginError::Invalid(problems) => ApiError::validation(problems.to_string()),
        PluginError::Source(problem) => ApiError::validation(problem.to_string()),
        PluginError::Storage(message) => {
            tracing::error!(%message, "the plugin store failed");
            ApiError::internal()
        }
    }
}
