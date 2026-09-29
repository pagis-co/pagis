use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool};

/// Open (creating if absent) the database file with the pinned pragmas:
/// WAL journal, busy_timeout, foreign keys on, secure delete on.
///
/// `secure_delete` overwrites the content of a freed page instead of
/// leaving it readable in the file. An owner forget therefore erases the
/// prose at the delete, and does not depend on the later `VACUUM` that
/// only reclaims the space.
pub async fn connect(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .pragma("secure_delete", "ON")
        .foreign_keys(true);
    SqlitePoolOptions::new().connect_with(options).await
}

/// Open one real in-memory SQLite connection for module tests.
pub async fn connect_memory() -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")?
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
}

/// Start a transaction that writes.
///
/// `BEGIN` (what `Pool::begin` sends) opens a deferred transaction: it
/// takes a read snapshot, and it asks for the write lock only at the
/// first write. If another connection committed in between, SQLite
/// gives `SQLITE_BUSY_SNAPSHOT` (code 517) at once and never calls the
/// busy handler, so `busy_timeout` cannot save it. `BEGIN IMMEDIATE`
/// takes the write lock up front, where the busy handler does apply,
/// so concurrent writers queue instead of failing.
pub async fn begin_write(
    pool: &SqlitePool,
) -> Result<sqlx::Transaction<'static, Sqlite>, sqlx::Error> {
    pool.begin_with("BEGIN IMMEDIATE").await
}
