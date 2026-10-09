//! Grants REST: the settings page lists live grants with
//! their allow rules, edits the rules, and revokes a grant. Revocation
//! is the off switch — the next call re-enters the approval flow. A
//! host grant's rules are command prefixes; a credential grant's rules
//! are registrable domains, capped at five. A host grant also holds its
//! session allow rules, the widest Session Approval Mode of its Agent on
//! its machine and whether that Agent may use an Unattended Mode there
//! (ADR-0033). The Person sets the last two for an Agent and a machine.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use pagis_core::{
    AgentId, ConnectionId, Grant, GrantId, HostId, NewEvent, SessionAllowRule, SessionApprovalMode,
    now_ms,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

#[derive(Debug, Serialize, ToSchema)]
pub struct GrantDto {
    pub id: String,
    pub agent_id: String,
    /// Denormalized so the settings page renders standalone.
    pub agent_name: String,
    /// `host`, `credential`, or `connection`.
    pub resource_kind: String,
    /// What the grant reaches: the Connection, the Plugin, or the machine
    /// of a host grant. A credential grant names none.
    pub resource_id: Option<String>,
    /// The allow rules — command prefixes on a host grant, registrable
    /// domains on a credential grant. Empty is per-action mode.
    pub allow: Vec<String>,
    /// Named capabilities on a Connection grant.
    pub capabilities: Vec<String>,
    /// The session allow rules of a host grant: each lets the Agent start
    /// the sessions of one Coding Harness in one directory of the machine,
    /// and in each directory under it, with no card. The other kinds hold
    /// none.
    pub sessions: Vec<SessionAllowRule>,
    /// The widest Session Approval Mode of the Agent on the machine of a
    /// host grant. The other kinds hold none.
    pub session_approval_mode: Option<SessionApprovalMode>,
    /// Whether the Agent may use an Unattended Mode on the machine of a
    /// host grant. The other kinds hold none.
    pub unattended_modes: Option<bool>,
    pub revision: i64,
    pub created_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GrantPage {
    pub items: Vec<GrantDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetGrantRulesRequest {
    /// The full replacement rule list.
    pub allow: Vec<String>,
    /// The full replacement list of the session allow rules of a host
    /// grant. Without it, the session allow rules stay.
    pub sessions: Option<Vec<SessionAllowRule>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateConnectionGrantRequest {
    pub agent_id: String,
    pub connection_id: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetGrantCapabilitiesRequest {
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetSessionApprovalModeRequest {
    /// The widest mode that the Agent may use on the machine.
    pub mode: SessionApprovalMode,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetUnattendedModesRequest {
    /// Whether the Agent may use an Unattended Mode on the machine.
    pub allowed: bool,
}

/// The widest Session Approval Mode of a host grant, and none for the
/// other kinds.
fn host_session_approval_mode(grant: &Grant) -> Option<SessionApprovalMode> {
    (grant.resource_kind == Grant::HOST_KIND).then(|| grant.session_approval_mode())
}

/// The allowance of Unattended Modes of a host grant, and none for the
/// other kinds.
fn host_unattended_modes(grant: &Grant) -> Option<bool> {
    (grant.resource_kind == Grant::HOST_KIND).then(|| grant.unattended_modes())
}

pub(crate) fn grant_dto(grant: &Grant, agent_name: &str) -> GrantDto {
    GrantDto {
        id: grant.id.to_string(),
        agent_id: grant.agent_id.to_string(),
        agent_name: agent_name.to_string(),
        resource_kind: grant.resource_kind.clone(),
        resource_id: grant.resource_id.clone(),
        allow: grant.allow_rules(),
        capabilities: grant.capabilities(),
        sessions: grant.session_allow_rules(),
        session_approval_mode: host_session_approval_mode(grant),
        unattended_modes: host_unattended_modes(grant),
        revision: grant.revision,
        created_at: grant.created_at,
    }
}

async fn agent_names(
    state: &AppState,
    tenant: &Tenant,
) -> Result<HashMap<String, String>, ApiError> {
    Ok(state
        .agent_store
        .list_by_workspace(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(|agent| (agent.id.to_string(), agent.name))
        .collect())
}

async fn validate_connection_capabilities(
    state: &AppState,
    tenant: &Tenant,
    connection_id: &ConnectionId,
    capabilities: &[String],
    require_connected: bool,
) -> Result<(), ApiError> {
    if capabilities.is_empty() {
        return Err(ApiError::validation(
            "a Connection grant requires at least one capability",
        ));
    }
    let connection = state
        .connections
        .get(&tenant.workspace_id, connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("connection"))?;
    if require_connected && connection.status != pagis_core::Connection::CONNECTED {
        return Err(ApiError::validation(
            "a Connection must be connected before it can be granted",
        ));
    }
    let declared = state
        .broker
        .connection_capabilities(&tenant.workspace_id, &connection.provider);
    for capability in capabilities {
        if !declared.contains(capability) {
            return Err(ApiError::validation(format!(
                "capability {capability} is not declared by the installed {} manifest",
                connection.provider
            )));
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/grants",
    responses(
        (status = 200, body = GrantPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_grants(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<GrantPage>, ApiError> {
    let names = agent_names(&state, &tenant).await?;
    let items = state
        .grants
        .list_live(&tenant.workspace_id)
        .await?
        .iter()
        .map(|grant| {
            let name = names
                .get(grant.agent_id.as_str())
                .map(String::as_str)
                .unwrap_or("unknown agent");
            grant_dto(grant, name)
        })
        .collect();
    Ok(Json(GrantPage { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/grants",
    request_body = CreateConnectionGrantRequest,
    responses(
        (status = 201, body = GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn create_connection_grant(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<CreateConnectionGrantRequest>,
) -> Result<(StatusCode, Json<GrantDto>), ApiError> {
    let agent_id = AgentId::from(request.agent_id);
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    let connection_id = ConnectionId::from(request.connection_id);
    validate_connection_capabilities(&state, &tenant, &connection_id, &request.capabilities, true)
        .await?;

    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        agent_id,
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: Some(connection_id.to_string()),
        scope: Grant::connection_scope(&request.capabilities),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    };
    state.grants.create(&grant).await?;
    publish_grant_event(&state, &grant, "grant.created").await?;
    Ok((StatusCode::CREATED, Json(grant_dto(&grant, &agent.name))))
}

#[utoipa::path(
    put,
    path = "/api/v1/grants/{grant_id}/capabilities",
    params(("grant_id" = String, Path,)),
    request_body = SetGrantCapabilitiesRequest,
    responses(
        (status = 200, body = GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The grant is revoked", body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_grant_capabilities(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(grant_id): Path<String>,
    Json(request): Json<SetGrantCapabilitiesRequest>,
) -> Result<Json<GrantDto>, ApiError> {
    let grant_id = GrantId::from(grant_id);
    let grant = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    if grant.revoked_at.is_some() {
        return Err(ApiError::conflict("grant is revoked"));
    }
    if grant.resource_kind != Grant::CONNECTION_KIND {
        return Err(ApiError::validation(
            "only a Connection grant has capabilities",
        ));
    }
    let connection_id = grant
        .resource_id
        .as_deref()
        .ok_or_else(|| ApiError::validation("the Connection grant has no Connection"))?;
    validate_connection_capabilities(
        &state,
        &tenant,
        &ConnectionId::from(connection_id.to_string()),
        &request.capabilities,
        false,
    )
    .await?;

    let scope = Grant::connection_scope(&request.capabilities);
    if !state
        .grants
        .set_scope(&tenant.workspace_id, &grant_id, &scope)
        .await?
    {
        return Err(ApiError::conflict("grant is revoked"));
    }
    let changed = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    publish_grant_event(&state, &changed, "grant.changed").await?;
    let names = agent_names(&state, &tenant).await?;
    let name = names
        .get(changed.agent_id.as_str())
        .map(String::as_str)
        .unwrap_or("unknown agent");
    Ok(Json(grant_dto(&changed, name)))
}

#[utoipa::path(
    put,
    path = "/api/v1/grants/{grant_id}/rules",
    params(("grant_id" = String, Path,)),
    request_body = SetGrantRulesRequest,
    responses(
        (status = 200, body = GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The grant is revoked", body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_grant_rules(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(grant_id): Path<String>,
    Json(request): Json<SetGrantRulesRequest>,
) -> Result<Json<GrantDto>, ApiError> {
    let grant_id = GrantId::from(grant_id);
    let mut grant = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    if grant.revoked_at.is_some() {
        return Err(ApiError::conflict("grant is revoked"));
    }
    if grant.resource_kind == Grant::CONNECTION_KIND {
        return Err(ApiError::validation(
            "only host and credential grants have allow rules",
        ));
    }

    let mut allow = Vec::new();
    for rule in &request.allow {
        let rule = normalize_rule(&grant.resource_kind, rule);
        if rule.is_empty() {
            return Err(ApiError::validation("allow rules must not be empty"));
        }
        if !allow.contains(&rule) {
            allow.push(rule);
        }
    }
    check_rules(&grant.resource_kind, &allow)?;
    grant.scope = grant.with_allow_rules(&allow);
    if let Some(sessions) = &request.sessions {
        if grant.resource_kind != Grant::HOST_KIND {
            return Err(ApiError::validation(
                "only a host grant has session allow rules",
            ));
        }
        let mut rules: Vec<SessionAllowRule> = Vec::new();
        for rule in sessions {
            let rule = SessionAllowRule::new(&rule.harness, &rule.directory)
                .map_err(ApiError::validation)?;
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
        grant.scope = grant.with_session_allow_rules(&rules);
    }
    if !state
        .grants
        .set_scope(&tenant.workspace_id, &grant_id, &grant.scope)
        .await?
    {
        return Err(ApiError::conflict("grant is revoked"));
    }
    let grant = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    publish_grant_event(&state, &grant, "grant.changed").await?;

    let names = agent_names(&state, &tenant).await?;
    let name = names
        .get(grant.agent_id.as_str())
        .map(String::as_str)
        .unwrap_or("unknown agent");
    Ok(Json(grant_dto(&grant, name)))
}

/// Set the widest Session Approval Mode of an Agent on a machine. The
/// Person sets it before the Agent's first session there, when the
/// Agent can hold no host grant on the machine yet, so the first write
/// makes the grant. That grant makes the machine a host candidate of
/// the Agent (ADR-0015).
#[utoipa::path(
    put,
    path = "/api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode",
    params(("agent_id" = String, Path,), ("host_id" = String, Path,)),
    request_body = SetSessionApprovalModeRequest,
    responses(
        (status = 200, description = "The host grant holds the mode", body = GrantDto),
        (status = 201, description = "A new host grant holds the mode", body = GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The grant was revoked meanwhile", body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_session_approval_mode(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((agent_id, host_id)): Path<(String, String)>,
    Json(request): Json<SetSessionApprovalModeRequest>,
) -> Result<(StatusCode, Json<GrantDto>), ApiError> {
    set_host_grant_field(
        &state,
        &tenant,
        (agent_id, host_id),
        |grant| grant.session_approval_mode() == request.mode,
        |grant| grant.with_session_approval_mode(request.mode),
    )
    .await
}

/// Allow or stop the Unattended Modes of an Agent on a machine. As with
/// the widest mode, the first write makes the host grant.
#[utoipa::path(
    put,
    path = "/api/v1/agents/{agent_id}/hosts/{host_id}/unattended-modes",
    params(("agent_id" = String, Path,), ("host_id" = String, Path,)),
    request_body = SetUnattendedModesRequest,
    responses(
        (status = 200, description = "The host grant holds the allowance", body = GrantDto),
        (status = 201, description = "A new host grant holds the allowance", body = GrantDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The grant was revoked meanwhile", body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_unattended_modes(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((agent_id, host_id)): Path<(String, String)>,
    Json(request): Json<SetUnattendedModesRequest>,
) -> Result<(StatusCode, Json<GrantDto>), ApiError> {
    set_host_grant_field(
        &state,
        &tenant,
        (agent_id, host_id),
        |grant| grant.unattended_modes() == request.allowed,
        |grant| grant.with_unattended_modes(request.allowed),
    )
    .await
}

/// Write one field of the scope of the live host grant of an Agent on a
/// machine: `change` gives the new scope. With no live grant, a new
/// grant with no allow rules gets it (201). A grant that `holds` the
/// value already is not written. Each write is a revision and publishes
/// `grant.changed`.
async fn set_host_grant_field(
    state: &AppState,
    tenant: &Tenant,
    (agent_id, host_id): (String, String),
    holds: impl Fn(&Grant) -> bool,
    change: impl Fn(&Grant) -> serde_json::Value,
) -> Result<(StatusCode, Json<GrantDto>), ApiError> {
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &AgentId::from(agent_id))
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    let host = state
        .hosts
        .get(&tenant.workspace_id, &HostId::from(host_id))
        .await?
        .ok_or_else(|| ApiError::not_found("host"))?;
    let live = state
        .grants
        .live_for_resource(
            &tenant.workspace_id,
            &agent.id,
            Grant::HOST_KIND,
            host.id.as_str(),
        )
        .await?;
    let Some(grant) = live else {
        let mut grant = Grant {
            id: GrantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            agent_id: agent.id.clone(),
            resource_kind: Grant::HOST_KIND.to_string(),
            resource_id: Some(host.id.to_string()),
            scope: Grant::allow_scope(&[]),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        };
        grant.scope = change(&grant);
        state.grants.create(&grant).await?;
        publish_grant_event(state, &grant, "grant.changed").await?;
        return Ok((StatusCode::CREATED, Json(grant_dto(&grant, &agent.name))));
    };
    if holds(&grant) {
        return Ok((StatusCode::OK, Json(grant_dto(&grant, &agent.name))));
    }
    let scope = change(&grant);
    if !state
        .grants
        .set_scope(&tenant.workspace_id, &grant.id, &scope)
        .await?
    {
        return Err(ApiError::conflict("grant is revoked"));
    }
    let changed = state
        .grants
        .get(&tenant.workspace_id, &grant.id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    publish_grant_event(state, &changed, "grant.changed").await?;
    Ok((StatusCode::OK, Json(grant_dto(&changed, &agent.name))))
}

#[utoipa::path(
    delete,
    path = "/api/v1/grants/{grant_id}",
    params(("grant_id" = String, Path,)),
    responses(
        (status = 204, description = "Grant revoked"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "Already revoked", body = crate::error::ErrorBody),
    )
)]
pub async fn revoke_grant(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(grant_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let grant_id = GrantId::from(grant_id);
    let grant = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    if grant.revoked_at.is_some() {
        return Err(ApiError::conflict("grant is already revoked"));
    }
    if !state
        .grants
        .revoke(&tenant.workspace_id, &grant_id, now_ms())
        .await?
    {
        return Err(ApiError::conflict("grant is already revoked"));
    }
    let revoked = state
        .grants
        .get(&tenant.workspace_id, &grant_id)
        .await?
        .ok_or_else(|| ApiError::not_found("grant"))?;
    publish_grant_event(&state, &revoked, "grant.revoked").await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One rule as it is stored. A credential rule is a domain, and
/// domains are case-insensitive.
fn normalize_rule(resource_kind: &str, rule: &str) -> String {
    if resource_kind == Grant::CREDENTIAL_KIND {
        pagis_broker::normalize_domain(rule)
    } else {
        rule.trim().to_string()
    }
}

/// The per-kind rule limits. A host rule cannot name a program that
/// runs other programs, because it would allow every program
/// (ADR-0015). A credential grant carries at most five domains, and each
/// rule must read as a registrable domain: the match is exact, so a
/// path, a scheme or a port would never fire.
pub(crate) fn check_rules(resource_kind: &str, allow: &[String]) -> Result<(), ApiError> {
    if resource_kind == Grant::HOST_KIND {
        return match allow
            .iter()
            .find(|rule| pagis_broker::runs_other_programs(rule))
        {
            Some(rule) => Err(ApiError::validation(format!(
                "{rule:?} names a program that runs other programs; a host rule for it would \
                 allow every program"
            ))),
            None => Ok(()),
        };
    }
    if resource_kind != Grant::CREDENTIAL_KIND {
        return Ok(());
    }
    if allow.len() > pagis_broker::MAX_CREDENTIAL_RULES {
        return Err(ApiError::validation(format!(
            "a credential grant holds at most {} domains; remove one first",
            pagis_broker::MAX_CREDENTIAL_RULES
        )));
    }
    for rule in allow {
        if !is_domain(rule) {
            return Err(ApiError::validation(format!(
                "{rule:?} is not a domain; a credential rule is one registrable domain"
            )));
        }
    }
    Ok(())
}

/// A conservative domain shape: dot-separated labels of letters,
/// digits and inner hyphens, with at least one dot.
fn is_domain(rule: &str) -> bool {
    let labels: Vec<&str> = rule.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Publish one grant event; it drives WS invalidation of the settings
/// page.
pub(crate) async fn publish_grant_event(
    state: &AppState,
    grant: &Grant,
    event_type: &str,
) -> Result<(), ApiError> {
    state
        .bus
        .publish(NewEvent {
            workspace_id: grant.workspace_id.clone(),
            event_type: event_type.to_string(),
            agent_id: Some(grant.agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "grant_id": grant.id.as_str(),
                "resource_kind": grant.resource_kind,
                "resource_id": grant.resource_id,
                "allow": grant.allow_rules(),
                "capabilities": grant.capabilities(),
                "sessions": grant.session_allow_rules(),
                "session_approval_mode": host_session_approval_mode(grant),
                "unattended_modes": host_unattended_modes(grant),
                "revision": grant.revision,
            }),
        })
        .await?;
    Ok(())
}
