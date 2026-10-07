//! The Push Subscriptions of a Person, on both backends (ADR-0030).
//!
//! The endpoint is the identity of a row, and the Session owns it: the
//! row ends when the Session row ends. Name every new body in
//! `store_suite_push_subscriptions!` below; the guard test of the parent
//! module fails while one is missing.

use pagis_core::{
    ClientKind, PushSubscription, PushSubscriptionId, SESSION_LIFETIME_MS, Session, SessionId,
    UserId, Workspace, WorkspaceId,
};

use super::Backend;

const ENDPOINT: &str = "https://push.example.com/send/abc";

/// A browser Session of `user_id`, written, that lives until `expires_at`.
async fn session(backend: &Backend, user_id: &UserId, expires_at: i64) -> Session {
    let id = SessionId::generate();
    let session = Session {
        token_hash: format!("hash-{id}"),
        id,
        user_id: user_id.clone(),
        client_kind: ClientKind::Browser,
        client_name: Some("Safari on iPhone".to_string()),
        created_at: 1_000,
        last_used_at: 1_000,
        expires_at,
    };
    backend
        .stores()
        .sessions
        .create(&session)
        .await
        .expect("write the Session");
    session
}

/// A live Session of the Person who owns `workspace`.
async fn live_session(backend: &Backend, workspace: &Workspace) -> Session {
    session(backend, &workspace.user_id, SESSION_LIFETIME_MS).await
}

/// A Push Subscription of `session` in `workspace`, not yet written.
fn subscription(
    workspace: &Workspace,
    session: &Session,
    endpoint: &str,
    keys: (&str, &str),
    at: i64,
) -> PushSubscription {
    PushSubscription {
        id: PushSubscriptionId::generate(),
        workspace_id: workspace.id.clone(),
        session_id: session.id.clone(),
        endpoint: endpoint.to_string(),
        p256dh: keys.0.to_string(),
        auth: keys.1.to_string(),
        created_at: at,
        last_sent_at: None,
    }
}

/// A second Workspace of the same Org, for the reads that must not cross
/// one.
async fn second_workspace(backend: &Backend, first: &Workspace) -> Workspace {
    let second = Workspace {
        id: WorkspaceId::generate(),
        ..first.clone()
    };
    backend
        .stores()
        .workspaces
        .create(&second)
        .await
        .expect("write the second Workspace");
    second
}

pub async fn a_push_subscription_reads_back_with_its_keys(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    let written = subscription(&workspace, &session, ENDPOINT, ("key", "secret"), 1_000);

    let kept = store.upsert(&written).await.unwrap();

    assert_eq!(kept, written);
    assert_eq!(store.list(&workspace.id).await.unwrap(), vec![written]);
}

/// A client that subscribes again sends the same endpoint. The same
/// Session keeps its row as it was made, and takes the keys it sends.
pub async fn the_same_session_subscribes_again_onto_its_row(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    let first = store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("key-1", "secret-1"),
            1_000,
        ))
        .await
        .unwrap();
    assert!(
        store
            .mark_sent(&workspace.id, &first.id, 1_500)
            .await
            .unwrap()
    );

    let again = store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("key-2", "secret-2"),
            2_000,
        ))
        .await
        .unwrap();

    assert_eq!(again.id, first.id);
    assert_eq!(
        (again.p256dh.as_str(), again.auth.as_str()),
        ("key-2", "secret-2")
    );
    assert_eq!(again.created_at, 1_000);
    assert_eq!(again.last_sent_at, Some(1_500));
    assert_eq!(store.list(&workspace.id).await.unwrap(), vec![again]);
}

/// The browser is the authority for its endpoint. A known endpoint that
/// another Session sends moves to that Session and its Workspace, with
/// the new keys, as a new subscription of that Session.
pub async fn a_known_endpoint_moves_to_the_session_that_sends_it(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let first_session = live_session(backend, &workspace).await;
    let second_session = live_session(backend, &other).await;
    let store = &backend.stores().push_subscriptions;
    let first = store
        .upsert(&subscription(
            &workspace,
            &first_session,
            ENDPOINT,
            ("key-1", "secret-1"),
            1_000,
        ))
        .await
        .unwrap();
    assert!(
        store
            .mark_sent(&workspace.id, &first.id, 1_500)
            .await
            .unwrap()
    );

    let moved = store
        .upsert(&subscription(
            &other,
            &second_session,
            ENDPOINT,
            ("key-2", "secret-2"),
            2_000,
        ))
        .await
        .unwrap();

    assert_eq!(moved.id, first.id, "the endpoint keeps its one row");
    assert_eq!(moved.workspace_id, other.id);
    assert_eq!(moved.session_id, second_session.id);
    assert_eq!(
        (moved.p256dh.as_str(), moved.auth.as_str()),
        ("key-2", "secret-2")
    );
    assert_eq!(moved.created_at, 2_000);
    assert_eq!(
        moved.last_sent_at, None,
        "the new Session learns nothing of the pushes of the old one"
    );
    assert!(store.list(&workspace.id).await.unwrap().is_empty());
    assert_eq!(store.list(&other.id).await.unwrap(), vec![moved]);
}

/// The list holds the rows of one Workspace, oldest first, and none of
/// another Workspace.
pub async fn the_list_holds_one_workspace_only(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let laptop = live_session(backend, &workspace).await;
    let phone = live_session(backend, &workspace).await;
    let theirs = live_session(backend, &other).await;
    let store = &backend.stores().push_subscriptions;
    let first = store
        .upsert(&subscription(
            &workspace,
            &laptop,
            "https://push.example.com/laptop",
            ("k", "s"),
            1_000,
        ))
        .await
        .unwrap();
    let second = store
        .upsert(&subscription(
            &workspace,
            &phone,
            "https://push.example.com/phone",
            ("k", "s"),
            2_000,
        ))
        .await
        .unwrap();
    let other_row = store
        .upsert(&subscription(
            &other,
            &theirs,
            "https://push.example.com/theirs",
            ("k", "s"),
            1_500,
        ))
        .await
        .unwrap();

    assert_eq!(
        store.list(&workspace.id).await.unwrap(),
        vec![first, second]
    );
    assert_eq!(store.list(&other.id).await.unwrap(), vec![other_row]);
    assert!(
        store
            .list(&WorkspaceId::generate())
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn mark_sent_records_the_time_of_the_last_push(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    let row = store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("k", "s"),
            1_000,
        ))
        .await
        .unwrap();

    assert!(
        store
            .mark_sent(&workspace.id, &row.id, 5_000)
            .await
            .unwrap()
    );
    assert!(
        store
            .mark_sent(&workspace.id, &row.id, 7_000)
            .await
            .unwrap()
    );

    let read = store.list(&workspace.id).await.unwrap();
    assert_eq!(read[0].last_sent_at, Some(7_000));
    assert!(
        !store
            .mark_sent(&workspace.id, &PushSubscriptionId::generate(), 8_000)
            .await
            .unwrap(),
        "a missing row refuses"
    );
}

/// A push service that answers that an endpoint is gone ends its row,
/// and no other.
pub async fn delete_by_endpoint_removes_the_row_of_that_endpoint(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("k", "s"),
            1_000,
        ))
        .await
        .unwrap();
    let kept = store
        .upsert(&subscription(
            &workspace,
            &session,
            "https://push.example.com/kept",
            ("k", "s"),
            2_000,
        ))
        .await
        .unwrap();

    assert!(
        store
            .delete_by_endpoint(&workspace.id, ENDPOINT)
            .await
            .unwrap()
    );
    assert!(
        !store
            .delete_by_endpoint(&workspace.id, ENDPOINT)
            .await
            .unwrap(),
        "a gone endpoint refuses a second time"
    );
    assert_eq!(store.list(&workspace.id).await.unwrap(), vec![kept]);
}

pub async fn delete_removes_one_row(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    let row = store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("k", "s"),
            1_000,
        ))
        .await
        .unwrap();

    assert!(store.delete(&workspace.id, &row.id).await.unwrap());
    assert!(!store.delete(&workspace.id, &row.id).await.unwrap());
    assert!(store.list(&workspace.id).await.unwrap().is_empty());
}

/// One Person never removes or marks another Person's row: each write
/// names the Workspace, so a row of another Workspace reads as absent.
pub async fn one_workspace_never_writes_another_workspace_subscription(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let session = live_session(backend, &workspace).await;
    let store = &backend.stores().push_subscriptions;
    let mine = store
        .upsert(&subscription(
            &workspace,
            &session,
            ENDPOINT,
            ("k", "s"),
            1_000,
        ))
        .await
        .unwrap();

    assert!(!store.delete(&other.id, &mine.id).await.unwrap());
    assert!(!store.delete_by_endpoint(&other.id, ENDPOINT).await.unwrap());
    assert!(!store.mark_sent(&other.id, &mine.id, 5_000).await.unwrap());
    assert_eq!(store.list(&workspace.id).await.unwrap(), vec![mine]);
}

/// A Push Subscription ends with its Session. Each way a Session row
/// ends removes the Push Subscriptions of that Session and no other: a
/// sign-out or a removal from the Sessions list, the end of every
/// Session of a Person, and the expiry sweep.
pub async fn the_end_of_a_session_removes_its_push_subscriptions(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let signed_out = live_session(backend, &workspace).await;
    let expired = session(backend, &workspace.user_id, 5_000).await;
    let kept = live_session(backend, &workspace).await;
    let sessions = &backend.stores().sessions;
    let store = &backend.stores().push_subscriptions;
    for (session, endpoint) in [
        (&signed_out, "https://push.example.com/signed-out"),
        (&expired, "https://push.example.com/expired"),
        (&kept, "https://push.example.com/kept"),
    ] {
        store
            .upsert(&subscription(
                &workspace,
                session,
                endpoint,
                ("k", "s"),
                1_000,
            ))
            .await
            .unwrap();
    }

    assert!(sessions.delete(&signed_out.id).await.unwrap());
    assert_eq!(sessions.delete_expired(6_000).await.unwrap(), 1);

    let left = store.list(&workspace.id).await.unwrap();
    assert_eq!(
        left.iter()
            .map(|row| row.session_id.clone())
            .collect::<Vec<_>>(),
        vec![kept.id.clone()]
    );

    sessions.delete_for_user(&workspace.user_id).await.unwrap();
    assert!(store.list(&workspace.id).await.unwrap().is_empty());
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_push_subscriptions {
    ($emit:path) => {
        $emit!(
            push_subscriptions,
            a_push_subscription_reads_back_with_its_keys,
            the_same_session_subscribes_again_onto_its_row,
            a_known_endpoint_moves_to_the_session_that_sends_it,
            the_list_holds_one_workspace_only,
            mark_sent_records_the_time_of_the_last_push,
            delete_by_endpoint_removes_the_row_of_that_endpoint,
            delete_removes_one_row,
            one_workspace_never_writes_another_workspace_subscription,
            the_end_of_a_session_removes_its_push_subscriptions,
        );
    };
}
