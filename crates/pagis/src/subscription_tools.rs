//! The Event Subscription core tools (ADR-0006).
//!
//! These three tools join three modules the agent loop does not know
//! about — the Trigger module, the Connections, and the Channels — so
//! they execute here, in the composition crate, rather than inside the
//! agent's own tool runtime.
//!
//! Two rules keep an Agent inside its own authority. The tools omit
//! `agent_id`, so an Agent acts only for itself; and they name a
//! Connection by its alias and a conversation by its title, never by a
//! database id the model could guess.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CoreTool, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{
    AgentId, ChannelId, ChannelStore, Connection, ConnectionStore, CreatorKind,
    EventSubscriptionId, WorkspaceId, now_ms,
};
use pagis_trigger::{NewSubscription, SubscriptionAction, Trigger, TriggerError};

/// The `channel` value that means the Agent's DM with the user.
const USER_CHANNEL: &str = "user";

pub struct SubscriptionToolRuntime {
    trigger: Arc<Trigger>,
    connections: Arc<dyn ConnectionStore>,
    channels: Arc<dyn ChannelStore>,
}

impl SubscriptionToolRuntime {
    pub fn new(
        trigger: Arc<Trigger>,
        connections: Arc<dyn ConnectionStore>,
        channels: Arc<dyn ChannelStore>,
    ) -> Self {
        Self {
            trigger,
            connections,
            channels,
        }
    }

    /// True when this call belongs to this executor.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(
            route,
            ToolRoute::Core {
                tool: CoreTool::EventSubscriptionCreate
                    | CoreTool::EventSubscriptionList
                    | CoreTool::EventSubscriptionUpdate
            }
        )
    }

    async fn create(&self, call: &AuthorizedCall) -> ToolResult {
        let alias = call.arguments["connection"].as_str().unwrap_or_default();
        let connection = match self.connection(&call.workspace_id, alias).await {
            Ok(Some(connection)) => connection,
            Ok(None) => {
                return ToolResult::error(
                    "invalid_request",
                    format!("there is no connection named `{alias}`"),
                );
            }
            Err(result) => return result,
        };
        let channel_id = match self
            .channel(
                &call.workspace_id,
                &call.agent_id,
                call.arguments["channel"].as_str(),
            )
            .await
        {
            Ok(channel_id) => channel_id,
            Err(result) => return result,
        };
        let filter = match call.arguments.get("filter") {
            Some(filter) if filter.is_object() => filter.clone(),
            Some(serde_json::Value::Null) | None => serde_json::json!({}),
            Some(_) => {
                return ToolResult::error("invalid_request", "filter must be an object");
            }
        };
        let created = self
            .trigger
            .create_subscription(NewSubscription {
                workspace_id: call.workspace_id.clone(),
                agent_id: call.agent_id.clone(),
                connection_id: connection.id,
                event_kind: call.arguments["event_kind"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                name: call.arguments["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                instruction: call.arguments["instruction"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                channel_id,
                root_message_id: None,
                filter,
                creator: CreatorKind::Agent,
                now: now_ms(),
            })
            .await;
        match created {
            Ok(subscription) => ToolResult::success(
                serde_json::json!({
                    "event_subscription_id": subscription.id.as_str(),
                    "state": subscription.state.as_str(),
                    "event_kind": subscription.event_kind,
                    "connection": alias,
                })
                .to_string(),
            ),
            Err(error) => trigger_error(error),
        }
    }

    async fn list(&self, call: &AuthorizedCall) -> ToolResult {
        match self
            .trigger
            .list_subscriptions(&call.workspace_id, Some(&call.agent_id), None, 100)
            .await
        {
            Ok(subscriptions) => {
                let items: Vec<serde_json::Value> = subscriptions
                    .into_iter()
                    .map(|subscription| {
                        serde_json::json!({
                            "event_subscription_id": subscription.id.as_str(),
                            "name": subscription.name,
                            "event_kind": subscription.event_kind,
                            "instruction": subscription.instruction,
                            "filter": subscription.filter,
                            "state": subscription.state.as_str(),
                            "revision": subscription.revision,
                        })
                    })
                    .collect();
                ToolResult::success(serde_json::json!({"items": items}).to_string())
            }
            Err(error) => trigger_error(error),
        }
    }

    async fn update(&self, call: &AuthorizedCall) -> ToolResult {
        let id = EventSubscriptionId::from(
            call.arguments["event_subscription_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        let owned = match self.trigger.get_subscription(&call.workspace_id, &id).await {
            Ok(Some(subscription)) if subscription.agent_id == call.agent_id => subscription,
            // An Agent manages its own rules only. A rule that belongs
            // to the user or to another Agent reads as absent.
            Ok(_) => {
                return ToolResult::error("not_found", "there is no such event subscription");
            }
            Err(error) => return trigger_error(error),
        };
        let name = call.arguments["action"].as_str().unwrap_or_default();
        let action = match name {
            "edit" => SubscriptionAction::Edit {
                name: call.arguments["name"].as_str().map(str::to_string),
                instruction: call.arguments["instruction"].as_str().map(str::to_string),
                channel_id: None,
                root_message_id: None,
                filter: call
                    .arguments
                    .get("filter")
                    .filter(|filter| filter.is_object())
                    .cloned(),
            },
            other => match SubscriptionAction::parse(other) {
                Some(action) => action,
                None => {
                    return ToolResult::error(
                        "invalid_request",
                        format!("`{other}` is not an action"),
                    );
                }
            },
        };
        // A rule the user wrote is the user's to stop. The Agent may
        // narrow it with an edit and never take it away (ADR-0019).
        if owned.creator == CreatorKind::User
            && matches!(
                action,
                SubscriptionAction::Archive | SubscriptionAction::Pause
            )
        {
            return ToolResult::error(
                "not_allowed",
                "the user wrote this rule. You may narrow its filter, and only the user pauses or deletes it.",
            );
        }
        match self
            .trigger
            .update_subscription(&call.workspace_id, &owned.id, action, now_ms())
            .await
        {
            Ok(subscription) => ToolResult::success(
                serde_json::json!({
                    "event_subscription_id": subscription.id.as_str(),
                    "state": subscription.state.as_str(),
                    "revision": subscription.revision,
                })
                .to_string(),
            ),
            Err(error) => trigger_error(error),
        }
    }

    async fn connection(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<Connection>, ToolResult> {
        self.connections
            .list(workspace_id)
            .await
            .map(|connections| {
                connections
                    .into_iter()
                    .find(|connection| connection.alias == alias)
            })
            .map_err(|error| {
                tracing::error!(%error, "listing connections for a subscription failed");
                ToolResult::error(
                    "temporarily_unavailable",
                    "the connections are not readable",
                )
            })
    }

    /// The conversation a rule posts into. An unnamed target, or the
    /// literal `user`, is the Agent's own DM with the user.
    async fn channel(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        requested: Option<&str>,
    ) -> Result<ChannelId, ToolResult> {
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        if let Some(title) = requested.filter(|value| !value.eq_ignore_ascii_case(USER_CHANNEL)) {
            let channels = self
                .channels
                .list_by_workspace(workspace_id)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "listing channels for a subscription failed");
                    ToolResult::error("temporarily_unavailable", "the channels are not readable")
                })?;
            return channels
                .into_iter()
                .find(|channel| {
                    channel
                        .title
                        .as_deref()
                        .is_some_and(|found| found.eq_ignore_ascii_case(title))
                })
                .map(|channel| channel.id)
                .ok_or_else(|| {
                    ToolResult::error(
                        "invalid_request",
                        format!("there is no channel called `{title}`"),
                    )
                });
        }
        self.channels
            .find_user_dm(workspace_id, agent_id)
            .await
            .map_err(|error| {
                tracing::error!(%error, "loading the user DM for a subscription failed");
                ToolResult::error("temporarily_unavailable", "the channels are not readable")
            })?
            .map(|channel| channel.id)
            .ok_or_else(|| {
                ToolResult::error(
                    "invalid_request",
                    "you have no direct channel with the user",
                )
            })
    }
}

#[async_trait]
impl ToolExecutor for SubscriptionToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match &call.route {
            ToolRoute::Core {
                tool: CoreTool::EventSubscriptionCreate,
            } => self.create(&call).await,
            ToolRoute::Core {
                tool: CoreTool::EventSubscriptionList,
            } => self.list(&call).await,
            ToolRoute::Core {
                tool: CoreTool::EventSubscriptionUpdate,
            } => self.update(&call).await,
            _ => ToolResult::error(
                "invalid_request",
                format!("{} is not an event subscription tool", call.tool_name),
            ),
        }
    }
}

/// A Trigger failure the model can act on stays a validation message;
/// a storage failure does not name the database.
fn trigger_error(error: TriggerError) -> ToolResult {
    match error {
        TriggerError::Store(error) => {
            tracing::error!(%error, "an event subscription write failed");
            ToolResult::error(
                "temporarily_unavailable",
                "the event subscriptions are not writable",
            )
        }
        TriggerError::SubscriptionNotFound => {
            ToolResult::error("not_found", "there is no such event subscription")
        }
        other => ToolResult::error("invalid_request", other.to_string()),
    }
}
