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

/// Shared Workspaces have no owner and are part of every Workspace sweep.
pub async fn shared_workspace_membership_and_last_manager(backend: &Backend) {
    use pagis_core::{Author, WorkspaceKind, WorkspaceMember, WorkspaceRole};
    let private = backend.seeded_workspace().await;
    let stores = backend.stores();
    let person = private.user_id.clone().unwrap();
    let shared = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        kind: WorkspaceKind::Shared,
        user_id: None,
        name: "Support".into(),
        ..private.clone()
    };
    let members = &stores.workspace_members;
    let manager = WorkspaceMember {
        workspace_id: shared.id.clone(),
        person_id: person.clone(),
        role: WorkspaceRole::Manager,
        author: Author::Person(person.clone()),
    };
    members.create_workspace(&shared, &manager).await.unwrap();
    assert_eq!(
        members.list(&shared.id).await.unwrap(),
        vec![manager.clone()]
    );
    assert!(members.add(&manager).await.is_err());
    assert!(members.remove(&shared.id, &person).await.is_err());
    assert!(
        members
            .change(&WorkspaceMember {
                role: WorkspaceRole::Reader,
                ..manager.clone()
            })
            .await
            .is_err()
    );
    assert_eq!(
        members.get(&shared.id, &person).await.unwrap(),
        Some(manager)
    );
    assert!(stores.workspaces.list().await.unwrap().contains(&shared));
    assert_eq!(
        stores.workspaces.for_user(&person).await.unwrap(),
        Some(private.clone())
    );
    assert!(
        members
            .add(&WorkspaceMember {
                workspace_id: private.id,
                person_id: person.clone(),
                role: WorkspaceRole::Reader,
                author: Author::Person(person),
            })
            .await
            .is_err()
    );
    let absent = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        ..shared.clone()
    };
    let invalid = WorkspaceMember {
        workspace_id: absent.id.clone(),
        person_id: pagis_core::UserId::generate(),
        role: WorkspaceRole::Manager,
        author: Author::System,
    };
    assert!(members.create_workspace(&absent, &invalid).await.is_err());
    assert_eq!(stores.workspaces.get(&absent.id).await.unwrap(), None);
    let org = stores.orgs.list().await.unwrap().remove(0);
    let other = pagis_core::User::new(org.id, pagis_core::UserRole::Member, 1);
    stores.users.create(&other).await.unwrap();
    let second = WorkspaceMember {
        workspace_id: shared.id.clone(),
        person_id: other.id.clone(),
        role: WorkspaceRole::Manager,
        author: Author::System,
    };
    members.add(&second).await.unwrap();
    let changed = WorkspaceMember {
        role: WorkspaceRole::Responder,
        author: Author::System,
        ..members
            .get(&shared.id, &private.user_id.clone().unwrap())
            .await
            .unwrap()
            .unwrap()
    };
    assert!(members.change(&changed).await.unwrap());
    assert_eq!(
        members.get(&shared.id, &changed.person_id).await.unwrap(),
        Some(changed.clone())
    );
    assert!(members.remove(&shared.id, &other.id).await.is_err());
    members
        .change(&WorkspaceMember {
            role: WorkspaceRole::Manager,
            ..changed.clone()
        })
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        members.remove(&shared.id, &changed.person_id),
        members.remove(&shared.id, &other.id)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let error = match a {
        Err(error) => error,
        Ok(removed) => {
            assert!(removed);
            b.unwrap_err()
        }
    };
    assert!(matches!(error, StoreError::Conflict(_)));
    let remaining = members.list(&shared.id).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].role, WorkspaceRole::Manager);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_workspaces {
    ($emit:path) => {
        $emit!(
            workspaces,
            shared_workspace_membership_and_last_manager,
            workspace_roundtrips_through_store,
            missing_workspace_is_none,
            set_onboarded_keeps_the_first_timestamp,
            malformed_connection_authorization_is_reported_as_corrupt,
            the_orgs_workspace_belongs_to_no_person,
            every_connection_status_is_stored,
        );
    };
}
