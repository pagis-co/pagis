//! `connect` pins the required pragmas on every connection.

use sqlx::Row;

#[tokio::test]
async fn connect_pins_wal_busy_timeout_and_foreign_keys() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pagis_storage_sqlite::connect(&dir.path().join("pagis.db"))
        .await
        .unwrap();

    let journal_mode: String = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(journal_mode.to_lowercase(), "wal");

    let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(foreign_keys, 1);

    let busy_timeout: i64 = sqlx::query("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert!(busy_timeout > 0);
}

/// A write transaction must not lose the race against a commit from
/// another connection. `BEGIN IMMEDIATE` takes the write lock at the
/// start, so a second writer waits for the lock; a deferred `BEGIN`
/// starts at once and then fails at its first write with
/// `SQLITE_BUSY_SNAPSHOT`.
#[tokio::test]
async fn begin_write_takes_the_write_lock_up_front() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pagis_storage_sqlite::connect(&dir.path().join("pagis.db"))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE marks (id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();

    let first = pagis_storage_sqlite::begin_write(&pool).await.unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        pagis_storage_sqlite::begin_write(&pool),
    )
    .await;
    assert!(second.is_err(), "the second writer waits for the lock");

    first.rollback().await.unwrap();
    pool.close().await;
}
