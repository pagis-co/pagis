//! The Team Agent delegates Mail Assignments through the full daemon.
use super::{
    conversations::{hire, person},
    workspace_mailbox::MailboxApp,
};
use pagis_core::*;
use pagis_testkit::Script;
use serde_json::json;
use std::{sync::Arc, time::Duration};

type Member = (Workspace, UserId, String, &'static str);
struct AssignmentWorld {
    app: MailboxApp,
    shared: WorkspaceId,
    agent: AgentId,
    people: Vec<Member>,
    mail: pagis_mail::MessageId,
    dm: ChannelId,
}
async fn setup() -> AssignmentWorld {
    let app = MailboxApp::start().await;
    let stores = app.daemon.stores();
    let response = app
        .daemon
        .client()
        .post(format!(
            "{}/api/v1/administration/workspaces",
            app.daemon.administration_base_url
        ))
        .header("cookie", app.daemon.cookie())
        .json(&json!({
            "name": "Support",
            "manager_id": app.daemon.user_id,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let shared: serde_json::Value = response.json().await.unwrap();
    let shared = WorkspaceId::from(shared["id"].as_str().unwrap().to_string());
    let prefix = format!("/api/v1/workspaces/{shared}");
    let (status, agent) = app
        .post(
            &format!("{prefix}/agents"),
            json!({
                "name": "TeamAgent",
                "job": "Assign mail",
            }),
        )
        .await;
    assert_eq!(status, 201, "{agent}");
    let agent = AgentId::from(agent["id"].as_str().unwrap().to_string());
    assert_eq!(
        app.request(
            reqwest::Method::PUT,
            &format!("{prefix}/workspace/chief-of-staff"),
            Some(json!({
                "agent_id": agent,
            }))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        app.post(
            &format!("{prefix}/workspace/mailbox"),
            json!({
                "connection_id": app.connection_id,
                "local_part": "support",
            })
        )
        .await
        .0,
        201
    );
    stores
        .workspace_mailboxes
        .set_visibility(&shared, MailVisibility::Escalations)
        .await
        .unwrap();
    let mut people = Vec::new();
    for (name, chief) in [("First", "FirstChief"), ("Second", "SecondChief")] {
        let (private, cookie) = person(&app.daemon, name).await;
        hire(&app.daemon, &cookie, chief).await;
        let private = stores.workspaces.get(&private).await.unwrap().unwrap();
        let person = private.user_id.clone().unwrap();
        stores
            .workspace_members
            .add(&WorkspaceMember {
                workspace_id: shared.clone(),
                person_id: person.clone(),
                role: WorkspaceRole::Responder,
                author: Author::System,
            })
            .await
            .unwrap();
        app.daemon.state.person_activity.record(&person, now_ms());
        people.push((private, person, cookie, chief));
    }
    let mail = app.transport.deliver(
        "sender@example.com",
        "Decision",
        "Secret message text stays in the mailbox.",
    );
    let dm = stores
        .channels
        .find_user_dm(&shared, &agent)
        .await
        .unwrap()
        .unwrap()
        .id;
    AssignmentWorld {
        app,
        shared,
        agent,
        people,
        mail,
        dm,
    }
}

#[tokio::test]
async fn team_agent_assigns_two_chiefs_handle_and_escalate_and_fyi_starts_no_run() {
    let AssignmentWorld {
        app,
        shared,
        agent,
        people,
        mail,
        dm,
    } = setup().await;
    let stores = app.daemon.stores();
    let prefix = format!("/api/v1/workspaces/{shared}");
    let started = [
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    ];
    let release = [
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    ];
    for i in 0..2 {
        app.brain.push_for(
            people[i].3,
            Script::gate_tool_call(
                "colleagues_list",
                json!({}),
                started[i].clone(),
                release[i].clone(),
            ),
        );
    }
    app.brain.push_for(
        "TeamAgent",
        Script::tool_call_as(
            "assign",
            &[],
            "workspace_mail_triage",
            json!({
                "message_id": mail.as_str(),
                "outcome": "assigned",
                "note": "Choose a response.",
                "members": [people[0].1, people[1].1],
                "role": "decide",
                "summary": "Choose a response.",
            }),
        ),
    );
    app.brain
        .push_for("TeamAgent", Script::reply(&["Work assigned."]));
    app.brain
        .push_for("TeamAgent", Script::reply(&["The member accepted."]));
    app.brain.push_for(
        "TeamAgent",
        Script::reply(&["The member needs a decision."]),
    );
    let (status, body) = app
        .post(
            &format!("{prefix}/channels/{}/messages", dm),
            json!({
                "text": "Assign this message.",
                "pending_id": "assign-mail",
            }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    for signal in &started {
        tokio::time::timeout(Duration::from_secs(15), signal.notified())
            .await
            .expect("The assignment reaches an active member's Chief of Staff.");
    }
    let assignments = stores
        .mail_assignments
        .list(&shared, None, 50)
        .await
        .unwrap();
    assert_eq!(assignments.len(), 2);
    assert_ne!(assignments[0].channel_id, assignments[1].channel_id);
    for (i, (private, person, _, chief)) in people.iter().enumerate() {
        let row = assignments.iter().find(|a| &a.assignee == person).unwrap();
        let history = stores
            .messages
            .list_top_level(&shared, &row.channel_id, None, 50)
            .await
            .unwrap();
        assert!(
            history
                .iter()
                .all(|m| !m.message.text_content.contains("Secret message text"))
        );
        let state = if i == 0 { "handled" } else { "escalated" };
        app.brain.push_for(
            chief,
            Script::tool_call_as(
                "update",
                &[],
                "assignment_update",
                json!({
                    "assignment_id": row.id,
                    "state": state,
                    "note": if i == 0 { "Accepted." } else { "Please decide." },
                }),
            ),
        );
        app.brain.push_for(
            chief,
            Script::reply(&[if i == 0 {
                "Accepted."
            } else {
                "Please decide."
            }]),
        );
        assert!(
            stores
                .runs
                .list(&private.id, None, None, &[], None, 50)
                .await
                .unwrap()
                .iter()
                .any(|r| r.trigger_kind == TriggerKind::Conversation)
        );
        release[i].notify_one();
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let rows = stores
                .mail_assignments
                .list(&shared, None, 50)
                .await
                .unwrap();
            let notes = stores
                .messages
                .list_top_level(&shared, &dm, None, 50)
                .await
                .unwrap();
            if rows
                .iter()
                .all(|a| a.state != MailAssignmentState::Assigned)
                && notes
                    .iter()
                    .any(|m| m.message.text_content.contains("The member accepted."))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let queue = app
        .daemon
        .state
        .needs_you
        .derive(&people[1].0.id, now_ms())
        .await
        .unwrap();
    assert_eq!(
        queue
            .items
            .iter()
            .filter(|i| i.kind() == "assignment")
            .count(),
        1
    );
    let before = stores
        .runs
        .list(&people[0].0.id, None, None, &[], None, 50)
        .await
        .unwrap()
        .len();
    let fyi = app
        .daemon
        .state
        .assignments
        .assign(
            &shared,
            &agent,
            None,
            "fyi",
            mail.as_str(),
            std::slice::from_ref(&people[0].1),
            MailAssignmentRole::Fyi,
            "Read the decision.",
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        stores
            .runs
            .list(&people[0].0.id, None, None, &[], None, 50)
            .await
            .unwrap()
            .len(),
        before
    );
    let feed = app
        .daemon
        .state
        .assignments
        .feed(&people[0].1, now_ms(), "UTC")
        .await
        .unwrap();
    assert!(feed.iter().any(|a| a.id == fyi[0].id));
    assert!(
        !app.daemon
            .state
            .needs_you
            .derive(&people[0].0.id, now_ms())
            .await
            .unwrap()
            .items
            .iter()
            .any(|i| i.kind() == "assignment")
    );
}

async fn wait_for_text(world: &AssignmentWorld, text: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let history = world
                .app
                .daemon
                .stores()
                .messages
                .list_top_level(&world.shared, &world.dm, None, 50)
                .await
                .unwrap();
            if history.iter().any(|m| m.message.text_content == text) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn assignment_limit_refuses_a_second_tool_assignment() {
    let world = setup().await;
    let app = &world.app;
    assert_eq!(
        app.request(
            reqwest::Method::PUT,
            &format!(
                "/api/v1/workspaces/{}/workspace/mailbox/assignment-settings",
                world.shared
            ),
            Some(json!({
                "assignment_limit": 1,
                "assignment_timeout_minutes": 1440,
            }))
        )
        .await
        .0,
        200
    );
    for (call, reply) in [
        ("first", "Assigned the FYI."),
        ("second", "The member is at the limit."),
    ] {
        app.brain.push_for(
            "TeamAgent",
            Script::tool_call_as(
                call,
                &[],
                "workspace_mail_triage",
                json!({
                    "message_id": world.mail.as_str(),
                    "outcome": "assigned",
                    "note": "Read this summary.",
                    "members": [world.people[0].1],
                    "role": "fyi",
                    "summary": "Read this summary.",
                }),
            ),
        );
        app.brain.push_for("TeamAgent", Script::reply(&[reply]));
        assert_eq!(
            app.post(
                &format!(
                    "/api/v1/workspaces/{}/channels/{}/messages",
                    world.shared, world.dm
                ),
                json!({
                    "text": "Assign this FYI.",
                    "pending_id": call,
                })
            )
            .await
            .0,
            201
        );
        wait_for_text(&world, reply).await;
    }
    assert_eq!(
        app.daemon
            .stores()
            .mail_assignments
            .list(&world.shared, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        app.brain
            .requests()
            .iter()
            .any(|r| format!("{:?}", r.messages).contains("assignment limit"))
    );
}

#[tokio::test]
async fn daemon_assignment_timeout_uses_next_member_then_needs_you() {
    let world = setup().await;
    let service = &world.app.daemon.state.assignments;
    let rows = service
        .assign(
            &world.shared,
            &world.agent,
            None,
            "timeout",
            world.mail.as_str(),
            std::slice::from_ref(&world.people[0].1),
            MailAssignmentRole::Fyi,
            "Read the summary.",
        )
        .await
        .unwrap();
    for pass in 1..=3 {
        service
            .sweep(rows[0].due_at + (pass - 1) * 86_400_001)
            .await
            .unwrap();
    }
    let rows = service
        .records()
        .list(&world.shared, Some("timeout"), 50)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|a| a.state == MailAssignmentState::TimedOut)
    );
    assert_eq!(
        world
            .app
            .daemon
            .stores()
            .workspace_mailboxes
            .triage(&world.shared, "timeout")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        MailTriageOutcome::NeedsYou
    );
}

#[tokio::test]
async fn daemon_assignment_links_restrict_mail_and_keep_text_out_of_conversations() {
    let world = setup().await;
    let app = &world.app;
    let mut mailbox = app
        .daemon
        .state
        .mailbox_desk
        .open_workspace(&world.shared)
        .await
        .unwrap();
    let message = mailbox.message(&world.mail).await.unwrap();
    mailbox.close().await.unwrap();
    let rows = app
        .daemon
        .state
        .assignments
        .assign(
            &world.shared,
            &world.agent,
            None,
            message.message.summary.assessment_key(),
            world.mail.as_str(),
            std::slice::from_ref(&world.people[0].1),
            MailAssignmentRole::Fyi,
            "Read the summary.",
        )
        .await
        .unwrap();
    let url = format!(
        "{}/api/v1/workspaces/{}/workspace/mail/messages/{}",
        app.daemon.base_url, world.shared, world.mail
    );
    for (cookie, status) in [(&world.people[0].2, 200), (&world.people[1].2, 404)] {
        assert_eq!(
            app.daemon
                .client()
                .get(&url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
    }
    app.daemon
        .stores()
        .workspace_mailboxes
        .set_visibility(&world.shared, MailVisibility::AgentOnly)
        .await
        .unwrap();
    assert_eq!(
        app.daemon
            .client()
            .get(&url)
            .header("cookie", &world.people[0].2)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let history = app
        .daemon
        .stores()
        .messages
        .list_top_level(&world.shared, &rows[0].channel_id, None, 50)
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .all(|m| !m.message.text_content.contains("Secret message text"))
    );
}
