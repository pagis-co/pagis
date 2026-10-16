//! Agent Mailbox rules over mail fakes, on native and wasm targets.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::fake::{FakeDatabase, MemorySecretStore};
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxState, AgentStatus, Connection, ConnectionId,
    SecretStore, Stores, WorkspaceId,
};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, FakeStandingRule, HostCall};
use pagis_mail::{
    DEFAULT_OUTGOING_CAP, Endpoint, HostErrorCode, MIGADU_PROVIDER, MailboxCapabilities,
    MailboxDesk, MailboxDeskDeps, MailboxError, MailboxHost, MailboxProvider, NewMailbox,
    ProofTiming, TransportErrorCode, host_api_key_secret_name, mailbox_password_secret_name,
};
mod trigger_rules;

struct World {
    trigger: Arc<pagis_trigger::Trigger>,
    desk: Arc<MailboxDesk>,
    stores: Stores,
    host: Arc<FakeMailboxHost>,
    transport: Arc<FakeMailTransport>,
    secrets: Arc<MemorySecretStore>,
    standing: Arc<FakeStandingRule>,
    workspace: WorkspaceId,
    agent: AgentId,
    connection: ConnectionId,
}

impl World {
    async fn new() -> Self {
        Self::with_trigger(false).await
    }

    async fn with_trigger(real_rule: bool) -> Self {
        let database = FakeDatabase::default();
        let stores = database.stores();
        let workspace = WorkspaceId::from("person".to_string());
        let org_workspace = WorkspaceId::from("org".to_string());
        stores
            .workspaces
            .create(&pagis_core::Workspace {
                kind: pagis_core::WorkspaceKind::Private,
                id: workspace.clone(),
                user_id: Some(pagis_core::UserId::generate()),
                name: "Workspace".into(),
                timezone: "UTC".into(),
                created_at: 0,
                onboarded_at: None,
                chief_of_staff_agent_id: None,
                report_schedule_id: None,
                home_exit_host_id: None,
                legal_hold: None,
                monthly_spend_cap_usd: None,
            })
            .await
            .unwrap();
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.clone(),
            name: "Ada".into(),
            job: "assistant".into(),
            description: String::new(),
            personality: String::new(),
            model_alias: "default".into(),
            avatar: Default::default(),
            voice: None,
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: 0,
            updated_at: 0,
        };
        stores.agents.create(&agent).await.unwrap();
        let host = Arc::new(FakeMailboxHost::default());
        let settings = MailboxProvider {
            account: None,
            domain: "example.com".into(),
            imap: Some(Endpoint::new("imap.example.com", 993)),
            smtp: Some(Endpoint::new("smtp.example.com", 465)),
            capabilities: MailboxCapabilities::of(host.capabilities(), true),
        };
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: org_workspace.clone(),
            provider: MIGADU_PROVIDER.into(),
            alias: "mail".into(),
            display_name: "Mail".into(),
            status: Connection::CONNECTED.into(),
            authorized_capabilities: Vec::new(),
            config: settings.config(),
            created_at: 0,
        };
        stores.connections.create(&connection).await.unwrap();
        let secrets = Arc::new(MemorySecretStore::default());
        secrets
            .set(&host_api_key_secret_name("mail"), "host-key")
            .unwrap();
        let transport = Arc::new(FakeMailTransport::default());
        let standing = Arc::new(FakeStandingRule::default());
        let trigger = trigger_rules::trigger(&stores, &database, org_workspace.clone());
        let standing_rule: Arc<dyn pagis_mail::StandingMailRule> = if real_rule {
            let deferred = Arc::new(pagis_mail::DeferredStandingRule::default());
            deferred.set(Arc::new(pagis_mail::StandingMailRules::new(
                trigger.clone(),
                stores.channels.clone(),
            )));
            deferred
        } else {
            standing.clone()
        };
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            grants: stores.grants.clone(),
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
            workspaces: stores.workspaces.clone(),
            workspace_mailboxes: stores.workspace_mailboxes.clone(),
            mailboxes: stores.agent_mailboxes.clone(),
            connections: stores.connections.clone(),
            org_workspace_id: org_workspace,
            agents: stores.agents.clone(),
            host: host.clone(),
            transport: transport.clone(),
            secrets: secrets.clone(),
            bus: Arc::new(database.bus()),
            standing_rule,
            proof: ProofTiming {
                first_wait: Duration::from_millis(1),
                max_wait: Duration::from_millis(2),
                budget: Duration::from_millis(10),
            },
        }));
        Self {
            trigger,
            desk,
            stores,
            host,
            transport,
            secrets,
            standing,
            workspace,
            agent: agent.id,
            connection: connection.id,
        }
    }

    fn request(&self, local_part: &str) -> NewMailbox {
        NewMailbox {
            connection_id: self.connection.clone(),
            local_part: local_part.into(),
            outgoing_cap: None,
            password: None,
        }
    }

    async fn make(&self) -> AgentMailbox {
        self.desk
            .provision(&self.workspace, &self.agent, self.request("ada"))
            .await
            .unwrap()
    }

    async fn settled(&self, mailbox: &AgentMailbox) -> AgentMailbox {
        // The same wait runs on both hosts and gives the proof task time to run.
        pagis_rt::timeout(Duration::from_secs(5), async {
            loop {
                let record = self
                    .stores
                    .agent_mailboxes
                    .get(&self.workspace, &mailbox.id)
                    .await
                    .unwrap()
                    .unwrap();
                if record.state != AgentMailboxState::Provisioning {
                    return record;
                }
                pagis_rt::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the proof task ends")
    }

    async fn event_types(&self) -> Vec<String> {
        self.stores
            .events
            .list_after(Some(&self.workspace), 0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.event_type)
            .collect()
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn make_reserves_the_address_keeps_the_password_and_proves_the_login() {
    let world = World::new().await;
    let mailbox = world.make().await;
    assert_eq!(mailbox.state, AgentMailboxState::Provisioning);
    assert_eq!(mailbox.address, "ada@example.com");
    assert_eq!(mailbox.workspace_id, world.workspace);
    assert_eq!(mailbox.outgoing_cap, DEFAULT_OUTGOING_CAP);
    assert_eq!(
        world.host.calls(),
        [HostCall::Create {
            local_part: "ada".into(),
            outgoing_cap: DEFAULT_OUTGOING_CAP
        }]
    );
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace,
                &mailbox.address
            ))
            .unwrap(),
        world.host.password_of(&mailbox.address)
    );
    assert_eq!(world.standing.created(), std::slice::from_ref(&mailbox.id));
    let active = world.settled(&mailbox).await;
    assert_eq!(active.state, AgentMailboxState::Active);
    assert!(active.cursor.is_some());
    assert_eq!(world.transport.logins(), [mailbox.address]);
    assert_eq!(
        world.event_types().await,
        ["agent_mailbox.provisioned", "agent_mailbox.active"]
    );
    assert_eq!(
        world
            .desk
            .for_agent(&world.workspace, &world.agent)
            .await
            .unwrap(),
        Some(active.clone())
    );
    assert_eq!(world.desk.list(&world.workspace).await.unwrap(), [active]);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn make_frees_the_address_when_the_host_refuses_it() {
    let world = World::new().await;
    world.host.fail_with(Some(HostErrorCode::AddressTaken));
    assert!(matches!(
        world
            .desk
            .provision(&world.workspace, &world.agent, world.request("ada"))
            .await,
        Err(MailboxError::Host(_))
    ));
    assert!(!world.desk.address_taken("ada@example.com").await.unwrap());
    assert!(world.desk.list(&world.workspace).await.unwrap().is_empty());
    assert!(world.standing.created().is_empty());
    assert!(world.event_types().await.is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn check_reads_the_address_ledger_before_an_agent_exists() {
    let world = World::new().await;
    world.desk.check(&world.request("grace")).await.unwrap();
    assert!(!world.desk.address_taken("grace@example.com").await.unwrap());
    assert!(world.host.calls().is_empty());
    assert!(matches!(
        world.desk.check(&world.request("postmaster")).await,
        Err(MailboxError::Validation(_))
    ));
    let mailbox = world.make().await;
    world.settled(&mailbox).await;
    assert!(world.desk.address_taken("ADA@EXAMPLE.COM").await.unwrap());
    assert!(matches!(
        world.desk.check(&world.request("ada")).await,
        Err(MailboxError::Conflict(_))
    ));
    world
        .desk
        .delete(&world.workspace, &mailbox.id, &mailbox.address)
        .await
        .unwrap();
    assert!(matches!(
        world.desk.check(&world.request("ada")).await,
        Err(MailboxError::Conflict(_))
    ));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn sleep_keeps_the_host_mailbox_and_password_and_publishes_its_state() {
    let world = World::new().await;
    let mailbox = world.make().await;
    world.settled(&mailbox).await;
    let secret = world
        .secrets
        .get(&mailbox_password_secret_name(
            &world.workspace,
            &mailbox.address,
        ))
        .unwrap();
    let dormant = world
        .desk
        .make_dormant_for_agent(&world.workspace, &world.agent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dormant.state, AgentMailboxState::Dormant);
    assert_eq!(dormant.address, mailbox.address);
    assert_eq!(
        world.host.addresses(),
        std::slice::from_ref(&mailbox.address)
    );
    assert_eq!(world.host.calls().len(), 1);
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace,
                &mailbox.address
            ))
            .unwrap(),
        secret
    );
    assert_eq!(
        world.event_types().await,
        [
            "agent_mailbox.provisioned",
            "agent_mailbox.active",
            "agent_mailbox.dormant"
        ]
    );
    assert!(
        world
            .desk
            .make_dormant_for_agent(&world.workspace, &AgentId::generate())
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn recipient_domains_stay_in_order_without_duplicates_and_within_the_workspace() {
    let world = World::new().await;
    let mailbox = world.make().await;
    world.settled(&mailbox).await;
    assert_eq!(
        world
            .desk
            .allow_recipient_domains(
                &world.workspace,
                &mailbox.id,
                &["example.org".into(), "example.net".into()]
            )
            .await
            .unwrap(),
        ["example.org", "example.net"]
    );
    assert_eq!(
        world
            .desk
            .allow_recipient_domains(
                &world.workspace,
                &mailbox.id,
                &["example.org".into(), "example.edu".into()]
            )
            .await
            .unwrap(),
        ["example.org", "example.net", "example.edu"]
    );
    assert!(
        world
            .desk
            .allow_recipient_domains(
                &WorkspaceId::from("other".to_string()),
                &mailbox.id,
                &["other.org".into()]
            )
            .await
            .is_err()
    );
    assert_eq!(
        world
            .desk
            .for_agent(&world.workspace, &world.agent)
            .await
            .unwrap()
            .unwrap()
            .allow_rules,
        ["example.org", "example.net", "example.edu"]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_proof_task_retries_with_the_host_timer_then_marks_the_mailbox_unavailable() {
    let world = World::new().await;
    world
        .transport
        .refuse_login_with(Some(TransportErrorCode::Unauthorized));
    let mailbox = world.make().await;
    let unavailable = world.settled(&mailbox).await;
    assert_eq!(unavailable.state, AgentMailboxState::Unavailable);
    assert!(unavailable.reason.unwrap().contains("unauthorized"));
    assert_eq!(
        world.event_types().await,
        ["agent_mailbox.provisioned", "agent_mailbox.unavailable"]
    );
}
