use async_trait::async_trait;
use pagis_core::{
    AgentId, Connection, ConnectionId, ConnectionStore, Credential, CredentialId,
    CredentialProvenance, CredentialStore, RunId, SealedSecret, StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct PostgresConnectionStore {
    pool: PgPool,
}

impl PostgresConnectionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const CONNECTION_COLUMNS: &str = "id, workspace_id, provider, alias, display_name, status, \
     authorized_capabilities, config, created_at";

/// A stored `config` that will not parse reads as empty: the provider
/// then reports its own missing-configuration error, which is clearer
/// than a store failure on an unrelated read.
fn connection_from_row(row: &sqlx::postgres::PgRow) -> Result<Connection, StoreError> {
    let authorized_capabilities =
        serde_json::from_str(&row.get::<String, _>("authorized_capabilities"))
            .map_err(|error| StoreError::Corrupt(format!("Connection capabilities: {error}")))?;
    Ok(Connection {
        id: ConnectionId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        provider: row.get("provider"),
        alias: row.get("alias"),
        display_name: row.get("display_name"),
        status: row.get("status"),
        authorized_capabilities,
        config: serde_json::from_str(&row.get::<String, _>("config"))
            .unwrap_or_else(|_| serde_json::json!({})),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl ConnectionStore for PostgresConnectionStore {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO connections ({CONNECTION_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        ))
        .bind(connection.id.as_str())
        .bind(connection.workspace_id.as_str())
        .bind(&connection.provider)
        .bind(&connection.alias)
        .bind(&connection.display_name)
        .bind(&connection.status)
        .bind(serde_json::to_string(&connection.authorized_capabilities).expect("string list"))
        .bind(connection.config.to_string())
        .bind(connection.created_at)
        .execute(&self.pool)
        .await
        .map_err(|error| match unique_violation(&error) {
            true => StoreError::Conflict(format!("alias {} is taken", connection.alias)),
            false => db_err(error),
        })?;
        Ok(())
    }

    async fn set_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError> {
        let updated =
            sqlx::query("UPDATE connections SET status = $1 WHERE workspace_id = $2 AND id = $3")
                .bind(status)
                .bind(workspace_id.as_str())
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_authorization(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError> {
        let capabilities = serde_json::to_string(capabilities)
            .map_err(|error| StoreError::Corrupt(format!("Connection capabilities: {error}")))?;
        let updated = sqlx::query(
            "UPDATE connections SET status = 'connected', authorized_capabilities = $1 \
             WHERE workspace_id = $2 AND id = $3",
        )
        .bind(capabilities)
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn set_config(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let updated =
            sqlx::query("UPDATE connections SET config = $1 WHERE workspace_id = $2 AND id = $3")
                .bind(config.to_string())
                .bind(workspace_id.as_str())
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {CONNECTION_COLUMNS} FROM connections WHERE workspace_id = $1 ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(connection_from_row).collect()
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {CONNECTION_COLUMNS} FROM connections WHERE workspace_id = $1 AND id = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(connection_from_row).transpose()
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        delete_resource(
            &self.pool,
            workspace_id,
            id.as_str(),
            "connections",
            "connection",
            revoked_at,
        )
        .await
    }

    async fn set_refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
        token: Option<&pagis_core::SealedSecret>,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE connections SET refresh_token = $1 WHERE workspace_id = $2 AND id = $3",
        )
        .bind(token.map(|token| token.0.clone()))
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() > 0)
    }

    async fn refresh_token(
        &self,
        workspace_id: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<pagis_core::SealedSecret>, StoreError> {
        let row = sqlx::query(
            "SELECT refresh_token FROM connections WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row
            .and_then(|row| row.get::<Option<Vec<u8>>, _>("refresh_token"))
            .map(pagis_core::SealedSecret))
    }
}

#[derive(Clone)]
pub struct PostgresCredentialStore {
    pool: PgPool,
}

impl PostgresCredentialStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const CREDENTIAL_COLUMNS: &str = "id, workspace_id, domain, username, login_url, secret, \
     totp_seed, recipe, owner_agent_id, provenance, created_run_id, created_at";

fn row_to_credential(row: &sqlx::postgres::PgRow) -> Result<Credential, StoreError> {
    let provenance: String = row.get("provenance");
    Ok(Credential {
        id: CredentialId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        domain: row.get("domain"),
        username: row.get("username"),
        login_url: row.get("login_url"),
        secret: SealedSecret(row.get::<Vec<u8>, _>("secret")),
        totp_seed: row.get::<Option<Vec<u8>>, _>("totp_seed").map(SealedSecret),
        recipe: row.get("recipe"),
        owner_agent_id: row
            .get::<Option<String>, _>("owner_agent_id")
            .map(AgentId::from),
        provenance: CredentialProvenance::parse(&provenance).ok_or_else(|| {
            StoreError::Corrupt(format!("unknown credential provenance {provenance:?}"))
        })?,
        created_run_id: row
            .get::<Option<String>, _>("created_run_id")
            .map(RunId::from),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl CredentialStore for PostgresCredentialStore {
    async fn create(&self, credential: &Credential) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO credentials (id, workspace_id, domain, username, login_url, secret, \
             totp_seed, recipe, owner_agent_id, provenance, created_run_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(credential.id.as_str())
        .bind(credential.workspace_id.as_str())
        .bind(&credential.domain)
        .bind(&credential.username)
        .bind(&credential.login_url)
        .bind(&credential.secret.0)
        .bind(credential.totp_seed.as_ref().map(|seed| seed.0.clone()))
        .bind(&credential.recipe)
        .bind(credential.owner_agent_id.as_ref().map(|id| id.to_string()))
        .bind(credential.provenance.as_str())
        .bind(credential.created_run_id.as_ref().map(|id| id.to_string()))
        .bind(credential.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CredentialId,
    ) -> Result<Option<Credential>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_credential).transpose()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        domain: Option<&str>,
    ) -> Result<Vec<Credential>, StoreError> {
        let filter = match domain {
            Some(_) => "AND domain = $2",
            None => "",
        };
        let sql = format!(
            "SELECT {CREDENTIAL_COLUMNS} FROM credentials \
             WHERE workspace_id = $1 {filter} ORDER BY created_at DESC, id"
        );
        let mut query = sqlx::query(&sql).bind(workspace_id.as_str());
        if let Some(domain) = domain {
            query = query.bind(domain);
        }
        let rows = query.fetch_all(&self.pool).await.map_err(db_err)?;
        rows.iter().map(row_to_credential).collect()
    }

    async fn delete_and_revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &CredentialId,
        revoked_at: i64,
    ) -> Result<bool, StoreError> {
        delete_resource(
            &self.pool,
            workspace_id,
            id.as_str(),
            "credentials",
            "credential",
            revoked_at,
        )
        .await
    }

    async fn release_owner(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u64, StoreError> {
        let released = sqlx::query(
            "UPDATE credentials SET owner_agent_id = NULL \
             WHERE owner_agent_id = $1 AND workspace_id = $2",
        )
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(released.rows_affected())
    }
}

async fn delete_resource(
    pool: &PgPool,
    workspace_id: &WorkspaceId,
    id: &str,
    table: &str,
    kind: &str,
    revoked_at: i64,
) -> Result<bool, StoreError> {
    let mut transaction = crate::pool::begin_write(pool).await.map_err(db_err)?;
    let deleted = sqlx::query(&format!(
        "DELETE FROM {table} WHERE workspace_id = $1 AND id = $2"
    ))
    .bind(workspace_id.as_str())
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(db_err)?;
    if deleted.rows_affected() == 0 {
        transaction.rollback().await.map_err(db_err)?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE grants SET revoked_at = $1 \
         WHERE workspace_id = $2 AND resource_kind = $3 AND resource_id = $4 AND revoked_at IS NULL",
    )
    .bind(revoked_at)
    .bind(workspace_id.as_str())
    .bind(kind)
    .bind(id)
    .execute(&mut *transaction)
    .await
    .map_err(db_err)?;
    transaction.commit().await.map_err(db_err)?;
    Ok(true)
}
