//! The Sessions of a Person and the Sign-In Links that hand them out,
//! on both backends (ADR-0028).
//!
//! A Session ends a fixed time after its last use, so a use moves its
//! expiry. A Sign-In Link is spent once, while it lives, and only by the
//! route of its own kind. Name every new body in
//! `store_suite_sign_in!` below; the guard test of the parent module
//! fails while one is missing.

use pagis_core::{
    ClientKind, Session, SessionId, SignInLink, SignInLinkId, SignInLinkKind, User, UserId,
    UserRole, now_ms, seed_org_and_administrator,
};

use super::Backend;

/// The seeded Administrator of a new Org.
async fn administrator(backend: &Backend) -> User {
    seed_org_and_administrator(
        backend.stores().orgs.as_ref(),
        backend.stores().users.as_ref(),
        "Org",
        now_ms(),
    )
    .await
    .expect("seed the Org and its administrator")
}

/// A browser Session of `user_id`, made at `created_at`, that ends at
/// `expires_at`.
fn session(user_id: &UserId, created_at: i64, expires_at: i64) -> Session {
    let id = SessionId::generate();
    Session {
        token_hash: format!("hash-{id}"),
        id,
        user_id: user_id.clone(),
        client_kind: ClientKind::Browser,
        client_name: Some("Safari on macOS".to_string()),
        created_at,
        last_used_at: created_at,
        expires_at,
    }
}

/// A link of `kind` for `user_id`, with the secret hash `token_hash`,
/// that lives until `expires_at`.
fn link(user_id: &UserId, token_hash: &str, kind: SignInLinkKind, expires_at: i64) -> SignInLink {
    SignInLink {
        id: SignInLinkId::generate(),
        user_id: user_id.clone(),
        token_hash: token_hash.to_string(),
        kind,
        created_at: 1_000,
        expires_at,
        used_at: None,
    }
}

/// A use moves the expiry of a Session, so it lives on past the end it
/// had at the sign-in, and it ends at the new expiry.
pub async fn a_use_moves_the_expiry_of_a_session(backend: &Backend) {
    let person = administrator(backend).await;
    let sessions = &backend.stores().sessions;
    let used = session(&person.id, 1_000, 9_000);
    sessions.create(&used).await.unwrap();

    sessions.touch(&used.id, 5_000, 20_000).await.unwrap();

    let found = sessions
        .find_live(&used.token_hash, 10_000)
        .await
        .unwrap()
        .expect("the Session lives past its first expiry");
    assert_eq!(found.last_used_at, 5_000);
    assert_eq!(found.expires_at, 20_000);
    assert_eq!(found.created_at, 1_000, "a use is not a new sign-in");
    assert!(
        sessions
            .find_live(&used.token_hash, 20_000)
            .await
            .unwrap()
            .is_none(),
        "the Session ends at its new expiry"
    );
}

/// A Person reads their own live Sessions, newest first, and none of
/// another Person's and none that expired.
pub async fn a_person_lists_their_own_live_sessions_newest_first(backend: &Backend) {
    let administrator = administrator(backend).await;
    let member = User {
        email: Some("grace@example.com".to_string()),
        ..User::new(administrator.org_id.clone(), UserRole::Member, now_ms())
    };
    backend.stores().users.create(&member).await.unwrap();
    let sessions = &backend.stores().sessions;
    let laptop = session(&member.id, 1_000, 9_000);
    let phone = session(&member.id, 2_000, 9_000);
    let expired = session(&member.id, 3_000, 4_000);
    let someone_else = session(&administrator.id, 2_500, 9_000);
    for row in [&laptop, &phone, &expired, &someone_else] {
        sessions.create(row).await.unwrap();
    }

    let own = sessions
        .list_live_for_user(&member.id, 5_000)
        .await
        .unwrap();

    assert_eq!(
        own.iter().map(|row| row.id.clone()).collect::<Vec<_>>(),
        vec![phone.id.clone(), laptop.id.clone()]
    );
    assert_eq!(own[0].client_name.as_deref(), Some("Safari on macOS"));
    assert!(
        sessions
            .list_live_for_user(&UserId::generate(), 5_000)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A link answers its Person once, and a second spend answers nothing.
/// A link past its expiry answers nothing at all.
pub async fn a_sign_in_link_is_spent_once_while_it_lives(backend: &Backend) {
    let person = administrator(backend).await;
    let links = &backend.stores().sign_in_links;
    links
        .create(&link(
            &person.id,
            "hash-live",
            SignInLinkKind::PublicOrigin,
            5_000,
        ))
        .await
        .unwrap();
    links
        .create(&link(
            &person.id,
            "hash-old",
            SignInLinkKind::PublicOrigin,
            2_000,
        ))
        .await
        .unwrap();

    assert_eq!(
        links
            .consume("hash-live", SignInLinkKind::PublicOrigin, 4_000)
            .await
            .unwrap(),
        Some(person.id.clone())
    );
    assert_eq!(
        links
            .consume("hash-live", SignInLinkKind::PublicOrigin, 4_000)
            .await
            .unwrap(),
        None,
        "a link is good for one use"
    );
    assert_eq!(
        links
            .consume("hash-old", SignInLinkKind::PublicOrigin, 2_000)
            .await
            .unwrap(),
        None,
        "a link ends at its expiry"
    );
    assert_eq!(
        links
            .consume("hash-nobody-made", SignInLinkKind::PublicOrigin, 1_500)
            .await
            .unwrap(),
        None
    );
}

/// Each route spends a link of its own kind alone. A link offered as
/// the other kind stays unspent, so it still opens the way it was made
/// for.
pub async fn a_sign_in_link_is_spent_only_as_its_own_kind(backend: &Backend) {
    let person = administrator(backend).await;
    let links = &backend.stores().sign_in_links;
    links
        .create(&link(
            &person.id,
            "hash-start",
            SignInLinkKind::Start,
            9_000,
        ))
        .await
        .unwrap();
    links
        .create(&link(
            &person.id,
            "hash-public",
            SignInLinkKind::PublicOrigin,
            9_000,
        ))
        .await
        .unwrap();

    assert_eq!(
        links
            .consume("hash-start", SignInLinkKind::PublicOrigin, 2_000)
            .await
            .unwrap(),
        None,
        "a start link is not a link of the Public Origin"
    );
    assert_eq!(
        links
            .consume("hash-public", SignInLinkKind::Start, 2_000)
            .await
            .unwrap(),
        None,
        "a link of the Public Origin is not a start link"
    );
    assert_eq!(
        links
            .consume("hash-start", SignInLinkKind::Start, 2_000)
            .await
            .unwrap(),
        Some(person.id.clone())
    );
    assert_eq!(
        links
            .consume("hash-public", SignInLinkKind::PublicOrigin, 2_000)
            .await
            .unwrap(),
        Some(person.id.clone())
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_sign_in {
    ($emit:path) => {
        $emit!(
            sign_in,
            a_use_moves_the_expiry_of_a_session,
            a_person_lists_their_own_live_sessions_newest_first,
            a_sign_in_link_is_spent_once_while_it_lives,
            a_sign_in_link_is_spent_only_as_its_own_kind,
        );
    };
}
