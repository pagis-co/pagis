//! The Workspace and Connection trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_workspaces!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::{ConnectionId, StoreError, WorkspaceId};

use super::{Backend, Bind};

pub async fn workspace_roundtrips_through_store(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let workspaces = &backend.stores().workspaces;

    assert_eq!(
        workspaces.get(&workspace.id).await.unwrap(),
        Some(workspace.clone())
    );
    assert_eq!(workspaces.list().await.unwrap(), vec![workspace]);
}

pub async fn missing_workspace_is_none(backend: &Backend) {
    let workspaces = &backend.stores().workspaces;

    assert_eq!(
        workspaces.get(&WorkspaceId::generate()).await.unwrap(),
        None
    );
}

pub async fn set_onboarded_keeps_the_first_timestamp(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let workspaces = &backend.stores().workspaces;
    assert_eq!(
        workspaces
            .get(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .onboarded_at,
        None
    );

    workspaces
        .set_onboarded(&workspace.id, 1_000)
        .await
        .unwrap();
    workspaces
        .set_onboarded(&workspace.id, 2_000)
        .await
        .unwrap();

    assert_eq!(
        workspaces
            .get(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .onboarded_at,
        Some(1_000)
    );
}

pub async fn malformed_connection_authorization_is_reported_as_corrupt(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    // The column holds a JSON array and nothing checks it at the write,
    // so a row that holds prose is reachable. The read must name it.
    backend
        .execute(
            "INSERT INTO connections \
             (id, workspace_id, provider, alias, display_name, status, \
              authorized_capabilities, config, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Bind::from("conn-work"),
                Bind::from(workspace.id.as_str()),
                Bind::from("google"),
                Bind::from("work"),
                Bind::from("Work Google"),
                Bind::from("connected"),
                Bind::from("not json"),
                Bind::from("{}"),
                Bind::from(1_i64),
            ],
        )
        .await
        .expect("plant the broken row");

    let error = backend
        .stores()
        .connections
        .get(&workspace.id, &ConnectionId::from("conn-work".to_string()))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, StoreError::Corrupt(message) if message.contains("Connection capabilities")),
        "{error:?}"
    );
}

/// The Org's Workspace belongs to no person. The Org writes it, the
/// Workspace store never answers it, so no sweep of the people's
/// Workspaces reaches it, and it still holds the Org's records.
pub async fn the_orgs_workspace_belongs_to_no_person(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let stores = backend.stores();
    let org = stores.orgs.list().await.unwrap().remove(0);

    assert_ne!(org.workspace_id, workspace.id);
    assert_eq!(
        stores.workspaces.get(&org.workspace_id).await.unwrap(),
        None
    );
    assert_eq!(stores.workspaces.list().await.unwrap(), vec![workspace]);

    let carrier = pagis_core::Connection {
        id: ConnectionId::generate(),
        workspace_id: org.workspace_id.clone(),
        provider: "telnyx".to_string(),
        alias: "telephony".to_string(),
        display_name: "Telnyx".to_string(),
        status: pagis_core::Connection::CONNECTED.to_string(),
        authorized_capabilities: Vec::new(),
        config: serde_json::json!({}),
        created_at: 1,
    };
    stores.connections.create(&carrier).await.unwrap();
    assert_eq!(
        stores.connections.list(&org.workspace_id).await.unwrap(),
        vec![carrier]
    );
}

/// The store keeps every status a Connection defines, when it creates
/// the record and when it moves the record to a new status.
pub async fn every_connection_status_is_stored(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let connections = &backend.stores().connections;

    for (index, status) in pagis_core::Connection::STATUSES.iter().enumerate() {
        let connection = pagis_core::Connection {
            id: ConnectionId::generate(),
            workspace_id: workspace.id.clone(),
            provider: "google".to_string(),
            alias: format!("account-{index}"),
            display_name: "Google".to_string(),
            status: status.to_string(),
            authorized_capabilities: Vec::new(),
            config: serde_json::json!({}),
            created_at: 1,
        };
        connections
            .create(&connection)
            .await
            .unwrap_or_else(|error| panic!("create a `{status}` Connection: {error}"));
        assert_eq!(
            connections
                .get(&workspace.id, &connection.id)
                .await
                .unwrap()
                .map(|stored| stored.status),
            Some(status.to_string())
        );
    }

    let moved = connections.list(&workspace.id).await.unwrap().remove(0);
    for status in pagis_core::Connection::STATUSES {
        assert!(
            connections
                .set_status(&workspace.id, &moved.id, status)
                .await
                .unwrap_or_else(|error| panic!("move a Connection to `{status}`: {error}"))
        );
        assert_eq!(
            connections
                .get(&workspace.id, &moved.id)
                .await
                .unwrap()
                .map(|stored| stored.status),
            Some(status.to_string())
        );
    }
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_workspaces {
    ($emit:path) => {
        $emit!(
            workspaces,
            workspace_roundtrips_through_store,
            missing_workspace_is_none,
            set_onboarded_keeps_the_first_timestamp,
            malformed_connection_authorization_is_reported_as_corrupt,
            the_orgs_workspace_belongs_to_no_person,
            every_connection_status_is_stored,
        );
    };
}
