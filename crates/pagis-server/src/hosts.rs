//! The machines a person's sprites act on.
//!
//! A Host registers itself over its own WebSocket, so there is nothing to
//! create here. What a surface needs is the list: which machines the
//! person has, what each can do, and which of them are here now.
//!
//! Presence is memory of the running daemon, so it is read from the
//! registry and never from a column. The record answers "last seen",
//! which is the useful thing to say about a machine that is not present.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
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
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HostsDto {
    pub items: Vec<HostDto>,
}

fn host_dto(state: &AppState, host: pagis_core::Host) -> HostDto {
    HostDto {
        present: state.host_presence.present(&host.id),
        id: host.id.to_string(),
        name: host.name,
        platform: host.platform,
        capabilities: host.capabilities,
        last_seen_at: host.last_seen_at,
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
pub async fn list_hosts(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<HostsDto>, ApiError> {
    let items = state
        .hosts
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(|host| host_dto(&state, host))
        .collect();
    Ok(Json(HostsDto { items }))
}
