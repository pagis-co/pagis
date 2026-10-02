//! Agents REST: the roster list and CRUD
//! (create/update/archive), the per-agent computer state and wake, and
//! the screen preview — a current frame while the computer is awake,
//! the last screenshot while it sleeps.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pagis_core::{
    Agent, AgentId, AgentStatus, AvatarAppearance, Channel, ChannelId, ChannelKind,
    ChannelParticipant, NewEvent, ParticipantId, ParticipantKind, now_ms,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::live_connections::LiveTenant;
use pagis_computer::{ComputerError, ComputerImageState, ComputerState, InputHolder};

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentDto {
    pub id: String,
    pub avatar: AvatarAppearance,
    pub name: String,
    pub job: String,
    /// One line on what to ask this Agent for, read by every other
    /// Agent in its sprite line.
    pub description: String,
    pub personality: String,
    /// The Agent Voice (ADR-0020): a catalogue name, or `null` for the
    /// provider's default.
    pub voice: Option<String>,
    /// The standing brief (ADR-0020): what an inbound call to the
    /// Agent's desk line is for, or `null` for none.
    pub standing_brief: Option<String>,
    pub status: String,
    /// The address of the Agent Mailbox it holds, or `null` when it
    /// holds none (ADR-0019). It is a read-through view of the mailbox
    /// record: the Agent itself carries no address.
    pub email_address: Option<String>,
}

/// The Agent, with the address of the mailbox it holds. The caller
/// reads the mailbox once for a roster and once for one Agent, so the
/// read-through costs one query and not one for each Agent.
fn agent_dto(agent: Agent, email_address: Option<String>) -> AgentDto {
    AgentDto {
        id: agent.id.to_string(),
        avatar: agent.avatar,
        name: agent.name,
        job: agent.job,
        description: agent.description,
        personality: agent.personality,
        voice: agent.voice,
        standing_brief: agent.standing_brief,
        status: agent.status.as_str().to_string(),
        email_address,
    }
}

/// The address the Agent's mailbox holds now, when it holds one.
async fn email_address_of(
    state: &AppState,
    tenant: &Tenant,
    agent_id: &AgentId,
) -> Result<Option<String>, ApiError> {
    Ok(state
        .mailbox_desk
        .for_agent(&tenant.workspace_id, agent_id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?
        .map(|mailbox| mailbox.address))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAgentRequest {
    #[serde(default)]
    pub avatar: AvatarAppearance,
    pub name: String,
    pub job: String,
    /// One line on what to ask this Agent for.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub personality: String,
    /// A name from `GET /api/v1/settings/voices`; absent or empty
    /// declares no voice.
    #[serde(default)]
    pub voice: Option<String>,
    /// What an inbound call to the Agent's desk line is for; absent or
    /// empty declares none.
    #[serde(default)]
    pub standing_brief: Option<String>,
    /// The mailbox section of the form (ADR-0019): the Agent gets its
    /// own Agent Mailbox as it is made. Absent gives it none, and the
    /// user may provision one later from the Agent's page.
    #[serde(default)]
    pub mailbox: Option<crate::mailboxes::NewMailboxRequest>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAgentRequest {
    pub name: String,
    pub job: String,
    #[serde(default)]
    pub description: String,
    pub personality: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub standing_brief: Option<String>,
}

/// An empty standing brief is no standing brief.
fn trimmed_standing_brief(brief: Option<String>) -> Option<String> {
    brief
        .map(|brief| brief.trim().to_string())
        .filter(|brief| !brief.is_empty())
}

/// An empty voice is no voice. Any other name must be a voice of the
/// model that speaks for the Workspace (ADR-0020), unless the Agent
/// already holds it: a voice stays when the speaking model changes, and
/// a reply then speaks in the model's default.
async fn validate_voice(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    voice: Option<String>,
    held: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(voice) = voice
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    if held == Some(voice.as_str()) {
        return Ok(Some(voice));
    }
    let Some(list) = crate::voice_list::speaking_voices(state, workspace_id).await? else {
        return Err(ApiError::validation(format!(
            "no key serves spoken replies, so no voice can be chosen; add a key for {}",
            crate::voice_list::speaking_providers()
        )));
    };
    if !list.has(&voice) {
        return Err(ApiError::validation(format!(
            "`{voice}` is not a voice of {}/{}; choose one of {}",
            list.provider.id(),
            list.model,
            list.names()
        )));
    }
    Ok(Some(voice))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentPage {
    pub items: Vec<AgentDto>,
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
}

fn computer_dto(
    state: ComputerState,
    image_state: ComputerImageState,
    holder: InputHolder,
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

#[utoipa::path(
    get,
    path = "/api/v1/agents",
    responses(
        (status = 200, body = AgentPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<AgentPage>, ApiError> {
    // One read of the mailboxes serves the whole roster.
    let mut addresses: std::collections::HashMap<String, String> = state
        .mailbox_desk
        .list(&tenant.workspace_id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?
        .into_iter()
        .map(|mailbox| (mailbox.agent_id.to_string(), mailbox.address))
        .collect();
    let items = state
        .agent_store
        .list_by_workspace(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(|agent| {
            let address = addresses.remove(agent.id.as_str());
            agent_dto(agent, address)
        })
        .collect();
    Ok(Json(AgentPage { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents",
    request_body = CreateAgentRequest,
    responses(
        (status = 201, body = AgentDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn create_agent(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<CreateAgentRequest>,
) -> Result<(StatusCode, Json<AgentDto>), ApiError> {
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::validation("agent name must not be empty"));
    }
    request.avatar.validate().map_err(ApiError::validation)?;
    let voice = validate_voice(&state, &tenant.workspace_id, request.voice, None).await?;
    // Everything the mailbox needs that does not need the Agent is
    // checked first, so a taken address or a reserved name answers the
    // form and makes no Agent (ADR-0019).
    let mailbox = request.mailbox.map(pagis_mail::NewMailbox::from);
    if let Some(mailbox) = &mailbox {
        state
            .mailbox_desk
            .check(mailbox)
            .await
            .map_err(crate::mailboxes::mailbox_error)?;
    }
    let now = now_ms();
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        name,
        job: request.job.trim().to_string(),
        description: request.description.trim().to_string(),
        personality: request.personality.trim().to_string(),
        model_alias: pagis_core::DEFAULT_MODEL_ALIAS.to_string(),
        avatar: request.avatar,
        voice,
        standing_brief: trimmed_standing_brief(request.standing_brief),
        status: AgentStatus::Active,
        created_at: now,
        updated_at: now,
    };
    state.agent_store.create(&agent).await?;

    // Every agent gets its DM on creation, the way the seeded
    // assistant does: the user plus the agent.
    let dm = Channel {
        id: ChannelId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        kind: ChannelKind::Dm,
        title: Some(agent.name.clone()),
        created_at: now,
        updated_at: now,
    };
    state.channels.create(&dm).await?;
    state
        .participants
        .create(&ChannelParticipant {
            id: ParticipantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            channel_id: dm.id.clone(),
            kind: ParticipantKind::User,
            agent_id: None,
            joined_at: now,
        })
        .await?;
    state
        .participants
        .create(&ChannelParticipant {
            id: ParticipantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            channel_id: dm.id.clone(),
            kind: ParticipantKind::Agent,
            agent_id: Some(agent.id.clone()),
            joined_at: now,
        })
        .await?;

    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "agent.created".to_string(),
            agent_id: Some(agent.id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({ "name": agent.name }),
        })
        .await?;
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "channel.created".to_string(),
            agent_id: Some(agent.id.clone()),
            run_id: None,
            channel_id: Some(dm.id.clone()),
            payload: serde_json::json!({ "title": agent.name }),
        })
        .await?;

    let greeting = agent.greeting(dm.id);
    state.messages.insert_stamped(&greeting, &[]).await?;
    state
        .bus
        .publish(NewEvent {
            workspace_id: agent.workspace_id.clone(),
            event_type: "message.completed".to_string(),
            agent_id: Some(agent.id.clone()),
            run_id: None,
            channel_id: Some(greeting.channel_id.clone()),
            payload: serde_json::json!({
                "message_id": greeting.id,
                "parent_message_id": null,
                "author_kind": "agent",
            }),
        })
        .await?;

    // The host makes the mailbox now, so a host refusal reaches the
    // form. The login is proven afterwards (ADR-0019).
    let email_address = match mailbox {
        None => None,
        Some(mailbox) => Some(
            state
                .mailbox_desk
                .provision(&tenant.workspace_id, &agent.id, mailbox)
                .await
                .map_err(crate::mailboxes::mailbox_error)?
                .address,
        ),
    };
    Ok((StatusCode::CREATED, Json(agent_dto(agent, email_address))))
}

#[utoipa::path(
    put,
    path = "/api/v1/agents/{agent_id}",
    params(("agent_id" = String, Path,)),
    request_body = UpdateAgentRequest,
    responses(
        (status = 200, body = AgentDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn update_agent(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
    Json(request): Json<UpdateAgentRequest>,
) -> Result<Json<AgentDto>, ApiError> {
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::validation("agent name must not be empty"));
    }
    let agent_id = AgentId::from(agent_id);
    let mut agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    let voice = validate_voice(
        &state,
        &tenant.workspace_id,
        request.voice,
        agent.voice.as_deref(),
    )
    .await?;
    agent.name = name;
    agent.job = request.job.trim().to_string();
    agent.description = request.description.trim().to_string();
    agent.personality = request.personality.trim().to_string();
    agent.voice = voice;
    agent.standing_brief = trimmed_standing_brief(request.standing_brief);
    agent.updated_at = now_ms();
    state.agent_store.update(&agent).await?;
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "agent.updated".to_string(),
            agent_id: Some(agent.id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({ "name": agent.name }),
        })
        .await?;
    let email_address = email_address_of(&state, &tenant, &agent.id).await?;
    Ok(Json(agent_dto(agent, email_address)))
}

#[utoipa::path(
    put, path = "/api/v1/agents/{agent_id}/appearance",
    params(("agent_id" = String, Path,)), request_body = AvatarAppearance,
    responses((status = 200, body = AgentDto), (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody))
)]
pub async fn update_appearance(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
    Json(avatar): Json<AvatarAppearance>,
) -> Result<Json<AgentDto>, ApiError> {
    avatar.validate().map_err(ApiError::validation)?;
    let agent_id = AgentId::from(agent_id);
    if !state
        .agent_store
        .update_avatar(&tenant.workspace_id, &agent_id, &avatar, now_ms())
        .await?
    {
        return Err(ApiError::not_found("agent"));
    }
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "agent.updated".into(),
            agent_id: Some(agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({"appearance": true}),
        })
        .await?;
    let agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    let email_address = email_address_of(&state, &tenant, &agent_id).await?;
    Ok(Json(agent_dto(agent, email_address)))
}

#[utoipa::path(
    post,
    path = "/api/v1/agents/{agent_id}/archive",
    params(("agent_id" = String, Path,)),
    responses(
        (status = 200, body = AgentDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn archive_agent(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<AgentDto>, ApiError> {
    let agent_id = AgentId::from(agent_id);
    let mut agent = state
        .agent_store
        .get(&tenant.workspace_id, &agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))?;
    if agent.status == AgentStatus::Archived {
        let email_address = email_address_of(&state, &tenant, &agent.id).await?;
        return Ok(Json(agent_dto(agent, email_address)));
    }
    agent.status = AgentStatus::Archived;
    agent.updated_at = now_ms();
    state.agent_store.update(&agent).await?;
    // Archiving the Chief of Staff moves the designation to the next
    // active Agent, oldest first; a Workspace with no active Agent has
    // none (ADR-0022).
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    let chief_moved = workspace.chief_of_staff_agent_id.as_ref() == Some(&agent.id);
    if chief_moved {
        let successor = crate::workspace::active_agents_oldest_first(&state, &tenant)
            .await?
            .into_iter()
            .next()
            .map(|next| next.id);
        crate::workspace::designate(&state, &tenant, successor.as_ref()).await?;
    }
    // Archival must not destroy account access (ADR-0013): there
    // is no backup and no export, so the agent's minted credentials
    // stay in the vault and pass to the user.
    let released = state
        .credentials
        .release_owner(&tenant.workspace_id, &agent.id)
        .await?;
    // Archiving an Agent unassigns its number (ADR-0018). The Workspace
    // keeps the number and keeps paying for it, and the Calls stay with
    // the Agent that made them.
    let unassigned = state
        .numbers
        .unassign_for_agent(&tenant.workspace_id, &agent.id)
        .await
        .map_err(crate::phone_numbers::number_error)?;
    // Archiving an Agent puts its mailbox to sleep (ADR-0019). Unlike
    // the number, the mailbox stays with the Agent: the mail is kept,
    // nothing wakes and nothing sends.
    let dormant = state
        .mailbox_desk
        .make_dormant_for_agent(&tenant.workspace_id, &agent.id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?;
    let email_address = dormant.as_ref().map(|mailbox| mailbox.address.clone());
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "agent.archived".to_string(),
            agent_id: Some(agent.id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "name": agent.name,
                "released_credentials": released,
                "unassigned_e164": unassigned.map(|number| number.e164),
                "dormant_mailbox": email_address,
                "chief_of_staff_moved": chief_moved,
            }),
        })
        .await?;
    Ok(Json(agent_dto(agent, email_address)))
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
    let computer = state.computers.get(&tenant.workspace_id);
    let computer_state = computer.state(&agent_id).await;
    let image_state = computer.image_state().await;
    Ok(Json(computer_dto(
        computer_state,
        image_state,
        computer.holder(&agent_id),
    )))
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
    let computer = state.computers.get(&tenant.workspace_id);
    let computer_state = computer.wake(&agent_id).await.map_err(computer_error)?;
    let image_state = computer.image_state().await;
    Ok((
        StatusCode::ACCEPTED,
        Json(computer_dto(
            computer_state,
            image_state,
            computer.holder(&agent_id),
        )),
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
    let computer = state.computers.get(&tenant.workspace_id);
    let computer_state = computer.sleep(&agent_id).await.map_err(computer_error)?;
    let image_state = computer.image_state().await;
    Ok(Json(computer_dto(
        computer_state,
        image_state,
        computer.holder(&agent_id),
    )))
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
    let bytes = match state.computers.get(&tenant.workspace_id).resources().await {
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
    let computer = state.computers.get(&tenant.workspace_id);
    computer.takeover(&agent_id).await.map_err(computer_error)?;
    let computer_state = computer.state(&agent_id).await;
    let image_state = computer.image_state().await;
    Ok(Json(computer_dto(
        computer_state,
        image_state,
        computer.holder(&agent_id),
    )))
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
    let computer = state.computers.get(&tenant.workspace_id);
    computer
        .handback(&agent_id, "explicit")
        .await
        .map_err(computer_error)?;
    let computer_state = computer.state(&agent_id).await;
    let image_state = computer.image_state().await;
    Ok(Json(computer_dto(
        computer_state,
        image_state,
        computer.holder(&agent_id),
    )))
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    _tenant: Tenant,
) -> Result<Json<ScreenIceBody>, ApiError> {
    Ok(Json(ScreenIceBody {
        ice_servers: state
            .computers
            .ice_servers()
            .into_iter()
            .chain(crate::remote_access::turn_ice_server(
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
