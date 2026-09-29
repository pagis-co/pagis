//! The Software List (ADR-0016).
//!
//! A publish writes the Version row and moves the package to it in one
//! transaction, so a package never names a Version that is not stored.

use async_trait::async_trait;
use pagis_core::{
    AgentId, RunId, SoftwarePackage, SoftwarePackageId, SoftwareStore, SoftwareVersion, StoreError,
    WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::{db_err, unique_violation};

#[derive(Clone)]
pub struct SqliteSoftwareStore {
    pool: SqlitePool,
}

impl SqliteSoftwareStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const PACKAGE_COLUMNS: &str = "id, workspace_id, name, author_agent_id, description, keywords, \
     latest_version, origin_package_id, origin_version, created_at, updated_at";
const VERSION_COLUMNS: &str =
    "package_id, version, notes, commit_id, manifest, published_at, run_id";

fn package_from(row: &sqlx::sqlite::SqliteRow) -> Result<SoftwarePackage, StoreError> {
    let keywords: String = row.get("keywords");
    Ok(SoftwarePackage {
        id: SoftwarePackageId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        name: row.get("name"),
        author_agent_id: AgentId::from(row.get::<String, _>("author_agent_id")),
        description: row.get("description"),
        keywords: serde_json::from_str(&keywords)
            .map_err(|error| StoreError::Corrupt(format!("keywords: {error}")))?,
        latest_version: row.get("latest_version"),
        origin_package_id: row
            .get::<Option<String>, _>("origin_package_id")
            .map(SoftwarePackageId::from),
        origin_version: row.get("origin_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn version_from(row: &sqlx::sqlite::SqliteRow) -> Result<SoftwareVersion, StoreError> {
    let manifest: String = row.get("manifest");
    Ok(SoftwareVersion {
        package_id: SoftwarePackageId::from(row.get::<String, _>("package_id")),
        version: row.get("version"),
        notes: row.get("notes"),
        commit_id: row.get("commit_id"),
        manifest: serde_json::from_str(&manifest)
            .map_err(|error| StoreError::Corrupt(format!("manifest: {error}")))?,
        published_at: row.get("published_at"),
        run_id: RunId::from(row.get::<String, _>("run_id")),
    })
}

#[async_trait]
impl SoftwareStore for SqliteSoftwareStore {
    async fn create_package(&self, package: &SoftwarePackage) -> Result<(), StoreError> {
        let keywords = serde_json::to_string(&package.keywords)
            .map_err(|error| StoreError::Corrupt(format!("keywords: {error}")))?;
        sqlx::query(&format!(
            "INSERT INTO software_packages ({PACKAGE_COLUMNS}) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(package.id.as_str())
        .bind(package.workspace_id.as_str())
        .bind(&package.name)
        .bind(package.author_agent_id.as_str())
        .bind(&package.description)
        .bind(&keywords)
        .bind(&package.latest_version)
        .bind(package.origin_package_id.as_ref().map(|id| id.as_str()))
        .bind(package.origin_version.as_deref())
        .bind(package.created_at)
        .bind(package.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict(format!("the package {} already exists", package.name))
            } else {
                db_err(error)
            }
        })?;
        Ok(())
    }

    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<SoftwarePackage>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {PACKAGE_COLUMNS} FROM software_packages WHERE workspace_id = ? AND name = ?"
        ))
        .bind(workspace_id.as_str())
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(package_from).transpose()
    }

    async fn list_packages(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<SoftwarePackage>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {PACKAGE_COLUMNS} FROM software_packages WHERE workspace_id = ? ORDER BY name"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(package_from).collect()
    }

    async fn add_version(
        &self,
        package: &SoftwarePackage,
        version: &SoftwareVersion,
    ) -> Result<(), StoreError> {
        let manifest = serde_json::to_string(&version.manifest)
            .map_err(|error| StoreError::Corrupt(format!("manifest: {error}")))?;
        let keywords = serde_json::to_string(&package.keywords)
            .map_err(|error| StoreError::Corrupt(format!("keywords: {error}")))?;
        let mut tx = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(&format!(
            "INSERT INTO software_versions ({VERSION_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(version.package_id.as_str())
        .bind(&version.version)
        .bind(&version.notes)
        .bind(&version.commit_id)
        .bind(&manifest)
        .bind(version.published_at)
        .bind(version.run_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if unique_violation(&error) {
                StoreError::Conflict(format!("{} is already published", version.version))
            } else {
                db_err(error)
            }
        })?;
        // The description and the keywords of a package are those of
        // its newest Version: a search reads one row per package.
        sqlx::query(
            "UPDATE software_packages SET latest_version = ?, description = ?, keywords = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&package.latest_version)
        .bind(&package.description)
        .bind(&keywords)
        .bind(package.updated_at)
        .bind(package.id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn list_versions(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<SoftwareVersion>, StoreError> {
        // `software_versions` carries no workspace of its own, so the
        // read joins the Package that owns the Version.
        let rows = sqlx::query(&format!(
            "SELECT {} FROM software_versions v \
             JOIN software_packages p ON p.id = v.package_id \
             WHERE v.package_id = ? AND p.workspace_id = ? \
             ORDER BY v.published_at, v.version",
            VERSION_COLUMNS
                .split(", ")
                .map(|column| format!("v.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(package_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(version_from).collect()
    }
}
