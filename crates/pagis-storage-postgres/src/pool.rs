use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};

/// How long a statement waits for a lock another transaction holds
/// before Postgres refuses it. The SQLite backend gives the same five
/// seconds to its busy handler, so a contended write behaves the same
/// on both backends: it waits, and it reports the wait when the lock
/// does not come free.
const LOCK_TIMEOUT: &str = "5s";

/// Open a pool against the database the configuration names.
///
/// Postgres holds one writer per row, not one per database, so the pool
/// carries several connections. `lock_timeout` bounds the wait for a
/// row another transaction holds.
pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let options: PgConnectOptions = url.parse()?;
    PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query(&format!("SET lock_timeout = '{LOCK_TIMEOUT}'"))
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
}

/// Start a transaction that writes.
///
/// Postgres takes its locks row by row as the transaction writes, so
/// there is nothing to take up front and this is a plain `BEGIN`. The
/// function exists so the two storage crates read the same: the SQLite
/// one needs `BEGIN IMMEDIATE` to make concurrent writers queue instead
/// of fail, and a reader who diffs the two files sees one call in both.
pub async fn begin_write(pool: &PgPool) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    pool.begin().await
}
