//! The machines a person's sprites act on.
//!
//! A Host registers itself over its own WebSocket, so there is nothing to
//! create here. What a surface needs is the list: which machines the
//! person has, what each can do, and which of them are here now.
//!
//! Presence is memory of the running daemon, so it is read from the
//! registry and never from a column. The record answers "last seen",
//! which is the useful thing to say about a machine that is not present.
//! The sign-in report of each harness is daemon memory too.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use pagis_core::harness;
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One machine of one Person, with whether it is connected now.
#[derive(Debug, Serialize, ToSchema)]
pub struct HostDto {
    pub id: String,
    pub name: String,
    /// The operating system family the client reported.
    pub platform: String,
    /// What the client declared it can do, for example `shell`.
    pub capabilities: Vec<String>,
    /// True while the daemon holds this machine's connection. A host
    /// action runs only on a present machine.
    pub present: bool,
    pub last_seen_at: i64,
    /// One entry for each `harness:<id>` capability, in the order of the
    /// capabilities.
    pub harnesses: Vec<HostHarnessDto>,
}

/// One Coding Harness that a Host declares.
#[derive(Debug, Serialize, ToSchema)]
pub struct HostHarnessDto {
    /// The id of the harness in the Harness Catalog.
    pub id: String,
    /// True when the last attempt showed that the harness needs a Harness
    /// Sign-In on this machine. After a restart of the daemon it is false
    /// until a start fails again.
    pub needs_sign_in: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HostsDto {
    pub items: Vec<HostDto>,
}

async fn host_dto(state: &AppState, host: pagis_core::Host) -> HostDto {
    let mut harnesses = Vec::new();
    for capability in &host.capabilities {
        if let Some(id) = capability.strip_prefix(harness::CAPABILITY_PREFIX) {
            let needs_sign_in = state
                .sign_in_reports
                .needs_sign_in_since(&host.id, id)
                .await
                .is_some();
            harnesses.push(HostHarnessDto {
                id: id.to_string(),
                needs_sign_in,
            });
        }
    }
    HostDto {
        present: state.host_presence.present(&host.id),
        id: host.id.to_string(),
        name: host.name,
        platform: host.platform,
        capabilities: host.capabilities,
        last_seen_at: host.last_seen_at,
        harnesses,
    }
}

/// The signed-in person's own machines, with presence.
#[utoipa::path(
    get,
    path = "/api/v1/hosts",
    responses(
        (status = 200, body = HostsDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_my_hosts(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<HostsDto>, ApiError> {
    let mut items = Vec::new();
    for host in state.hosts.list(&tenant.workspace_id).await? {
        items.push(host_dto(&state, host).await);
    }
    Ok(Json(HostsDto { items }))
}
