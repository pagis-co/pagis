//! `connect` gives every connection the settings the daemon needs.

use pagis_testkit::postgres;

#[tokio::test]
async fn connect_bounds_the_wait_for_a_lock() {
    let Some(database) = postgres::database().await else {
        return;
    };

    // The SQLite backend gives its busy handler five seconds, so a
    // contended write behaves the same on both: it waits, and it reports
    // the wait instead of waiting for ever.
    let lock_timeout: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&database.pool)
        .await
        .unwrap();

    assert_eq!(lock_timeout, "5s");
}

#[tokio::test]
async fn a_write_transaction_needs_no_lock_up_front() {
    let Some(database) = postgres::database().await else {
        return;
    };

    // `begin_write` is a plain `BEGIN` here. The SQLite backend needs
    // `BEGIN IMMEDIATE` to make concurrent writers queue instead of
    // fail; Postgres takes its locks row by row as the transaction
    // writes, so two of them start at once.
    let first = pagis_storage_postgres::begin_write(&database.pool)
        .await
        .unwrap();
    let second = pagis_storage_postgres::begin_write(&database.pool)
        .await
        .unwrap();

    first.rollback().await.unwrap();
    second.rollback().await.unwrap();
}
