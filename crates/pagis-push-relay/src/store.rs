//! The SQLite file of the relay. It is the relay's own: the relay runs
//! apart from every Pagis installation and shares no store with one.

use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::registration::{NewRegistration, Platform, RegistrationId, Token, VapidKey};
use crate::transport::Registration;

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

/// The `registrations` table. A change or a removal by the installation
/// names the id and the SHA-256 of the secret together, so a wrong
/// secret and an unknown id are the same miss. A push names the id
/// alone, because the VAPID token proves the sender.
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

    /// The device and the VAPID Key of the registration `id`, or `None`
    /// when no registration has this id.
    pub(crate) async fn find(&self, id: &RegistrationId) -> Result<Option<Stored>, sqlx::Error> {
        let row: Option<(String, Option<String>, String, Vec<u8>)> = sqlx::query_as(
            "SELECT platform, environment, token, vapid_key FROM registrations WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        let Some((platform, environment, token, vapid_key)) = row else {
            return Ok(None);
        };
        let platform = Platform::from_names(&platform, environment.as_deref())
            .ok_or_else(|| corrupt("platform"))?;
        let vapid_key = VapidKey::from_bytes(&vapid_key).ok_or_else(|| corrupt("vapid_key"))?;
        Ok(Some(Stored {
            registration: Registration { platform, token },
            vapid_key,
        }))
    }

    /// Count one push of the registration `id` in the UTC day of `now`.
    /// `false`, and no count, when the registration already had `limit`
    /// pushes in that day. A count of an earlier day starts again at
    /// one.
    pub(crate) async fn count_push(
        &self,
        id: &RegistrationId,
        now: SystemTime,
        limit: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE registrations \
             SET pushes_today = CASE WHEN day = date(?1, 'unixepoch') \
                                     THEN pushes_today + 1 ELSE 1 END, \
                 day = date(?1, 'unixepoch') \
             WHERE id = ?2 AND (day IS NOT date(?1, 'unixepoch') OR pushes_today < ?3)",
        )
        .bind(unix_seconds(now))
        .bind(id.as_str())
        .bind(limit)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Take back the count of one push that [`count_push`] made at `now`.
    ///
    /// [`count_push`]: Self::count_push
    pub(crate) async fn uncount_push(
        &self,
        id: &RegistrationId,
        now: SystemTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE registrations SET pushes_today = pushes_today - 1 \
             WHERE id = ? AND day = date(?, 'unixepoch') AND pushes_today > 0",
        )
        .bind(id.as_str())
        .bind(unix_seconds(now))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Keep `now` as the time of the last delivered push.
    pub(crate) async fn delivered(
        &self,
        id: &RegistrationId,
        now: SystemTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE registrations SET last_push_at = ? WHERE id = ?")
            .bind(unix_millis(now))
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove the registration of a device that is gone. The push service
    /// says so, so no secret is needed.
    pub(crate) async fn remove_gone(&self, id: &RegistrationId) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM registrations WHERE id = ?")
            .bind(id.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// A registration as a push reads it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub(crate) registration: Registration,
    pub(crate) vapid_key: VapidKey,
}

/// A row that breaks a rule of the table.
fn corrupt(column: &str) -> sqlx::Error {
    sqlx::Error::Decode(format!("the registrations row has a bad {column}").into())
}

fn now_millis() -> i64 {
    unix_millis(SystemTime::now())
}

fn unix_millis(time: SystemTime) -> i64 {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_millis()).expect("the time fits in i64")
}

fn unix_seconds(time: SystemTime) -> i64 {
    unix_millis(time) / 1000
}
