//! Event Subscription management (ADR-0006).
//!
//! Everything here is the durable rule half of incoming events: what
//! the user or an Agent asks for, what a manifest allows them to ask
//! for, and how a rule's state changes. Provider transport stops at
//! `Trigger::ingest`, and Run execution starts at `claim_wakeups`, so
//! neither reaches this file.

use pagis_core::{
    AgentId, ChannelId, ConnectionId, CreatorKind, EventSubscription, EventSubscriptionState,
    MessageId, WorkspaceId,
};

/// What a caller asks for when it creates a subscription. The daemon
/// supplies the source version and the watermark; neither is a field
/// the caller can choose.
#[derive(Debug, Clone)]
pub struct NewSubscription {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub connection_id: ConnectionId,
    pub event_kind: String,
    pub name: String,
    pub instruction: String,
    pub channel_id: ChannelId,
    pub root_message_id: Option<MessageId>,
    pub filter: serde_json::Value,
    pub creator: CreatorKind,
    pub now: i64,
}

/// One validated change to a subscription. A field change makes a new
/// revision; a lifecycle action does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionAction {
    Pause,
    Resume,
    Archive,
    /// Change the fields the caller supplied. `None` leaves a field,
    /// with one exception: a Thread root is valid only inside its own
    /// Channel, so a new Channel with no named root drops the Thread.
    /// `Some(None)` clears the Thread.
    Edit {
        name: Option<String>,
        instruction: Option<String>,
        channel_id: Option<ChannelId>,
        root_message_id: Option<Option<MessageId>>,
        filter: Option<serde_json::Value>,
    },
}

impl SubscriptionAction {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "archive" => Some(Self::Archive),
            _ => None,
        }
    }
}

/// The states a live rule can leave and re-enter without the user.
pub(crate) fn is_manageable(state: EventSubscriptionState) -> bool {
    !matches!(state, EventSubscriptionState::Archived)
}

/// The rule as it reads after one lifecycle action.
pub(crate) fn apply_action(
    subscription: &EventSubscription,
    action: &SubscriptionAction,
    now: i64,
) -> EventSubscription {
    let mut next = subscription.clone();
    next.updated_at = now;
    match action {
        SubscriptionAction::Pause => next.state = EventSubscriptionState::Paused,
        SubscriptionAction::Resume => {
            next.state = EventSubscriptionState::Active;
            // A resume does not catch up: the rule starts again from
            // now, and mail from the pause is not news (ADR-0006).
            next.watermark_at = Some(now);
        }
        SubscriptionAction::Archive => {
            next.state = EventSubscriptionState::Archived;
            next.archived_at = Some(now);
        }
        SubscriptionAction::Edit {
            name,
            instruction,
            channel_id,
            root_message_id,
            filter,
        } => {
            if let Some(name) = name {
                next.name = name.clone();
            }
            if let Some(instruction) = instruction {
                next.instruction = instruction.clone();
            }
            if let Some(channel_id) = channel_id {
                next.channel_id = channel_id.clone();
            }
            next.root_message_id = match root_message_id {
                Some(root_message_id) => root_message_id.clone(),
                None => crate::kept_thread(
                    &subscription.channel_id,
                    &subscription.root_message_id,
                    &next.channel_id,
                ),
            };
            if let Some(filter) = filter {
                next.filter = filter.clone();
            }
            // A changed rule is a new revision. The call that made it
            // carried the user's approval, so the new revision is
            // approved as it lands.
            next.revision += 1;
            next.approved_revision = Some(next.revision);
        }
    }
    next
}
