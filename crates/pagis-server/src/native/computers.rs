//! Computer lifecycle, screen control and Docker routes.

use crate::administration::PersonDto;
use crate::{
    AppState, Peer,
    auth::{Administrator, Tenant},
    error::ApiError,
    live_connections::LiveTenant,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use pagis_computer::{ComputerImageState, ComputerState, ImagePullError};
use pagis_core::{AgentId, ComputerError, InputHolder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
/// 404 unless the agent exists in this workspace.
async fn require_agent(
    state: &AppState,
    tenant: &Tenant,
    agent_id: &AgentId,
) -> Result<(), ApiError> {
    state
        .agent_store
        .get(&tenant.workspace_id, agent_id)
        .await?
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("agent"))
}

/// One agent computer's state. `percent` is set while pulling;
/// `holder` is the input switch: `agent`, `user`, or
/// `daemon` while a vault fill runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComputerDto {
    /// `off`, `pulling`, `starting`, `awake`, or `failed`.
    pub state: String,
    /// `absent`, `present`, `mismatched`, or `unavailable`.
    pub image: String,
    pub percent: Option<u8>,
    pub error: Option<String>,
    pub holder: String,
    /// Which exit an awake Computer in Home mode uses (ADR-0029), such as
    /// `exit: MacBook Pro` or `exit: server`. Null in Direct mode, which
    /// is the mode of a Person with no Home Exit, and while the Computer
    /// is not awake. `computer.exit_changed` events carry each change.
    pub exit: Option<String>,
}

/// The view of one Agent's Computer in `state`.
async fn computer_view(
    computer: &pagis_computer::ComputerManager,
    agent_id: &AgentId,
    state: ComputerState,
) -> ComputerDto {
    let image_state = computer.image_state().await;
    let exit = computer
        .exit_in_use(agent_id)
        .await
        .map(|exit| exit.label());
    computer_dto(state, image_state, computer.holder(agent_id), exit)
}

fn computer_dto(
    state: ComputerState,
    image_state: ComputerImageState,
    holder: InputHolder,
    exit: Option<String>,
) -> ComputerDto {
    let holder = holder.as_str().to_string();
    let (image, image_error) = match image_state {
        ComputerImageState::Absent => ("absent", None),
        ComputerImageState::Present => ("present", None),
        ComputerImageState::Mismatched { found } => (
            "mismatched",
            Some(format!(
                "expected image version {}, found {found:?}",
                pagis_computer::IMAGE_VERSION
            )),
        ),
        ComputerImageState::Unavailable { message } => ("unavailable", Some(message)),
    };
    let (state, percent, state_error) = match state {
        ComputerState::Off => ("off", None, None),
        ComputerState::Pulling { percent } => ("pulling", Some(percent), None),
        ComputerState::Starting => ("starting", None, None),
        ComputerState::Awake => ("awake", None, None),
        ComputerState::Failed { message } => ("failed", None, Some(message)),
    };
    ComputerDto {
        state: state.to_string(),
        image: image.to_string(),
        percent,
        error: state_error.or(image_error),
        holder,
        exit,
    }
}

fn computer_error(err: ComputerError) -> ApiError {
    match err {
        ComputerError::VersionMismatch { .. } => ApiError {
            status: StatusCode::CONFLICT,
            code: "image_version_mismatch",
            message: err.to_string(),
        },
        ComputerError::Asleep => ApiError {
            status: StatusCode::CONFLICT,
            code: "computer_asleep",
            message: err.to_string(),
        },
        ComputerError::Busy => ApiError {
            status: StatusCode::CONFLICT,
            code: "computer_busy",
            message: err.to_string(),
        },
        // The cap on simultaneously awake Computers. It is the
        // machine that is full, not the request that is wrong, so the
        // client may ask again after a computer sleeps.
        ComputerError::AwakeCapReached { .. } => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "awake_cap_reached",
            message: err.to_string(),
        },
        // Every port of the relay's range is taken: the
        // installation is full now, so the viewer may ask again.
        ComputerError::NoMediaPath { .. } => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "no_media_path",
            message: err.to_string(),
        },
        ComputerError::NoPreview => ApiError::not_found("screen preview"),
        // A manager of another tenant was asked for this Agent.
        // Every route resolves the manager from the Session, so this is
        // a bug and not a request: it reads as an absent sprite.
        ComputerError::ForeignAgent => ApiError::not_found("agent"),
        ComputerError::SwitchHeld { .. } => ApiError {
            status: StatusCode::CONFLICT,
            code: "switch_held",
            message: err.to_string(),
        },
        ComputerError::Runtime(message) => {
            tracing::error!(error = %message, "computer runtime error");
            ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "computer_unavailable",
                message,
            }
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{agent_id}/computer",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, body = ComputerDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn computer_state(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<ComputerDto>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let computer = state.native.computers.get(&tenant.workspace_id);
    let computer_state = computer.state(&agent_id).await;
    Ok(Json(
        computer_view(&computer, &agent_id, computer_state).await,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/computer/wake",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 202, body = ComputerDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The local image version mismatches the pin", body = crate::error::ErrorBody),
        (status = 503, description = "Docker is unreachable", body = crate::error::ErrorBody),
    )
)]
pub async fn wake_computer(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<(StatusCode, Json<ComputerDto>), ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let computer = state.native.computers.get(&tenant.workspace_id);
    let computer_state = computer.wake(&agent_id).await.map_err(computer_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(computer_view(&computer, &agent_id, computer_state).await),
    ))
}

/// Put the agent's computer to sleep now, ahead of the idle
/// sweep. The disk survives, so the next wake finds the same data.
#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/computer/sleep",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, body = ComputerDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The computer is running a command", body = crate::error::ErrorBody),
        (status = 503, description = "Docker is unreachable", body = crate::error::ErrorBody),
    )
)]
pub async fn sleep_computer(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<ComputerDto>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let computer = state.native.computers.get(&tenant.workspace_id);
    let computer_state = computer.sleep(&agent_id).await.map_err(computer_error)?;
    Ok(Json(
        computer_view(&computer, &agent_id, computer_state).await,
    ))
}

/// What one office keeps on disk: the bytes of that
/// tenant's Agent volumes, which the Desk Panel says in its footer. The
/// figure counts the volumes of one tenant and never the server's.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComputerDiskDto {
    /// The bytes the Agent volumes of the signed-in person's Workspace
    /// hold together; `null` when Docker cannot say.
    pub bytes: Option<u64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/computers/disk",
    responses(
        (status = 200, body = ComputerDiskDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn computer_disk(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ComputerDiskDto>, ApiError> {
    // A disk figure is not worth a failed page: Docker that cannot
    // answer leaves the footer without it.
    let bytes = match state
        .native
        .computers
        .get(&tenant.workspace_id)
        .resources()
        .await
    {
        Ok(resources) => Some(resources.volume_bytes),
        Err(error) => {
            tracing::warn!(%error, "disk usage read failed");
            None
        }
    };
    Ok(Json(ComputerDiskDto { bytes }))
}

/// Take over the agent's computer: the daemon flips the input
/// switch to the user and denies the agent's screen leases.
#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/screen/takeover",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, body = ComputerDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The computer is not awake", body = crate::error::ErrorBody),
    )
)]
pub async fn screen_takeover(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<ComputerDto>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let computer = state.native.computers.get(&tenant.workspace_id);
    computer.takeover(&agent_id).await.map_err(computer_error)?;
    let computer_state = computer.state(&agent_id).await;
    Ok(Json(
        computer_view(&computer, &agent_id, computer_state).await,
    ))
}

/// Hand the computer back to the agent: the parked run resumes
/// with the "screen may have changed" note. A handback without a
/// takeover is a no-op.
#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/screen/handback",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, body = ComputerDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn screen_handback(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<ComputerDto>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let computer = state.native.computers.get(&tenant.workspace_id);
    computer
        .handback(&agent_id, "explicit")
        .await
        .map_err(computer_error)?;
    let computer_state = computer.state(&agent_id).await;
    Ok(Json(
        computer_view(&computer, &agent_id, computer_state).await,
    ))
}

/// One WebRTC SDP payload: the browser's offer up, screend's
/// answer back.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SdpBody {
    pub sdp: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/screen/offer",
    params(("agent_id" = String, Path,)),
    request_body = SdpBody,
    responses(
        (status = 200, body = SdpBody),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "The computer is not awake", body = crate::error::ErrorBody),
    )
)]
pub async fn screen_offer(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    Path(agent_id): Path<String>,
    Json(body): Json<SdpBody>,
) -> Result<Json<SdpBody>, ApiError> {
    let tenant = &live.tenant;
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, tenant, &agent_id).await?;
    // The Media Relay path belongs to the Session that asked for it, and
    // it closes when that Session ends.
    let answer = state
        .native
        .computers
        .get(&tenant.workspace_id)
        .offer(&agent_id, &body.sdp, live.session_ended)
        .await
        .map_err(computer_error)?;
    Ok(Json(SdpBody { sdp: answer }))
}

/// One ICE server the browser configures before it offers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IceServerDto {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

/// What a browser needs before it opens a screen session.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ScreenIceBody {
    /// Empty when the browser reaches the Media Relay by itself, which
    /// is what a local installation and the `daemon` relay do.
    pub ice_servers: Vec<IceServerDto>,
}

/// The ICE servers of the installation's Media Relay (ADR-0014), and in
/// Remote Access the TURN server that carries the live screen to a
/// browser on another machine over the Funnel (ADR-0028).
///
/// A browser reads this before it makes its offer, because ICE servers
/// are fixed when the peer connection is made. The `turn` relay and the
/// TURN server of Remote Access mint credentials for each answer, so the
/// answer is never cached.
#[utoipa::path(
    get,
    path = "/api/v1/screen/ice",
    responses(
        (status = 200, body = ScreenIceBody),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn screen_ice(
    State(state): State<Arc<AppState>>,
    Peer(peer): Peer,
    headers: HeaderMap,
    _tenant: Tenant,
) -> Result<Json<ScreenIceBody>, ApiError> {
    Ok(Json(ScreenIceBody {
        ice_servers: state
            .native
            .computers
            .ice_servers()
            .into_iter()
            .chain(crate::remote_access_native::turn_ice_server(
                &state, peer, &headers,
            ))
            .map(|server| IceServerDto {
                urls: server.urls,
                username: server.username,
                credential: server.credential,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{agent_id}/screen/preview.png",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, description = "PNG bytes; a live frame when awake, the last screenshot when asleep", content_type = "image/png"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn screen_preview(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Response, ApiError> {
    let agent_id = AgentId::from(agent_id);
    require_agent(&state, &tenant, &agent_id).await?;
    let preview = state
        .native
        .computers
        .get(&tenant.workspace_id)
        .preview(&agent_id)
        .await
        .map_err(computer_error)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png".to_string()),
            (
                header::HeaderName::from_static("x-pagis-live"),
                preview.live.to_string(),
            ),
        ],
        preview.png,
    )
        .into_response())
}

/// What each Person's Computers take of the host.
#[utoipa::path(
    get,
    path = "/api/v1/administration/resources",
    responses(
        (status = 200, body = ResourcesDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn installation_resources(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<ResourcesDto>, ApiError> {
    let org = crate::administration::org(&state).await?;
    let caps = state.native.computers.ceiling().caps();
    let mut items = Vec::new();
    for person in state.users.list_by_org(&org.id).await? {
        let workspace = state.workspaces.for_user(&person.id).await?;
        let person_dto = crate::administration::person_dto(
            &person,
            workspace.as_ref().map(|workspace| &workspace.id),
        );
        let Some(workspace) = workspace else {
            // A Person with no Workspace holds no Docker object either.
            items.push(PersonResourcesDto {
                person: person_dto,
                containers: 0,
                volumes: 0,
                volume_bytes: Some(0),
                awake_computers: 0,
            });
            continue;
        };
        // Docker that cannot answer leaves the figures out and the page
        // standing, as the Desk's disk read does.
        let resources = match state.native.computers.get(&workspace.id).resources().await {
            Ok(resources) => Some(resources),
            Err(error) => {
                tracing::warn!(%error, workspace = %workspace.id, "the resource read failed");
                None
            }
        };
        items.push(PersonResourcesDto {
            person: person_dto,
            containers: resources.map(|read| read.containers).unwrap_or(0),
            volumes: resources.map(|read| read.volumes).unwrap_or(0),
            volume_bytes: resources.map(|read| read.volume_bytes),
            awake_computers: state.native.computers.ceiling().awake(&workspace.id),
        });
    }
    Ok(Json(ResourcesDto {
        items,
        awake_on_server: state.native.computers.ceiling().awake_on_server(),
        awake_cap_per_server: caps.per_server,
        awake_cap_per_tenant: caps.per_tenant,
    }))
}

/// The Computer Image that the Client App asks the daemon to pull.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ComputerImagePullRequest {
    /// `<repository>@sha256:<64 lowercase hexadecimal characters>`. The
    /// repository is the repository of the pinned Computer Image.
    pub image: String,
}

/// The Computer Image that the daemon pulled.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComputerImageDto {
    pub image: String,
}

/// Pull a Computer Image, and answer when the pull ends.
///
/// Before a restart to an Update, the Client App of a Local Installation
/// asks for the Computer Image that the next release pins, so the new
/// daemon finds it present (ADR-0027). The daemon pulls only an image of
/// the repository of its own pinned image, and only by its digest. A
/// second request for the same image joins the pull that runs, and a
/// request that goes away does not stop the pull. A failed pull answers
/// `502`, and `503` where Docker does not answer.
#[utoipa::path(
    post,
    path = "/api/v1/administration/computer-image/pull",
    request_body = ComputerImagePullRequest,
    responses(
        (status = 200, body = ComputerImageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    )
)]
pub async fn pull_computer_image(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<ComputerImagePullRequest>,
) -> Result<Json<ComputerImageDto>, ApiError> {
    state
        .native
        .computers
        .pull_image(&request.image)
        .await
        .map_err(|error| match error {
            ImagePullError::Refused(message) => ApiError::validation(message),
            ImagePullError::NoDocker(_) => ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "docker_unavailable",
                message: error.to_string(),
            },
            ImagePullError::Failed(_) => ApiError {
                status: StatusCode::BAD_GATEWAY,
                code: "image_pull_failed",
                message: error.to_string(),
            },
        })?;
    Ok(Json(ComputerImageDto {
        image: request.image,
    }))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/onboarding/docker-endpoint",
    request_body = SetOnboardingDockerEndpointRequest,
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The computer step's Docker endpoint. It answers an Administrator
/// while their Workspace has not finished onboarding, and `409`
/// afterwards.
pub async fn set_onboarding_docker_endpoint(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Json(request): Json<SetOnboardingDockerEndpointRequest>,
) -> Result<StatusCode, ApiError> {
    crate::settings::refuse_after_onboarding(&state, &tenant).await?;
    let endpoint =
        crate::system::checked_docker_endpoint(&state, request.docker_endpoint.as_deref()).await?;
    crate::system::save_docker_endpoint(&state, endpoint)?;
    Ok(StatusCode::NO_CONTENT)
}

/// What one Person's Computers take of the host.
#[derive(Debug, Serialize, ToSchema)]
pub struct PersonResourcesDto {
    pub person: PersonDto,
    /// The tenant's containers, awake or not.
    pub containers: u32,
    /// The named volumes of the tenant's Agents.
    pub volumes: u32,
    /// The bytes those volumes take, or `null` where Docker did not
    /// answer: a missing figure is not a failed page.
    pub volume_bytes: Option<u64>,
    /// How many Computers of this Person are awake now.
    pub awake_computers: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResourcesDto {
    pub items: Vec<PersonResourcesDto>,
    /// How many Computers the whole server holds awake now, and how
    /// many it may.
    pub awake_on_server: u32,
    pub awake_cap_per_server: u32,
    pub awake_cap_per_tenant: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetOnboardingDockerEndpointRequest {
    /// A socket path, a `unix://` or a `tcp://` endpoint. Null gives
    /// discovery the choice back. Pagis pings the endpoint before it
    /// saves.
    pub docker_endpoint: Option<String>,
}
