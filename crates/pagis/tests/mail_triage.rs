//! Mail Triage intake uses the supplied mail store without a listener.
use pagis::mail_triage::MailTriagePoll;
use pagis_core::*;
use pagis_mail::{fake::*, *};
use pagis_testkit::fixture;
use std::{collections::HashMap, sync::Arc};

struct Catalog;
impl EventCatalog for Catalog {
    fn declaration(&self, _: &WorkspaceId, name: &str, provider: &str) -> Option<EventDeclaration> {
        let manifest = pagis_broker::mail_manifest();
        let kind = manifest
            .event_kinds
            .into_iter()
            .find(|kind| kind.name == name && kind.provider == provider)?;
        Some(EventDeclaration {
            name: kind.name,
            provider: kind.provider,
            metadata_schema: kind.metadata_schema,
            filter_schema: kind.filter_schema,
            matcher: kind.matcher,
            required_capability: kind.required_capability,
            source_resource: kind.source_resource,
            source_version: manifest.source_version,
        })
    }
}

struct ExistingMailHost {
    inner: MemoryWorkspaceMailHost,
    existing: bool,
}
#[async_trait::async_trait]
impl WorkspaceMailHost for ExistingMailHost {
    fn supports_automatic_provisioning(&self, connection: &Connection) -> bool {
        self.inner.supports_automatic_provisioning(connection)
    }
    async fn check(
        &self,
        connection: &Connection,
        request: &NewMailbox,
    ) -> Result<Vec<HostedMailbox>, MailboxError> {
        self.inner.check(connection, request).await
    }
    async fn provision(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        request: &NewMailbox,
        context: MailReadContext,
        records: &dyn WorkspaceMailboxRecordStore,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let store = self
            .inner
            .provision(connection, mailbox, request, context, records)
            .await?;
        if self.existing {
            self.inner.store(&mailbox.workspace_id).unwrap().deliver(
                "sender@example.com",
                "Existing mail",
                "Old mail",
            );
        }
        Ok(store)
    }
    async fn open(
        &self,
        connection: &Connection,
        mailbox: &WorkspaceMailbox,
        context: MailReadContext,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        self.inner.open(connection, mailbox, context).await
    }
}

struct World {
    stores: Stores,
    workspace: Workspace,
    chief: Agent,
    mail: MemoryWorkspaceMailStore,
    poll: MailTriagePoll,
    activity: Arc<pagis_needs_you::PersonActivity>,
}
impl World {
    async fn new() -> Self {
        Self::with_kind(WorkspaceKind::Private).await
    }

    async fn with_kind(kind: WorkspaceKind) -> Self {
        Self::with_existing_mail(kind, false).await
    }

    async fn with_existing_mail(kind: WorkspaceKind, existing: bool) -> Self {
        let stores = pagis_core::fake::stores();
        let mut workspace = fixture::workspace();
        let person = workspace.user_id.clone().unwrap();
        workspace.kind = kind;
        if kind == WorkspaceKind::Shared {
            workspace.user_id = None;
        }
        let chief = fixture::agent(&workspace.id);
        workspace.chief_of_staff_agent_id = Some(chief.id.clone());
        stores.workspaces.create(&workspace).await.unwrap();
        stores.agents.create(&chief).await.unwrap();
        let channel = fixture::channel(&workspace.id);
        stores.channels.create(&channel).await.unwrap();
        stores
            .participants
            .create(&fixture::agent_participant(
                &workspace.id,
                &channel.id,
                &chief.id,
            ))
            .await
            .unwrap();
        stores
            .participants
            .create(&fixture::person_participant(
                &workspace.id,
                &person,
                &channel.id,
            ))
            .await
            .unwrap();
        let settings = MailboxProvider {
            account: None,
            domain: "example.com".into(),
            imap: None,
            smtp: None,
            capabilities: MailboxCapabilities::of(
                HostCapabilities {
                    api_key: false,
                    outgoing_cap: true,
                    delete_mailbox: true,
                    reset_password: true,
                },
                false,
            ),
        };
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: workspace.id.clone(),
            provider: MANUAL_PROVIDER.into(),
            alias: "mail".into(),
            display_name: "Mail".into(),
            status: Connection::CONNECTED.into(),
            authorized_capabilities: vec![],
            config: settings.config(),
            created_at: now_ms(),
        };
        stores.connections.create(&connection).await.unwrap();
        let host = Arc::new(ExistingMailHost {
            inner: MemoryWorkspaceMailHost::default(),
            existing,
        });
        let bus = Arc::new(pagis_core::fake::FakeEventBus::default());
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            workspace_mail: host.clone(),
            grants: stores.grants.clone(),
            intake: Arc::new(MailIntake {
                workspace_mailboxes: stores.workspace_mailboxes.clone(),
                junk: stores.junk.clone(),
                opinions: Arc::new(FakeMailOpinions::default()),
                calendar: stores.calendar.clone(),
                workspaces: stores.workspaces.clone(),
                agents: stores.agents.clone(),
                source: Arc::new(HostSpamVerdicts),
                records: stores.mail_assessments.clone(),
                trust: Arc::new(ListedSenders::new(
                    stores.trust_list.clone(),
                    stores.connections.clone(),
                )),
            }),
            workspaces: stores.workspaces.clone(),
            workspace_mailboxes: stores.workspace_mailboxes.clone(),
            mailboxes: stores.agent_mailboxes.clone(),
            connections: stores.connections.clone(),
            org_workspace_id: workspace.id.clone(),
            agents: stores.agents.clone(),
            host: Arc::new(FakeMailboxHost::default()),
            transport: Arc::new(FakeMailTransport::default()),
            secrets: Arc::new(MemorySecretStore::default()),
            bus: bus.clone(),
            standing_rule: Arc::new(FakeStandingRule::default()),
            proof: ProofTiming::default(),
        }));
        let request = NewMailbox {
            connection_id: connection.id,
            local_part: "person".into(),
            outgoing_cap: None,
            password: None,
        };
        let prepared = desk.prepare_workspace(request).await.unwrap();
        desk.provision_workspace(&workspace.id, prepared, Author::System)
            .await
            .unwrap();
        let mail = host.inner.store(&workspace.id).unwrap();
        let trigger = Arc::new(pagis_trigger::Trigger::new(pagis_trigger::TriggerDeps {
            schedules: stores.schedules.clone(),
            store: stores.triggers.clone(),
            subscriptions: stores.subscriptions.clone(),
            connections: stores.connections.clone(),
            agents: stores.agents.clone(),
            channels: stores.channels.clone(),
            messages: stores.messages.clone(),
            org_workspace_id: workspace.id.clone(),
            grants: stores.grants.clone(),
            catalog: Arc::new(Catalog),
            matchers: HashMap::from([(
                MAIL_MATCHER.into(),
                Arc::new(MailMessageMatcher) as Arc<dyn EventMatcher>,
            )]),
            events: bus,
            forget_keys: Arc::new(pagis_core::fake::FakeForgetKeys::default()),
        }));
        let activity = Arc::new(pagis_needs_you::PersonActivity::new());
        let poll = MailTriagePoll::new(
            stores.clone(),
            desk,
            trigger,
            activity.clone(),
            Arc::new(SystemClock),
        );
        Self {
            stores,
            workspace,
            chief,
            mail,
            poll,
            activity,
        }
    }
    async fn poll(&self) {
        self.poll.poll_workspace(&self.workspace.id).await.unwrap();
    }
}

#[tokio::test]
async fn triage_rule_skips_the_run_and_only_needs_you_enters_the_private_queue() {
    let world = World::new().await;
    world
        .stores
        .workspace_mailboxes
        .replace_triage_rules(
            &world.workspace.id,
            &[MailTriageRule {
                sender: "example.com".into(),
                action: MailTriageAction::NeedsYou,
            }],
        )
        .await
        .unwrap();
    let id = world
        .mail
        .deliver("Sender@EXAMPLE.COM", "Decision", "Please decide.");
    world.poll().await;
    world.poll().await;
    assert!(
        world
            .stores
            .triggers
            .pending_agents()
            .await
            .unwrap()
            .is_empty()
    );
    let outcomes = world
        .stores
        .workspace_mailboxes
        .open_needs_you(&world.workspace.id, None, 100)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].1.outcome, MailTriageOutcome::NeedsYou);
    assert_eq!(outcomes[0].1.author, Author::System);
    let queue = pagis_needs_you::NeedsYou::new(&world.stores);
    let items = queue.derive(&world.workspace.id, now_ms()).await.unwrap();
    assert_eq!(items.count, 1);
    assert!(items.items[0].url().contains("/mail?message="));
    assert_eq!(outcomes[0].1.message_id, id.as_str());
    let mut handled = outcomes[0].1.clone();
    handled.outcome = MailTriageOutcome::Handled;
    world
        .stores
        .workspace_mailboxes
        .record_triage(&world.workspace.id, &outcomes[0].0, &handled)
        .await
        .unwrap();
    assert_eq!(
        queue
            .derive(&world.workspace.id, now_ms())
            .await
            .unwrap()
            .count,
        0
    );
}

#[tokio::test]
async fn triage_activity_and_delegate_everything_select_the_chief() {
    for (delegate, agent_rule) in [(false, false), (true, false), (false, true)] {
        let mut world = World::new().await;
        world
            .activity
            .record(world.workspace.user_id.as_ref().unwrap(), now_ms());
        world.workspace.delegate_everything = delegate;
        world
            .stores
            .workspaces
            .set_delegate_everything(&world.workspace.id, delegate)
            .await
            .unwrap();
        if agent_rule {
            world
                .stores
                .workspace_mailboxes
                .replace_triage_rules(
                    &world.workspace.id,
                    &[MailTriageRule {
                        sender: "example.com".into(),
                        action: MailTriageAction::Agent,
                    }],
                )
                .await
                .unwrap();
        }
        world
            .mail
            .deliver("sender@example.com", "Decision", "Please decide.");
        world.poll().await;
        let pending = world.stores.triggers.pending_agents().await.unwrap();
        assert_eq!(!pending.is_empty(), delegate || agent_rule);
        assert_eq!(
            world
                .stores
                .workspace_mailboxes
                .open_needs_you(&world.workspace.id, None, 100)
                .await
                .unwrap()
                .len(),
            usize::from(!delegate && !agent_rule)
        );
    }
}

#[tokio::test]
async fn triage_terminal_run_without_an_outcome_is_deterministic_and_deduplicated() {
    let world = World::new().await;
    world
        .mail
        .deliver_raw(b"From: sender@example.com\r\nSubject: Decision\r\n\r\nPlease decide.")
        .unwrap();
    world.poll().await;
    let claims = world
        .stores
        .triggers
        .claim_wakeups(
            &world.workspace.id,
            &world.chief.id,
            RunSlots {
                conversation: 1,
                arrival: 0,
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .pending_triage(&world.workspace.id, None, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut run = claims[0].run.clone();
    run.state = RunState::Failed;
    run.ended_at = Some(now_ms());
    world.stores.runs.update(&run).await.unwrap();
    // Repair finds only pending Mail Triage work after a missed finish event.
    world.poll().await;
    world.poll.finish_run(&run).await.unwrap();
    assert!(
        world
            .stores
            .workspace_mailboxes
            .pending_triage(&world.workspace.id, None, None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    let outcomes = world
        .stores
        .workspace_mailboxes
        .open_needs_you(&world.workspace.id, None, 100)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        outcomes[0].1.note,
        "The Chief of Staff recorded no outcome."
    );
    assert_eq!(
        pagis_needs_you::NeedsYou::new(&world.stores)
            .derive(&world.workspace.id, now_ms())
            .await
            .unwrap()
            .count,
        1
    );
}

#[tokio::test]
async fn triage_poll_is_bounded_and_resets_on_uidvalidity_change() {
    let world = World::new().await;
    world
        .stores
        .workspace_mailboxes
        .replace_triage_rules(
            &world.workspace.id,
            &[MailTriageRule {
                sender: "example.com".into(),
                action: MailTriageAction::NeedsYou,
            }],
        )
        .await
        .unwrap();
    for _ in 0..51 {
        world
            .mail
            .deliver("sender@example.com", "Decision", "Please decide.");
    }
    world.poll().await;
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .len(),
        50
    );
    world.mail.renumber("INBOX");
    world.poll().await;
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .len(),
        50
    );
    world
        .mail
        .deliver("sender@example.com", "New", "Please decide.");
    world.poll().await;
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .len(),
        51
    );
}

#[tokio::test]
async fn triage_without_a_chief_has_a_system_needs_you_outcome() {
    let world = World::new().await;
    world
        .stores
        .workspaces
        .set_chief_of_staff(&world.workspace.id, None)
        .await
        .unwrap();
    world
        .mail
        .deliver("sender@example.com", "Decision", "Please decide.");
    world.poll().await;
    let outcomes = world
        .stores
        .workspace_mailboxes
        .open_needs_you(&world.workspace.id, None, 100)
        .await
        .unwrap();
    assert_eq!(outcomes[0].1.note, "There is no active Chief of Staff.");
    assert_eq!(outcomes[0].1.author, Author::System);
    assert!(
        world
            .stores
            .triggers
            .pending_agents()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn triage_shared_mail_starts_the_chief_and_needs_you_marks_mail_without_a_queue_item() {
    let world = World::with_kind(WorkspaceKind::Shared).await;
    world
        .mail
        .deliver("sender@example.com", "Decision", "Please decide.");
    world.poll().await;
    let claims = world
        .stores
        .triggers
        .claim_wakeups(
            &world.workspace.id,
            &world.chief.id,
            RunSlots {
                conversation: 1,
                arrival: 0,
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    let mut run = claims[0].run.clone();
    run.state = RunState::Completed;
    world.poll.finish_run(&run).await.unwrap();
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()[0]
            .1
            .outcome,
        MailTriageOutcome::NeedsYou
    );
    assert_eq!(
        pagis_needs_you::NeedsYou::new(&world.stores)
            .derive(&world.workspace.id, now_ms())
            .await
            .unwrap()
            .count,
        0
    );
}

#[tokio::test]
async fn triage_skips_junk_and_quarantine_before_rules_in_score_only_mode() {
    let world = World::new().await;
    world
        .stores
        .workspace_mailboxes
        .replace_triage_rules(
            &world.workspace.id,
            &[MailTriageRule {
                sender: "example.com".into(),
                action: MailTriageAction::NeedsYou,
            }],
        )
        .await
        .unwrap();
    for header in [
        "X-Spamd-Result: default: False [20 / 15]; SPAM(20)[]",
        "X-Spamd-Result: default: False [20 / 15]; CLAM_VIRUS(20)[]",
    ] {
        world.mail.deliver_raw(format!("From: sender@example.com\r\nTo: person@example.com\r\nSubject: Junk\r\n{header}\r\n\r\nUnsafe content.").as_bytes()).unwrap();
    }
    world.poll().await;
    assert!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        world
            .stores
            .triggers
            .pending_agents()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn triage_queue_opens_the_current_mail_location_after_a_move() {
    let world = World::new().await;
    world
        .stores
        .workspace_mailboxes
        .replace_triage_rules(
            &world.workspace.id,
            &[MailTriageRule {
                sender: "example.com".into(),
                action: MailTriageAction::NeedsYou,
            }],
        )
        .await
        .unwrap();
    world.mail.deliver_raw(b"Message-ID: <decision@example.com>\r\nFrom: sender@example.com\r\nSubject: Decision\r\n\r\nPlease decide.").unwrap();
    world.poll().await;
    world
        .stores
        .workspace_mailboxes
        .relocate_triage(
            &world.workspace.id,
            "<decision@example.com>",
            "<decision@example.com>",
            "Filed:7",
        )
        .await
        .unwrap();
    let queue = pagis_needs_you::NeedsYou::new(&world.stores)
        .derive(&world.workspace.id, now_ms())
        .await
        .unwrap();
    assert_eq!(queue.items[0].url(), "/mail?message=Filed%3A7");
}

#[tokio::test]
async fn triage_starts_at_the_connected_mailbox_head() {
    let world = World::with_existing_mail(WorkspaceKind::Private, true).await;
    world
        .stores
        .workspace_mailboxes
        .replace_triage_rules(
            &world.workspace.id,
            &[MailTriageRule {
                sender: "example.com".into(),
                action: MailTriageAction::NeedsYou,
            }],
        )
        .await
        .unwrap();
    world.poll().await;
    assert!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    let id = world
        .mail
        .deliver("sender@example.com", "New mail", "New mail");
    world.poll().await;
    let outcomes = world
        .stores
        .workspace_mailboxes
        .open_needs_you(&world.workspace.id, None, 100)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].1.message_id, id.as_str());
}

#[tokio::test]
async fn triage_without_a_cursor_skips_the_existing_inbox() {
    let world = World::new().await;
    let mailbox = world
        .stores
        .workspace_mailboxes
        .get(&world.workspace.id)
        .await
        .unwrap()
        .unwrap();
    world
        .stores
        .workspace_mailboxes
        .drop_reservation(&world.workspace.id, &mailbox.address)
        .await
        .unwrap();
    world
        .mail
        .deliver("sender@example.com", "Existing mail", "Old mail");
    world
        .stores
        .workspace_mailboxes
        .create(&mailbox)
        .await
        .unwrap();
    world.poll().await;
    assert!(
        world
            .stores
            .triggers
            .pending_agents()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        world
            .stores
            .workspace_mailboxes
            .open_needs_you(&world.workspace.id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    world
        .mail
        .deliver("sender@example.com", "New mail", "New mail");
    world.poll().await;
    assert_eq!(
        world
            .stores
            .workspace_mailboxes
            .pending_triage(&world.workspace.id, None, None, 50)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn triage_reuses_its_system_subscription_after_more_than_two_hundred_rules() {
    let world = World::new().await;
    world.mail.deliver("sender@example.com", "First", "Body");
    world.poll().await;
    let mailbox = world
        .stores
        .workspace_mailboxes
        .get(&world.workspace.id)
        .await
        .unwrap()
        .unwrap();
    let source = EventSource::connection(mailbox.connection_id);
    let rules = world
        .stores
        .subscriptions
        .live_for_source(&world.workspace.id, &source, MAIL_MESSAGE_RECEIVED)
        .await
        .unwrap();
    assert_eq!(rules.len(), 1);
    let mut standing = rules[0].clone();
    standing.id = EventSubscriptionId::generate();
    standing.name = "Standing Mail Rule".into();
    standing.filter = serde_json::json!({"mailbox":"agent"});
    world.stores.subscriptions.create(&standing).await.unwrap();
    for _ in 0..205 {
        let mut rule = rules[0].clone();
        rule.id = EventSubscriptionId::generate();
        rule.created_by = Author::Agent(world.chief.id.clone());
        rule.filter = serde_json::json!({"mailbox":"agent"});
        world.stores.subscriptions.create(&rule).await.unwrap();
    }
    world.mail.deliver("sender@example.com", "Second", "Body");
    world.poll().await;
    let rules = world
        .stores
        .subscriptions
        .live_for_source(&world.workspace.id, &source, MAIL_MESSAGE_RECEIVED)
        .await
        .unwrap();
    assert_eq!(rules.len(), 207);
    assert_eq!(
        rules
            .iter()
            .filter(
                |rule| rule.name == "Workspace Mailbox triage" && rule.created_by == Author::System
            )
            .count(),
        1
    );
}

#[tokio::test]
async fn triage_repairs_at_most_fifty_pending_messages_per_poll() {
    let world = World::new().await;
    for _ in 0..101 {
        world.mail.deliver("sender@example.com", "Decision", "Body");
    }
    for _ in 0..3 {
        world.poll().await;
    }
    let pending = || {
        world
            .stores
            .workspace_mailboxes
            .pending_triage(&world.workspace.id, None, None, 200)
    };
    assert_eq!(pending().await.unwrap().len(), 101);
    let claims = world
        .stores
        .triggers
        .claim_wakeups(
            &world.workspace.id,
            &world.chief.id,
            RunSlots {
                conversation: 1,
                arrival: 0,
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    let mut run = claims[0].run.clone();
    run.state = RunState::Completed;
    run.ended_at = Some(now_ms());
    world.stores.runs.update(&run).await.unwrap();
    world.poll().await;
    assert_eq!(pending().await.unwrap().len(), 51);
    world.poll().await;
    assert_eq!(pending().await.unwrap().len(), 1);
    world.poll().await;
    assert!(pending().await.unwrap().is_empty());
}
