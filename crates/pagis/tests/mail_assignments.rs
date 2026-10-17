//! Mail Assignments keep mail in its Shared Workspace.
use pagis_core::*;
use pagis_testkit::fixture;
use std::sync::Arc;

async fn world() -> (Stores, MailAssignments, Workspace, Agent, Vec<Workspace>) {
    let stores = fake::stores();
    let owner =
        seed_org_and_administrator(stores.orgs.as_ref(), stores.users.as_ref(), "Org", now_ms())
            .await
            .unwrap();
    let mut shared = fixture::workspace();
    shared.kind = WorkspaceKind::Shared;
    shared.user_id = None;
    let agent = fixture::agent(&shared.id);
    stores.workspaces.create(&shared).await.unwrap();
    stores.agents.create(&agent).await.unwrap();
    stores
        .workspace_mailboxes
        .create(&WorkspaceMailbox {
            author: Author::System,
            workspace_id: shared.id.clone(),
            connection_id: ConnectionId::generate(),
            address: "shared@example.com".into(),
            outgoing_cap: 100,
            visibility: MailVisibility::Escalations,
            spam_mode: MailScoringMode::ScoreOnly,
            created_at: now_ms(),
        })
        .await
        .unwrap();
    let mut members = Vec::new();
    for _ in 0..3 {
        let user = User::new(owner.org_id.clone(), UserRole::Member, now_ms());
        stores.users.create(&user).await.unwrap();
        let mut private = fixture::workspace();
        private.user_id = Some(user.id);
        let chief = fixture::agent(&private.id);
        private.chief_of_staff_agent_id = Some(chief.id.clone());
        stores.workspaces.create(&private).await.unwrap();
        stores.agents.create(&chief).await.unwrap();
        stores
            .workspace_members
            .add(&WorkspaceMember {
                workspace_id: shared.id.clone(),
                person_id: private.user_id.clone().unwrap(),
                role: WorkspaceRole::Responder,
                author: Author::System,
            })
            .await
            .unwrap();
        members.push(private);
    }
    let service = MailAssignments::new(
        stores.clone(),
        Arc::new(fake::FakeEventBus::default()),
        Arc::new(SystemClock),
    );
    (stores, service, shared, agent, members)
}

#[tokio::test]
async fn assignments_have_separate_shared_conversations_and_private_escalations() {
    let (stores, service, shared, agent, members) = world().await;
    let rows = service
        .assign(
            &shared.id,
            &agent.id,
            None,
            "mail-key",
            "INBOX:1",
            &members[..2]
                .iter()
                .map(|w| w.user_id.clone().unwrap())
                .collect::<Vec<_>>(),
            MailAssignmentRole::Decide,
            "Choose a response.",
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].channel_id, rows[1].channel_id);
    for row in &rows {
        let history = stores
            .messages
            .list_top_level(&shared.id, &row.channel_id, None, 50)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert!(
            history[0]
                .message
                .text_content
                .contains("Choose a response.")
        );
        let participants = stores
            .participants
            .list_for_channel(&shared.id, &row.channel_id)
            .await
            .unwrap();
        assert_eq!(participants.len(), 2);
    }
    service
        .update_by_chief(
            &rows[0].private_workspace_id,
            members[0].chief_of_staff_agent_id.as_ref().unwrap(),
            &rows[0].id,
            MailAssignmentState::Handled,
            "Accepted.",
        )
        .await
        .unwrap();
    service
        .update_by_chief(
            &rows[1].private_workspace_id,
            members[1].chief_of_staff_agent_id.as_ref().unwrap(),
            &rows[1].id,
            MailAssignmentState::Escalated,
            "Please decide.",
        )
        .await
        .unwrap();
    assert!(
        service
            .update_by_chief(
                &rows[1].private_workspace_id,
                &agent.id,
                &rows[1].id,
                MailAssignmentState::Handled,
                "Done."
            )
            .await
            .is_err()
    );
    let queue = pagis_needs_you::NeedsYou::new(&stores)
        .derive(&members[1].id, now_ms())
        .await
        .unwrap();
    assert_eq!(
        queue
            .items
            .iter()
            .filter(|i| i.id() == format!("assignment:{}", rows[1].id))
            .count(),
        1
    );
    service
        .finish_by_person(&members[1].user_id.clone().unwrap(), &rows[1].id)
        .await
        .unwrap();
    assert_eq!(
        pagis_needs_you::NeedsYou::new(&stores)
            .derive(&members[1].id, now_ms())
            .await
            .unwrap()
            .count,
        0
    );
}

#[tokio::test]
async fn fyi_has_no_queue_item_and_limits_refuse_the_whole_batch() {
    let (stores, service, shared, agent, members) = world().await;
    stores
        .mail_assignments
        .set_settings(
            &shared.id,
            &MailAssignmentSettings {
                assignment_limit: Some(1),
                assignment_timeout_minutes: 1440,
            },
        )
        .await
        .unwrap();
    let person = members[0].user_id.clone().unwrap();
    let rows = service
        .assign(
            &shared.id,
            &agent.id,
            None,
            "mail",
            "INBOX:1",
            std::slice::from_ref(&person),
            MailAssignmentRole::Fyi,
            "Read this summary.",
        )
        .await
        .unwrap();
    assert_eq!(
        pagis_needs_you::NeedsYou::new(&stores)
            .derive(&members[0].id, now_ms())
            .await
            .unwrap()
            .count,
        0
    );
    assert_eq!(service.feed(&person, now_ms(), "UTC").await.unwrap(), rows);
    assert!(
        service
            .assign(
                &shared.id,
                &agent.id,
                None,
                "other",
                "INBOX:2",
                &[members[1].user_id.clone().unwrap(), person.clone()],
                MailAssignmentRole::Approve,
                "Approve this."
            )
            .await
            .is_err()
    );
    assert!(
        stores
            .mail_assignments
            .list(&shared.id, Some("other"), 50)
            .await
            .unwrap()
            .is_empty()
    );
    service
        .finish_by_person(&person, &rows[0].id)
        .await
        .unwrap();
    assert!(
        service
            .feed(&person, now_ms(), "UTC")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn timeouts_use_each_member_once_then_require_a_person() {
    let (stores, service, shared, agent, members) = world().await;
    let rows = service
        .assign(
            &shared.id,
            &agent.id,
            None,
            "mail",
            "INBOX:1",
            &[members[0].user_id.clone().unwrap()],
            MailAssignmentRole::Approve,
            "Approve this.",
        )
        .await
        .unwrap();
    for offset in 1..=3 {
        service.sweep(now_ms() + offset * 86_400_001).await.unwrap();
    }
    let history = stores
        .mail_assignments
        .list(&shared.id, Some("mail"), 50)
        .await
        .unwrap();
    assert_eq!(history.len(), 3);
    assert!(
        history
            .iter()
            .all(|a| a.state == MailAssignmentState::TimedOut)
    );
    assert_eq!(
        stores
            .workspace_mailboxes
            .triage(&shared.id, &rows[0].message_key)
            .await
            .unwrap()
            .unwrap()
            .outcome,
        MailTriageOutcome::NeedsYou
    );
}

#[tokio::test]
async fn assignments_refuse_non_members_before_they_create_any_rows() {
    let (stores, service, shared, agent, members) = world().await;
    let error = service
        .assign(
            &shared.id,
            &agent.id,
            None,
            "mail",
            "INBOX:1",
            &[members[0].user_id.clone().unwrap(), UserId::generate()],
            MailAssignmentRole::Approve,
            "Approve the request.",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("current member"));
    assert!(
        stores
            .mail_assignments
            .list(&shared.id, None, 50)
            .await
            .unwrap()
            .is_empty()
    );
}
