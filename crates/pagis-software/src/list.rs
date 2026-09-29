//! The Software List of one Workspace (ADR-0016).
//!
//! It joins the three halves a published package has: the records in
//! the database, the bare repository that holds the files, and the search
//! index the list is browsed through. It is also the [`VersionSource`]
//! the runner materializes a version from.
//!
//! `refresh` reads the whole list, installs the Capability Manifest of
//! every package's latest Version in the broker, and builds the index
//! again. The daemon calls it on start and after every publish. A run
//! that already started keeps the snapshot it took, so a publish never
//! changes a running run, the author's own included (ADR-0005).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use pagis_broker::CapabilityManifest;
use pagis_computer::ComputerManagers;
use pagis_core::{
    AgentId, AgentStore, EventBus, NewEvent, RunId, SoftwarePackage, SoftwarePackageId,
    SoftwareStore, SoftwareVersion, WorkspaceId, now_ms,
};
use tokio::sync::Semaphore;

use crate::git::{GitStoreError, PublishAuthor, SoftwareGitStore};
use crate::index::{IndexedPackage, IndexedTool, ToolIndex};
use crate::manifest::PackageVersion;
use crate::materialize::VersionSource;
use crate::note::{SoftwareNotes, fork_note, publish_note};
use crate::origin::Origin;

/// Where an agent keeps the working copy of a package it writes.
const WORKING_COPY_ROOT: &str = "/data/agent/software";
/// How many publishes the daemon runs at once. A publish holds a spool
/// file of up to `MAX_ARCHIVE_BYTES` and a scratch tree of up to
/// `MAX_PACKAGE_BYTES`, so the limit bounds the disk that publishes
/// take. A publish over the limit waits for one to end.
const MAX_CONCURRENT_PUBLISHES: usize = 2;

/// Where the Capability Manifest of a published Version goes. The
/// broker fills it in the daemon; a test fills it with a recorder.
/// The Software List never reads the broker back.
///
/// A Software Package belongs to one tenant, so the Workspace is an
/// argument of the install.
pub trait ManifestSink: Send + Sync {
    fn install(
        &self,
        workspace_id: &WorkspaceId,
        manifest: CapabilityManifest,
    ) -> Result<(), String>;
}

pub struct SoftwareListDeps {
    pub packages: Arc<dyn SoftwareStore>,
    pub agents: Arc<dyn AgentStore>,
    pub git: Arc<SoftwareGitStore>,
    /// Every tenant's Computer manager. A publish and a fork
    /// reach the Computer of the tenant the caller names.
    pub computers: Arc<ComputerManagers>,
    pub manifests: Arc<dyn ManifestSink>,
    pub bus: Arc<dyn EventBus>,
    /// Where a publish and a fork are written down.
    pub notes: Arc<dyn SoftwareNotes>,
}

pub struct SoftwareList {
    deps: SoftwareListDeps,
    /// The search index of each tenant. A tool search reads the
    /// index of the Workspace that asked and of no other, and a rebuild
    /// replaces one tenant's index whole, so a reader never sees a
    /// half-built one.
    indexes: RwLock<HashMap<WorkspaceId, Arc<ToolIndex>>>,
    /// One permit for each publish that runs.
    publishes: Semaphore,
}

impl SoftwareList {
    pub fn new(deps: SoftwareListDeps) -> Self {
        Self {
            deps,
            indexes: RwLock::new(HashMap::new()),
            publishes: Semaphore::new(MAX_CONCURRENT_PUBLISHES),
        }
    }

    /// The index of one tenant as it stands. A Workspace that has
    /// published nothing has an empty one.
    pub fn index(&self, workspace_id: &WorkspaceId) -> Arc<ToolIndex> {
        self.indexes
            .read()
            .expect("software index lock")
            .get(workspace_id)
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(ToolIndex::empty()))
    }

    /// Read the whole Software List of one tenant: install the
    /// Capability Manifest of every package's latest Version, and build
    /// that tenant's search index again.
    pub async fn refresh(&self, workspace_id: &WorkspaceId) -> Result<(), String> {
        let packages = self
            .deps
            .packages
            .list_packages(workspace_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut indexed = Vec::new();
        for package in packages {
            let Some(version) = self.latest_version(workspace_id, &package).await? else {
                continue;
            };
            let parsed = parse_version(&version)?;
            self.deps
                .manifests
                .install(
                    workspace_id,
                    parsed.to_capability_manifest(&version.version),
                )
                .map_err(|error| format!("{}: {error}", package.name))?;
            indexed.push(IndexedPackage {
                name: package.name.clone(),
                version: version.version.clone(),
                author_name: self
                    .author_name(workspace_id, &package.author_agent_id)
                    .await,
                description: package.description.clone(),
                keywords: package.keywords.clone(),
                tools: parsed
                    .manifest
                    .tools
                    .iter()
                    .map(|tool| IndexedTool {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                    })
                    .collect(),
            });
        }
        let index = Arc::new(ToolIndex::build(indexed)?);
        self.indexes
            .write()
            .expect("software index lock")
            .insert(workspace_id.clone(), index);
        Ok(())
    }

    /// Build every tenant's index at boot. One tenant whose list
    /// cannot be read does not stop the others: the failure is logged and
    /// that Workspace keeps an empty index until its next publish.
    pub async fn refresh_all(&self, workspaces: &[WorkspaceId]) -> Result<(), String> {
        let mut first: Option<String> = None;
        for workspace_id in workspaces {
            if let Err(problem) = self.refresh(workspace_id).await {
                tracing::error!(%workspace_id, problem, "one tenant's Software List did not load");
                first = first.or(Some(problem));
            }
        }
        match first {
            Some(problem) => Err(problem),
            None => Ok(()),
        }
    }

    /// Publish the working copy at `~/software/<name>` as the next
    /// Version. The text that comes back names the Version; an
    /// `Err` is what the model reads as the failure. At most
    /// `MAX_CONCURRENT_PUBLISHES` publishes run at once.
    pub async fn publish(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        name: &str,
        notes: &str,
    ) -> Result<String, String> {
        let _running = self
            .publishes
            .acquire()
            .await
            .expect("the publish semaphore is never closed");
        let held = self
            .deps
            .packages
            .get_by_name(workspace_id, name)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(held) = &held
            && &held.author_agent_id != agent_id
        {
            return Err(format!(
                "{name} is owned by {}, fork it",
                self.author_name(workspace_id, &held.author_agent_id).await
            ));
        }

        let (parsed, version_tar, origin) =
            self.read_working_copy(workspace_id, agent_id, name).await?;
        if parsed.manifest.package.name != name {
            return Err(format!(
                "the manifest names the package {:?}, and you publish {name:?}; they must be the \
                 same",
                parsed.manifest.package.name
            ));
        }

        let previous = held.as_ref().map(|held| held.latest_version.clone());
        // The origin is written when the Fork claims its name, and it
        // never changes after that (ADR-0016).
        let origin = match (&held, origin) {
            (None, Some(origin)) => Some(self.resolve_origin(workspace_id, &origin).await?),
            _ => None,
        };
        let version = next_version(previous.as_deref());
        let author =
            PublishAuthor::of_agent(&self.author_name(workspace_id, agent_id).await, agent_id);
        let commit_id = self
            .deps
            .git
            .commit_version(
                workspace_id,
                crate::git::NewVersion {
                    package: name,
                    version: &version,
                    previous: previous.as_deref(),
                    notes,
                    author: &author,
                    tar: version_tar,
                },
            )
            .await
            .map_err(|error| match error {
                GitStoreError::NoChange(previous) => format!("no change since {previous}"),
                GitStoreError::Storage(problem) => problem,
            })?;

        let now = now_ms();
        let package = match held {
            Some(held) => SoftwarePackage {
                description: parsed.manifest.package.description.clone(),
                keywords: parsed.manifest.package.keywords.clone(),
                latest_version: version.clone(),
                updated_at: now,
                ..held
            },
            None => SoftwarePackage {
                id: SoftwarePackageId::generate(),
                workspace_id: workspace_id.clone(),
                name: name.to_string(),
                author_agent_id: agent_id.clone(),
                description: parsed.manifest.package.description.clone(),
                keywords: parsed.manifest.package.keywords.clone(),
                latest_version: version.clone(),
                origin_package_id: origin.as_ref().map(|(id, _)| id.clone()),
                origin_version: origin.as_ref().map(|(_, version)| version.clone()),
                created_at: now,
                updated_at: now,
            },
        };
        if previous.is_none() {
            self.deps
                .packages
                .create_package(&package)
                .await
                .map_err(|error| error.to_string())?;
        }
        let manifest = serde_json::to_value(&parsed)
            .map_err(|error| format!("cannot store the manifest: {error}"))?;
        self.deps
            .packages
            .add_version(
                &package,
                &SoftwareVersion {
                    package_id: package.id.clone(),
                    version: version.clone(),
                    notes: notes.to_string(),
                    commit_id,
                    manifest,
                    published_at: now,
                    run_id: run_id.clone(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;

        self.deps
            .manifests
            .install(workspace_id, parsed.to_capability_manifest(&version))?;
        // The Version is stored and its tools are installed. A search
        // index that cannot be built again keeps the one it has: it
        // must not turn a landed publish into a failure.
        if let Err(problem) = self.refresh(workspace_id).await {
            tracing::error!(problem, "the Software List index was not built again");
        }
        self.deps
            .bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "software.published".to_string(),
                agent_id: Some(agent_id.clone()),
                run_id: Some(run_id.clone()),
                channel_id: None,
                payload: serde_json::json!({
                    "package": name,
                    "version": version,
                    "tools": parsed
                        .manifest
                        .tools
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect::<Vec<_>>(),
                }),
            })
            .await
            .map_err(|error| error.to_string())?;
        self.record(
            workspace_id,
            agent_id,
            run_id,
            &publish_note(name, &version, notes),
        )
        .await;

        Ok(format!(
            "Published {name} {version}. Its tools reach other runs from their next start."
        ))
    }

    /// Copy one Version of `source` into the caller's own Computer as
    /// `~/software/<new_name>` (ADR-0016). `source` names the
    /// package, or `<package>@v3` for one Version of it. The copy is a
    /// plain directory: no `.git`, and the manifest names the fork.
    /// The origin marker the copy carries is what the fork's first
    /// publish records.
    pub async fn fork(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        source: &str,
        new_name: &str,
    ) -> Result<String, String> {
        let (source_name, wanted) = match source.split_once('@') {
            Some((name, version)) => (name, Some(version)),
            None => (source, None),
        };
        if !pagis_broker::is_provider_safe_name(new_name) {
            return Err(format!(
                "the fork name {new_name:?} is not 1 to {} ASCII letters, digits, underscores or \
                 hyphens",
                pagis_broker::MAX_NAME
            ));
        }
        if pagis_broker::is_native_namespace(new_name) {
            return Err(format!("the fork name {new_name:?} is reserved"));
        }
        if self
            .deps
            .packages
            .get_by_name(workspace_id, new_name)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err(format!(
                "{new_name} is taken in this workspace; pick another fork name"
            ));
        }

        let held = self
            .deps
            .packages
            .get_by_name(workspace_id, source_name)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("there is no package {source_name}"))?;
        let version = match wanted {
            Some(wanted) => {
                let versions = self
                    .deps
                    .packages
                    .list_versions(workspace_id, &held.id)
                    .await
                    .map_err(|error| error.to_string())?;
                if !versions.iter().any(|version| version.version == wanted) {
                    return Err(format!("{source_name} has no version {wanted}"));
                }
                wanted.to_string()
            }
            None => held.latest_version.clone(),
        };

        let version_tar = self
            .deps
            .git
            .version_tar(workspace_id, source_name, &version)
            .await
            .map_err(|error| error.to_string())?;
        let origin = Origin {
            package: source_name.to_string(),
            version: version.clone(),
        };
        let working_copy = fork_tar(&version_tar, new_name, &origin)?;
        self.deps
            .computers
            .get(workspace_id)
            .shell(
                agent_id,
                pagis_computer::ShellCommand {
                    command: format!("mkdir -p {WORKING_COPY_ROOT}"),
                    timeout: std::time::Duration::from_secs(60),
                    cwd: None,
                    stdin: None,
                    output_cap: None,
                },
            )
            .await
            .map_err(|error| format!("cannot prepare ~/software: {error}"))?;
        self.deps
            .computers
            .get(workspace_id)
            .upload_archive(agent_id, WORKING_COPY_ROOT, working_copy)
            .await
            .map_err(|error| format!("cannot write ~/software/{new_name}: {error}"))?;
        self.record(
            workspace_id,
            agent_id,
            run_id,
            &fork_note(source_name, &version, new_name),
        )
        .await;

        Ok(format!(
            "Forked {source_name} {version} into ~/software/{new_name}. Change it, then \
             software_publish it as {new_name}."
        ))
    }

    /// Read `~/software/<name>` out of the Computer, pack it, and
    /// check it. The tar that comes back is what the repository holds;
    /// the origin marker is read out of the working copy and dropped
    /// from the tar. The download stops at `MAX_ARCHIVE_BYTES`, and the
    /// pack reads the spool file from disk.
    async fn read_working_copy(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        name: &str,
    ) -> Result<(PackageVersion, Vec<u8>, Option<Origin>), String> {
        let container_tar = self
            .deps
            .computers
            .get(workspace_id)
            .download_archive(agent_id, &format!("{WORKING_COPY_ROOT}/{name}"))
            .await
            .map_err(|error| format!("cannot read ~/software/{name}: {error}"))?;
        tokio::task::spawn_blocking(move || check_working_copy(container_tar))
            .await
            .map_err(|error| format!("the check of ~/software/{name} stopped: {error}"))?
    }

    /// Write one line into the acting Agent's software notes.
    /// A note that cannot be written is logged: the action it
    /// describes already landed, and a note never undoes it.
    async fn record(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        line: &str,
    ) {
        if let Err(problem) = self
            .deps
            .notes
            .record(workspace_id, agent_id, run_id, line)
            .await
        {
            tracing::error!(problem, line, "the software note was not written");
        }
    }

    /// The package and the Version the origin marker names. A marker
    /// that names a package the Workspace lost stops the publish: the
    /// origin of a Fork must be a package that exists.
    async fn resolve_origin(
        &self,
        workspace_id: &WorkspaceId,
        origin: &Origin,
    ) -> Result<(SoftwarePackageId, String), String> {
        let held = self
            .deps
            .packages
            .get_by_name(workspace_id, &origin.package)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "this working copy was forked from {}, and there is no such package any more",
                    origin.package
                )
            })?;
        Ok((held.id, origin.version.clone()))
    }

    /// The Version record a package's `latest_version` names.
    async fn latest_version(
        &self,
        workspace_id: &WorkspaceId,
        package: &SoftwarePackage,
    ) -> Result<Option<SoftwareVersion>, String> {
        Ok(self
            .deps
            .packages
            .list_versions(workspace_id, &package.id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|version| version.version == package.latest_version))
    }

    /// What an Agent is called. An Agent the store no longer holds is
    /// named by its id, so a listing never fails on a missing row.
    async fn author_name(&self, workspace_id: &WorkspaceId, agent_id: &AgentId) -> String {
        match self.deps.agents.get(workspace_id, agent_id).await {
            Ok(Some(agent)) => agent.name,
            _ => agent_id.to_string(),
        }
    }
}

#[async_trait]
impl VersionSource for SoftwareList {
    async fn version(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<PackageVersion, String> {
        let held = self
            .deps
            .packages
            .get_by_name(workspace_id, package)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("there is no package {package}"))?;
        let record = self
            .deps
            .packages
            .list_versions(workspace_id, &held.id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|held| held.version == version)
            .ok_or_else(|| format!("there is no version {version}"))?;
        parse_version(&record)
    }

    async fn tar(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, String> {
        self.deps
            .git
            .version_tar(workspace_id, package, version)
            .await
            .map_err(|error| error.to_string())
    }

    async fn sandbox_csp(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
        widget: &str,
    ) -> Option<crate::WidgetCsp> {
        self.version(workspace_id, package, version)
            .await
            .ok()?
            .manifest
            .widget(widget)
            .map(|spec| spec.csp.clone())
    }

    async fn file(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
        path: &str,
    ) -> Result<Vec<u8>, String> {
        self.deps
            .git
            .version_file(workspace_id, package, version, path)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Pack the Docker archive of one working copy, unpack the version tar
/// into a scratch directory, and validate it. Only the version tar
/// reaches the scratch directory, and only after the pack is inside
/// its limits.
fn check_working_copy(
    container_tar: std::fs::File,
) -> Result<(PackageVersion, Vec<u8>, Option<Origin>), String> {
    let packed = crate::pack::pack(container_tar)?;
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut archive = tar::Archive::new(packed.tar.as_slice());
    archive.set_preserve_permissions(true);
    archive
        .unpack(scratch.path())
        .map_err(|error| format!("cannot read the package: {error}"))?;
    let parsed = crate::validate::validate(scratch.path()).map_err(|errors| errors.to_string())?;
    Ok((parsed, packed.tar, packed.origin))
}

/// The working copy of a Fork, as a tar the Computer extracts into
/// `~/software`. Every entry moves under the fork's own
/// directory, the manifest names the fork, and the origin marker joins
/// the root.
fn fork_tar(version_tar: &[u8], new_name: &str, origin: &Origin) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let mut builder = tar::Builder::new(Vec::new());
    let mut renamed = false;
    let mut archive = tar::Archive::new(version_tar);
    for entry in archive
        .entries()
        .map_err(|error| format!("cannot read the version tar: {error}"))?
    {
        let mut entry = entry.map_err(|error| format!("cannot read a tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("a tar entry has no usable path: {error}"))?
            .into_owned();
        let mut header = entry.header().clone();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if path == std::path::Path::new(crate::manifest::MANIFEST_FILE) {
            let manifest = String::from_utf8(data).map_err(|error| {
                format!("{} is not text: {error}", crate::manifest::MANIFEST_FILE)
            })?;
            data = crate::origin::rename_package(&manifest, new_name)?.into_bytes();
            renamed = true;
        }
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                std::path::Path::new(new_name).join(&path),
                data.as_slice(),
            )
            .map_err(|error| format!("cannot write the fork tar: {error}"))?;
    }
    if !renamed {
        return Err(format!(
            "the version has no {}",
            crate::manifest::MANIFEST_FILE
        ));
    }

    let marker = origin.to_toml();
    let mut header = tar::Header::new_gnu();
    header.set_size(marker.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(
            &mut header,
            std::path::Path::new(new_name).join(crate::origin::ORIGIN_FILE),
            marker.as_bytes(),
        )
        .map_err(|error| format!("cannot write the fork tar: {error}"))?;
    builder
        .into_inner()
        .map_err(|error| format!("cannot close the fork tar: {error}"))
}

/// The manifest a Version record carries.
fn parse_version(version: &SoftwareVersion) -> Result<PackageVersion, String> {
    serde_json::from_value(version.manifest.clone()).map_err(|error| {
        format!(
            "the stored manifest of {} is corrupt: {error}",
            version.version
        )
    })
}

/// The tag of the next Version. The daemon mints it; the author never
/// picks one (ADR-0016).
pub fn next_version(previous: Option<&str>) -> String {
    let number = previous
        .and_then(|previous| previous.strip_prefix('v'))
        .and_then(|number| number.parse::<u64>().ok())
        .unwrap_or(0);
    format!("v{}", number + 1)
}
