//! One conversation in the Org's Workspace, read by its participants.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{
    Author, Block, Channel, ChannelId, ChannelKind, Message, MessageId, MessageStatus, NewEvent,
    SendOutcome, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::channels::{MessageDto, TimelineQuery};
use crate::{AppState, auth::Tenant, error::ApiError};

#[derive(Debug, Serialize, ToSchema)]
pub struct ColleagueDto {
    pub person_id: String,
    pub workspace_id: String,
    pub name: String,
    /// `in_a_meeting` during a busy Event in the Private Workspace Calendar;
    /// otherwise `active` or `away`, from client input. No presence when
    /// the activity of the Person cannot be read.
    pub presence: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ColleaguePage {
    pub items: Vec<ColleagueDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationDto {
    pub id: String,
    pub participants: Vec<ColleagueDto>,
    pub last_message: Option<MessageDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationPage {
    pub items: Vec<ConversationDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StartConversationRequest {
    pub workspace_ids: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConversationMessageRequest {
    #[serde(default)]
    pub for_person_only: bool,
    pub pending_id: String,
    pub text: String,
    pub parent_message_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationMessages {
    pub items: Vec<MessageDto>,
    pub next_cursor: Option<String>,
}

async fn person_dto(
    state: &AppState,
    person: pagis_core::User,
    workspace_id: &WorkspaceId,
) -> Result<ColleagueDto, ApiError> {
    let now = state.clock.now_ms();
    let meeting = !pagis_core::calendar_access::free_busy(
        state.calendar.as_ref(),
        state.workspaces.as_ref(),
        state.workspace_members.as_ref(),
        state.users.as_ref(),
        workspace_id,
        pagis_core::CalendarRange::new(now, now + 1)?,
    )
    .await?
    .is_empty();
    let presence = if meeting {
        Some("in_a_meeting")
    } else {
        match state
            .person_activity_reader
            .hold_until(&person.id, now)
            .await
        {
            Ok(Some(_)) => Some("active"),
            Ok(None) => Some("away"),
            Err(error) => {
                tracing::warn!(%error, "the activity of a colleague cannot be read");
                None
            }
        }
    };
    Ok(ColleagueDto {
        person_id: person.id.to_string(),
        workspace_id: workspace_id.to_string(),
        name: person
            .name
            .or(person.email)
            .unwrap_or_else(|| "Person".into()),
        presence: presence.map(Into::into),
    })
}

async fn people(state: &AppState, tenant: &Tenant) -> Result<Vec<ColleagueDto>, ApiError> {
    let person = state
        .users
        .get(&tenant.user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let mut items = Vec::new();
    for person in state.users.list_by_org(&person.org_id).await? {
        if person.is_disabled() {
            continue;
        }
        if let Some(workspace) = state.workspaces.for_user(&person.id).await? {
            items.push(person_dto(state, person, &workspace.id).await?);
        }
    }
    Ok(items)
}

#[utoipa::path(get, path = "/api/v1/colleagues", responses((status = 200, body = ColleaguePage)))]
pub async fn list_colleagues(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ColleaguePage>, ApiError> {
    let items = people(&state, &tenant)
        .await?
        .into_iter()
        .filter(|p| p.person_id != tenant.user_id.as_str())
        .collect();
    Ok(Json(ColleaguePage { items }))
}

async fn participant_ids(
    state: &AppState,
    storage: &WorkspaceId,
    id: &ChannelId,
) -> Result<Vec<WorkspaceId>, ApiError> {
    Ok(state
        .participants
        .list_for_channel(storage, id)
        .await?
        .into_iter()
        .filter_map(|p| p.participant_workspace_id)
        .collect())
}

async fn storage(state: &AppState, id: &ChannelId) -> Result<WorkspaceId, ApiError> {
    Ok(state
        .assignments
        .records()
        .by_channel(id)
        .await?
        .map_or_else(|| state.org_workspace_id.clone(), |a| a.workspace_id))
}

/// Check access before a route reads or writes conversation content.
pub(crate) async fn entrance(
    state: &AppState,
    tenant: &Tenant,
    id: &ChannelId,
) -> Result<Vec<WorkspaceId>, ApiError> {
    let storage = storage(state, id).await?;
    let assignment = state.assignments.records().by_channel(id).await?;
    if let Some(assignment) = &assignment {
        let member = state
            .workspace_members
            .get(&assignment.workspace_id, &tenant.user_id)
            .await?;
        if !member.is_some_and(|member| {
            assignment.assignee == tenant.user_id
                || member.role >= pagis_core::WorkspaceRole::Responder
        }) {
            return Err(ApiError::not_found("conversation"));
        }
    }
    state
        .channels
        .get(&storage, id)
        .await?
        .filter(|c| c.kind == ChannelKind::Conversation)
        .ok_or_else(|| ApiError::not_found("conversation"))?;
    let ids = participant_ids(state, &storage, id).await?;
    if assignment.is_none() && !ids.contains(&tenant.workspace_id) {
        return Err(ApiError::not_found("conversation"));
    }
    Ok(ids)
}

async fn dto(
    state: &AppState,
    tenant: &Tenant,
    channel: Channel,
) -> Result<ConversationDto, ApiError> {
    let ids = entrance(state, tenant, &channel.id).await?;
    let storage = channel.workspace_id.clone();
    let mut participants = Vec::new();
    for id in ids {
        let workspace = state
            .workspaces
            .get(&id)
            .await?
            .ok_or_else(|| ApiError::not_found("participant"))?;
        let Some(person_id) = workspace.user_id else {
            participants.push(ColleagueDto {
                person_id: String::new(),
                workspace_id: workspace.id.to_string(),
                name: workspace.name,
                presence: None,
            });
            continue;
        };
        let person = state
            .users
            .get(&person_id)
            .await?
            .ok_or_else(|| ApiError::not_found("participant"))?;
        participants.push(person_dto(state, person, &id).await?);
    }
    let last_message = match state
        .messages
        .list_channel(&storage, &channel.id, None, 1)
        .await?
        .into_iter()
        .next()
    {
        Some(message) => Some(
            MessageDto::read(
                state,
                crate::channels::readable_message(state, message).await?,
            )
            .await?,
        ),
        None => None,
    };
    Ok(ConversationDto {
        id: channel.id.to_string(),
        participants,
        last_message,
    })
}

#[utoipa::path(get, path = "/api/v1/conversations", responses((status = 200, body = ConversationPage)))]
pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ConversationPage>, ApiError> {
    let mut items = Vec::new();
    for channel in state
        .channels
        .list_by_workspace(&state.org_workspace_id)
        .await?
    {
        if channel.kind == ChannelKind::Conversation
            && participant_ids(&state, &state.org_workspace_id, &channel.id)
                .await?
                .contains(&tenant.workspace_id)
        {
            items.push((channel.created_at, dto(&state, &tenant, channel).await?));
        }
    }
    for assignment in state
        .assignments
        .records()
        .for_person(&tenant.user_id, 500)
        .await?
    {
        if state
            .workspace_members
            .get(&assignment.workspace_id, &tenant.user_id)
            .await?
            .is_none()
        {
            continue;
        }
        if let Some(channel) = state
            .channels
            .get(&assignment.workspace_id, &assignment.channel_id)
            .await?
        {
            items.push((channel.created_at, dto(&state, &tenant, channel).await?));
        }
    }
    items.sort_by(|(a_at, a), (b_at, b)| {
        let a_key = a
            .last_message
            .as_ref()
            .map_or((*a_at, a.id.as_str()), |m| (m.created_at, m.id.as_str()));
        let b_key = b
            .last_message
            .as_ref()
            .map_or((*b_at, b.id.as_str()), |m| (m.created_at, m.id.as_str()));
        b_key.cmp(&a_key)
    });
    Ok(Json(ConversationPage {
        items: items
            .into_iter()
            .map(|(_, conversation)| conversation)
            .collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/conversations", request_body = StartConversationRequest, responses((status = 200, body = ConversationDto)))]
pub async fn start_conversation(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<StartConversationRequest>,
) -> Result<Json<ConversationDto>, ApiError> {
    let people = people(&state, &tenant).await?;
    let mut ids = vec![tenant.workspace_id.clone()];
    for id in request.workspace_ids {
        if !people.iter().any(|p| p.workspace_id == id) {
            return Err(ApiError::not_found("colleague"));
        }
        ids.push(WorkspaceId::from(id));
    }
    ids.sort();
    ids.dedup();
    if ids.len() < 2 {
        return Err(ApiError::validation("select at least one colleague"));
    }
    let channel = state
        .channels
        .get_or_create_conversation(&state.org_workspace_id, &ids)
        .await?;
    Ok(Json(dto(&state, &tenant, channel).await?))
}

#[utoipa::path(get, path = "/api/v1/conversations/{conversation_id}/messages", params(("conversation_id" = String, Path), TimelineQuery), responses((status = 200, body = ConversationMessages), (status = 404, body = crate::error::ErrorBody)))]
pub async fn list_conversation_messages(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<ConversationMessages>, ApiError> {
    let id = ChannelId::from(id);
    entrance(&state, &tenant, &id).await?;
    let storage = storage(&state, &id).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let before = query.before.map(MessageId::from);
    let rows = state
        .messages
        .list_channel(&storage, &id, before.as_ref(), limit + 1)
        .await?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for message in rows.into_iter().take(limit as usize) {
        items.push(
            MessageDto::read(
                &state,
                crate::channels::readable_message(&state, message).await?,
            )
            .await?,
        );
    }
    let next_cursor = if more {
        items.last().map(|m| m.id.clone())
    } else {
        None
    };
    Ok(Json(ConversationMessages { items, next_cursor }))
}

#[utoipa::path(post, path = "/api/v1/conversations/{conversation_id}/messages", params(("conversation_id" = String, Path)), request_body = ConversationMessageRequest, responses((status = 201, body = MessageDto), (status = 200, body = MessageDto), (status = 404, body = crate::error::ErrorBody)))]
pub async fn send_conversation_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<ConversationMessageRequest>,
) -> Result<(StatusCode, Json<MessageDto>), ApiError> {
    let id = ChannelId::from(id);
    let participants = entrance(&state, &tenant, &id).await?;
    let storage = storage(&state, &id).await?;
    let assignment = state.assignments.records().by_channel(&id).await?;
    let sender = assignment.as_ref().map_or_else(
        || tenant.workspace_id.clone(),
        |a| {
            if a.assignee == tenant.user_id {
                a.private_workspace_id.clone()
            } else {
                a.workspace_id.clone()
            }
        },
    );
    if request.text.trim().is_empty() || request.pending_id.trim().is_empty() {
        return Err(ApiError::validation(
            "text and pending_id must not be empty",
        ));
    }
    let parent_message_id = match request.parent_message_id {
        Some(parent) => {
            let parent = state
                .messages
                .get(&storage, &MessageId::from(parent))
                .await?
                .filter(|m| m.channel_id == id)
                .ok_or_else(|| {
                    ApiError::validation("reply must name a message in this conversation")
                })?;
            Some(parent.id)
        }
        None => None,
    };
    let now = state.clock.now_ms();
    let message = Message {
        for_person_only: request.for_person_only,
        author_workspace_id: None,
        id: MessageId::generate(),
        workspace_id: storage.clone(),
        channel_id: id.clone(),
        parent_message_id,
        author: Author::Person(tenant.user_id.clone()),
        run_id: None,
        status: MessageStatus::Complete,
        blocks: vec![Block::markdown(&request.text)],
        text_content: request.text,
        pending_id: Some(request.pending_id),
        created_at: now,
        completed_at: Some(now),
    };
    let outcome = state.messages.insert(&message).await?;
    let (status, message) = match outcome {
        SendOutcome::Created(m) => (StatusCode::CREATED, m),
        SendOutcome::Deduplicated(m) => (StatusCode::OK, m),
    };
    let message = MessageDto::read(&state, message).await?;
    if status == StatusCode::CREATED {
        if let Some(assignment) = state.assignments.records().by_channel(&id).await?
            && assignment.assignee == tenant.user_id
            && assignment.state == pagis_core::MailAssignmentState::Escalated
        {
            state
                .assignments
                .finish_by_person(&tenant.user_id, &assignment.id)
                .await?;
        }
        for workspace_id in participants {
            state
                .bus
                .publish(NewEvent {
                    workspace_id,
                    event_type: "conversation.message".into(),
                    agent_id: None,
                    run_id: None,
                    channel_id: Some(id.clone()),
                    payload: serde_json::json!({ "conversation_id": id, "message_id": message.id, "storage_workspace_id": storage, "sender_workspace_id": sender }),
                })
                .await?;
        }
    }
    Ok((status, Json(message)))
}
