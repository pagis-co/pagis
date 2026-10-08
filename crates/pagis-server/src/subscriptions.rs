//! Event Subscription reads and management (ADR-0006).
//!
//! A list or detail read answers what the user has to decide from: the
//! rule's current revision and state, who created it, where it posts,
//! its source and filter, its collector health, and its paginated
//! Incoming Event and Wake-up history.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{
    AgentId, ChannelId, ConnectionId, CreatorKind, EventSource, EventSubscriptionId,
    IncomingEventId, MessageId, SourceBatch, WakeupId, now_ms,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::schedules::{PageQuery, WakeupDto, WakeupPage, double_option, page, trigger_error};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 100;

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSubscriptionRequest {
    pub agent_id: String,
    pub connection_id: String,
    /// The qualified kind, for example `mail.message_received`.
    pub event_kind: String,
    pub name: String,
    pub instruction: String,
    pub channel_id: String,
    pub root_message_id: Option<String>,
    /// The typed filter for this event kind.
    pub filter: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSubscriptionRequest {
    /// `pause`, `resume`, `archive`, or `edit`.
    pub action: String,
    pub name: Option<String>,
    pub instruction: Option<String>,
    pub channel_id: Option<String>,
    /// The Thread root. Absent keeps the Thread while the Channel stays
    /// the same, and drops it when the Channel changes. `null` clears
    /// the Thread.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub root_message_id: Option<Option<String>>,
    pub filter: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionDto {
    pub id: String,
    pub workspace_id: String,
    pub agent_id: String,
    pub connection_id: String,
    pub event_kind: String,
    /// The Capability Manifest version the rule pins.
    pub source_version: String,
    pub name: String,
    pub instruction: String,
    pub channel_id: String,
    pub root_message_id: Option<String>,
    pub filter: serde_json::Value,
    pub creator: String,
    pub state: String,
    pub revision: u32,
    pub approved_revision: Option<u32>,
    pub watermark_at: Option<i64>,
    /// Why a blocked rule is blocked.
    pub blocked_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

/// The Connection of a rule or an event that this surface serves. The
/// Trigger module leaves out every rule of a Coding Session source, and
/// a Connection rule matches the events of its Connection alone, so each
/// one here has a Connection (ADR-0033).
fn connection_of(source: &EventSource) -> String {
    source
        .connection_id()
        .map(ToString::to_string)
        .unwrap_or_default()
}

impl From<pagis_core::EventSubscription> for SubscriptionDto {
    fn from(value: pagis_core::EventSubscription) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            agent_id: value.agent_id.to_string(),
            connection_id: connection_of(&value.source),
            event_kind: value.event_kind,
            source_version: value.source_version,
            name: value.name,
            instruction: value.instruction,
            channel_id: value.channel_id.to_string(),
            root_message_id: value.root_message_id.map(|id| id.to_string()),
            filter: value.filter,
            creator: value.creator.as_str().to_string(),
            state: value.state.as_str().to_string(),
            revision: value.revision,
            approved_revision: value.approved_revision,
            watermark_at: value.watermark_at,
            blocked_reason: value
                .blocked_reason
                .map(|reason| reason.as_str().to_string()),
            created_at: value.created_at,
            updated_at: value.updated_at,
            archived_at: value.archived_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionPage {
    pub items: Vec<SubscriptionDto>,
    pub next_cursor: Option<String>,
}

/// One collection pass, as the collector health card reads it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceBatchDto {
    pub id: String,
    pub collected_at: i64,
    pub collected_count: u32,
    pub stored_count: u32,
    pub wakeup_count: u32,
    pub outcome: String,
    /// A stable failure code, never provider text.
    pub detail: Option<String>,
}

impl From<SourceBatch> for SourceBatchDto {
    fn from(value: SourceBatch) -> Self {
        Self {
            id: value.id.to_string(),
            collected_at: value.collected_at,
            collected_count: value.collected_count,
            stored_count: value.stored_count,
            wakeup_count: value.wakeup_count,
            outcome: value.outcome.as_str().to_string(),
            detail: value.detail,
        }
    }
}

/// The subscription with everything a detail view needs beside it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionDetailDto {
    #[serde(flatten)]
    pub subscription: SubscriptionDto,
    /// The most recent collection pass on this source.
    pub last_collection: Option<SourceBatchDto>,
    /// The most recent pass that did not fail.
    pub last_successful_collection: Option<SourceBatchDto>,
    /// The newest Incoming Event this rule matched.
    pub last_matched_event: Option<IncomingEventDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IncomingEventDto {
    pub id: String,
    pub connection_id: String,
    pub event_kind: String,
    pub provider_event_id: String,
    /// Untrusted normalized provider metadata.
    pub metadata: serde_json::Value,
    pub occurred_at: i64,
    pub received_at: i64,
}

impl From<pagis_core::IncomingEvent> for IncomingEventDto {
    fn from(value: pagis_core::IncomingEvent) -> Self {
        Self {
            id: value.id.to_string(),
            connection_id: connection_of(&value.source),
            event_kind: value.event_kind,
            provider_event_id: value.provider_event_id,
            metadata: value.metadata,
            occurred_at: value.occurred_at,
            received_at: value.received_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IncomingEventPage {
    pub items: Vec<IncomingEventDto>,
    pub next_cursor: Option<String>,
}

#[utoipa::path(post, path = "/api/v1/event-subscriptions", request_body = CreateSubscriptionRequest, responses(
    (status = 201, body = SubscriptionDto),
    (status = 401, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
    (status = 422, body = crate::error::ErrorBody),
))]
pub async fn create_subscription(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(body): Json<CreateSubscriptionRequest>,
) -> Result<(StatusCode, Json<SubscriptionDto>), ApiError> {
    let subscription = state
        .trigger
        .create_subscription(pagis_trigger::NewSubscription {
            workspace_id: tenant.workspace_id.clone(),
            agent_id: AgentId::from(body.agent_id),
            source: EventSource::connection(ConnectionId::from(body.connection_id)),
            event_kind: body.event_kind,
            name: body.name,
            instruction: body.instruction,
            channel_id: ChannelId::from(body.channel_id),
            root_message_id: body.root_message_id.map(MessageId::from),
            filter: body.filter.unwrap_or_else(|| serde_json::json!({})),
            creator: CreatorKind::User,
            now: now_ms(),
        })
        .await
        .map_err(trigger_error)?;
    Ok((StatusCode::CREATED, Json(subscription.into())))
}

#[utoipa::path(get, path = "/api/v1/event-subscriptions", params(PageQuery), responses(
    (status = 200, body = SubscriptionPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_subscriptions(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<PageQuery>,
) -> Result<Json<SubscriptionPage>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let subscriptions = state
        .trigger
        .list_subscriptions(
            &tenant.workspace_id,
            None,
            query.before.map(EventSubscriptionId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(subscriptions, limit, |item| item.id.to_string());
    Ok(Json(SubscriptionPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/event-subscriptions/{subscription_id}", params(("subscription_id" = String, Path)), responses(
    (status = 200, body = SubscriptionDetailDto),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn get_subscription(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(subscription_id): Path<String>,
) -> Result<Json<SubscriptionDetailDto>, ApiError> {
    let subscription = owned(&state, &tenant, EventSubscriptionId::from(subscription_id)).await?;
    let Some(connection_id) = subscription.source.connection_id() else {
        return Err(ApiError::not_found("event subscription"));
    };
    let (last, succeeded) = state
        .trigger
        .collector_health(
            &tenant.workspace_id,
            connection_id,
            &subscription.event_kind,
        )
        .await
        .map_err(trigger_error)?;
    let last_matched_event = state
        .trigger
        .list_subscription_events(&tenant.workspace_id, &subscription.id, None, 1)
        .await
        .map_err(trigger_error)?
        .into_iter()
        .next();
    Ok(Json(SubscriptionDetailDto {
        subscription: subscription.into(),
        last_collection: last.map(Into::into),
        last_successful_collection: succeeded.map(Into::into),
        last_matched_event: last_matched_event.map(Into::into),
    }))
}

#[utoipa::path(post, path = "/api/v1/event-subscriptions/{subscription_id}", params(("subscription_id" = String, Path)), request_body = UpdateSubscriptionRequest, responses(
    (status = 200, body = SubscriptionDto),
    (status = 404, body = crate::error::ErrorBody),
    (status = 422, body = crate::error::ErrorBody),
))]
pub async fn update_subscription(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(subscription_id): Path<String>,
    Json(body): Json<UpdateSubscriptionRequest>,
) -> Result<Json<SubscriptionDto>, ApiError> {
    let subscription = owned(&state, &tenant, EventSubscriptionId::from(subscription_id)).await?;
    let action = match body.action.as_str() {
        "edit" => pagis_trigger::SubscriptionAction::Edit {
            name: body.name,
            instruction: body.instruction,
            channel_id: body.channel_id.map(ChannelId::from),
            root_message_id: body.root_message_id.map(|root| root.map(MessageId::from)),
            filter: body.filter,
        },
        other => pagis_trigger::SubscriptionAction::parse(other)
            .ok_or_else(|| ApiError::validation(format!("`{other}` is not an action")))?,
    };
    let updated = state
        .trigger
        .update_subscription(&tenant.workspace_id, &subscription.id, action, now_ms())
        .await
        .map_err(trigger_error)?;
    Ok(Json(updated.into()))
}

#[utoipa::path(get, path = "/api/v1/event-subscriptions/{subscription_id}/events", params(("subscription_id" = String, Path), PageQuery), responses(
    (status = 200, body = IncomingEventPage),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn list_events(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(subscription_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<IncomingEventPage>, ApiError> {
    let subscription = owned(&state, &tenant, EventSubscriptionId::from(subscription_id)).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let events = state
        .trigger
        .list_subscription_events(
            &tenant.workspace_id,
            &subscription.id,
            query.before.map(IncomingEventId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(events, limit, |item| item.id.to_string());
    Ok(Json(IncomingEventPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/event-subscriptions/{subscription_id}/wakeups", params(("subscription_id" = String, Path), PageQuery), responses(
    (status = 200, body = WakeupPage),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn list_subscription_wakeups(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(subscription_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<WakeupPage>, ApiError> {
    let subscription = owned(&state, &tenant, EventSubscriptionId::from(subscription_id)).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let wakeups = state
        .trigger
        .list_subscription_wakeups(
            &tenant.workspace_id,
            &subscription.id,
            query.before.map(WakeupId::from).as_ref(),
            limit + 1,
        )
        .await
        .map_err(trigger_error)?;
    let (items, next_cursor) = page(wakeups, limit, |item| item.id.to_string());
    Ok(Json(WakeupPage {
        items: items.into_iter().map(WakeupDto::from).collect(),
        next_cursor,
    }))
}

async fn owned(
    state: &AppState,
    tenant: &Tenant,
    id: EventSubscriptionId,
) -> Result<pagis_core::EventSubscription, ApiError> {
    state
        .trigger
        .get_subscription(&tenant.workspace_id, &id)
        .await
        .map_err(trigger_error)?
        .ok_or_else(|| ApiError::not_found("event subscription"))
}
