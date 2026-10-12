//! Workspace rules run with no task, clock or timer, on native and wasm targets.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use pagis_core::{Event, EventId, StoreError, WorkspaceId};
use pagis_needs_you::{
    Action, HeldItem, NeedsYouItem, NeedsYouKeypad, NeedsYouQueue, NotificationRule,
    PersonActivity, QueueChanges, QueueReader, QueuedItem,
};
use serde_json::{Value, json};

fn workspace(id: &str) -> WorkspaceId {
    WorkspaceId::from(id.to_string())
}

fn item() -> NeedsYouItem {
    NeedsYouItem::Keypad(NeedsYouKeypad {
        id: "keypad".into(),
        line: "Calls need a keypad code".into(),
        url: "/settings/phone".into(),
        at: 0,
        failed_attempts: 6,
        suspended_until: 60_000,
    })
}

fn queued() -> QueuedItem {
    QueuedItem {
        item: item(),
        count: 1,
    }
}

fn event(id: &str, kind: &str, payload: Value) -> Event {
    Event {
        id: EventId::from("event".to_string()),
        seq: 1,
        workspace_id: workspace(id),
        event_type: kind.into(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload,
        created_at: 0,
    }
}

fn added(id: &str) -> Event {
    event(id, "needs_you.added", json!({"item": item(), "count": 1}))
}

fn held(queued: Option<QueuedItem>) -> HeldItem {
    HeldItem {
        item_id: "keypad".into(),
        queued,
    }
}

#[derive(Default)]
struct Queue {
    items: Mutex<Vec<NeedsYouItem>>,
    reads: Mutex<usize>,
    fail: Mutex<bool>,
}

impl QueueReader for Queue {
    async fn queue(&self, _: &WorkspaceId) -> Result<NeedsYouQueue, StoreError> {
        *self.reads.lock().unwrap() += 1;
        if *self.fail.lock().unwrap() {
            return Err(StoreError::Corrupt("queue".into()));
        }
        let items = self.items.lock().unwrap().clone();
        Ok(NeedsYouQueue {
            count: items.len(),
            items,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_item_enters_and_leaves_the_workspace_queue() {
    block_on(async {
        let queue = Arc::new(Queue::default());
        let mut rule = QueueChanges::baseline(queue.clone(), workspace("a"))
            .await
            .unwrap();
        queue.items.lock().unwrap().push(item());
        let changed = event("a", "call.keypad_failed", json!({}));
        let changes = rule.on_event(&changed).await.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].workspace_id, workspace("a"));
        assert_eq!(changes[0].event_type, "needs_you.added");
        assert_eq!(changes[0].payload, json!({"item": item(), "count": 1}));
        assert!(rule.on_event(&changed).await.unwrap().is_empty());
        queue.items.lock().unwrap().clear();
        let changes = rule
            .on_event(&event("a", "keypad.cleared", json!({})))
            .await
            .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].event_type, "needs_you.removed");
        assert_eq!(changes[0].payload, json!({"item_id": "keypad", "count": 0}));
    });
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_baseline_publishes_nothing_for_an_existing_item() {
    block_on(async {
        let queue = Arc::new(Queue::default());
        queue.items.lock().unwrap().push(item());
        let mut rule = QueueChanges::baseline(queue, workspace("a")).await.unwrap();
        assert!(
            rule.on_event(&event("a", "call.keypad_failed", json!({})))
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn another_workspace_and_an_unrelated_event_do_not_read_the_queue() {
    block_on(async {
        let queue = Arc::new(Queue::default());
        let mut rule = QueueChanges::baseline(queue.clone(), workspace("a"))
            .await
            .unwrap();
        queue.items.lock().unwrap().push(item());
        assert!(
            rule.on_event(&event("b", "call.keypad_failed", json!({})))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(rule.on_event(&added("a")).await.unwrap().is_empty());
        assert_eq!(*queue.reads.lock().unwrap(), 1);
        assert_eq!(
            rule.on_event(&event("a", "call.keypad_failed", json!({})))
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_failed_read_does_not_change_the_last_item_ids() {
    block_on(async {
        let queue = Arc::new(Queue::default());
        let mut rule = QueueChanges::baseline(queue.clone(), workspace("a"))
            .await
            .unwrap();
        queue.items.lock().unwrap().push(item());
        *queue.fail.lock().unwrap() = true;
        let changed = event("a", "call.keypad_failed", json!({}));
        assert!(rule.on_event(&changed).await.is_err());
        *queue.fail.lock().unwrap() = false;
        assert_eq!(rule.on_event(&changed).await.unwrap().len(), 1);
    });
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_notification_goes_at_once_without_activity() {
    let activity = Arc::new(PersonActivity::new());
    let mut rule = NotificationRule::new(workspace("a"), activity);
    assert_eq!(rule.on_event(&added("a"), 0), vec![Action::Send(queued())]);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_hold_ends_120_seconds_after_the_last_activity() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("a"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity);
    assert_eq!(
        rule.on_event(&added("a"), 1),
        vec![Action::Hold {
            item_id: "keypad".into(),
            until: 120_000
        }]
    );
    assert_eq!(
        rule.on_hold_end(held(Some(queued())), 120_000),
        vec![
            Action::EndHold {
                item_id: "keypad".into()
            },
            Action::Send(queued())
        ]
    );
    assert!(rule.on_hold_end(held(Some(queued())), 120_001).is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_hold_that_ends_while_the_person_is_active_waits_again() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("a"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity.clone());
    rule.on_event(&added("a"), 0);
    activity.record(&workspace("a"), 100_000);
    assert_eq!(
        rule.on_hold_end(held(Some(queued())), 120_000),
        vec![Action::Hold {
            item_id: "keypad".into(),
            until: 220_000
        }]
    );
    assert_eq!(
        rule.on_hold_end(held(Some(queued())), 220_000),
        vec![
            Action::EndHold {
                item_id: "keypad".into()
            },
            Action::Send(queued())
        ]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_item_that_leaves_ends_its_hold_at_once() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("a"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity);
    rule.on_event(&added("a"), 0);
    assert_eq!(
        rule.on_event(
            &event(
                "a",
                "needs_you.removed",
                json!({"item_id": "keypad", "count": 0})
            ),
            60_000
        ),
        vec![Action::EndHold {
            item_id: "keypad".into()
        }]
    );
    assert!(rule.on_hold_end(held(Some(queued())), 120_000).is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn another_workspace_cannot_start_or_end_a_hold() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("b"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity.clone());
    assert!(rule.on_event(&added("b"), 0).is_empty());
    assert_eq!(rule.on_event(&added("a"), 0), vec![Action::Send(queued())]);
    activity.record(&workspace("a"), 0);
    rule.on_event(&added("a"), 0);
    assert!(
        rule.on_event(
            &event("b", "needs_you.removed", json!({"item_id": "keypad"})),
            1
        )
        .is_empty()
    );
    assert_eq!(rule.on_hold_end(held(Some(queued())), 120_000).len(), 2);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn malformed_and_unrelated_events_change_nothing() {
    let activity = Arc::new(PersonActivity::new());
    let mut rule = NotificationRule::new(workspace("a"), activity);
    assert!(
        rule.on_event(&event("a", "needs_you.added", json!({})), 0)
            .is_empty()
    );
    assert!(
        rule.on_event(&event("a", "needs_you.removed", json!({})), 0)
            .is_empty()
    );
    assert!(
        rule.on_event(&event("a", "run.completed", json!({})), 0)
            .is_empty()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_item_that_leaves_without_an_event_ends_its_hold_without_a_send() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("a"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity);
    rule.on_event(&added("a"), 0);
    assert_eq!(
        rule.on_hold_end(held(None), 120_000),
        vec![Action::EndHold {
            item_id: "keypad".into()
        }]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_released_item_uses_the_current_item_and_count() {
    let activity = Arc::new(PersonActivity::new());
    activity.record(&workspace("a"), 0);
    let mut rule = NotificationRule::new(workspace("a"), activity);
    rule.on_event(&added("a"), 0);
    let current = QueuedItem {
        item: item(),
        count: 5,
    };
    assert_eq!(
        rule.on_hold_end(held(Some(current.clone())), 120_000),
        vec![
            Action::EndHold {
                item_id: "keypad".into()
            },
            Action::Send(current)
        ]
    );
}
