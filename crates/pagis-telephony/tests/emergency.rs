//! The two emergency guards (ADR-0018): the broker guard before
//! the call tool acts, and the dial guard in the dial path. Each refuses
//! the E.164 and the national form of the emergency numbers of the
//! regions Pagis sells numbers in, writes one audit event, and makes no
//! Call record.

use std::sync::Arc;

use pagis_audit::AuditEventBus;
use pagis_core::{
    Agent, AgentStatus, AgentStore, ConnectionId, EventLog, MemorySecretStore, PhoneNumber,
    PhoneNumberId, PhoneNumberStatus, PhoneNumberStore, PurchaseIntent, PurchaseIntentId,
    PurchaseIntentState, RunId, SecretStore, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteConnectionStore, SqliteEventLog, SqlitePhoneNumberStore,
    SqliteWorkspaceStore,
};
use pagis_telephony::fake::{
    FakeCallTransport, FakeNumberCatalog, FakeStandingCallRule, TokioClock,
};
use pagis_telephony::{
    EmergencyRefused, Endpoints, EndpointsDeps, NoActiveCalls, NumberCatalogs, NumberDesk,
    NumberDeskDeps, NumberError, TELNYX_PROVIDER, TextTransports,
};
use sqlx::SqlitePool;

struct World {
    desk: NumberDesk,
    numbers: Arc<SqlitePhoneNumberStore>,
    events: Arc<SqliteEventLog>,
    workspace_id: WorkspaceId,
    agent_id: pagis_core::AgentId,
    run_id: RunId,
}

impl World {
    async fn build(pool: SqlitePool) -> Self {
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
        };
        workspaces.create(&workspace).await.unwrap();

        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let agent = Agent {
            id: pagis_core::AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: "Robin".to_string(),
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

        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let bus: Arc<dyn pagis_core::EventBus> =
            Arc::new(AuditEventBus::new(Arc::clone(&events) as _));
        let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
            transport: Arc::new(FakeCallTransport::new(Arc::new(TokioClock))),
            numbers: Arc::clone(&numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            secrets: Arc::clone(&secrets),
            bus: Arc::clone(&bus),
        }));
        let desk = NumberDesk::new(NumberDeskDeps {
            numbers: Arc::clone(&numbers) as _,
            connections,
            org_workspace_id: workspace.id.clone(),
            agents,
            catalogs: Arc::new(NumberCatalogs::single(
                TELNYX_PROVIDER,
                Arc::new(FakeNumberCatalog::default()),
            )),
            // Nothing in these tests texts, so no carrier here
            // carries a text.
            text_transports: Arc::new(TextTransports::new()),
            secrets,
            calls: Arc::new(NoActiveCalls),
            bus,
            endpoints,
            standing_rule: Arc::new(FakeStandingCallRule::default()),
        });
        Self {
            desk,
            numbers,
            events,
            workspace_id: workspace.id,
            agent_id: agent.id,
            run_id: RunId::generate(),
        }
    }

    /// A line the Workspace holds, unassigned. The dial guard takes the
    /// record itself, so nothing is bought.
    fn line(&self, e164: &str) -> PhoneNumber {
        PhoneNumber::new(
            PhoneNumberId::generate(),
            self.workspace_id.clone(),
            ConnectionId::generate(),
            e164.to_string(),
            format!("carrier-{e164}"),
            None,
            now_ms(),
        )
    }

    /// The Agent's own desk line, written to the store the way a
    /// purchase writes it, so the broker guard can read it.
    async fn held_line(&self, e164: &str) -> PhoneNumber {
        let at = now_ms();
        let number = PhoneNumber {
            agent_id: Some(self.agent_id.clone()),
            status: PhoneNumberStatus::Assigned,
            assigned_at: Some(at),
            ..self.line(e164)
        };
        let intent = PurchaseIntent {
            id: PurchaseIntentId::generate(),
            workspace_id: self.workspace_id.clone(),
            connection_id: number.connection_id.clone(),
            e164: e164.to_string(),
            agent_id: Some(self.agent_id.clone()),
            state: PurchaseIntentState::Pending,
            created_at: at,
            settled_at: None,
        };
        self.numbers.create_intent(&intent).await.unwrap();
        self.numbers
            .confirm_purchase(&self.workspace_id, &intent.id, &number, at)
            .await
            .unwrap();
        number
    }

    async fn events(&self) -> Vec<pagis_core::Event> {
        self.events.list_after(None, 0, 100).await.unwrap()
    }
}

fn refused(error: NumberError) -> EmergencyRefused {
    match error {
        NumberError::EmergencyRefused(refused) => refused,
        other => panic!("expected an emergency refusal, got {other:?}"),
    }
}

// --- the broker guard ---

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_broker_guard_refuses_the_e164_form_and_audits_it(pool: SqlitePool) {
    let world = World::build(pool).await;
    let line = world.held_line("+14155550123").await;

    let error = world
        .desk
        .guard_call_tool(&world.workspace_id, &world.agent_id, &world.run_id, "+1911")
        .await
        .unwrap_err();

    let refused = refused(error);
    assert_eq!(refused.to, "+1911");
    assert_eq!(refused.region, "US");
    assert_eq!(EmergencyRefused::CODE, "emergency_number_refused");
    assert!(refused.to_string().contains("The person must dial it now"));

    let events = world.events().await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.event_type, "phone_number.emergency_refused");
    assert_eq!(event.agent_id.as_ref(), Some(&world.agent_id));
    assert_eq!(event.run_id.as_ref(), Some(&world.run_id));
    assert_eq!(event.payload["phone_number_id"], line.id.as_str());
    assert_eq!(event.payload["e164"], "+14155550123");
    assert_eq!(event.payload["to"], "+1911");
    assert_eq!(event.payload["region"], "US");
    assert_eq!(event.payload["agent_id"], world.agent_id.as_str());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_broker_guard_refuses_the_keypad_form_before_the_e164_check(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.held_line("+14155550123").await;

    for to in ["911", "112", "9116666666", "+911"] {
        let error = world
            .desk
            .guard_call_tool(&world.workspace_id, &world.agent_id, &world.run_id, to)
            .await
            .unwrap_err();
        assert_eq!(refused(error).to, to);
    }
    assert_eq!(world.events().await.len(), 4);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_broker_guard_passes_an_ordinary_number_and_gives_back_the_line(pool: SqlitePool) {
    let world = World::build(pool).await;
    let line = world.held_line("+14155550123").await;

    let number = world
        .desk
        .guard_call_tool(
            &world.workspace_id,
            &world.agent_id,
            &world.run_id,
            "+14155550124",
        )
        .await
        .unwrap();

    assert_eq!(number, line);
    assert!(world.events().await.is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_broker_guard_fails_closed_for_an_agent_with_no_line(pool: SqlitePool) {
    let world = World::build(pool).await;

    let error = world
        .desk
        .guard_call_tool(
            &world.workspace_id,
            &world.agent_id,
            &world.run_id,
            "+14155550124",
        )
        .await
        .unwrap_err();

    assert!(matches!(error, NumberError::Validation(_)), "{error:?}");
    assert!(world.events().await.is_empty());
}

// --- the dial guard ---

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_dial_guard_refuses_every_region_pagis_sells_in(pool: SqlitePool) {
    let world = World::build(pool).await;
    let cases = [
        ("+14155550123", "US", "+1911"),
        ("+14155550123", "US", "+1112"),
        ("+16135550123", "US", "+1911"),
        ("+442079460000", "GB", "+44999"),
        ("+442079460000", "GB", "+44112"),
        ("+493012345678", "DE", "+49110"),
        ("+493012345678", "DE", "+49112"),
        ("+33155123456", "FR", "+3315"),
        ("+33155123456", "FR", "+33112"),
        ("+61212345678", "AU", "+61000"),
        ("+5511912345678", "BR", "+55190"),
        ("+5511912345678", "BR", "+55192"),
    ];

    for (held, region, to) in cases {
        let line = world.line(held);
        let error = world.desk.guard_dial(&line, None, to).await.unwrap_err();
        let refused = refused(error);
        assert_eq!(
            (refused.to.as_str(), refused.region),
            (to, region),
            "{held} → {to}"
        );
    }
    let events = world.events().await;
    assert_eq!(events.len(), cases.len());
    assert!(
        events
            .iter()
            .all(|event| event.event_type == "phone_number.emergency_refused")
    );
    // A line nobody holds names no Agent and no Run.
    assert!(events.iter().all(|event| event.agent_id.is_none()));
    assert!(events.iter().all(|event| event.run_id.is_none()));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_dial_guard_refuses_the_keypad_form_too(pool: SqlitePool) {
    let world = World::build(pool).await;

    for (held, to) in [
        ("+14155550123", "911"),
        ("+442079460000", "999"),
        ("+493012345678", "110"),
        ("+61212345678", "000"),
        ("+5511912345678", "190"),
    ] {
        let line = world.line(held);
        let error = world.desk.guard_dial(&line, None, to).await.unwrap_err();
        assert_eq!(refused(error).to, to, "{held} → {to}");
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_dial_guard_passes_ordinary_numbers_without_a_trace(pool: SqlitePool) {
    let world = World::build(pool).await;

    for (held, to) in [
        ("+14155550123", "+14155550124"),
        ("+14155550123", "+442079460000"),
        ("+442079460000", "+442079460001"),
        ("+493012345678", "+493012345679"),
        // A Paris landline starts with 1 5, and it is not the SAMU.
        ("+33155123456", "+33155123457"),
        ("+61212345678", "+61212345679"),
        // Brazil demands an exact match: 1900 is not 190.
        ("+5511912345678", "+551900"),
    ] {
        let line = world.line(held);
        world
            .desk
            .guard_dial(&line, None, to)
            .await
            .unwrap_or_else(|error| panic!("{held} → {to}: {error}"));
    }
    assert!(world.events().await.is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_dial_guard_names_the_run_that_tried(pool: SqlitePool) {
    let world = World::build(pool).await;
    let line = world.held_line("+493012345678").await;

    world
        .desk
        .guard_dial(&line, Some(&world.run_id), "+49112")
        .await
        .unwrap_err();

    let events = world.events().await;
    assert_eq!(events[0].run_id.as_ref(), Some(&world.run_id));
    assert_eq!(events[0].agent_id.as_ref(), Some(&world.agent_id));
}
