//! The number lifecycle (ADR-0018): buy, assign, unassign and
//! release, against a real database and a carrier that answers from an
//! inventory. `#[sqlx::test]` gives each test a fresh database with the
//! migrations applied.

use std::sync::Arc;

use pagis_audit::AuditEventBus;
use pagis_core::{
    Agent, AgentStatus, AgentStore, Connection, ConnectionId, ConnectionStore, EventLog,
    MemorySecretStore, PhoneNumberId, PhoneNumberStatus, PhoneNumberStore, SecretStore,
    WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteConnectionStore, SqliteEventLog, SqlitePhoneNumberStore,
    SqliteWorkspaceStore,
};
use pagis_telephony::fake::{
    CarrierCall, FakeCallTransport, FakeNumberCatalog, FakeStandingCallRule, FakeTextTransport,
    LineCall, TokioClock,
};
use pagis_telephony::{
    ActiveCalls, CARRIER_ACCOUNT_KEY, Endpoints, EndpointsDeps, NoActiveCalls, NumberCatalogs,
    NumberDesk, NumberDeskDeps, NumberError, Prepared, RegistrationFailure, RegistrationState,
    SIP_DOMAIN_KEY, SIP_USERNAME_KEY, TELNYX_PROVIDER, TextCapabilities, TextError, TextTransports,
    carrier_key_secret_name, sip_password_secret_name,
};
use sqlx::SqlitePool;

const ALIAS: &str = "carrier";

/// A call that the test starts and ends. It stands in for a live call
/// on the number.
#[derive(Default)]
struct ScriptedCalls(std::sync::Mutex<Option<PhoneNumberId>>);

impl ScriptedCalls {
    fn start(&self, number_id: &PhoneNumberId) {
        *self.0.lock().unwrap() = Some(number_id.clone());
    }
}

#[async_trait::async_trait]
impl ActiveCalls for ScriptedCalls {
    async fn is_active(&self, number_id: &PhoneNumberId) -> bool {
        self.0.lock().unwrap().as_ref() == Some(number_id)
    }
}

struct World {
    desk: NumberDesk,
    numbers: Arc<SqlitePhoneNumberStore>,
    connections: Arc<SqliteConnectionStore>,
    secrets: Arc<dyn SecretStore>,
    events: Arc<SqliteEventLog>,
    catalog: Arc<FakeNumberCatalog>,
    text_transport: Arc<FakeTextTransport>,
    transport: Arc<FakeCallTransport>,
    standing_rule: Arc<FakeStandingCallRule>,
    workspace_id: WorkspaceId,
    connection_id: ConnectionId,
    agent_id: pagis_core::AgentId,
    other_agent_id: pagis_core::AgentId,
}

impl World {
    async fn build(pool: SqlitePool, status: &str, calls: Arc<dyn ActiveCalls>) -> Self {
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = pagis_core::Workspace {
            user_id: person.id.clone(),
            id: WorkspaceId::generate(),
            name: "Workspace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now_ms(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        };
        workspaces.create(&workspace).await.unwrap();

        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let agent_id = seed_agent(&agents, &workspace.id, "Robin").await;
        let other_agent_id = seed_agent(&agents, &workspace.id, "Wren").await;

        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: workspace.id.clone(),
            provider: TELNYX_PROVIDER.to_string(),
            alias: ALIAS.to_string(),
            display_name: "Telnyx".to_string(),
            status: status.to_string(),
            auth_mode: Connection::AUTH_MODE_BYO.to_string(),
            authorized_capabilities: Vec::new(),
            config: serde_json::json!({}),
            created_at: now_ms(),
        };
        connections.create(&connection).await.unwrap();

        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        secrets
            .set(&carrier_key_secret_name(TELNYX_PROVIDER, ALIAS), "test-key")
            .unwrap();

        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let catalog = Arc::new(FakeNumberCatalog::offering(&[
            "+14155550123",
            "+14155550124",
        ]));
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let text_transport = Arc::new(FakeTextTransport::default());
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::clone(&transport) as _,
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            secrets: Arc::clone(&secrets),
            bus: Arc::clone(&bus),
        }));
        let standing_rule = Arc::new(FakeStandingCallRule::default());
        let desk = NumberDesk::new(NumberDeskDeps {
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            agents: Arc::clone(&agents) as _,
            catalogs: Arc::new(NumberCatalogs::single(
                TELNYX_PROVIDER,
                Arc::clone(&catalog) as _,
            )),
            text_transports: Arc::new(TextTransports::single(
                TELNYX_PROVIDER,
                Arc::clone(&text_transport) as _,
            )),
            secrets: Arc::clone(&secrets),
            calls,
            bus,
            endpoints,
            standing_rule: Arc::clone(&standing_rule) as _,
        });
        Self {
            desk,
            numbers,
            connections,
            secrets,
            events,
            catalog,
            text_transport,
            transport,
            standing_rule,
            workspace_id: workspace.id,
            connection_id: connection.id,
            agent_id,
            other_agent_id,
        }
    }

    async fn connected(pool: SqlitePool) -> Self {
        Self::build(pool, Connection::CONNECTED, Arc::new(NoActiveCalls)).await
    }

    /// The same records after a restart: fresh stores over the same
    /// database, a fresh registry with no line running, and the same
    /// secret store, as the platform keychain is.
    async fn restarted(&self, pool: SqlitePool) -> Self {
        self.with_catalogs(
            pool,
            NumberCatalogs::single(TELNYX_PROVIDER, Arc::clone(&self.catalog) as _),
        )
        .await
    }

    /// The same records over a desk that holds these catalogs.
    async fn with_catalogs(&self, pool: SqlitePool, catalogs: NumberCatalogs) -> Self {
        let standing_rule = Arc::new(FakeStandingCallRule::default());
        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::clone(&transport) as _,
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: self.workspace_id.clone(),
            secrets: Arc::clone(&self.secrets),
            bus: Arc::clone(&bus),
        }));
        let desk = NumberDesk::new(NumberDeskDeps {
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: self.workspace_id.clone(),
            agents: agents as _,
            catalogs: Arc::new(catalogs),
            text_transports: Arc::new(TextTransports::single(
                TELNYX_PROVIDER,
                Arc::clone(&self.text_transport) as _,
            )),
            secrets: Arc::clone(&self.secrets),
            calls: Arc::new(NoActiveCalls),
            bus,
            endpoints,
            standing_rule: Arc::clone(&standing_rule) as _,
        });
        Self {
            desk,
            numbers,
            connections,
            secrets: Arc::clone(&self.secrets),
            events,
            catalog: Arc::clone(&self.catalog),
            text_transport: Arc::clone(&self.text_transport),
            transport,
            workspace_id: self.workspace_id.clone(),
            connection_id: self.connection_id.clone(),
            agent_id: self.agent_id.clone(),
            other_agent_id: self.other_agent_id.clone(),
            standing_rule,
        }
    }

    /// The audit trail of the desk's own acts. The line's registration
    /// events arrive on their own time, so they are not in this list.
    async fn event_types(&self) -> Vec<String> {
        self.events
            .list_after(None, 0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.event_type)
            .filter(|event_type| event_type != "connection.registration_changed")
            .collect()
    }

    /// Enter the SIP credential on the carrier, as the Connections tab
    /// does: the public half in the record, the password in the store.
    async fn enter_sip_credential(&self) {
        self.connections
            .set_config(
                &self.workspace_id,
                &self.connection_id,
                &serde_json::json!({
                    SIP_USERNAME_KEY: "robin",
                    SIP_DOMAIN_KEY: "sip.telnyx.com",
                }),
            )
            .await
            .unwrap();
        self.secrets
            .set(
                &sip_password_secret_name(TELNYX_PROVIDER, ALIAS),
                "sip-secret",
            )
            .unwrap();
    }

    /// The registration state the number shows once the line settles:
    /// not `registering`. The number is read again, so the state follows
    /// who holds it now.
    async fn settled_registration(&self, id: &PhoneNumberId) -> Option<RegistrationState> {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let number = self
                    .numbers
                    .get(&self.workspace_id, id)
                    .await
                    .unwrap()
                    .expect("the number is stored");
                match self.desk.registration(&number) {
                    Some(RegistrationState::Registering) => {}
                    state => return state,
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the line settles")
    }

    /// How many times the carrier's line opened a socket.
    fn opens(&self) -> usize {
        self.transport
            .calls()
            .iter()
            .filter(|call| matches!(call, LineCall::Open { .. }))
            .count()
    }
}

async fn seed_agent(
    agents: &SqliteAgentStore,
    workspace_id: &WorkspaceId,
    name: &str,
) -> pagis_core::AgentId {
    let agent = Agent {
        id: pagis_core::AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
        job: "assistant".to_string(),
        description: String::new(),
        personality: "warm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    agents.create(&agent).await.unwrap();
    agent.id
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn buying_for_an_agent_assigns_the_desk_line(pool: SqlitePool) {
    let world = World::connected(pool).await;

    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    assert_eq!(number.e164, "+14155550123");
    assert_eq!(number.status, PhoneNumberStatus::Assigned);
    assert_eq!(number.agent_id.as_ref(), Some(&world.agent_id));
    assert_eq!(number.connection_id, world.connection_id);
    assert_eq!(
        world
            .numbers
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        Some(number)
    );
    assert_eq!(
        world.event_types().await,
        vec!["phone_number.purchased", "phone_number.assigned"]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_carrier_sees_the_intent_id_as_the_idempotency_key(pool: SqlitePool) {
    let world = World::connected(pool).await;

    world
        .desk
        .buy(&world.workspace_id, "+14155550123", None)
        .await
        .unwrap();

    let bought = world
        .catalog
        .calls()
        .into_iter()
        .find_map(|call| match call {
            CarrierCall::Buy {
                idempotency_key, ..
            } => Some(idempotency_key),
            _ => None,
        })
        .expect("the carrier was asked to sell");
    assert!(!bought.is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_search_needs_a_connected_carrier(pool: SqlitePool) {
    let world = World::build(pool, Connection::REAUTH_REQUIRED, Arc::new(NoActiveCalls)).await;

    let refused = world.desk.search("US", Some("415"), None).await;

    assert!(matches!(
        refused,
        Err(NumberError::CarrierUnavailable(status)) if status == Connection::REAUTH_REQUIRED
    ));
}

/// The key carries the account id from the Connection's `config` and
/// the secret from the secret store: Twilio and Plivo sign with
/// both, and Telnyx with the secret alone.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_key_carries_the_account_from_the_connection(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world
        .connections
        .set_config(
            &world.workspace_id,
            &world.connection_id,
            &serde_json::json!({ CARRIER_ACCOUNT_KEY: "AC123" }),
        )
        .await
        .unwrap();

    world.desk.search("US", Some("415"), None).await.unwrap();

    let key = world.catalog.last_key().expect("the carrier was asked");
    assert_eq!(key.account(), "AC123");
    assert_eq!(key.expose_secret(), "test-key");
}

/// A carrier Connection whose provider this daemon has no client for
/// is refused with the reason, not with a panic.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_provider_with_no_catalog_is_refused(pool: SqlitePool) {
    let world = World::connected(pool.clone())
        .await
        .with_catalogs(pool, NumberCatalogs::new())
        .await;

    let refused = world.desk.search("US", Some("415"), None).await;

    assert!(matches!(
        refused,
        Err(NumberError::UnknownProvider(provider)) if provider == TELNYX_PROVIDER
    ));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn one_agent_holds_one_number(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    let second = world
        .desk
        .buy(&world.workspace_id, "+14155550124", None)
        .await
        .unwrap();

    let refused = world
        .desk
        .assign(&world.workspace_id, &second.id, &world.agent_id)
        .await;

    assert!(matches!(refused, Err(NumberError::Conflict(_))));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_moves_to_a_second_agent_through_unassign(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    let refused = world
        .desk
        .assign(&world.workspace_id, &number.id, &world.other_agent_id)
        .await;
    assert!(matches!(refused, Err(NumberError::Conflict(_))));

    let unassigned = world
        .desk
        .unassign(&world.workspace_id, &number.id)
        .await
        .unwrap();
    assert_eq!(unassigned.status, PhoneNumberStatus::Unassigned);
    assert_eq!(unassigned.agent_id, None);

    let moved = world
        .desk
        .assign(&world.workspace_id, &number.id, &world.other_agent_id)
        .await
        .unwrap();
    assert_eq!(moved.agent_id.as_ref(), Some(&world.other_agent_id));
    assert_eq!(
        world
            .numbers
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn releasing_a_number_gives_it_back_and_keeps_the_record(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    let released = world
        .desk
        .release(&world.workspace_id, &number.id)
        .await
        .unwrap();

    assert_eq!(released.status, PhoneNumberStatus::Released);
    assert_eq!(released.agent_id, None);
    assert!(world.catalog.held().is_empty());
    // The record stays, for the Calls that point at it.
    let stored = world
        .numbers
        .get(&world.workspace_id, &number.id)
        .await
        .unwrap()
        .expect("the released record stays");
    assert_eq!(stored.status, PhoneNumberStatus::Released);
    assert_eq!(stored.e164, "+14155550123");
    // A released number is gone for good.
    let refused = world.desk.release(&world.workspace_id, &number.id).await;
    assert!(matches!(refused, Err(NumberError::Conflict(_))));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn every_act_refuses_while_a_call_is_active(pool: SqlitePool) {
    let calls = Arc::new(ScriptedCalls::default());
    let world = World::build(pool, Connection::CONNECTED, Arc::clone(&calls) as _).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    let spare = world
        .desk
        .buy(&world.workspace_id, "+14155550124", None)
        .await
        .unwrap();

    calls.start(&number.id);

    assert!(matches!(
        world.desk.unassign(&world.workspace_id, &number.id).await,
        Err(NumberError::CallActive)
    ));
    assert!(matches!(
        world.desk.release(&world.workspace_id, &number.id).await,
        Err(NumberError::CallActive)
    ));
    assert!(matches!(
        world
            .desk
            .unassign_for_agent(&world.workspace_id, number.agent_id.as_ref().unwrap())
            .await,
        Err(NumberError::CallActive)
    ));
    // A number that is not on the call is untouched by the refusal.
    assert!(
        world
            .desk
            .assign(&world.workspace_id, &spare.id, &world.other_agent_id)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn archiving_an_agent_takes_its_line_back(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    let unassigned = world
        .desk
        .unassign_for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap()
        .expect("the Agent held a number");

    assert_eq!(unassigned.id, number.id);
    assert_eq!(unassigned.status, PhoneNumberStatus::Unassigned);
    assert_eq!(
        world
            .desk
            .unassign_for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn deleting_the_carrier_refuses_while_a_number_points_at_it(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", None)
        .await
        .unwrap();

    assert!(
        world
            .desk
            .connection_in_use(&world.workspace_id, &world.connection_id)
            .await
            .unwrap()
    );

    world
        .desk
        .release(&world.workspace_id, &number.id)
        .await
        .unwrap();

    assert!(
        !world
            .desk
            .connection_in_use(&world.workspace_id, &world.connection_id)
            .await
            .unwrap()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn startup_reconciliation_does_not_take_over_a_live_purchase(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let pending_at_boot = world.numbers.list_pending_intents().await.unwrap();
    assert!(pending_at_boot.is_empty());
    let (entered, release) = world.catalog.pause_next_purchase();
    let purchase = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id));
    tokio::pin!(purchase);
    tokio::select! {
        result = &mut purchase => panic!("purchase replied before the barrier: {result:?}"),
        result = entered => result.unwrap(),
    }

    // Recovery starts late, while a purchase made after boot awaits its reply.
    world
        .desk
        .reconcile_purchases(pending_at_boot)
        .await
        .unwrap();
    release.send(()).unwrap();
    let purchased = purchase
        .await
        .expect("the live purchase retains its intent");
    assert_eq!(purchased.agent_id.as_ref(), Some(&world.agent_id));
    assert_eq!(
        world.numbers.list(&world.workspace_id).await.unwrap().len(),
        1
    );
    assert!(
        !world
            .catalog
            .calls()
            .iter()
            .any(|call| matches!(call, CarrierCall::FindPurchased(_)))
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_purchase_the_daemon_lost_is_reconciled_and_not_bought_again(pool: SqlitePool) {
    let world = World::connected(pool).await;
    // The carrier sells the number and the answer never arrives.
    world.catalog.swallow_next_purchase();
    let lost = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await;
    assert!(matches!(lost, Err(NumberError::Carrier(_))));
    assert_eq!(
        world.numbers.list(&world.workspace_id).await.unwrap(),
        vec![]
    );

    world
        .desk
        .reconcile_purchases(world.numbers.list_pending_intents().await.unwrap())
        .await
        .unwrap();

    let numbers = world.numbers.list(&world.workspace_id).await.unwrap();
    assert_eq!(numbers.len(), 1);
    assert_eq!(numbers[0].e164, "+14155550123");
    assert_eq!(numbers[0].agent_id.as_ref(), Some(&world.agent_id));
    // One order reached the carrier, and only one.
    let orders = world
        .catalog
        .calls()
        .into_iter()
        .filter(|call| matches!(call, CarrierCall::Buy { .. }))
        .count();
    assert_eq!(orders, 1);
    // The intent is settled, so a second reconciliation does nothing.
    world
        .desk
        .reconcile_purchases(world.numbers.list_pending_intents().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        world.numbers.list(&world.workspace_id).await.unwrap().len(),
        1
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_intent_the_carrier_never_honoured_is_abandoned(pool: SqlitePool) {
    let world = World::connected(pool).await;
    // A number the carrier does not offer is refused outright.
    let refused = world
        .desk
        .buy(&world.workspace_id, "+14155559999", None)
        .await;

    assert!(matches!(refused, Err(NumberError::Carrier(_))));
    assert!(
        world
            .numbers
            .list_pending_intents()
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_that_is_not_e164_is_refused_before_the_carrier(pool: SqlitePool) {
    let world = World::connected(pool).await;

    let refused = world
        .desk
        .buy(&world.workspace_id, "415 555 0123", None)
        .await;

    assert!(matches!(refused, Err(NumberError::Validation(_))));
    assert!(world.catalog.calls().is_empty());
}

/// The number as a person writes it reads as the E.164 the carrier
/// holds.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_written_with_punctuation_is_the_same_number(pool: SqlitePool) {
    let world = World::connected(pool).await;

    let number = world
        .desk
        .buy(&world.workspace_id, "+1 (415) 555-0123", None)
        .await
        .unwrap();

    assert_eq!(number.e164, "+14155550123");
}

// The line of the carrier (ADR-0020): one registration for the
// carrier's SIP credential carries every number. A number an Agent
// holds shows the state of that line; buying, assigning, unassigning
// and releasing a number leave the line as it is.

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_held_number_shows_the_carriers_line_and_moving_it_leaves_the_line(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world.enter_sip_credential().await;
    world.desk.start_carrier().await.unwrap();

    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    assert_eq!(
        world.settled_registration(&number.id).await,
        Some(RegistrationState::Registered)
    );
    assert!(world.transport.is_registered("robin"));

    // A number no Agent holds takes no call, so it shows no line.
    world
        .desk
        .unassign(&world.workspace_id, &number.id)
        .await
        .unwrap();
    assert_eq!(world.settled_registration(&number.id).await, None);

    world
        .desk
        .assign(&world.workspace_id, &number.id, &world.other_agent_id)
        .await
        .unwrap();
    assert_eq!(
        world.settled_registration(&number.id).await,
        Some(RegistrationState::Registered)
    );

    world
        .desk
        .release(&world.workspace_id, &number.id)
        .await
        .unwrap();
    assert_eq!(world.settled_registration(&number.id).await, None);

    // The line registered once, and nothing above changed it.
    assert!(world.transport.is_registered("robin"));
    assert_eq!(world.opens(), 1);
    assert!(
        !world
            .transport
            .calls()
            .iter()
            .any(|call| matches!(call, LineCall::Unregister { .. })),
        "{:?}",
        world.transport.calls()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn archiving_an_agent_leaves_the_carriers_line(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world.enter_sip_credential().await;
    world.desk.start_carrier().await.unwrap();
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    world.settled_registration(&number.id).await;

    world
        .desk
        .unassign_for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap();

    assert_eq!(world.settled_registration(&number.id).await, None);
    assert!(world.transport.is_registered("robin"));
    assert_eq!(world.opens(), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_carrier_with_no_sip_credential_says_so_until_one_is_entered(pool: SqlitePool) {
    let world = World::connected(pool).await;

    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    // The carrier has no line before the daemon starts one.
    assert_eq!(
        world.settled_registration(&number.id).await,
        Some(RegistrationState::Unregistered)
    );

    world.desk.start_carrier().await.unwrap();
    assert_eq!(
        world.settled_registration(&number.id).await,
        Some(RegistrationState::Failed(RegistrationFailure::NoCredential))
    );
    assert!(world.transport.calls().is_empty());

    world.enter_sip_credential().await;
    world
        .desk
        .sip_credential_changed(&world.connection_id)
        .await;

    assert_eq!(
        world.settled_registration(&number.id).await,
        Some(RegistrationState::Registered)
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_new_sip_credential_prepares_the_carrier_connection(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world.enter_sip_credential().await;

    world
        .desk
        .sip_credential_changed(&world.connection_id)
        .await;

    assert!(
        world
            .catalog
            .calls()
            .contains(&CarrierCall::PrepareSipConnection {
                username: "robin".to_string(),
            }),
        "{:?}",
        world.catalog.calls()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_carrier_connection_that_is_gone_has_no_line(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world.enter_sip_credential().await;
    world.desk.start_carrier().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !world.transport.is_registered("robin") {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the line registers");

    assert!(
        world
            .connections
            .delete_and_revoke(&world.workspace_id, &world.connection_id, now_ms())
            .await
            .unwrap()
    );
    world
        .desk
        .sip_credential_changed(&world.connection_id)
        .await;

    assert!(!world.transport.is_registered("robin"));
    assert!(matches!(
        world.transport.calls().last(),
        Some(LineCall::Unregister { username }) if username == "robin"
    ));
    assert_eq!(world.opens(), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_daemon_prepares_the_carrier_connection_at_start(pool: SqlitePool) {
    let world = World::connected(pool.clone()).await;
    world.enter_sip_credential().await;
    world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    let restarted = world.restarted(pool).await;
    restarted.desk.start_carrier().await.unwrap();

    assert!(
        restarted
            .catalog
            .calls()
            .contains(&CarrierCall::PrepareSipConnection {
                username: "robin".to_string(),
            }),
        "{:?}",
        restarted.catalog.calls()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_daemon_starts_one_line_for_the_carrier(pool: SqlitePool) {
    let world = World::connected(pool.clone()).await;
    world.enter_sip_credential().await;
    let held = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    let other = world
        .desk
        .buy(
            &world.workspace_id,
            "+14155550124",
            Some(&world.other_agent_id),
        )
        .await
        .unwrap();

    // The daemon restarts: a fresh registry over the same records.
    let restarted = world.restarted(pool).await;
    restarted.desk.start_carrier().await.unwrap();
    // A second start, as when a credential arrives during the boot,
    // keeps the line that runs.
    restarted.desk.start_carrier().await.unwrap();

    assert_eq!(
        restarted.settled_registration(&held.id).await,
        Some(RegistrationState::Registered)
    );
    assert_eq!(
        restarted.settled_registration(&other.id).await,
        Some(RegistrationState::Registered)
    );
    assert!(restarted.transport.is_registered("robin"));
    assert_eq!(restarted.opens(), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn assigning_a_number_prepares_the_carriers_texting(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world.text_transport.set_prepared(Prepared {
        messaging_object_id: Some("MG9".to_string()),
        connection_config: [(
            "twilio_messaging_service_sid".to_string(),
            serde_json::json!("MG9"),
        )]
        .into_iter()
        .collect(),
    });

    let held = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    // The carrier was asked once, with the Connection's own config, so
    // a Workspace object that is already there is reused.
    let prepares = world.text_transport.prepares();
    assert_eq!(prepares.len(), 1);
    assert_eq!(prepares[0].e164, "+14155550123");
    // The number keeps its messaging object.
    let stored = world
        .numbers
        .get(&world.workspace_id, &held.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.messaging_object_id.as_deref(), Some("MG9"));
    // The Workspace-level id joins the carrier Connection's config.
    let connection = world
        .connections
        .get(&world.workspace_id, &world.connection_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        connection.config["twilio_messaging_service_sid"],
        serde_json::json!("MG9")
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_carrier_that_carries_no_text_prepares_nothing(pool: SqlitePool) {
    let world = World::connected(pool).await;
    world
        .text_transport
        .set_capabilities(TextCapabilities::ABSENT);

    let held = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    assert!(world.text_transport.prepares().is_empty());
    let stored = world
        .numbers
        .get(&world.workspace_id, &held.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.messaging_object_id, None);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_daemon_start_prepares_a_number_whose_messaging_object_is_missing(pool: SqlitePool) {
    let world = World::connected(pool.clone()).await;
    // The carrier refused at assignment, so the number holds no
    // messaging object and the line still works.
    world
        .text_transport
        .fail_with(Some(TextError::Unreachable("no route".to_string())));
    let held = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    let spare = world
        .desk
        .buy(&world.workspace_id, "+14155550124", None)
        .await
        .unwrap();
    assert_eq!(
        world
            .numbers
            .get(&world.workspace_id, &held.id)
            .await
            .unwrap()
            .unwrap()
            .messaging_object_id,
        None
    );

    // The daemon restarts and the carrier answers this time.
    let restarted = world.restarted(pool).await;
    restarted.text_transport.fail_with(None);
    restarted
        .desk
        .prepare_assigned_texting(&restarted.workspace_id)
        .await
        .unwrap();

    let stored = restarted
        .numbers
        .get(&restarted.workspace_id, &held.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.messaging_object_id.as_deref(),
        Some("messaging-object-+14155550123")
    );
    // A number no Agent holds is not prepared.
    assert_eq!(
        restarted
            .numbers
            .get(&restarted.workspace_id, &spare.id)
            .await
            .unwrap()
            .unwrap()
            .messaging_object_id,
        None
    );
    // A number that is in place already is not prepared again.
    restarted
        .desk
        .prepare_assigned_texting(&restarted.workspace_id)
        .await
        .unwrap();
    let asked = restarted
        .text_transport
        .prepares()
        .into_iter()
        .filter(|prepare| prepare.e164 == "+14155550123")
        .count();
    assert_eq!(asked, 1, "the carrier prepared the number one time");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_line_an_agent_holds_carries_a_standing_call_rule(pool: SqlitePool) {
    let world = World::connected(pool).await;

    // Buying from the Agent's page gives it the line at once, so the
    // rule is written with it (ADR-0020).
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();
    assert_eq!(
        world.standing_rule.created(),
        vec![(number.e164.clone(), world.agent_id.clone())]
    );
    assert!(world.standing_rule.archived().is_empty());

    // Taking the line back stops the rule with it.
    world
        .desk
        .unassign(&world.workspace_id, &number.id)
        .await
        .unwrap();
    assert_eq!(
        world.standing_rule.archived(),
        vec![(number.e164.clone(), world.agent_id.clone())]
    );

    // The second Agent gets its own rule on the same line.
    world
        .desk
        .assign(&world.workspace_id, &number.id, &world.other_agent_id)
        .await
        .unwrap();
    assert_eq!(
        world.standing_rule.created(),
        vec![
            (number.e164.clone(), world.agent_id.clone()),
            (number.e164.clone(), world.other_agent_id.clone()),
        ]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn releasing_a_line_stops_its_standing_call_rule(pool: SqlitePool) {
    let world = World::connected(pool).await;
    let number = world
        .desk
        .buy(&world.workspace_id, "+14155550123", Some(&world.agent_id))
        .await
        .unwrap();

    world
        .desk
        .release(&world.workspace_id, &number.id)
        .await
        .unwrap();

    assert_eq!(
        world.standing_rule.archived(),
        vec![(number.e164, world.agent_id.clone())]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_number_nobody_holds_carries_no_standing_call_rule(pool: SqlitePool) {
    let world = World::connected(pool).await;

    world
        .desk
        .buy(&world.workspace_id, "+14155550123", None)
        .await
        .unwrap();

    assert!(world.standing_rule.created().is_empty());
}
