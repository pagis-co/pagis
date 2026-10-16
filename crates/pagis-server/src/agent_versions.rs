//! Agent configuration snapshots and their Person authors.
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Path, State},
};
use pagis_core::{AgentId, AgentVersion, AuditEntry, AuditEntryId, Author, WorkspaceRole};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

/// Record the current configuration after one successful configuration write.
pub(crate) async fn record(
    state: &AppState,
    tenant: &Tenant,
    agent_id: &AgentId,
) -> Result<(), ApiError> {
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    if workspace.kind != pagis_core::WorkspaceKind::Shared {
        return Ok(());
    }
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent"))?;
    let grants = state
        .grants
        .list_live_for_agent(&tenant.workspace_id, agent_id)
        .await?;
    let mailbox = state
        .mailbox_desk
        .for_agent(&tenant.workspace_id, agent_id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?
        .map(|mailbox| {
            serde_json::json!({
                "id": mailbox.id,
                "connection_id": mailbox.connection_id,
                "address": mailbox.address,
                "outgoing_cap": mailbox.outgoing_cap,
                "allow_rules": mailbox.allow_rules,
            })
        });
    state
        .governance
        .record_version(
            agent_id,
            &serde_json::json!({"agent": agent, "grants": grants, "mailbox": mailbox}),
            &AuditEntry {
                id: AuditEntryId::generate(),
                org_workspace_id: state.org_workspace_id.clone(),
                actor: Author::Person(tenant.user_id.clone()),
                action: "agent.changed".into(),
                subject_workspace_id: tenant.workspace_id.clone(),
                reason: None,
                at: state.clock.now_ms(),
            },
        )
        .await?;
    Ok(())
}
#[derive(Serialize, ToSchema)]
pub struct VersionPage {
    pub items: Vec<AgentVersion>,
}
#[utoipa::path(get, operation_id = "list_agent_versions", path = "/api/v1/agents/{agent_id}/versions", params(("agent_id" = String, Path)), responses((status = 200, body = VersionPage), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)))]
pub async fn list(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent): Path<String>,
) -> Result<Json<VersionPage>, ApiError> {
    // An Agent of another Workspace reads as absent before the role check.
    let agent = AgentId::from(agent);
    state
        .agent_store
        .get(&tenant.workspace_id, &agent)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent"))?;
    if tenant.workspace_role != Some(WorkspaceRole::Manager) {
        return Err(ApiError::forbidden("Only a Manager reads Agent Versions."));
    }
    Ok(Json(VersionPage {
        items: state
            .governance
            .versions(&tenant.workspace_id, &agent)
            .await?,
    }))
}
