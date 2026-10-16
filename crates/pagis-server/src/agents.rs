//! Agents REST: the roster, create, update and archive.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use pagis_core::{
    Agent, AgentId, AgentStatus, AvatarAppearance, Channel, ChannelId, ChannelKind,
    ChannelParticipant, NewEvent, ParticipantId, ParticipantKind, now_ms,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

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
    /// The standing brief (ADR-0020): what a Call the Agent
    /// answers is for, or `null` for none.
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
    /// What a Call the Agent answers is for; absent or
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
            person_id: Some(tenant.user_id.clone()),
            id: ParticipantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            channel_id: dm.id.clone(),
            kind: ParticipantKind::Person,
            participant_workspace_id: None,
            agent_id: None,
            joined_at: now,
        })
        .await?;
    state
        .participants
        .create(&ChannelParticipant {
            person_id: None,
            id: ParticipantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            channel_id: dm.id.clone(),
            kind: ParticipantKind::Agent,
            participant_workspace_id: None,
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
                "author": {"kind": "agent", "id": agent.id, "name": agent.name},
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
    crate::agent_versions::record(&state, &tenant, &agent.id).await?;
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
    crate::agent_versions::record(&state, &tenant, &agent.id).await?;
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
    crate::agent_versions::record(&state, &tenant, &agent_id).await?;
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

/// The stores and services that archive an Agent's identity.
#[derive(Clone)]
pub(crate) struct ArchiveState {
    pub agent_store: Arc<dyn pagis_core::AgentStore>,
    pub workspaces: Arc<dyn pagis_core::WorkspaceStore>,
    pub credentials: Arc<dyn pagis_core::CredentialStore>,
    pub mailbox_desk: Arc<pagis_mail::MailboxDesk>,
    pub bus: Arc<dyn pagis_core::EventBus>,
}

impl axum::extract::FromRef<Arc<AppState>> for ArchiveState {
    fn from_ref(state: &Arc<AppState>) -> Self {
        Self {
            agent_store: state.agent_store.clone(),
            workspaces: state.workspaces.clone(),
            credentials: state.credentials.clone(),
            mailbox_desk: state.mailbox_desk.clone(),
            bus: state.bus.clone(),
        }
    }
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
    let id = AgentId::from(agent_id.clone());
    let changed = state
        .agent_store
        .get(&tenant.workspace_id, &id)
        .await?
        .is_some_and(|agent| agent.status != AgentStatus::Archived);
    let previous_chief = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .and_then(|w| w.chief_of_staff_agent_id);
    let response = archive(
        State(axum::extract::FromRef::from_ref(&state)),
        tenant.clone(),
        Path(agent_id),
    )
    .await?;
    if changed {
        crate::agent_versions::record(&state, &tenant, &id).await?;
        if previous_chief.as_ref() == Some(&id)
            && let Some(next) = state
                .workspaces
                .get(&tenant.workspace_id)
                .await?
                .and_then(|w| w.chief_of_staff_agent_id)
        {
            crate::agent_versions::record(&state, &tenant, &next).await?;
        }
    }
    Ok(response)
}

pub(crate) async fn archive(
    State(state): State<ArchiveState>,
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
        let email_address = state
            .mailbox_desk
            .for_agent(&tenant.workspace_id, &agent.id)
            .await
            .map_err(crate::mailboxes::mailbox_error)?
            .map(|mailbox| mailbox.address);
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
        let successor =
            crate::workspace::active_agents_oldest_first(state.agent_store.as_ref(), &tenant)
                .await?
                .into_iter()
                .next()
                .map(|next| next.id);
        crate::workspace::designate(
            state.workspaces.as_ref(),
            state.bus.as_ref(),
            state.mailbox_desk.as_ref(),
            &tenant,
            successor.as_ref(),
        )
        .await?;
    }
    // Archival must not destroy account access (ADR-0013): there
    // is no backup and no export, so the agent's minted credentials
    // stay in the vault and pass to the user.
    let released = state
        .credentials
        .release_owner(&tenant.workspace_id, &agent.id)
        .await?;

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
                "dormant_mailbox": email_address,
                "chief_of_staff_moved": chief_moved,
            }),
        })
        .await?;
    Ok(Json(agent_dto(agent, email_address)))
}
