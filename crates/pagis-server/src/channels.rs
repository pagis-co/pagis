//! Channels and messages REST: create channel, cursor-paginated
//! timeline with thread rollups, thread fetch, send with `pending_id`
//! dedup.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{
    AgentId, AgentStatus, Artifact, ArtifactId, AuthorKind, Block, Channel, ChannelId, ChannelKind,
    ChannelParticipant, KnownBlock, Message, MessageId, MessageStatus, NewEvent, ParticipantId,
    ParticipantKind, SendOutcome, TimelineEntry, blocks_text, now_ms,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateChannelRequest {
    pub title: Option<String>,
    /// The agent participants of the new group channel.
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelDto {
    pub id: String,
    pub workspace_id: String,
    /// `dm` or `group`.
    pub kind: String,
    pub title: Option<String>,
    /// The agent participants.
    pub agent_ids: Vec<String>,
    /// True when the user is a participant. A channel two agents opened
    /// between themselves has agents alone, and the user reads it
    /// without writing in it (ADR-0003).
    pub user_member: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ChannelDto {
    fn new(c: Channel, agent_ids: Vec<AgentId>, user_member: bool) -> Self {
        ChannelDto {
            id: c.id.to_string(),
            workspace_id: c.workspace_id.to_string(),
            kind: c.kind.as_str().to_string(),
            title: c.title,
            agent_ids: agent_ids.into_iter().map(|a| a.to_string()).collect(),
            user_member,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessageDto {
    pub id: String,
    pub channel_id: String,
    pub parent_message_id: Option<String>,
    /// `user`, `agent`, or `system`.
    pub author_kind: String,
    pub author_agent_id: Option<String>,
    pub run_id: Option<String>,
    /// `streaming`, `complete`, or `failed`.
    pub status: String,
    /// The typed block array (ADR-0004). A block type this daemon does
    /// not know is served unchanged and renders as the UI fallback, so
    /// the union is open at both ends.
    #[schema(value_type = Vec<KnownBlock>)]
    pub blocks: Vec<Block>,
    pub text_content: String,
    /// The client-generated send dedup key; present on user sends.
    pub pending_id: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

impl From<Message> for MessageDto {
    fn from(m: Message) -> Self {
        MessageDto {
            id: m.id.to_string(),
            channel_id: m.channel_id.to_string(),
            parent_message_id: m.parent_message_id.map(|p| p.to_string()),
            author_kind: m.author_kind.as_str().to_string(),
            author_agent_id: m.author_agent_id.map(|a| a.to_string()),
            run_id: m.run_id.map(|r| r.to_string()),
            status: m.status.as_str().to_string(),
            blocks: m.blocks,
            text_content: m.text_content,
            pending_id: m.pending_id,
            created_at: m.created_at,
            completed_at: m.completed_at,
        }
    }
}

/// A top-level message with its thread rollup, computed in the
/// timeline query.
#[derive(Debug, Serialize, ToSchema)]
pub struct TimelineMessageDto {
    #[serde(flatten)]
    #[schema(inline)]
    pub message: MessageDto,
    /// The count of replies in this message's thread.
    pub reply_count: u32,
    /// The `created_at` of the newest reply.
    pub last_reply_at: Option<i64>,
    /// The different authors of the replies, the newest reply first,
    /// at most three.
    pub reply_authors: Vec<ReplyAuthorDto>,
}

/// One author of the replies in a thread.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReplyAuthorDto {
    /// `user` or `agent`.
    pub author_kind: String,
    pub author_agent_id: Option<String>,
}

impl From<TimelineEntry> for TimelineMessageDto {
    fn from(entry: TimelineEntry) -> Self {
        TimelineMessageDto {
            message: entry.message.into(),
            reply_count: entry.reply_count,
            last_reply_at: entry.last_reply_at,
            reply_authors: entry
                .reply_authors
                .into_iter()
                .map(|author| ReplyAuthorDto {
                    author_kind: author.kind.as_str().to_string(),
                    author_agent_id: author.agent_id.map(|id| id.to_string()),
                })
                .collect(),
        }
    }
}

/// A derived DM pointer entry: this DM's agent posted in
/// another channel. Computed at query time from that message; `id` is
/// the source message's id, so the page cursor stays uniform.
#[derive(Debug, Serialize, ToSchema)]
pub struct PointerDto {
    /// The source message's id — also this item's cursor position.
    pub id: String,
    /// The channel the agent posted in.
    pub channel_id: String,
    pub channel_title: Option<String>,
    pub agent_id: String,
    /// The source message's text, for the pointer's preview line.
    pub preview: String,
    pub created_at: i64,
}

/// One timeline entry: a message, or a derived DM pointer.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineItem {
    Message(TimelineMessageDto),
    Pointer(PointerDto),
}

impl TimelineItem {
    /// The ULID this item sorts and paginates by.
    fn cursor_id(&self) -> &str {
        match self {
            TimelineItem::Message(m) => &m.message.id,
            TimelineItem::Pointer(p) => &p.id,
        }
    }
}

/// One thread: the root message plus its replies, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct ThreadDto {
    pub root: MessageDto,
    pub replies: Vec<MessageDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TimelinePage {
    pub items: Vec<TimelineItem>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TimelineQuery {
    /// Exclusive ULID cursor: only messages older than this id.
    pub before: Option<String>,
    /// Page size, default 50, max 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SendMessageRequest {
    /// Client-generated dedup key; resending the same value in the same
    /// channel returns the original message instead of a duplicate.
    pub pending_id: String,
    pub text: String,
    /// Reply target; must be a top-level message in the same channel.
    pub parent_message_id: Option<String>,
    /// Pre-uploaded attachments, rendered as `image`/`file`
    /// blocks after the text.
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChannelPage {
    pub items: Vec<ChannelDto>,
}

#[utoipa::path(
    get,
    path = "/api/v1/channels",
    responses(
        (status = 200, body = ChannelPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ChannelPage>, ApiError> {
    let channels = state
        .channels
        .list_by_workspace(&tenant.workspace_id)
        .await?;
    let mut items = Vec::with_capacity(channels.len());
    for channel in channels {
        let participants = state
            .participants
            .list_for_channel(&tenant.workspace_id, &channel.id)
            .await?;
        items.push(ChannelDto::new(
            channel,
            agent_participants(&participants),
            has_user(&participants),
        ));
    }
    Ok(Json(ChannelPage { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/channels",
    request_body = CreateChannelRequest,
    responses(
        (status = 201, body = ChannelDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<CreateChannelRequest>,
) -> Result<(StatusCode, Json<ChannelDto>), ApiError> {
    // User-created channels are group channels; DMs come from
    // agent creation. Every listed agent must exist and be active.
    let mut agent_ids = Vec::new();
    for agent_id in &request.agent_ids {
        let agent_id = AgentId::from(agent_id.clone());
        let agent = state
            .agent_store
            .get(&tenant.workspace_id, &agent_id)
            .await?
            .ok_or_else(|| ApiError::validation("agent does not exist"))?;
        if agent.status == AgentStatus::Archived {
            return Err(ApiError::validation("agent is archived"));
        }
        if !agent_ids.contains(&agent_id) {
            agent_ids.push(agent_id);
        }
    }

    let now = now_ms();
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        kind: ChannelKind::Group,
        title: request.title,
        created_at: now,
        updated_at: now,
    };
    state.channels.create(&channel).await?;
    state
        .participants
        .create(&ChannelParticipant {
            id: ParticipantId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            channel_id: channel.id.clone(),
            kind: ParticipantKind::User,
            agent_id: None,
            joined_at: now,
        })
        .await?;
    for agent_id in &agent_ids {
        state
            .participants
            .create(&ChannelParticipant {
                id: ParticipantId::generate(),
                workspace_id: tenant.workspace_id.clone(),
                channel_id: channel.id.clone(),
                kind: ParticipantKind::Agent,
                agent_id: Some(agent_id.clone()),
                joined_at: now,
            })
            .await?;
    }
    state
        .bus
        .publish(NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "channel.created".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: Some(channel.id.clone()),
            payload: serde_json::json!({ "title": channel.title }),
        })
        .await?;
    // The user creates their own channels, so they are a member of it.
    Ok((
        StatusCode::CREATED,
        Json(ChannelDto::new(channel, agent_ids, true)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/messages",
    params(("channel_id" = String, Path,), TimelineQuery),
    responses(
        (status = 200, body = TimelinePage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(channel_id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<TimelinePage>, ApiError> {
    let channel_id = ChannelId::from(channel_id);
    let channel = state
        .channels
        .get(&tenant.workspace_id, &channel_id)
        .await?
        .ok_or_else(|| ApiError::not_found("channel"))?;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let before = query.before.map(MessageId::from);
    let entries = state
        .messages
        .list_top_level(&tenant.workspace_id, &channel_id, before.as_ref(), limit)
        .await?;
    let mut items = Vec::with_capacity(entries.len());
    for mut entry in entries {
        entry.message = readable_message(&state, entry.message).await?;
        items.push(TimelineItem::Message(entry.into()));
    }

    // DM pointer entries: in the user's DM with one agent,
    // interleave that agent's messages from other channels, derived at
    // query time under the same ULID cursor.
    if let Some(agent_id) = pointer_agent(&state, &channel).await? {
        let sources = state
            .messages
            .list_agent_elsewhere(
                &tenant.workspace_id,
                &agent_id,
                &channel_id,
                before.as_ref(),
                limit,
            )
            .await?;
        for source in sources {
            let source = readable_message(&state, source).await?;
            let title = match state
                .channels
                .get(&tenant.workspace_id, &source.channel_id)
                .await?
            {
                Some(channel) => channel.title,
                None => None,
            };
            items.push(TimelineItem::Pointer(PointerDto {
                id: source.id.to_string(),
                channel_id: source.channel_id.to_string(),
                channel_title: title,
                agent_id: agent_id.to_string(),
                preview: source.text_content,
                created_at: source.created_at,
            }));
        }
        // Merge to one newest-first page; both sources are ULID-keyed,
        // so the truncated page keeps a uniform `before` cursor.
        items.sort_by(|a, b| b.cursor_id().cmp(a.cursor_id()));
        items.truncate(limit as usize);
    }
    Ok(Json(TimelinePage { items }))
}

/// Current source permission applies to every stored-text rendering.
/// The stamp names what its author could read, so the author's own
/// live Grants settle it (ADR-0004).
pub(crate) async fn message_is_readable(
    state: &AppState,
    message: &Message,
) -> Result<bool, ApiError> {
    Ok(
        pagis_core::message_source_is_live(state.messages.as_ref(), state.grants.as_ref(), message)
            .await?,
    )
}

/// Keep timeline identity and pagination stable while withholding expired source prose.
async fn readable_message(state: &AppState, mut message: Message) -> Result<Message, ApiError> {
    if !message_is_readable(state, &message).await? {
        message.text_content =
            "This message is unavailable because its source access cannot be verified.".into();
        message.blocks = vec![Block::markdown(&message.text_content)];
    }
    Ok(message)
}

/// The agents of one channel, in the order they joined.
fn agent_participants(participants: &[ChannelParticipant]) -> Vec<AgentId> {
    participants
        .iter()
        .filter_map(|p| p.agent_id.clone())
        .collect()
}

/// True when the user is one of these participants.
fn has_user(participants: &[ChannelParticipant]) -> bool {
    participants.iter().any(|p| p.kind == ParticipantKind::User)
}

/// The agent whose outside messages this DM derives pointers for:
/// the channel is a DM between the user and exactly one agent.
async fn pointer_agent(state: &AppState, channel: &Channel) -> Result<Option<AgentId>, ApiError> {
    if channel.kind != ChannelKind::Dm {
        return Ok(None);
    }
    let participants = state
        .participants
        .list_for_channel(&channel.workspace_id, &channel.id)
        .await?;
    if !has_user(&participants) {
        return Ok(None);
    }
    match agent_participants(&participants).as_slice() {
        [agent_id] => Ok(Some(agent_id.clone())),
        _ => Ok(None),
    }
}

/// The block array of a user send (ADR-0004): one `markdown` block for
/// the text, then one `image` or `file` block per attachment.
fn user_blocks(text: &str, attachments: &[Artifact]) -> Vec<Block> {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(Block::markdown(text));
    }
    for artifact in attachments {
        if artifact.is_image() {
            blocks.push(Block::image(
                artifact.id.as_str(),
                artifact.filename.clone(),
            ));
        } else {
            blocks.push(Block::file(
                artifact.id.as_str(),
                artifact
                    .filename
                    .clone()
                    .unwrap_or_else(|| "file".to_string()),
                Some(artifact.mime.clone()),
                Some(artifact.size_bytes),
            ));
        }
    }
    blocks
}

#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/threads/{root_message_id}",
    params(("channel_id" = String, Path,), ("root_message_id" = String, Path,)),
    responses(
        (status = 200, body = ThreadDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn get_thread(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((channel_id, root_message_id)): Path<(String, String)>,
) -> Result<Json<ThreadDto>, ApiError> {
    let channel_id = ChannelId::from(channel_id);
    state
        .channels
        .get(&tenant.workspace_id, &channel_id)
        .await?
        .ok_or_else(|| ApiError::not_found("channel"))?;
    let root_id = MessageId::from(root_message_id);
    let root = state
        .messages
        .get(&tenant.workspace_id, &root_id)
        .await?
        .filter(|root| root.channel_id == channel_id)
        .ok_or_else(|| ApiError::not_found("thread"))?;
    if root.parent_message_id.is_some() {
        return Err(ApiError::validation(
            "threads are one level deep; the root is itself a reply",
        ));
    }
    let mut replies = Vec::new();
    for message in state
        .messages
        .list_thread(&tenant.workspace_id, &root_id)
        .await?
    {
        if message.id != root.id {
            replies.push(readable_message(&state, message).await?.into());
        }
    }
    let root = readable_message(&state, root).await?;
    Ok(Json(ThreadDto {
        root: root.into(),
        replies,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/messages/{message_id}",
    params(("channel_id" = String, Path,), ("message_id" = String, Path,)),
    responses(
        (status = 200, body = MessageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((channel_id, message_id)): Path<(String, String)>,
) -> Result<Json<MessageDto>, ApiError> {
    let channel_id = ChannelId::from(channel_id);
    let message = state
        .messages
        .get(&tenant.workspace_id, &MessageId::from(message_id))
        .await?
        .filter(|message| message.channel_id == channel_id)
        .ok_or_else(|| ApiError::not_found("message"))?;
    Ok(Json(readable_message(&state, message).await?.into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/messages",
    params(("channel_id" = String, Path,)),
    request_body = SendMessageRequest,
    responses(
        (status = 201, description = "Message created", body = MessageDto),
        (status = 200, description = "Duplicate pending_id; the original message", body = MessageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, description = "The channel is between agents", body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(channel_id): Path<String>,
    Json(request): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<MessageDto>), ApiError> {
    if request.pending_id.is_empty() {
        return Err(ApiError::validation("pending_id must not be empty"));
    }
    if request.text.trim().is_empty() && request.artifact_ids.is_empty() {
        return Err(ApiError::validation(
            "message needs text or at least one artifact",
        ));
    }
    let channel_id = ChannelId::from(channel_id);
    state
        .channels
        .get(&tenant.workspace_id, &channel_id)
        .await?
        .ok_or_else(|| ApiError::not_found("channel"))?;
    // A channel the agents opened between themselves is read-only for
    // the user: a message in it would start an agent-to-agent loop the
    // user has no part in (ADR-0003).
    let participants = state
        .participants
        .list_for_channel(&tenant.workspace_id, &channel_id)
        .await?;
    if !has_user(&participants) {
        return Err(ApiError::forbidden(
            "this conversation is between agents; the user reads it only",
        ));
    }

    let parent_message_id = match request.parent_message_id {
        None => None,
        Some(parent_id) => {
            let parent = state
                .messages
                .get(&tenant.workspace_id, &MessageId::from(parent_id))
                .await?
                .ok_or_else(|| ApiError::validation("parent message does not exist"))?;
            if parent.channel_id != channel_id {
                return Err(ApiError::validation("parent message is in another channel"));
            }
            if parent.parent_message_id.is_some() {
                return Err(ApiError::validation(
                    "threads are one level deep; the parent is itself a reply",
                ));
            }
            Some(parent.id)
        }
    };

    let mut attachments = Vec::new();
    for artifact_id in &request.artifact_ids {
        let artifact = state
            .artifacts
            .get(&tenant.workspace_id, &ArtifactId::from(artifact_id.clone()))
            .await?
            .ok_or_else(|| ApiError::validation("artifact does not exist"))?;
        attachments.push(artifact);
    }

    // Use the daemon's logical time for the stored owner message.
    let now = state.clock.now_ms();
    let blocks = user_blocks(&request.text, &attachments);
    let message = Message {
        id: MessageId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        channel_id: channel_id.clone(),
        parent_message_id,
        author_kind: AuthorKind::User,
        author_agent_id: None,
        run_id: None,
        status: MessageStatus::Complete,
        text_content: blocks_text(&blocks),
        blocks,
        pending_id: Some(request.pending_id),
        created_at: now,
        completed_at: Some(now),
    };

    match state.messages.insert(&message).await? {
        SendOutcome::Created(message) => {
            state
                .bus
                .publish(NewEvent {
                    workspace_id: tenant.workspace_id.clone(),
                    event_type: "message.completed".to_string(),
                    agent_id: None,
                    run_id: None,
                    channel_id: Some(channel_id),
                    payload: serde_json::json!({
                        "message_id": message.id.as_str(),
                        "parent_message_id": message.parent_message_id.as_ref().map(|p| p.as_str()),
                        "author_kind": message.author_kind,
                    }),
                })
                .await?;
            Ok((StatusCode::CREATED, Json(message.into())))
        }
        SendOutcome::Deduplicated(original) => Ok((StatusCode::OK, Json(original.into()))),
    }
}
