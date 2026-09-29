//! The Software List REST (ADR-0022, ADR-0016).
//!
//! Packages are written by Agents and only read by the user, so the
//! desk reads them and never writes them: there are three GETs and no
//! mutating path. The list row carries what the destination shows
//! without a second call; the detail adds the tools, the Versions, the
//! origin of a Fork, and the Contributions without their patches. One
//! Contribution reads on its own address, and only that answer carries
//! the patch.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use pagis_core::{AgentId, Contribution, ContributionId, SoftwarePackage, SoftwareVersion};
use pagis_software::PackageVersion;
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One row of the Software List.
#[derive(Debug, Serialize, ToSchema)]
pub struct SoftwarePackageDto {
    pub name: String,
    pub description: String,
    pub latest_version: String,
    pub author_agent_id: String,
    /// The Agent that wrote the package, by name.
    pub author_name: String,
    pub keywords: Vec<String>,
    /// How many tools the latest Version carries.
    pub tool_count: usize,
    /// How many Contributions to this package are still open.
    pub open_contributions: usize,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SoftwarePackagePage {
    pub items: Vec<SoftwarePackageDto>,
}

/// One tool of the latest Version.
#[derive(Debug, Serialize, ToSchema)]
pub struct SoftwareToolDto {
    pub name: String,
    pub description: String,
}

/// One published Version. The files stay in the repository.
#[derive(Debug, Serialize, ToSchema)]
pub struct SoftwareVersionDto {
    pub version: String,
    pub notes: String,
    pub published_at: i64,
}

/// One Contribution as the list shows it: the record without its
/// patch. The patch reads on the Contribution's own address.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContributionSummaryDto {
    pub id: String,
    pub package: String,
    pub fork_package: String,
    pub base_version: String,
    pub fork_version: String,
    pub summary: String,
    /// `open`, `merged`, or `declined`.
    pub status: String,
    pub outcome_reason: Option<String>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

/// One Contribution with its patch.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContributionDto {
    #[serde(flatten)]
    pub record: ContributionSummaryDto,
    /// The unified diff between the base tree and the Fork tree.
    pub patch: String,
}

/// One package in full.
#[derive(Debug, Serialize, ToSchema)]
pub struct SoftwarePackageDetailDto {
    pub name: String,
    pub description: String,
    pub latest_version: String,
    pub author_agent_id: String,
    pub author_name: String,
    pub keywords: Vec<String>,
    /// The package this one was forked from, when it is a Fork.
    pub origin_package: Option<String>,
    pub origin_version: Option<String>,
    pub tools: Vec<SoftwareToolDto>,
    /// Every Version, newest first.
    pub versions: Vec<SoftwareVersionDto>,
    /// Every Contribution offered to this package, newest first.
    pub contributions: Vec<ContributionSummaryDto>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[utoipa::path(get, path = "/api/v1/software", responses(
    (status = 200, body = SoftwarePackagePage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_software(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<SoftwarePackagePage>, ApiError> {
    let packages = state.software.list_packages(&tenant.workspace_id).await?;
    let mut items = Vec::with_capacity(packages.len());
    for package in packages {
        let versions = state
            .software
            .list_versions(&tenant.workspace_id, &package.id)
            .await?;
        let contributions = state
            .contributions
            .list_by_package(&tenant.workspace_id, &package.id)
            .await?;
        items.push(SoftwarePackageDto {
            name: package.name.clone(),
            description: package.description.clone(),
            latest_version: package.latest_version.clone(),
            author_agent_id: package.author_agent_id.to_string(),
            author_name: author_name(&state, &tenant.workspace_id, &package.author_agent_id).await,
            keywords: package.keywords.clone(),
            tool_count: tools(&versions, &package.latest_version).len(),
            open_contributions: contributions.iter().filter(|held| is_open(held)).count(),
            updated_at: package.updated_at,
        });
    }
    Ok(Json(SoftwarePackagePage { items }))
}

#[utoipa::path(get, path = "/api/v1/software/{name}", params(
    ("name" = String, Path, description = "The package name"),
), responses(
    (status = 200, body = SoftwarePackageDetailDto),
    (status = 401, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn get_software(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(name): Path<String>,
) -> Result<Json<SoftwarePackageDetailDto>, ApiError> {
    let package = package_named(&state, &tenant, &name).await?;
    let mut versions = state
        .software
        .list_versions(&tenant.workspace_id, &package.id)
        .await?;
    let tools = tools(&versions, &package.latest_version);
    versions.reverse();
    let mut contributions = state
        .contributions
        .list_by_package(&tenant.workspace_id, &package.id)
        .await?;
    contributions.reverse();
    let mut records = Vec::with_capacity(contributions.len());
    for contribution in contributions {
        records.push(summary(&state, &tenant, &contribution, &package.name).await?);
    }
    let origin_package = match &package.origin_package_id {
        Some(origin_id) => state
            .software
            .list_packages(&tenant.workspace_id)
            .await?
            .into_iter()
            .find(|held| &held.id == origin_id)
            .map(|held| held.name),
        None => None,
    };
    Ok(Json(SoftwarePackageDetailDto {
        name: package.name.clone(),
        description: package.description.clone(),
        latest_version: package.latest_version.clone(),
        author_agent_id: package.author_agent_id.to_string(),
        author_name: author_name(&state, &tenant.workspace_id, &package.author_agent_id).await,
        keywords: package.keywords.clone(),
        origin_package,
        origin_version: package.origin_version.clone(),
        tools,
        versions: versions
            .into_iter()
            .map(|version| SoftwareVersionDto {
                version: version.version,
                notes: version.notes,
                published_at: version.published_at,
            })
            .collect(),
        contributions: records,
        created_at: package.created_at,
        updated_at: package.updated_at,
    }))
}

#[utoipa::path(get, path = "/api/v1/software/{name}/contributions/{contribution_id}", params(
    ("name" = String, Path, description = "The package name"),
    ("contribution_id" = String, Path, description = "The Contribution id"),
), responses(
    (status = 200, body = ContributionDto),
    (status = 401, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn get_contribution(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((name, contribution_id)): Path<(String, String)>,
) -> Result<Json<ContributionDto>, ApiError> {
    let package = package_named(&state, &tenant, &name).await?;
    let contribution = state
        .contributions
        .get(&tenant.workspace_id, &ContributionId::from(contribution_id))
        .await?
        .filter(|held| held.package_id == package.id)
        .ok_or_else(|| ApiError::not_found("contribution"))?;
    Ok(Json(ContributionDto {
        patch: contribution.patch.clone(),
        record: summary(&state, &tenant, &contribution, &package.name).await?,
    }))
}

/// The package of this Workspace with that name.
async fn package_named(
    state: &AppState,
    tenant: &Tenant,
    name: &str,
) -> Result<SoftwarePackage, ApiError> {
    state
        .software
        .get_by_name(&tenant.workspace_id, name)
        .await?
        .ok_or_else(|| ApiError::not_found("package"))
}

/// The record without its patch. The Fork is named, because the row
/// says where the change comes from.
async fn summary(
    state: &AppState,
    tenant: &Tenant,
    contribution: &Contribution,
    package: &str,
) -> Result<ContributionSummaryDto, ApiError> {
    let fork_package = state
        .software
        .list_packages(&tenant.workspace_id)
        .await?
        .into_iter()
        .find(|held| held.id == contribution.fork_package_id)
        .map(|held| held.name)
        .unwrap_or_else(|| contribution.fork_package_id.to_string());
    Ok(ContributionSummaryDto {
        id: contribution.id.to_string(),
        package: package.to_string(),
        fork_package,
        base_version: contribution.base_version.clone(),
        fork_version: contribution.fork_version.clone(),
        summary: contribution.summary.clone(),
        status: contribution.status.as_str().to_string(),
        outcome_reason: contribution.outcome_reason.clone(),
        created_at: contribution.created_at,
        closed_at: contribution.closed_at,
    })
}

fn is_open(contribution: &Contribution) -> bool {
    contribution.status == pagis_core::ContributionStatus::Open
}

/// The tools of one Version, out of the manifest the record holds. A
/// manifest that does not parse shows no tool: the desk reads, and it
/// never fails on one corrupt row.
fn tools(versions: &[SoftwareVersion], version: &str) -> Vec<SoftwareToolDto> {
    versions
        .iter()
        .find(|held| held.version == version)
        .and_then(|held| serde_json::from_value::<PackageVersion>(held.manifest.clone()).ok())
        .map(|parsed| {
            parsed
                .manifest
                .tools
                .into_iter()
                .map(|tool| SoftwareToolDto {
                    name: tool.name,
                    description: tool.description,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What an Agent is called. An Agent the roster lost is named by its
/// id, so a listing never fails on a missing row.
async fn author_name(
    state: &AppState,
    workspace_id: &pagis_core::WorkspaceId,
    agent_id: &AgentId,
) -> String {
    match state.agent_store.get(workspace_id, agent_id).await {
        Ok(Some(agent)) => agent.name,
        _ => agent_id.to_string(),
    }
}
