//! The SQLite file of the relay. It is the relay's own: the relay runs
//! apart from every Pagis installation and shares no store with one.

use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::registration::{NewRegistration, RegistrationId, Token};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Open the SQLite file at `path` in WAL mode, make it when it is
/// absent, and run the migrations.
pub async fn connect(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(BUSY_TIMEOUT);
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Open one in-memory SQLite connection with the migrations run, for a
/// test.
pub async fn connect_memory() -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")?.busy_timeout(BUSY_TIMEOUT);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// The `registrations` table. A change or a removal names the id and
/// the SHA-256 of the secret together, so a wrong secret and an unknown
/// id are the same miss.
#[derive(Debug, Clone)]
pub(crate) struct Registrations {
    pool: SqlitePool,
}

impl Registrations {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn insert(
        &self,
        id: &RegistrationId,
        secret_hash: &[u8; 32],
        registration: &NewRegistration,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO registrations \
             (id, secret_hash, platform, environment, token, vapid_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.as_str())
        .bind(secret_hash.as_slice())
        .bind(registration.platform.name())
        .bind(registration.platform.environment())
        .bind(registration.token.as_str())
        .bind(registration.vapid_key.as_bytes())
        .bind(now_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Change the token. `false` when no registration has this id and
    /// this secret.
    pub(crate) async fn change_token(
        &self,
        id: &RegistrationId,
        secret_hash: &[u8; 32],
        token: &Token,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("UPDATE registrations SET token = ? WHERE id = ? AND secret_hash = ?")
                .bind(token.as_str())
                .bind(id.as_str())
                .bind(secret_hash.as_slice())
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Remove the registration. `false` when no registration has this id
    /// and this secret.
    pub(crate) async fn remove(
        &self,
        id: &RegistrationId,
        secret_hash: &[u8; 32],
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM registrations WHERE id = ? AND secret_hash = ?")
            .bind(id.as_str())
            .bind(secret_hash.as_slice())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}

fn now_millis() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_millis()).expect("the time fits in i64")
}
