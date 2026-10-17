//! Mail Assignments keep their state and enforce each member's limit.
use super::Backend;
use pagis_core::*;

async fn check(stores: &Stores, workspace: &WorkspaceId, private: &WorkspaceId, person: &UserId) {
    let store = &stores.mail_assignments;
    assert_eq!(
        store.settings(workspace).await.unwrap(),
        MailAssignmentSettings::default()
    );
    let assignment = MailAssignment {
        id: MailAssignmentId::generate(),
        workspace_id: workspace.clone(),
        message_key: "<mail@example.com>".into(),
        message_id: "INBOX:1".into(),
        assignee: person.clone(),
        private_workspace_id: private.clone(),
        role: MailAssignmentRole::Decide,
        state: MailAssignmentState::Assigned,
        assigning_agent_id: AgentId::generate(),
        assigning_run_id: None,
        channel_id: ChannelId::generate(),
        summary: "Decide on the request.".into(),
        note: String::new(),
        due_at: 100,
        created_at: 1,
        changed_at: 1,
    };
    store
        .create_batch(workspace, std::slice::from_ref(&assignment))
        .await
        .unwrap();
    assert_eq!(
        stores
            .channels
            .get(workspace, &assignment.channel_id)
            .await
            .unwrap()
            .unwrap()
            .kind,
        ChannelKind::Conversation
    );
    let participants = stores
        .participants
        .list_for_channel(workspace, &assignment.channel_id)
        .await
        .unwrap();
    assert_eq!(participants.len(), 2);
    assert!(
        participants
            .iter()
            .any(|p| p.participant_workspace_id.as_ref() == Some(private))
    );
    assert_eq!(
        store
            .list(workspace, Some(&assignment.message_key), 50)
            .await
            .unwrap(),
        std::slice::from_ref(&assignment)
    );
    assert!(
        store
            .list(&WorkspaceId::generate(), None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.overdue(99, 50).await.unwrap().is_empty());
    assert_eq!(
        store.overdue(100, 1).await.unwrap(),
        std::slice::from_ref(&assignment)
    );
    assert_eq!(
        store.for_person(person, 50).await.unwrap(),
        std::slice::from_ref(&assignment)
    );
    assert!(
        store
            .for_person(&UserId::generate(), 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .create_batch(workspace, std::slice::from_ref(&assignment))
            .await
            .is_err()
    );
    let extra = MailAssignment {
        id: MailAssignmentId::generate(),
        channel_id: ChannelId::generate(),
        ..assignment.clone()
    };
    store
        .create_batch(workspace, std::slice::from_ref(&extra))
        .await
        .unwrap();
    assert!(
        store
            .update(
                workspace,
                &extra.id,
                MailAssignmentState::Assigned,
                MailAssignmentState::Handled,
                "Done",
                2
            )
            .await
            .unwrap()
    );
    store
        .set_settings(
            workspace,
            &MailAssignmentSettings {
                assignment_limit: Some(1),
                assignment_timeout_minutes: 30,
            },
        )
        .await
        .unwrap();
    let next = MailAssignment {
        id: MailAssignmentId::generate(),
        channel_id: ChannelId::generate(),
        ..assignment.clone()
    };
    assert!(
        store
            .create_batch(workspace, std::slice::from_ref(&next))
            .await
            .is_err()
    );
    assert!(
        !store
            .update(
                &WorkspaceId::generate(),
                &assignment.id,
                MailAssignmentState::Assigned,
                MailAssignmentState::Handled,
                "Done",
                2
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .update(
                workspace,
                &assignment.id,
                MailAssignmentState::Assigned,
                MailAssignmentState::Escalated,
                "Please decide.",
                2
            )
            .await
            .unwrap()
    );
    assert!(store.overdue(100, 50).await.unwrap().is_empty());
    assert!(
        store
            .create_batch(workspace, std::slice::from_ref(&next))
            .await
            .is_err()
    );
    assert!(
        store
            .update(
                workspace,
                &assignment.id,
                MailAssignmentState::Escalated,
                MailAssignmentState::Handled,
                "Done",
                3
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .update(
                workspace,
                &assignment.id,
                MailAssignmentState::Assigned,
                MailAssignmentState::TimedOut,
                "Late",
                4
            )
            .await
            .unwrap()
    );
    store.create_batch(workspace, &[next]).await.unwrap();
    store
        .set_settings(workspace, &MailAssignmentSettings::default())
        .await
        .unwrap();
    assert_eq!(
        store.settings(workspace).await.unwrap().assignment_limit,
        None
    );
    assert!(
        store
            .set_settings(
                workspace,
                &MailAssignmentSettings {
                    assignment_limit: Some(0),
                    assignment_timeout_minutes: 0,
                }
            )
            .await
            .is_err()
    );
}

pub async fn mail_assignments_keep_state_and_enforce_limits(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let private = backend.seeded_workspace().await;
    check(
        backend.stores(),
        &workspace.id,
        &private.id,
        private.user_id.as_ref().unwrap(),
    )
    .await;
}

#[tokio::test]
async fn fake_mail_assignments_keep_state_and_enforce_limits() {
    let stores = pagis_core::fake::stores();
    check(
        &stores,
        &WorkspaceId::generate(),
        &WorkspaceId::generate(),
        &UserId::generate(),
    )
    .await;
}

#[macro_export]
macro_rules! store_suite_mail_assignments {
    ($emit:path) => {
        $emit!(
            mail_assignments,
            mail_assignments_keep_state_and_enforce_limits,
        );
    };
}
