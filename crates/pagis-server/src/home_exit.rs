//! The Home Exit of the signed-in Person (ADR-0029): the one Host of
//! theirs through which their Computers on a Server reach the internet,
//! so that sites see their own connection and not a data-center address.
//!
//! The Person reads and sets it in Settings. Only a Host of their own
//! that declared `exit` can be chosen: the Host read names the Person's
//! Workspace, so the Host of another Person is absent, and the store
//! writes a Host of the same Workspace alone. A change switches the Exit
//! Proxy of each awake Computer of the Person at once, with no restart.
//! A Computer that does not switch keeps its mode, and the answer names
//! it.
//!
//! A Local Installation has no Home Exit: its Computers run on the
//! owner's machine and leave from the owner's connection. The read says
//! so, and a change answers `409`. While the Administrator turned the
//! Home Exit off for the installation, the choice stays and is not in
//! effect, the read says so, and a new choice answers `409`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use pagis_core::{EXIT_CAPABILITY, Host, HostId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::system::is_local;

/// Why a Local Installation answers no change of the Home Exit.
const NO_HOME_EXIT_HERE: &str = "a Local Installation has no Home Exit: its Computers run on \
     this machine and reach the internet from its own connection";

/// One Host of the Person that can be their Home Exit.
#[derive(Debug, Serialize, ToSchema)]
pub struct HomeExitHostDto {
    pub id: String,
    /// The name the machine calls itself, which the exit in use shows.
    pub name: String,
    /// The operating system family the client reported.
    pub platform: String,
    /// True while the exit socket of the Host is open, so it carries the
    /// connections of the Person's Computers when it is their Home Exit.
    pub present: bool,
}

/// The Person's Home Exit, as Settings shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct HomeExitDto {
    /// False on a Local Installation, which has no Home Exit. Settings
    /// then shows no Home Exit.
    pub available: bool,
    /// True while the Administrator turned the Home Exit off for the
    /// installation. The Person's choice stays, and it is in effect
    /// again when the Administrator turns the Home Exit on.
    pub administrator_turned_off: bool,
    /// The Host that the Person chose, or null for none.
    pub chosen: Option<HomeExitHostDto>,
    /// The Person's Hosts that declared `exit`: the Hosts that the Person
    /// can choose. A Client App that is connected to the Server declares
    /// it.
    pub hosts: Vec<HomeExitHostDto>,
}

/// An awake Computer whose Exit Proxy did not switch. It keeps its mode,
/// and it takes the choice at its next wake.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExitSwitchFailureDto {
    pub agent_id: String,
    pub agent_name: String,
    /// Why, as the Exit Proxy answered.
    pub error: String,
}

/// The Home Exit after a change, and the Computers that did not switch.
#[derive(Debug, Serialize, ToSchema)]
pub struct SwitchedHomeExitDto {
    pub home_exit: HomeExitDto,
    pub not_switched: Vec<ExitSwitchFailureDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetHomeExitRequest {
    /// One of the Person's own Hosts that declared `exit`.
    pub host_id: String,
}

fn host_dto(state: &AppState, host: Host) -> HomeExitHostDto {
    HomeExitHostDto {
        present: state.home_exits.is_open(&host.id),
        id: host.id.to_string(),
        name: host.name,
        platform: host.platform,
    }
}

/// The Home Exit of the Person of `tenant`, as the store holds it now.
async fn read(state: &AppState, tenant: &Tenant) -> Result<HomeExitDto, ApiError> {
    if is_local(state) {
        return Ok(HomeExitDto {
            available: false,
            administrator_turned_off: false,
            chosen: None,
            hosts: Vec::new(),
        });
    }
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    let hosts = state.hosts.list(&tenant.workspace_id).await?;
    let chosen = workspace
        .home_exit_host_id
        .and_then(|chosen| hosts.iter().find(|host| host.id == chosen).cloned())
        .map(|host| host_dto(state, host));
    Ok(HomeExitDto {
        available: true,
        administrator_turned_off: !state.home_exits.is_enabled(),
        chosen,
        hosts: hosts
            .into_iter()
            .filter(|host| host.can(EXIT_CAPABILITY))
            .map(|host| host_dto(state, host))
            .collect(),
    })
}

/// Write the choice and switch each awake Computer of the Person.
async fn change(
    state: &AppState,
    tenant: &Tenant,
    host_id: Option<&HostId>,
) -> Result<SwitchedHomeExitDto, ApiError> {
    if !state
        .workspaces
        .set_home_exit(&tenant.workspace_id, host_id)
        .await?
    {
        // The Host went between the read and the write.
        return Err(ApiError::not_found("host"));
    }
    tracing::info!(
        workspace_id = %tenant.workspace_id,
        host_id = host_id.map(HostId::as_str),
        "the Person changed their Home Exit"
    );
    // While the System Setting is off the change is not in effect, and
    // every Computer stays in Direct mode with its connections.
    let computers = state.computers.get(&tenant.workspace_id);
    let failures = match state.home_exits.is_enabled() {
        true => computers.switch_exit().await,
        false => computers.settle_exit().await,
    };
    let names: HashMap<_, _> = match failures.is_empty() {
        true => HashMap::new(),
        false => state
            .agent_store
            .list_by_workspace(&tenant.workspace_id)
            .await?
            .into_iter()
            .map(|agent| (agent.id, agent.name))
            .collect(),
    };
    Ok(SwitchedHomeExitDto {
        home_exit: read(state, tenant).await?,
        not_switched: failures
            .into_iter()
            .map(|failure| ExitSwitchFailureDto {
                agent_name: names
                    .get(&failure.agent_id)
                    .cloned()
                    .unwrap_or_else(|| failure.agent_id.to_string()),
                agent_id: failure.agent_id.to_string(),
                error: failure.error,
            })
            .collect(),
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/home-exit",
    responses(
        (status = 200, body = HomeExitDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
/// The signed-in Person's Home Exit, and the Hosts they can choose.
pub async fn get_home_exit(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<HomeExitDto>, ApiError> {
    Ok(Json(read(&state, &tenant).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/home-exit",
    request_body = SetHomeExitRequest,
    responses(
        (status = 200, body = SwitchedHomeExitDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, description = "No Host of the Person has this id", body = crate::error::ErrorBody),
        (status = 409, description = "A Local Installation, or the Administrator turned the Home Exit off", body = crate::error::ErrorBody),
        (status = 422, description = "The Host declared no `exit`", body = crate::error::ErrorBody),
    )
)]
/// Choose one of the Person's own Hosts that declared `exit` as their
/// Home Exit, and switch each of their awake Computers to it at once.
pub async fn set_home_exit(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SetHomeExitRequest>,
) -> Result<Json<SwitchedHomeExitDto>, ApiError> {
    if is_local(&state) {
        return Err(ApiError::conflict(NO_HOME_EXIT_HERE));
    }
    if !state.home_exits.is_enabled() {
        return Err(ApiError::conflict(
            "the Administrator turned the Home Exit off for this installation, so no Host can \
             be your Home Exit until they turn it on",
        ));
    }
    // The read names the Workspace, so the Host of another Person is
    // absent here.
    let host = state
        .hosts
        .get(&tenant.workspace_id, &HostId::from(request.host_id))
        .await?
        .ok_or_else(|| ApiError::not_found("host"))?;
    if !host.can(EXIT_CAPABILITY) {
        return Err(ApiError::validation(format!(
            "{} declared no `exit` capability, so it carries no exit traffic: open the Pagis \
             Client App on it, connected to this server",
            host.name
        )));
    }
    Ok(Json(change(&state, &tenant, Some(&host.id)).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/settings/home-exit",
    responses(
        (status = 200, body = SwitchedHomeExitDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 409, description = "A Local Installation", body = crate::error::ErrorBody),
    )
)]
/// Turn the Person's Home Exit off: their Computers reach the internet
/// from the server again, at once. The Client App's tray calls this too.
pub async fn clear_home_exit(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<SwitchedHomeExitDto>, ApiError> {
    if is_local(&state) {
        return Err(ApiError::conflict(NO_HOME_EXIT_HERE));
    }
    Ok(Json(change(&state, &tenant, None).await?))
}
