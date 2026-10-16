//! Workspace Number purchase and release through a fake carrier.
use pagis_core::{
    Connection, ConnectionId, MemorySecretStore, PhoneNumberStatus, SecretStore, WorkspaceId,
    now_ms,
};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberCatalog, FakeTextTransport, TokioClock};
use pagis_telephony::{
    Endpoints, EndpointsDeps, NoActiveCalls, NumberCatalogs, NumberDesk, NumberDeskDeps,
    NumberError, TextTransports, carrier_key_secret_name,
};
use std::sync::Arc;

async fn desk() -> (
    Arc<NumberDesk>,
    WorkspaceId,
    Arc<FakeNumberCatalog>,
    Arc<FakeTextTransport>,
) {
    let stores = pagis_core::fake::stores();
    let workspace = pagis_core::Workspace {
        delegate_everything: false,
        kind: pagis_core::WorkspaceKind::Private,
        id: WorkspaceId::generate(),
        user_id: Some(pagis_core::UserId::generate()),
        name: "Workspace".into(),
        timezone: "UTC".into(),
        legal_hold: None,
        monthly_spend_cap_usd: None,
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    stores.workspaces.create(&workspace).await.unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace.id.clone(),
        provider: "telnyx".into(),
        alias: "carrier".into(),
        display_name: "Telnyx".into(),
        status: "connected".into(),
        authorized_capabilities: vec![],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    stores.connections.create(&connection).await.unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    secrets
        .set(&carrier_key_secret_name("telnyx", "carrier"), "key")
        .unwrap();
    let bus: Arc<dyn pagis_core::EventBus> =
        Arc::new(pagis_audit::AuditEventBus::new(stores.events.clone()));
    let endpoints = Arc::new(Endpoints::new(EndpointsDeps {
        transport: Arc::new(FakeCallTransport::new(Arc::new(TokioClock))),
        numbers: stores.phone_numbers.clone(),
        connections: stores.connections.clone(),
        workspaces: stores.workspaces.clone(),
        agents: stores.agents.clone(),
        org_workspace_id: workspace.id.clone(),
        secrets: secrets.clone(),
        bus: bus.clone(),
    }));
    let catalog = Arc::new(FakeNumberCatalog::offering(&[
        "+14155550123",
        "+14155550124",
    ]));
    let text = Arc::new(FakeTextTransport::default());
    let desk = Arc::new(NumberDesk::new(NumberDeskDeps {
        numbers: stores.phone_numbers,
        connections: stores.connections,
        org_workspace_id: workspace.id.clone(),
        catalogs: Arc::new(NumberCatalogs::single("telnyx", catalog.clone())),
        text_transports: Arc::new(TextTransports::single("telnyx", text.clone())),
        secrets,
        calls: Arc::new(NoActiveCalls),
        bus,
        endpoints,
    }));
    (desk, workspace.id, catalog, text)
}
#[tokio::test]
async fn a_workspace_buys_one_number_and_release_frees_it() {
    let (desk, workspace, _, _) = desk().await;
    let number = desk.buy(&workspace, "+14155550123").await.unwrap();
    assert_eq!(number.status, PhoneNumberStatus::Active);
    assert!(matches!(
        desk.buy(&workspace, "+14155550124").await,
        Err(NumberError::Conflict(_))
    ));
    assert_eq!(
        desk.release(&workspace, &number.id).await.unwrap().status,
        PhoneNumberStatus::Released
    );
    assert_eq!(
        desk.buy(&workspace, "+14155550124").await.unwrap().status,
        PhoneNumberStatus::Active
    );
}
#[tokio::test]
async fn concurrent_purchases_do_not_buy_two_numbers() {
    let (desk, workspace, _, _) = desk().await;
    let (first, second) = tokio::join!(
        desk.buy(&workspace, "+14155550123"),
        desk.buy(&workspace, "+14155550124")
    );
    assert_ne!(first.is_ok(), second.is_ok());
}
#[tokio::test]
async fn buying_prepares_texting() {
    let (desk, workspace, _, text) = desk().await;
    let number = desk.buy(&workspace, "+14155550123").await.unwrap();
    assert!(number.messaging_object_id.is_some());
    assert_eq!(text.prepares().len(), 1);
}
