//! The Org, the people in it, the Sessions they hold, and the one-time
//! sign-in links.

use async_trait::async_trait;
use pagis_core::{
    ClientKind, Org, OrgId, OrgStore, Session, SessionId, SessionStore, SignInLink,
    SignInLinkStore, StoreError, UnixMillis, User, UserId, UserRole, UserStore, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct PostgresOrgStore {
    pool: PgPool,
}

impl PostgresOrgStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OrgStore for PostgresOrgStore {
    async fn create(&self, org: &Org) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(db_err)?;
        // The Org's Workspace has no person: user_id stays NULL, so the
        // Workspace store never answers it.
        sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES ($1, $2, $3)")
            .bind(org.workspace_id.as_str())
            .bind(&org.name)
            .bind(org.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO orgs (id, name, google_client_id, workspace_id, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(org.id.as_str())
        .bind(&org.name)
        .bind(org.google_client_id.as_deref())
        .bind(org.workspace_id.as_str())
        .bind(org.created_at)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Org>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, google_client_id, workspace_id, created_at FROM orgs ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| Org {
                id: OrgId::from(row.get::<String, _>("id")),
                name: row.get("name"),
                google_client_id: row.get("google_client_id"),
                workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn set_google_client_id(
        &self,
        id: &OrgId,
        client_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query("UPDATE orgs SET google_client_id = $1 WHERE id = $2")
            .bind(client_id)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }
}

#[derive(Clone)]
pub struct PostgresUserStore {
    pool: PgPool,
}

impl PostgresUserStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const USER_COLUMNS: &str = "id, org_id, email, name, password_hash, role, disabled_at, \
     last_signed_in_at, monthly_spend_cap_usd, created_at, updated_at";

fn row_to_user(row: &sqlx::postgres::PgRow) -> Result<User, StoreError> {
    let role: String = row.get("role");
    Ok(User {
        id: UserId::from(row.get::<String, _>("id")),
        org_id: OrgId::from(row.get::<String, _>("org_id")),
        email: row.get("email"),
        name: row.get("name"),
        password_hash: row.get("password_hash"),
        role: UserRole::parse(&role)
            .ok_or_else(|| StoreError::Corrupt(format!("unknown user role {role}")))?,
        disabled_at: row.get("disabled_at"),
        last_signed_in_at: row.get("last_signed_in_at"),
        monthly_spend_cap_usd: row.get("monthly_spend_cap_usd"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[async_trait]
impl UserStore for PostgresUserStore {
    async fn create(&self, user: &User) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO users ({USER_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"
        ))
        .bind(user.id.as_str())
        .bind(user.org_id.as_str())
        .bind(user.email.as_deref().map(str::to_lowercase))
        .bind(user.name.as_deref())
        .bind(user.password_hash.as_deref())
        .bind(user.role.as_str())
        .bind(user.disabled_at)
        .bind(user.last_signed_in_at)
        .bind(user.monthly_spend_cap_usd)
        .bind(user.created_at)
        .bind(user.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict("that email address already has a person".to_string())
            } else {
                db_err(error)
            }
        })?;
        Ok(())
    }

    async fn get(&self, id: &UserId) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.as_ref().map(row_to_user).transpose()
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email = $1"
        ))
        .bind(email.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_user).transpose()
    }

    async fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<User>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE org_id = $1 ORDER BY id"
        ))
        .bind(org_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_user).collect()
    }

    async fn set_name(&self, id: &UserId, name: &str, at: UnixMillis) -> Result<(), StoreError> {
        sqlx::query("UPDATE users SET name = $1, updated_at = $2 WHERE id = $3")
            .bind(name)
            .bind(at)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_email_and_password(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE users SET email = $1, password_hash = $2, updated_at = $3 WHERE id = $4",
        )
        .bind(email.trim().to_lowercase())
        .bind(password_hash)
        .bind(at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict("that email address already has a person".to_string())
            } else {
                db_err(error)
            }
        })?;
        Ok(updated.rows_affected() > 0)
    }

    async fn claim_first_administrator(
        &self,
        id: &UserId,
        email: &str,
        password_hash: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        // Every claim in one Org queues on the Org's row. Postgres locks
        // row by row, and without this lock two claims on two
        // Administrators could each read the other as without a
        // password. The update after the lock reads a new snapshot, so
        // it sees the password of a claim that went first.
        sqlx::query(
            "SELECT id FROM orgs WHERE id = (SELECT org_id FROM users WHERE id = $1) FOR UPDATE",
        )
        .bind(id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let updated = sqlx::query(
            "UPDATE users SET email = $1, password_hash = $2, updated_at = $3 \
             WHERE id = $4 AND role = 'administrator' AND password_hash IS NULL \
             AND NOT EXISTS (SELECT 1 FROM users AS holder \
                 WHERE holder.org_id = users.org_id \
                 AND holder.role = 'administrator' \
                 AND holder.password_hash IS NOT NULL)",
        )
        .bind(email.trim().to_lowercase())
        .bind(password_hash)
        .bind(at)
        .bind(id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict("that email address already has a person".to_string())
            } else {
                db_err(error)
            }
        })?;
        transaction.commit().await.map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_password_hash(
        &self,
        id: &UserId,
        hash: Option<&str>,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated =
            sqlx::query("UPDATE users SET password_hash = $1, updated_at = $2 WHERE id = $3")
                .bind(hash)
                .bind(at)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_disabled(
        &self,
        id: &UserId,
        disabled_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated =
            sqlx::query("UPDATE users SET disabled_at = $1, updated_at = $2 WHERE id = $3")
                .bind(disabled_at)
                .bind(at)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_monthly_spend_cap(
        &self,
        id: &UserId,
        cap_usd: Option<f64>,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE users SET monthly_spend_cap_usd = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(cap_usd)
        .bind(at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn record_sign_in(&self, id: &UserId, at: UnixMillis) -> Result<(), StoreError> {
        sqlx::query("UPDATE users SET last_signed_in_at = $1 WHERE id = $2")
            .bind(at)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct PostgresSessionStore {
    pool: PgPool,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SESSION_COLUMNS: &str =
    "id, user_id, token_hash, client_kind, client_name, created_at, last_used_at, expires_at";

fn row_to_session(row: &sqlx::postgres::PgRow) -> Result<Session, StoreError> {
    let kind: String = row.get("client_kind");
    Ok(Session {
        id: SessionId::from(row.get::<String, _>("id")),
        user_id: UserId::from(row.get::<String, _>("user_id")),
        token_hash: row.get("token_hash"),
        client_kind: ClientKind::parse(&kind)
            .ok_or_else(|| StoreError::Corrupt(format!("unknown client kind {kind}")))?,
        client_name: row.get("client_name"),
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
        expires_at: row.get("expires_at"),
    })
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn create(&self, session: &Session) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO sessions ({SESSION_COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        ))
        .bind(session.id.as_str())
        .bind(session.user_id.as_str())
        .bind(&session.token_hash)
        .bind(session.client_kind.as_str())
        .bind(session.client_name.as_deref())
        .bind(session.created_at)
        .bind(session.last_used_at)
        .bind(session.expires_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn find_live(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE token_hash = $1 AND expires_at > $2"
        ))
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_session).transpose()
    }

    async fn find_live_by_id(
        &self,
        id: &SessionId,
        now: UnixMillis,
    ) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE id = $1 AND expires_at > $2"
        ))
        .bind(id.as_str())
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_session).transpose()
    }

    async fn list_live(&self, now: UnixMillis) -> Result<Vec<Session>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE expires_at > $1 \
             ORDER BY created_at DESC, id DESC"
        ))
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_session).collect()
    }

    async fn touch(&self, id: &SessionId, at: UnixMillis) -> Result<(), StoreError> {
        sqlx::query("UPDATE sessions SET last_used_at = $1 WHERE id = $2")
            .bind(at)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn delete(&self, id: &SessionId) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_for_user(&self, user_id: &UserId) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected())
    }

    async fn delete_expired(&self, now: UnixMillis) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected())
    }
}

#[derive(Clone)]
pub struct PostgresSignInLinkStore {
    pool: PgPool,
}

impl PostgresSignInLinkStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SignInLinkStore for PostgresSignInLinkStore {
    async fn create(&self, link: &SignInLink) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO sign_in_links (id, user_id, token_hash, created_at, expires_at, used_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(link.id.as_str())
        .bind(link.user_id.as_str())
        .bind(&link.token_hash)
        .bind(link.created_at)
        .bind(link.expires_at)
        .bind(link.used_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn consume(
        &self,
        token_hash: &str,
        now: UnixMillis,
    ) -> Result<Option<UserId>, StoreError> {
        // The UPDATE is the guard: one row moves from unused to used, so
        // two requests with the same link cannot both sign in.
        let spent = sqlx::query(
            "UPDATE sign_in_links SET used_at = $1 \
             WHERE token_hash = $2 AND used_at IS NULL AND expires_at > $3",
        )
        .bind(now)
        .bind(token_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if spent.rows_affected() == 0 {
            return Ok(None);
        }
        let row = sqlx::query("SELECT user_id FROM sign_in_links WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(Some(UserId::from(row.get::<String, _>("user_id"))))
    }
}

/// Write the one Org of an installation and its administrator over
/// this backend's stores. The rule is in
/// [`pagis_core::seed_org_and_administrator`]; this is the pool-shaped
/// call the seed and the tests of this crate make.
pub async fn seed_org_and_administrator(
    pool: &PgPool,
    name: &str,
    now: UnixMillis,
) -> Result<User, StoreError> {
    pagis_core::seed_org_and_administrator(
        &PostgresOrgStore::new(pool.clone()),
        &PostgresUserStore::new(pool.clone()),
        name,
        now,
    )
    .await
}
