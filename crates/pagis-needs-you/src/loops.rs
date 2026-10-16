//! Event Bus loops apply the Workspace rules with host tasks and timers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{StreamExt, channel::mpsc};
use pagis_core::{Clock, EventBus, EventScope, EventStream, StoreError, UserId, WorkspaceId};
use tokio_util::sync::CancellationToken;

use crate::needs_you::QUEUE_EVENTS;
use crate::notifications::Notifier;
use crate::{
    Action, HeldItem, NEEDS_YOU_ADDED, NEEDS_YOU_REMOVED, NeedsYou, NeedsYouQueue,
    NotificationRule, Notifications, PersonActivity, QueueChanges, QueueReader,
};

struct CurrentQueue {
    needs_you: Arc<NeedsYou>,
    clock: Arc<dyn Clock>,
}

impl QueueReader for CurrentQueue {
    async fn queue(&self, workspace_id: &WorkspaceId) -> Result<NeedsYouQueue, StoreError> {
        self.needs_you
            .derive(workspace_id, self.clock.now_ms())
            .await
    }
}

/// Subscribe before reading each startup baseline. Existing items publish
/// no event. A Workspace made after startup starts with no known items.
pub async fn run_needs_you(
    needs_you: Arc<NeedsYou>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
) -> Result<(), StoreError> {
    let mut events = bus.subscribe(EventScope::Installation, None).await;
    let queue = Arc::new(CurrentQueue { needs_you, clock });
    let mut rules = HashMap::new();
    for workspace in queue.needs_you.workspaces.list().await? {
        let rule = QueueChanges::baseline(queue.clone(), workspace.id.clone()).await?;
        rules.insert(workspace.id, rule);
    }
    pagis_rt::spawn(async move {
        loop {
            let event = tokio::select! {
                () = cancel.cancelled() => return,
                event = events.next() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
            if !QUEUE_EVENTS.contains(&event.event_type.as_str()) {
                continue;
            }
            let mut affected = vec![event.workspace_id.clone()];
            match queue.needs_you.workspaces.get(&event.workspace_id).await {
                Ok(Some(workspace)) if workspace.kind == pagis_core::WorkspaceKind::Shared => {
                    match queue.needs_you.workspace_members.list(&workspace.id).await {
                        Ok(members) => {
                            let mut persons: Vec<_> =
                                members.into_iter().map(|member| member.person_id).collect();
                            // A removed or demoted Person must lose the queue item too.
                            if let Some(id) = event
                                .payload
                                .get("person_id")
                                .and_then(serde_json::Value::as_str)
                            {
                                persons.push(pagis_core::UserId::from(id.to_string()));
                            }
                            persons.sort();
                            persons.dedup();
                            for person in persons {
                                match queue.needs_you.workspaces.for_user(&person).await {
                                    Ok(Some(private)) => affected.push(private.id),
                                    Ok(None) => {}
                                    Err(error) => {
                                        tracing::error!(%error, "the Person's queue Workspace was not read")
                                    }
                                }
                            }
                        }
                        Err(error) => tracing::error!(%error, "the queue members were not read"),
                    }
                }
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "the queue Workspace was not read"),
            }
            for workspace_id in affected {
                let rule = rules
                    .entry(workspace_id.clone())
                    .or_insert_with(|| QueueChanges::empty(queue.clone(), workspace_id.clone()));
                let changes = match rule.refresh().await {
                    Ok(changes) => changes,
                    Err(error) => {
                        tracing::error!(%error, %workspace_id, "the Needs-You Queue was not derived");
                        continue;
                    }
                };
                for change in changes {
                    if let Err(error) = bus.publish(change).await {
                        tracing::error!(%error, %workspace_id, "a Needs-You Queue change was not published");
                    }
                }
            }
        }
    });
    Ok(())
}

/// Subscribe before returning, so no entering item after startup is lost.
pub async fn run_notifications(
    notifications: Arc<Notifications>,
    activity: Arc<PersonActivity>,
    bus: Arc<dyn EventBus>,
    cancel: CancellationToken,
) {
    let events = bus.subscribe(EventScope::Installation, None).await;
    let clock = Arc::clone(&notifications.clock);
    pagis_rt::spawn(serve(notifications, activity, clock, events, cancel));
}

type ItemKey = (WorkspaceId, UserId, String);

pub(crate) async fn serve<N: Notifier>(
    notifier: Arc<N>,
    activity: Arc<PersonActivity>,
    clock: Arc<dyn Clock>,
    mut events: EventStream,
    cancel: CancellationToken,
) {
    let cancel = cancel.child_token();
    let _ended = cancel.clone().drop_guard();
    let mut rules: HashMap<(WorkspaceId, UserId), NotificationRule> = HashMap::new();
    let mut holds: HashMap<ItemKey, CancellationToken> = HashMap::new();
    let (ended, mut hold_ends) = mpsc::unbounded::<(ItemKey, CancellationToken)>();
    loop {
        let batches = tokio::select! {
            () = cancel.cancelled() => return,
            event = events.next() => {
                let Some(event) = event else { return; };
                if !matches!(event.event_type.as_str(), NEEDS_YOU_ADDED | NEEDS_YOU_REMOVED) {
                    continue;
                }
                let workspace_id = event.workspace_id.clone();
                let mut batches = Vec::new();
                for user_id in notifier.recipients(&workspace_id).await {
                    let rule = rules.entry((workspace_id.clone(), user_id.clone())).or_insert_with(|| {
                        NotificationRule::new(workspace_id.clone(), user_id.clone(), activity.clone())
                    });
                    batches.push((workspace_id.clone(), user_id, rule.on_event(&event, clock.now_ms())));
                }
                batches
            },
            end = hold_ends.next() => {
                let Some(((workspace_id, user_id, item_id), hold)) = end else { return; };
                // A replaced or removed hold can already have sent its completion.
                if hold.is_cancelled() { continue; }
                let queued = notifier.queued(&workspace_id, &item_id).await;
                let rule = rules.get_mut(&(workspace_id.clone(), user_id.clone())).expect("a hold has a Workspace rule");
                let actions = rule.on_hold_end(HeldItem { item_id, queued }, clock.now_ms());
                vec![(workspace_id, user_id, actions)]
            },
        };
        for (workspace_id, user_id, actions) in batches {
            for action in actions {
                match action {
                    Action::Send(queued) => {
                        notifier
                            .notify(&workspace_id, &user_id, &queued.item, queued.count, &cancel)
                            .await;
                    }
                    Action::EndHold { item_id } => {
                        if let Some(hold) =
                            holds.remove(&(workspace_id.clone(), user_id.clone(), item_id))
                        {
                            hold.cancel();
                        }
                    }
                    Action::Hold { item_id, until } => {
                        let key = (workspace_id.clone(), user_id.clone(), item_id);
                        let hold = cancel.child_token();
                        if let Some(old) = holds.insert(key.clone(), hold.clone()) {
                            old.cancel();
                        }
                        let ended = ended.clone();
                        let wait = Duration::from_millis(
                            u64::try_from(until - clock.now_ms()).unwrap_or(0),
                        );
                        pagis_rt::spawn(async move {
                            if hold
                                .run_until_cancelled(pagis_rt::sleep(wait))
                                .await
                                .is_some()
                            {
                                let _ = ended.unbounded_send((key, hold));
                            }
                        });
                    }
                }
            }
        }
    }
}
