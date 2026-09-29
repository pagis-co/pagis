//! The failed-attempt count of each Workspace's Keypad Code (ADR-0021),
//! on both backends.
//!
//! The count belongs to the Workspace, so every body here writes it
//! through the Workspace and reads it back the same way. Name every new
//! body in `store_suite_keypad_failures!` below; the guard test of the
//! parent module fails while one is missing.

use pagis_core::{KeypadFailures, Workspace, WorkspaceId};

use super::Backend;

const MINUTE: i64 = 60 * 1_000;
const NOW: i64 = 1_790_305_200_000;

/// A second Workspace of the same Org, for the counts that must not
/// cross one.
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

/// Count `failures` wrong codes in one Workspace, one second apart.
async fn fail(backend: &Backend, workspace_id: &WorkspaceId, failures: u32) -> KeypadFailures {
    let mut count = KeypadFailures::default();
    for index in 0..failures {
        count = backend
            .stores()
            .keypad_failures
            .record_failure(workspace_id, NOW + i64::from(index) * 1_000)
            .await
            .unwrap();
    }
    count
}

pub async fn a_workspace_with_no_failure_has_no_count_and_no_delay(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;

    assert_eq!(
        backend
            .stores()
            .keypad_failures
            .get(&workspace.id)
            .await
            .unwrap(),
        KeypadFailures::default()
    );
}

pub async fn each_failure_adds_to_the_count_of_the_workspace(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;

    let counted = fail(backend, &workspace.id, 3).await;

    assert_eq!(counted.failed_attempts, 3);
    assert_eq!(counted.suspended_until, None);
    assert_eq!(
        backend
            .stores()
            .keypad_failures
            .get(&workspace.id)
            .await
            .unwrap(),
        counted
    );
}

/// The count is the Workspace's: the wrong codes of one person's callers
/// change nothing of another person's count.
pub async fn the_failures_of_one_workspace_leave_another_alone(backend: &Backend) {
    let ada = backend.seeded_workspace().await;
    let grace = second_workspace(backend, &ada).await;

    fail(backend, &ada.id, 6).await;
    let failures = &backend.stores().keypad_failures;

    assert_eq!(failures.get(&ada.id).await.unwrap().failed_attempts, 6);
    assert_eq!(
        failures.get(&grace.id).await.unwrap(),
        KeypadFailures::default()
    );

    failures.clear(&grace.id).await.unwrap();
    assert_eq!(failures.get(&ada.id).await.unwrap().failed_attempts, 6);
}

/// The store keeps the delay the rule gives: none for five failures, one
/// minute from the sixth, twice that from the seventh.
pub async fn the_sixth_failure_starts_a_delay_and_the_next_doubles_it(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let failures = &backend.stores().keypad_failures;

    let fifth = fail(backend, &workspace.id, 5).await;
    assert_eq!(fifth.suspended_until, None);

    let sixth = failures
        .record_failure(&workspace.id, NOW + 10 * MINUTE)
        .await
        .unwrap();
    assert_eq!(sixth.failed_attempts, 6);
    assert_eq!(sixth.suspended_until, Some(NOW + 11 * MINUTE));

    let seventh = failures
        .record_failure(&workspace.id, NOW + 20 * MINUTE)
        .await
        .unwrap();
    assert_eq!(seventh.failed_attempts, 7);
    assert_eq!(seventh.suspended_until, Some(NOW + 22 * MINUTE));
    assert_eq!(failures.get(&workspace.id).await.unwrap(), seventh);
}

pub async fn a_clear_ends_the_count_and_the_delay(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let failures = &backend.stores().keypad_failures;
    fail(backend, &workspace.id, 7).await;

    failures.clear(&workspace.id).await.unwrap();

    assert_eq!(
        failures.get(&workspace.id).await.unwrap(),
        KeypadFailures::default()
    );
    let next = failures.record_failure(&workspace.id, NOW).await.unwrap();
    assert_eq!(next.failed_attempts, 1, "the count starts again");
    assert_eq!(next.suspended_until, None);
    failures.clear(&workspace.id).await.unwrap();
    failures.clear(&workspace.id).await.unwrap();
}

/// Two lines of one Workspace can take a wrong code at the same moment.
/// Each failure counts, so parallel Calls do not buy extra guesses.
pub async fn failures_at_the_same_moment_all_count(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let mut lines = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let failures = backend.stores().keypad_failures.clone();
        let workspace_id = workspace.id.clone();
        lines.spawn(async move { failures.record_failure(&workspace_id, NOW).await });
    }
    while let Some(counted) = lines.join_next().await {
        counted.expect("the task ran").expect("the failure counted");
    }

    let count = backend
        .stores()
        .keypad_failures
        .get(&workspace.id)
        .await
        .unwrap();
    assert_eq!(count.failed_attempts, 8);
    assert_eq!(count.suspended_until, Some(NOW + 4 * MINUTE));
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_keypad_failures {
    ($emit:path) => {
        $emit!(
            keypad_failures,
            a_workspace_with_no_failure_has_no_count_and_no_delay,
            each_failure_adds_to_the_count_of_the_workspace,
            the_failures_of_one_workspace_leave_another_alone,
            the_sixth_failure_starts_a_delay_and_the_next_doubles_it,
            a_clear_ends_the_count_and_the_delay,
            failures_at_the_same_moment_all_count,
        );
    };
}
