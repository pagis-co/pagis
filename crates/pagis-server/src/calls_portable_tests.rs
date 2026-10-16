use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::post;
use pagis_core::*;
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, FakeStandingRule};
use tower::ServiceExt;

use crate::agents::{ArchiveState, archive_agent};
use crate::auth::Tenant;

#[tokio::test]
async fn portable_archive_releases_credentials_sleeps_mailbox_and_reports_no_number() {
    let stores = pagis_core::fake::stores();
    let workspace_id = WorkspaceId::generate();
    let user_id = UserId::generate();
    let agent_id = AgentId::generate();
    let agent = Agent {
        id: agent_id.clone(),
        workspace_id: workspace_id.clone(),
        name: "Agent".into(),
        job: String::new(),
        description: String::new(),
        personality: String::new(),
        model_alias: DEFAULT_MODEL_ALIAS.into(),
        avatar: AvatarAppearance::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: 1,
        updated_at: 1,
    };
    stores.agents.create(&agent).await.unwrap();
    stores
        .workspaces
        .create(&Workspace {
            kind: pagis_core::WorkspaceKind::Private,
            monthly_spend_cap_usd: None,
            id: workspace_id.clone(),
            user_id: Some(user_id.clone()),
            name: "Workspace".into(),
            timezone: "UTC".into(),
            created_at: 1,
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        })
        .await
        .unwrap();
    let credential_id = CredentialId::generate();
    stores
        .credentials
        .create(&Credential {
            created_by: pagis_core::Author::System,
            id: credential_id.clone(),
            workspace_id: workspace_id.clone(),
            domain: "example.com".into(),
            username: "agent".into(),
            login_url: "https://example.com".into(),
            secret: SealedSecret(vec![1]),
            totp_seed: None,
            recipe: String::new(),
            owner_agent_id: Some(agent_id.clone()),
            provenance: CredentialProvenance::AgentMinted,
            created_run_id: Some(RunId::generate()),
            created_at: 1,
        })
        .await
        .unwrap();
    let mailbox = AgentMailbox {
        id: AgentMailboxId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        connection_id: ConnectionId::generate(),
        address: "agent@example.com".into(),
        state: AgentMailboxState::Active,
        reason: None,
        outgoing_cap: 50,
        allow_rules: vec![],
        cursor: None,
        sends_day: None,
        sends_today: 0,
        created_at: 1,
        deleted_at: None,
    };
    stores.agent_mailboxes.create(&mailbox).await.unwrap();
    let bus = Arc::new(pagis_core::fake::FakeEventBus::new(Default::default()));
    let desk = Arc::new(pagis_mail::MailboxDesk::new(pagis_mail::MailboxDeskDeps {
        intake: Arc::new(pagis_mail::MailIntake {
            workspaces: stores.workspaces.clone(),
            agents: stores.agents.clone(),
            source: Arc::new(pagis_mail::HostSpamVerdicts),
            records: stores.mail_assessments.clone(),
            trust: Arc::new(pagis_mail::ListedSenders::new(
                stores.trust_list.clone(),
                stores.connections.clone(),
            )),
        }),
        workspace_mailboxes: stores.workspace_mailboxes.clone(),
        mailboxes: stores.agent_mailboxes.clone(),
        connections: stores.connections.clone(),
        org_workspace_id: WorkspaceId::generate(),
        agents: stores.agents.clone(),
        host: Arc::new(FakeMailboxHost::default()),
        transport: Arc::new(FakeMailTransport::default()),
        secrets: Arc::new(MemorySecretStore::default()),
        bus: bus.clone(),
        standing_rule: Arc::new(FakeStandingRule::default()),
        proof: Default::default(),
    }));
    let router = Router::new()
        .route("/api/v1/agents/{agent_id}/archive", post(archive_agent))
        .with_state(ArchiveState {
            agent_store: stores.agents.clone(),
            workspaces: stores.workspaces.clone(),
            credentials: stores.credentials.clone(),
            mailbox_desk: desk,
            bus: bus.clone(),
        });
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/agents/{agent_id}/archive"))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(Tenant {
        workspace_role: None,
        workspace_id: workspace_id.clone(),
        user_id,
        role: UserRole::Member,
        session_id: SessionId::generate(),
        session_expires_at: i64::MAX,
    });
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["status"], "archived");
    assert_eq!(body["email_address"], mailbox.address);
    assert_eq!(
        stores
            .agents
            .get(&workspace_id, &agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Archived
    );
    assert_eq!(
        stores
            .credentials
            .get(&workspace_id, &credential_id)
            .await
            .unwrap()
            .unwrap()
            .owner_agent_id,
        None
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .for_agent(&workspace_id, &agent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        AgentMailboxState::Dormant
    );
    let mut events = bus
        .subscribe(EventScope::Workspace(workspace_id), Some(0))
        .await;
    use futures::StreamExt;
    let dormant = events.next().await.unwrap();
    assert_eq!(dormant.event_type, "agent_mailbox.dormant");
    let archived = events.next().await.unwrap();
    assert_eq!(archived.event_type, "agent.archived");
    assert_eq!(archived.payload["released_credentials"], 1);
    assert_eq!(archived.payload["dormant_mailbox"], mailbox.address);
}
