//! The production Docker runtime over bollard. One
//! container and one named volume per agent, both named and labelled by
//! tenant; the screend control port publishes to host loopback so the
//! daemon reaches it on Linux and macOS alike.
//!
//! The container is a boundary between tenants (ADR-0014). Three things
//! make it one here: the container joins its tenant's Tenant Network and
//! no other, so it reaches no container of another tenant; it carries
//! real limits, so one tenant cannot take the machine; and it holds a
//! screend token the daemon generated, so reaching the control port is
//! not the same as holding it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use futures::StreamExt;
use pagis_core::{AgentId, WorkspaceId};
use tokio::io::AsyncWriteExt;

use crate::docker::DockerDiscovery;
use crate::{
    AGENT_LABEL, BindMount, CONTROL_PORT, ComputerLimits, ComputerOwner, ComputerRuntime, IMAGE,
    MOUNTS_LABEL, RELAY_HOST, RunningComputer, SECCOMP_PROFILE, StartedComputer, TOKEN_MOUNT,
    TenantResources, VERSION_LABEL, WORKSPACE_LABEL, mounts_fingerprint, network_name,
};
/// How long a booting container gets to answer `/healthz`.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);
/// The daemon writes output frames of at most 32 KiB; a buffer
/// of that size takes each frame in one read.
const EXEC_FRAME_BYTES: usize = 32 * 1024;
/// How often the daemon asks whether the exec instance stopped.
const EXEC_POLL: Duration = Duration::from_millis(50);
/// The exit code Docker records when the runtime refuses to start the
/// process and no code arrives.
const EXEC_LAUNCH_REFUSED: i64 = 126;

/// What the runtime needs beside the endpoint: what one
/// container may take, and where the screend tokens live.
pub struct RuntimeOptions {
    pub limits: ComputerLimits,
    /// The directory of the per-container screend tokens. One file per
    /// (tenant, Agent), mode 0600, bind-mounted read-only into the
    /// container it belongs to.
    pub tokens_dir: PathBuf,
    /// The labels every container, volume and Tenant Network this
    /// runtime creates carries beside its owner labels. The daemon gives
    /// none; a test gives the mark of its
    /// [`TestDocker`](crate::test_docker::TestDocker).
    pub labels: Vec<(String, String)>,
}

/// The runtime over one discovered endpoint (ADR-0024). Every call
/// asks discovery again, so an endpoint the user changes takes effect
/// on the next connect and needs no restart.
pub struct BollardRuntime {
    discovery: Arc<DockerDiscovery>,
    options: RuntimeOptions,
    /// What this machine answered the first time the runtime asked for a
    /// volume quota. The Administration Interface reports it, and
    /// the fallback is logged once and not on every wake.
    volume_quota: std::sync::Mutex<crate::Quota>,
    /// What the storage driver answered the last container create with
    /// a layer size. The Administration Interface reports it, and the
    /// fallback is logged once and not on every wake.
    container_quota: std::sync::Mutex<crate::Quota>,
}

impl BollardRuntime {
    pub fn new(discovery: Arc<DockerDiscovery>, options: RuntimeOptions) -> Self {
        Self {
            discovery,
            options,
            volume_quota: std::sync::Mutex::new(crate::Quota::Unknown),
            container_quota: std::sync::Mutex::new(crate::Quota::Unknown),
        }
    }

    /// Record what the machine answered, and log a quota that is not in
    /// force once per daemon rather than once per volume.
    fn record_volume_quota(&self, answer: crate::Quota) {
        if newly_unsupported(&self.volume_quota, answer) {
            tracing::warn!(
                "this Docker daemon takes no volume quota, so no Computer volume has a \
                 size bound; put /var/lib/docker on XFS with pquota to bound them"
            );
        }
    }

    /// Record what the storage driver answered, and log a layer size
    /// that is not in force once per daemon rather than once per wake.
    fn record_container_quota(&self, answer: crate::Quota) {
        if newly_unsupported(&self.container_quota, answer) {
            tracing::warn!(
                "this Docker storage driver holds no Computer's writable layer to a size, \
                 so an Agent can fill the Docker host's disk outside /data; use the overlay2 \
                 storage driver with /var/lib/docker on XFS with pquota to bound it"
            );
        }
    }

    async fn docker(&self) -> Result<Docker, String> {
        let endpoint = self.discovery.endpoint().await.ok_or_else(|| {
            "cannot reach Docker: no endpoint answered. Install Docker Desktop or Colima, \
             or set the endpoint in Settings → System."
                .to_string()
        })?;
        crate::docker::connect(&endpoint)
    }

    /// The labels of one new Docker object: `own`, and the labels of
    /// the options.
    fn labels(&self, own: impl IntoIterator<Item = (String, String)>) -> HashMap<String, String> {
        own.into_iter()
            .chain(self.options.labels.iter().cloned())
            .collect()
    }

    /// The file that holds one container's screend token.
    fn token_path(&self, owner: &ComputerOwner) -> PathBuf {
        self.options
            .tokens_dir
            .join(owner.workspace_id.to_string())
            .join(format!("{}.token", owner.agent_id))
    }

    /// A fresh token for one container, written where only this OS user
    /// reads it. Every start mints a new one, so a token that leaked
    /// with a container dies with that container.
    fn mint_token(&self, owner: &ComputerOwner) -> Result<String, String> {
        let token = new_token();
        let path = self.token_path(owner);
        let parent = path
            .parent()
            .ok_or_else(|| "the token path has no directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("token directory create failed: {error}"))?;
        write_private(&path, &token).map_err(|error| format!("token write failed: {error}"))?;
        Ok(token)
    }

    /// The token of a container this daemon did not start, for adoption.
    fn read_token(&self, owner: &ComputerOwner) -> Option<String> {
        std::fs::read_to_string(self.token_path(owner))
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
    }

    /// The Tenant Network of one tenant, created when it is absent.
    /// Two containers on one network reach each other; a
    /// container reaches no container on another network, on any port,
    /// because Docker gives each bridge its own interface and subnet and
    /// filters between them.
    async fn ensure_network(
        &self,
        docker: &Docker,
        workspace_id: &WorkspaceId,
    ) -> Result<String, String> {
        let name = network_name(workspace_id);
        match docker
            .inspect_network(
                &name,
                None::<bollard::query_parameters::InspectNetworkOptions>,
            )
            .await
        {
            Ok(_) => return Ok(name),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(err) => return Err(format!("tenant network inspect failed: {err}")),
        }
        let created = docker
            .create_network(tenant_network(
                &name,
                self.labels([(WORKSPACE_LABEL.to_string(), workspace_id.to_string())]),
            ))
            .await;
        match created {
            Ok(_) => Ok(name),
            // Two wakes of one tenant can race for the first network.
            // The loser reads the winner's network and goes on.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(name),
            Err(err) => Err(format!("tenant network create failed: {err}")),
        }
    }

    /// Ask Docker whether it holds a volume to a size: make a probe
    /// volume with the size of the limits, read its options and remove
    /// it. A restart forgets the answer of the last daemon, and the
    /// probe gives the answer before the next Computer volume is made.
    async fn probe_volume_quota(&self, bytes: u64) -> Result<crate::Quota, String> {
        let docker = self.docker().await?;
        let name = format!("pagis-quota-probe-{}", &new_token()[..16]);
        let created = docker
            .create_volume(bollard::models::VolumeCreateOptions {
                name: Some(name.clone()),
                labels: Some(self.labels([])),
                driver_opts: Some(HashMap::from([("size".to_string(), bytes.to_string())])),
                ..Default::default()
            })
            .await;
        match created {
            Ok(volume) => {
                let answer = quota_in_force(&volume, bytes);
                docker
                    .remove_volume(
                        &name,
                        None::<bollard::query_parameters::RemoveVolumeOptions>,
                    )
                    .await
                    .map_err(|error| format!("probe volume remove failed: {error}"))?;
                Ok(answer)
            }
            Err(error) if quota_unsupported(&error) => Ok(crate::Quota::Unsupported),
            Err(error) => Err(format!("probe volume create failed: {error}")),
        }
    }

    /// The tenant's volume for one Agent, with the quota where the
    /// machine holds one.
    ///
    /// Docker's `local` driver refuses a `size` option on a filesystem
    /// that carries no project quotas, and only then: the volume is made
    /// without the quota and the daemon says so once. Every other
    /// failure of the create is this call's failure. A quota dropped on
    /// any error hides a full disk, a name taken by another driver and a
    /// daemon that has gone away, and it leaves every tenant unbounded on
    /// a machine that can hold them to a quota.
    async fn ensure_volume(&self, docker: &Docker, owner: &ComputerOwner) -> Result<(), String> {
        let labels = self.labels(owner.labels());
        if let Some(bytes) = self.options.limits.volume_bytes {
            let quota = bollard::models::VolumeCreateOptions {
                name: Some(owner.volume_name()),
                labels: Some(labels.clone()),
                driver_opts: Some(HashMap::from([("size".to_string(), bytes.to_string())])),
                ..Default::default()
            };
            match docker.create_volume(quota).await {
                Ok(volume) => {
                    self.record_volume_quota(quota_in_force(&volume, bytes));
                    return Ok(());
                }
                Err(error) if quota_unsupported(&error) => {
                    self.record_volume_quota(crate::Quota::Unsupported);
                }
                Err(error) => return Err(format!("volume create failed: {error}")),
            }
        }
        docker
            .create_volume(bollard::models::VolumeCreateOptions {
                name: Some(owner.volume_name()),
                labels: Some(labels),
                ..Default::default()
            })
            .await
            .map(|_| ())
            .map_err(|err| format!("volume create failed: {err}"))
    }

    /// Create the container of one wake, with the layer size of the
    /// limits where the storage driver holds the layer to it.
    ///
    /// A storage driver that cannot hold the layer to a size refuses the
    /// option, and only then is the container created again without it:
    /// the daemon records the answer and says so once. Docker removes a
    /// container whose create failed, so the name is free for the second
    /// create. Every other failure of the create is this call's failure,
    /// as it is for the volume. A create that takes the option records
    /// what the image store of the daemon makes of it.
    async fn create_container(
        &self,
        docker: &Docker,
        name: &str,
        config: bollard::models::ContainerCreateBody,
    ) -> Result<(), String> {
        let options = || {
            bollard::query_parameters::CreateContainerOptionsBuilder::default()
                .name(name)
                .build()
        };
        let created = |result: Result<_, bollard::errors::Error>| {
            result
                .map(|_| ())
                .map_err(|error| format!("container create failed: {error}"))
        };
        if self.options.limits.layer_bytes.is_none() {
            return created(docker.create_container(Some(options()), config).await);
        }
        let store = docker
            .info()
            .await
            .map_err(|error| format!("Docker info read failed: {error}"))?;
        match docker
            .create_container(Some(options()), config.clone())
            .await
        {
            Ok(_) => {
                self.record_container_quota(layer_size_in_force(&store));
                Ok(())
            }
            Err(error) if layer_size_refused(&error) => {
                self.record_container_quota(crate::Quota::Unsupported);
                let mut config = config;
                if let Some(host_config) = config.host_config.as_mut() {
                    host_config.storage_opt = None;
                }
                created(docker.create_container(Some(options()), config).await)
            }
            Err(error) => Err(format!("container create failed: {error}")),
        }
    }
}

/// Hold `answer` in `held`, and say whether it newly says that the
/// machine holds nothing to a size. The caller then logs that once for
/// each daemon and not once for each wake.
fn newly_unsupported(held: &std::sync::Mutex<crate::Quota>, answer: crate::Quota) -> bool {
    let mut held = held.lock().expect("the quota lock");
    let newly = answer == crate::Quota::Unsupported && *held != answer;
    *held = answer;
    newly
}

/// The Tenant Network `name` as Docker creates it. IPv6 is off: the
/// egress rules on the Docker host are IPv4 rules (ADR-0014), so a
/// Computer with an IPv6 address would reach past them.
fn tenant_network(
    name: &str,
    labels: HashMap<String, String>,
) -> bollard::models::NetworkCreateRequest {
    bollard::models::NetworkCreateRequest {
        name: name.to_string(),
        driver: Some("bridge".to_string()),
        enable_ipv6: Some(false),
        labels: Some(labels),
        ..Default::default()
    }
}

/// Whether the volume a create answered with carries the size bound.
///
/// Docker answers a create for a name that already exists with that
/// volume, as it was made, and applies none of the new options. So the
/// options of the answer, not the success of the call, say whether a
/// bound is in force: a volume that an earlier start made on a daemon
/// that takes no quota has none.
fn quota_in_force(volume: &bollard::models::Volume, bytes: u64) -> crate::Quota {
    match volume.options.get("size") {
        Some(size) if *size == bytes.to_string() => crate::Quota::Supported,
        _ => crate::Quota::Unsupported,
    }
}

/// Whether a failed volume create says the filesystem carries no project
/// quotas, which is the one failure the daemon carries on from.
///
/// Docker's `local` driver answers "Filesystem does not support, or has
/// not enabled quotas" for it. The word "quota" is what every wording of
/// that answer has in common, and no other create failure names it: a
/// name conflict, a full disk and a daemon that has gone away all say
/// something else.
fn quota_unsupported(error: &bollard::errors::Error) -> bool {
    error.to_string().to_lowercase().contains("quota")
}

/// Whether a failed container create says that the storage driver
/// cannot hold the writable layer to a size, which is the one create
/// failure the daemon carries on from.
///
/// `overlay2` off XFS with `pquota`, and `fuse-overlayfs`, refuse the
/// option with a sentence that names `--storage-opt`. A driver on
/// Docker's own project quota control says "Filesystem does not support,
/// or has not enabled quotas". No other create failure says either: a
/// name conflict, a full disk, a missing image and a daemon that has gone
/// away all say something else.
fn layer_size_refused(error: &bollard::errors::Error) -> bool {
    let message = error.to_string();
    message.contains("--storage-opt") || message.contains("has not enabled quotas")
}

/// Whether a container create that took the layer size holds the layer
/// to it, by the image store of the Docker daemon.
///
/// A graph driver that cannot hold the layer to a size refuses the
/// option, so under a graph driver a create that took it is a bound in
/// force. The containerd image store, which Docker Engine 29 uses on a
/// new installation, takes the option and gives it to no snapshotter, so
/// there a create that took it bounds nothing. `docker info` names that
/// store with the driver status `driver-type`, whose value is a
/// containerd snapshotter.
fn layer_size_in_force(info: &bollard::models::SystemInfo) -> crate::Quota {
    let containerd_store = info.driver_status.iter().flatten().any(|pair| {
        matches!(pair.as_slice(), [key, value]
            if key == "driver-type" && value.starts_with("io.containerd.snapshotter"))
    });
    if containerd_store {
        crate::Quota::Unsupported
    } else {
        crate::Quota::Supported
    }
}

/// One token: 32 random bytes as hexadecimal. It never reaches a
/// command line, so its length costs nothing.
fn new_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Write one secret where only this OS user reads it.
fn write_private(path: &Path, secret: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(secret.as_bytes())?;
    file.flush()
}

fn control_key() -> String {
    format!("{CONTROL_PORT}/tcp")
}

#[async_trait]
impl ComputerRuntime for BollardRuntime {
    async fn image_version(&self) -> Result<Option<String>, String> {
        let docker = self.docker().await?;
        match docker.inspect_image(IMAGE).await {
            Ok(image) => Ok(image
                .config
                .and_then(|config| config.labels)
                .and_then(|labels| labels.get(VERSION_LABEL).cloned())),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(err) => Err(format!("image inspect failed: {err}")),
        }
    }

    async fn pull_image(
        &self,
        progress: tokio::sync::mpsc::UnboundedSender<u8>,
    ) -> Result<(), String> {
        let docker = self.docker().await?;
        let options = bollard::query_parameters::CreateImageOptionsBuilder::default()
            .from_image(IMAGE)
            .build();
        let mut stream = docker.create_image(Some(options), None, None);
        // Whole-pull percent from the per-layer byte counters.
        let mut layers: HashMap<String, (i64, i64)> = HashMap::new();
        while let Some(info) = stream.next().await {
            let info = info.map_err(|err| format!("image pull failed: {err}"))?;
            if let (Some(id), Some(detail)) = (info.id, info.progress_detail)
                && let (Some(current), Some(total)) = (detail.current, detail.total)
                && total > 0
            {
                layers.insert(id, (current, total));
                let (done, all) = layers
                    .values()
                    .fold((0i64, 0i64), |(d, a), (c, t)| (d + c, a + t));
                if all > 0 {
                    let percent = ((done * 100) / all).clamp(0, 100) as u8;
                    let _ = progress.send(percent);
                }
            }
        }
        let _ = progress.send(100);
        Ok(())
    }

    async fn running(&self, owner: &ComputerOwner) -> Result<Option<RunningComputer>, String> {
        let docker = self.docker().await?;
        let inspect = docker
            .inspect_container(
                &owner.container_name(),
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await;
        let container = match inspect {
            Ok(container) => container,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok(None),
            Err(err) => return Err(format!("container inspect failed: {err}")),
        };
        let is_running = container
            .state
            .as_ref()
            .and_then(|state| state.running)
            .unwrap_or(false);
        if !is_running {
            return Ok(None);
        }
        // IMAGE may be a multi-platform index digest. Docker resolves
        // it to this host's platform image, whose config ID is what a
        // container records. Compare those resolved IDs instead of the
        // raw index digest.
        let resolved_image_id = match docker.inspect_image(IMAGE).await {
            Ok(image) => image.id,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => None,
            Err(err) => return Err(format!("locked image inspect failed: {err}")),
        };
        let image_matches = resolved_image_id.is_some()
            && resolved_image_id.as_deref() == container.image.as_deref();
        // Docker copies the image's labels onto the container, so this
        // is the version the container actually runs.
        let version = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| labels.get(VERSION_LABEL).cloned());
        // The mount set is the daemon's own label, written at create.
        // Docker keeps it beside the image's labels.
        let mounts = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| labels.get(MOUNTS_LABEL).cloned());
        // The token of a container this process did not start is the
        // one its start wrote down. Without it the daemon cannot
        // drive the container, so it is not adopted and the next wake
        // replaces it.
        let Some(token) = self.read_token(owner) else {
            tracing::info!(
                agent = %owner.agent_id,
                "a running computer has no screend token on this disk; the next wake replaces it"
            );
            return Ok(None);
        };
        let Some(computer) = started_computer(&container, token) else {
            return Ok(None);
        };
        if !healthy_now(&computer).await {
            return Ok(None);
        }
        Ok(Some(RunningComputer {
            computer,
            version,
            mounts,
            image_matches,
        }))
    }

    /// The filter is the owner label of the tenant and the labels of the
    /// options, so a test runtime finds the containers of its own test
    /// alone.
    async fn running_agents(&self, workspace_id: &WorkspaceId) -> Result<Vec<AgentId>, String> {
        let docker = self.docker().await?;
        let labels: Vec<String> = self
            .labels([(WORKSPACE_LABEL.to_string(), workspace_id.to_string())])
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let filters = HashMap::from([
            ("label", labels.iter().map(String::as_str).collect()),
            ("status", vec!["running"]),
        ]);
        let containers = docker
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
            .map_err(|err| format!("container list failed: {err}"))?;
        Ok(containers
            .into_iter()
            .filter_map(|container| container.labels?.remove(AGENT_LABEL))
            .map(AgentId::from)
            .collect())
    }

    async fn start(
        &self,
        owner: &ComputerOwner,
        mounts: &[BindMount],
        env: &[String],
    ) -> Result<StartedComputer, String> {
        let docker = self.docker().await?;
        let name = owner.container_name();

        // A stale stopped container from an earlier image blocks the
        // name: remove it (the volume holds all durable state).
        match docker
            .inspect_container(
                &name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(_) => {
                docker
                    .remove_container(
                        &name,
                        Some(
                            bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                                .force(true)
                                .build(),
                        ),
                    )
                    .await
                    .map_err(|err| format!("stale container remove failed: {err}"))?;
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(err) => return Err(format!("container inspect failed: {err}")),
        }

        self.ensure_volume(&docker, owner).await?;
        let network = self.ensure_network(&docker, &owner.workspace_id).await?;
        // A fresh token per start. It reaches the container as a
        // read-only mount and never as an argument or an environment
        // entry: every `docker exec` inherits the container environment,
        // and the sprite's own shell is such an exec.
        let token = self.mint_token(owner)?;
        let token_mount = BindMount {
            host: self.token_path(owner),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };

        let host_config = host_config(owner, &network, mounts, &token_mount, &self.options.limits);
        let mut labels = self.labels(owner.labels());
        // The fingerprint says which mount set this container
        // booted with, so a later wake can tell a stale one.
        labels.insert(MOUNTS_LABEL.to_string(), mounts_fingerprint(mounts));
        let config = bollard::models::ContainerCreateBody {
            image: Some(IMAGE.to_string()),
            host_config: Some(host_config),
            // The clock and the locale of this container. They
            // are set at create time, so every process the entrypoint
            // starts inherits them.
            env: Some(env.to_vec()),
            labels: Some(labels),
            exposed_ports: Some(HashMap::from([(control_key(), HashMap::new())])),
            ..Default::default()
        };
        self.create_container(&docker, &name, config).await?;
        docker
            .start_container(
                &name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|err| format!("container start failed: {err}"))?;

        let container = docker
            .inspect_container(
                &name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
            .map_err(|err| format!("container inspect failed: {err}"))?;
        let computer = started_computer(&container, token)
            .ok_or_else(|| "no published control port on the container".to_string())?;

        wait_ready(&computer, &self.token_path(owner), BOOT_TIMEOUT).await?;
        Ok(computer)
    }

    async fn stop(&self, owner: &ComputerOwner) -> Result<(), String> {
        let docker = self.docker().await?;
        let name = owner.container_name();
        docker
            .stop_container(
                &name,
                Some(
                    bollard::query_parameters::StopContainerOptionsBuilder::default()
                        .t(10)
                        .build(),
                ),
            )
            .await
            .map_err(|err| format!("container stop failed: {err}"))?;
        docker
            .remove_container(
                &name,
                None::<bollard::query_parameters::RemoveContainerOptions>,
            )
            .await
            .map_err(|err| format!("container remove failed: {err}"))?;
        Ok(())
    }

    /// One `df` call answers the whole set: Docker reports the
    /// containers and the volumes of the daemon together, each with its
    /// labels, so the tenant's figures are one round trip and not three.
    async fn resources(&self, workspace_id: &WorkspaceId) -> Result<TenantResources, String> {
        let docker = self.docker().await?;
        let usage = docker
            .df(None::<bollard::query_parameters::DataUsageOptions>)
            .await
            .map_err(|err| format!("disk usage read failed: {err}"))?;
        let owner = workspace_id.to_string();
        // The owner label, not the name, decides whose object it is:
        // the figures are one tenant's.
        let containers = usage
            .containers
            .unwrap_or_default()
            .iter()
            .filter(|container| {
                container
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(WORKSPACE_LABEL))
                    == Some(&owner)
            })
            .count();
        let volumes: Vec<_> = usage
            .volumes
            .unwrap_or_default()
            .into_iter()
            .filter(|volume| volume.labels.get(WORKSPACE_LABEL) == Some(&owner))
            .collect();
        Ok(TenantResources {
            containers: u32::try_from(containers).unwrap_or(u32::MAX),
            volumes: u32::try_from(volumes.len()).unwrap_or(u32::MAX),
            // A volume the daemon has not measured reports no size.
            volume_bytes: volumes
                .iter()
                .filter_map(|volume| volume.usage_data.as_ref())
                .map(|data| u64::try_from(data.size).unwrap_or(0))
                .sum(),
        })
    }

    async fn send_input(
        &self,
        computer: &StartedComputer,
        holder: crate::InputHolder,
        ops: &[crate::exec::InputOp],
    ) -> Result<(), String> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/input", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&serde_json::json!({ "holder": holder.as_str(), "ops": ops }))
            .send()
            .await
            .map_err(|err| format!("input send failed: {err}"))?;
        if !response.status().is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!("input failed: {detail}"));
        }
        Ok(())
    }

    async fn browser_open(&self, computer: &StartedComputer, url: &str) -> Result<String, String> {
        let response = reqwest::Client::new()
            .post(format!("http://{}/browser/open", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&serde_json::json!({ "url": url }))
            .send()
            .await
            .map_err(|err| format!("the browser channel did not answer: {err}"))?;
        page_address(response).await
    }

    async fn browser_page(&self, computer: &StartedComputer) -> Result<String, String> {
        let response = reqwest::Client::new()
            .get(format!("http://{}/browser/page", computer.control_addr))
            .bearer_auth(&computer.token)
            .send()
            .await
            .map_err(|err| format!("the browser channel did not answer: {err}"))?;
        page_address(response).await
    }

    async fn browser_fill(
        &self,
        computer: &StartedComputer,
        origin: &str,
        fields: &[crate::FillField],
    ) -> Result<(), String> {
        let response = reqwest::Client::new()
            .post(format!("http://{}/browser/fill", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&serde_json::json!({ "origin": origin, "fields": fields }))
            .send()
            .await
            .map_err(|err| format!("the browser channel did not answer: {err}"))?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        Ok(())
    }

    async fn relay_offer(
        &self,
        computer: &StartedComputer,
        offer: &str,
        path: &crate::MediaPath,
    ) -> Result<String, String> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/offer", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&offer_body(offer, path))
            .send()
            .await
            .map_err(|err| format!("offer relay failed: {err}"))?;
        if !response.status().is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!("offer refused: {detail}"));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| format!("answer read failed: {err}"))?;
        body["sdp"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "answer carries no sdp".to_string())
    }

    async fn set_holder(
        &self,
        computer: &StartedComputer,
        holder: crate::InputHolder,
    ) -> Result<(), String> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/holder", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&serde_json::json!({ "holder": holder.as_str() }))
            .send()
            .await
            .map_err(|err| format!("holder set failed: {err}"))?;
        if !response.status().is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!("holder set refused: {detail}"));
        }
        Ok(())
    }

    async fn user_input_idle_ms(&self, computer: &StartedComputer) -> Result<u64, String> {
        let response = reqwest::Client::new()
            .get(format!("http://{}/holder", computer.control_addr))
            .bearer_auth(&computer.token)
            .send()
            .await
            .map_err(|err| format!("holder status failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("holder status failed: {}", response.status()));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| format!("holder status read failed: {err}"))?;
        body["idle_ms"]
            .as_u64()
            .ok_or_else(|| "holder status carries no idle_ms".to_string())
    }

    /// One command in the container: create, start, then
    /// poll until the exec instance stops. The stream drains to its
    /// end even after the caps are full, because a command whose
    /// output nobody reads blocks on its next write.
    async fn exec(
        &self,
        computer: &StartedComputer,
        request: crate::ExecRequest,
    ) -> Result<crate::ExecOutcome, String> {
        let docker = self.docker().await?;
        let created = docker
            .create_exec(
                &computer.container,
                bollard::exec::CreateExecOptions {
                    attach_stdin: Some(request.stdin.is_some()),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    // A TTY merges the two streams and breaks the frame
                    // decoder.
                    tty: Some(false),
                    env: Some(request.env.clone()),
                    cmd: Some(request.argv.clone()),
                    user: Some(request.user.clone()),
                    working_dir: Some(request.cwd.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("exec create failed: {err}"))?;

        let started = docker
            .start_exec(
                &created.id,
                Some(bollard::exec::StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: Some(EXEC_FRAME_BYTES),
                }),
            )
            .await
            .map_err(|err| format!("exec start failed: {err}"))?;
        let bollard::exec::StartExecResults::Attached { mut output, input } = started else {
            return Err("the exec started detached and has no output".to_string());
        };

        if let Some(stdin) = request.stdin {
            let mut input = input;
            input
                .write_all(&stdin)
                .await
                .map_err(|err| format!("exec stdin write failed: {err}"))?;
            // The process sees the end of its input only after the
            // write half shuts down.
            input
                .shutdown()
                .await
                .map_err(|err| format!("exec stdin shutdown failed: {err}"))?;
        } else {
            drop(input);
        }

        let mut stdout = crate::CappedOutput::new(request.output_cap);
        let mut stderr = crate::CappedOutput::new(request.output_cap);
        while let Some(frame) = output.next().await {
            match frame.map_err(|err| format!("exec output read failed: {err}"))? {
                bollard::container::LogOutput::StdOut { message } => stdout.push(&message),
                bollard::container::LogOutput::StdErr { message } => stderr.push(&message),
                bollard::container::LogOutput::Console { message } => stdout.push(&message),
                bollard::container::LogOutput::StdIn { .. } => {}
            }
        }

        // The exit code is meaningful only after the instance stops:
        // Podman answers 0 for a running exec.
        let exit_code = loop {
            let inspect = docker
                .inspect_exec(&created.id)
                .await
                .map_err(|err| format!("exec inspect failed: {err}"))?;
            if inspect.running != Some(true) {
                break inspect.exit_code.unwrap_or(EXEC_LAUNCH_REFUSED);
            }
            tokio::time::sleep(EXEC_POLL).await;
        };

        Ok(crate::ExecOutcome {
            exit_code,
            truncated: stdout.truncated() || stderr.truncated(),
            stdout: stdout.into_text(),
            stderr: stderr.into_text(),
        })
    }

    /// One long-lived process in the container, with its streams
    /// attached. [`split_exec_output`] splits the one frame stream of
    /// bollard into stdout and stderr.
    async fn exec_stream(
        &self,
        computer: &StartedComputer,
        request: crate::ExecRequest,
    ) -> Result<crate::ExecStream, String> {
        let docker = self.docker().await?;
        let created = docker
            .create_exec(
                &computer.container,
                bollard::exec::CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    // A TTY merges the two streams and breaks the frame
                    // decoder.
                    tty: Some(false),
                    env: Some(request.env.clone()),
                    cmd: Some(request.argv.clone()),
                    user: Some(request.user.clone()),
                    working_dir: Some(request.cwd.clone()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("exec create failed: {err}"))?;
        let started = docker
            .start_exec(
                &created.id,
                Some(bollard::exec::StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: Some(EXEC_FRAME_BYTES),
                }),
            )
            .await
            .map_err(|err| format!("exec start failed: {err}"))?;
        let bollard::exec::StartExecResults::Attached { output, input } = started else {
            return Err("the exec started detached and has no output".to_string());
        };
        let (stdout, stderr) = split_exec_output(output);
        Ok(crate::ExecStream {
            stdin: input,
            stdout,
            stderr,
        })
    }

    /// Extract a tar into a directory of the container. The
    /// archive fits in one body, and `copyUIDGID` gives the extracted
    /// tree the uid and the gid of the destination directory.
    async fn upload_archive(
        &self,
        computer: &StartedComputer,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), String> {
        let docker = self.docker().await?;
        docker
            .upload_to_container(
                &computer.container,
                Some(
                    bollard::query_parameters::UploadToContainerOptionsBuilder::new()
                        .path(path)
                        .copy_uidgid("true")
                        .build(),
                ),
                bollard::body_full(tar.into()),
            )
            .await
            .map_err(|err| format!("archive upload failed: {err}"))
    }

    /// Read a path out of the container as one tar. The endpoint
    /// streams, and the stream goes to the caller chunk by chunk: the
    /// manager counts the bytes and stops at `MAX_ARCHIVE_BYTES`.
    async fn download_archive(
        &self,
        computer: &StartedComputer,
        path: &str,
    ) -> Result<crate::ArchiveStream, String> {
        let docker = self.docker().await?;
        let stream = docker.download_from_container(
            &computer.container,
            Some(
                bollard::query_parameters::DownloadFromContainerOptionsBuilder::new()
                    .path(path)
                    .build(),
            ),
        );
        Ok(Box::pin(
            stream.map(|chunk| chunk.map_err(std::io::Error::other)),
        ))
    }

    async fn volume_quota(&self) -> crate::Quota {
        let held = *self.volume_quota.lock().expect("the volume quota lock");
        if held != crate::Quota::Unknown {
            return held;
        }
        let Some(bytes) = self.options.limits.volume_bytes else {
            return held;
        };
        match self.probe_volume_quota(bytes).await {
            Ok(answer) => {
                self.record_volume_quota(answer);
                answer
            }
            Err(error) => {
                tracing::warn!(%error, "Docker did not answer the volume quota probe");
                held
            }
        }
    }

    async fn container_quota(&self) -> crate::Quota {
        *self
            .container_quota
            .lock()
            .expect("the container quota lock")
    }

    async fn fetch_frame(&self, computer: &StartedComputer) -> Result<Vec<u8>, String> {
        let response = reqwest::Client::new()
            .get(format!("http://{}/frame.png", computer.control_addr))
            .bearer_auth(&computer.token)
            .send()
            .await
            .map_err(|err| format!("frame fetch failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("frame fetch failed: {}", response.status()));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|err| format!("frame read failed: {err}"))
    }
}

/// What one container may take and where it may go.
///
/// `network_mode` is the tenant's own network and nothing else, so the
/// container reaches its tenant's containers and the internet and no
/// container of another tenant, on any port. The limits are the ceiling
/// of one desk: memory, CPU, processes, open files, shared memory and
/// the size of the writable layer. The container runs under
/// [`SECCOMP_PROFILE`] and with Docker's init process.
fn host_config(
    owner: &ComputerOwner,
    network: &str,
    mounts: &[BindMount],
    token: &BindMount,
    limits: &ComputerLimits,
) -> bollard::models::HostConfig {
    bollard::models::HostConfig {
        binds: Some(
            std::iter::once(format!("{}:/data", owner.volume_name()))
                .chain(std::iter::once(token.spec()))
                .chain(mounts.iter().map(BindMount::spec))
                .collect(),
        ),
        network_mode: Some(network.to_string()),
        memory: Some(limits.memory_bytes),
        nano_cpus: Some(limits.nano_cpus),
        pids_limit: Some(limits.pids),
        ulimits: Some(vec![bollard::models::ResourcesUlimits {
            name: Some("nofile".to_string()),
            soft: Some(limits.open_files),
            hard: Some(limits.open_files),
        }]),
        shm_size: Some(limits.shm_bytes),
        // Docker's `--storage-opt size=`: the bound of every write
        // outside `/data`, where the storage driver holds it.
        storage_opt: limits
            .layer_bytes
            .map(|bytes| HashMap::from([("size".to_string(), bytes.to_string())])),
        init: Some(true),
        // Chromium's sandbox needs the user-namespace calls.
        // The profile is Docker's default with those calls permitted,
        // so every other syscall stays as Docker filters it.
        security_opt: Some(vec![format!("seccomp={SECCOMP_PROFILE}")]),
        // The control port is the daemon's own, on loopback. No media
        // port is published at all (ADR-0014): the pipeline
        // registers outbound with the Media Relay for each viewer
        // session, so a firewall opens one address and one range for the
        // whole installation and the host has no port for any Computer.
        port_bindings: Some(HashMap::from([(
            control_key(),
            Some(vec![bollard::models::PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("0".to_string()),
            }]),
        )])),
        // The pipeline reaches the relay on the Docker host.
        // `host-gateway` is the host as the Docker daemon sees it: the
        // machine itself on Linux, and the machine outside the VM on
        // Docker Desktop.
        extra_hosts: Some(vec![format!("{RELAY_HOST}:host-gateway")]),
        ..Default::default()
    }
}

/// Where the daemon reaches one container: the control endpoint on the
/// Docker host's loopback, and the token it asks for. Media has
/// no address here: the pipeline registers with the Media Relay.
fn started_computer(
    container: &bollard::models::ContainerInspectResponse,
    token: String,
) -> Option<StartedComputer> {
    let ports = container.network_settings.as_ref()?.ports.as_ref()?;
    let host_port = |key: &str| -> Option<u16> {
        let bindings = ports.get(key)?.as_ref()?;
        bindings.first()?.host_port.as_ref()?.parse().ok()
    };
    let control = host_port(&control_key())?;
    Some(StartedComputer {
        container: container.id.clone()?,
        control_addr: format!("127.0.0.1:{control}"),
        token,
    })
}

/// The body of one `POST /offer`: the browser's offer, the
/// candidate screend advertises, and the relay path it registers with.
fn offer_body(offer: &str, path: &crate::MediaPath) -> serde_json::Value {
    serde_json::json!({
        "sdp": offer,
        "candidate": path.candidate,
        "relay": format!("{RELAY_HOST}:{}", path.port),
        "token": path.token,
    })
}

/// The top-level address in a `/browser/open` or `/browser/page`
/// answer, `{url}`. A refusal carries screend's reason as its body.
async fn page_address(response: reqwest::Response) -> Result<String, String> {
    if !response.status().is_success() {
        return Err(refusal(response).await);
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("the browser channel answer is not JSON: {err}"))?;
    body["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the browser channel answer carries no url".to_string())
}

/// Why screend refused a browser channel request: the reason in the
/// body, or the status when the body is empty.
async fn refusal(response: reqwest::Response) -> String {
    let status = response.status();
    match response.text().await {
        Ok(reason) if !reason.trim().is_empty() => reason,
        _ => format!("the browser channel answered {status}"),
    }
}

/// The Wayland app id of the browser that screend keeps open.
const BROWSER_APP_ID: &str = "chromium";

/// Poll until screend answers and the browser window is open, or until
/// `timeout` passes. The browser starts after screend, so a computer
/// that is ready at `/healthz` alone gives a first screenshot with a
/// terminal and no browser, and the Agent tries to start one itself.
///
/// A screend that refuses the token the daemon wrote at `token_file`
/// did not get that file. Docker reads a bind source on the Docker host,
/// so it mounted an empty file or directory: a Docker VM gives one for a
/// host path that it does not share, and a daemon in a container gives
/// one for a state directory that is not at the same path on the Docker
/// host. That fails at once and names the path.
async fn wait_ready(
    computer: &StartedComputer,
    token_file: &std::path::Path,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let healthy = healthy_now(computer).await;
        if healthy {
            match browser_window(computer).await {
                BrowserWindow::Open => return Ok(()),
                BrowserWindow::TokenRefused => return Err(token_not_shared(token_file)),
                BrowserWindow::NotYet => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(if healthy {
                "the computer browser did not open a window in time".to_string()
            } else {
                "the computer did not become healthy in time".to_string()
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// What screend's window list says about the browser.
#[derive(Debug, PartialEq, Eq)]
enum BrowserWindow {
    Open,
    NotYet,
    /// screend refused the token: it has none, or another one.
    TokenRefused,
}

/// Read screend's window list with the container token.
async fn browser_window(computer: &StartedComputer) -> BrowserWindow {
    #[derive(serde::Deserialize)]
    struct Window {
        app_id: String,
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("static HTTP client options are valid");
    let Ok(response) = client
        .get(format!("http://{}/windows", computer.control_addr))
        .bearer_auth(&computer.token)
        .send()
        .await
    else {
        return BrowserWindow::NotYet;
    };
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return BrowserWindow::TokenRefused;
    }
    match response.json::<Vec<Window>>().await {
        Ok(open) if open.iter().any(|window| window.app_id == BROWSER_APP_ID) => {
            BrowserWindow::Open
        }
        _ => BrowserWindow::NotYet,
    }
}

/// Why a computer refuses the token it was given, and what to do.
fn token_not_shared(token_file: &std::path::Path) -> String {
    let directory = token_file.parent().unwrap_or(token_file);
    format!(
        "the computer did not get its access token: Docker mounted nothing from {}, \
         because Docker reads a mount on the Docker host and that directory is not there. \
         Share that directory with the Docker VM, or set PAGIS_HOME to a directory it \
         shares, such as one in your home directory. A daemon in a container needs its \
         state directory mounted at the same path on the Docker host",
        directory.display()
    )
}

/// The frames of one attached exec, as the output stream of bollard
/// gives them.
type ExecFrames = std::pin::Pin<
    Box<
        dyn futures::Stream<Item = Result<bollard::container::LogOutput, bollard::errors::Error>>
            + Send,
    >,
>;

/// Split the frames of one attached exec into the stdout reader and
/// the stderr receiver of an [`crate::ExecStream`]. bollard multiplexes
/// stdout and stderr in one frame stream, so a task splits them: the
/// caller reads a protocol on stdout and writes the stderr chunks to a
/// log.
///
/// Each side holds at most [`crate::EXEC_STREAM_CAPACITY`] chunks. When
/// one side is full, the task waits and reads no more frames, so a slow
/// caller holds back the process and not the daemon's memory. The
/// frames of both streams come in one order, so a caller that stops
/// reading one stream also stops the other. A caller that drops stderr
/// stops nothing: its chunks are dropped.
fn split_exec_output(
    mut output: ExecFrames,
) -> (
    std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (out_tx, out_rx) = tokio::sync::mpsc::channel(crate::EXEC_STREAM_CAPACITY);
    let (err_tx, err_rx) = tokio::sync::mpsc::channel(crate::EXEC_STREAM_CAPACITY);
    tokio::spawn(async move {
        while let Some(frame) = output.next().await {
            let Ok(frame) = frame else { break };
            match frame {
                bollard::container::LogOutput::StdOut { message }
                | bollard::container::LogOutput::Console { message } => {
                    // The caller dropped stdout, so the session is over.
                    if out_tx.send(Ok(message)).await.is_err() {
                        break;
                    }
                }
                bollard::container::LogOutput::StdErr { message } => {
                    let _ = err_tx.send(message.to_vec()).await;
                }
                bollard::container::LogOutput::StdIn { .. } => {}
            }
        }
    });
    // One stream of the stdout chunks, so `StreamReader` can turn
    // them into the `AsyncRead` the caller expects.
    let chunks = Box::pin(futures::stream::unfold(out_rx, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|chunk: Result<bytes::Bytes, std::io::Error>| (chunk, receiver))
    }));
    (Box::pin(tokio_util::io::StreamReader::new(chunks)), err_rx)
}

/// One bounded readiness probe. Adoption uses one probe, while a new
/// start repeats it under the full boot deadline.
async fn healthy_now(computer: &StartedComputer) -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("static HTTP client options are valid");
    client
        .get(format!("http://{}/healthz", computer.control_addr))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::AgentId;

    fn owner() -> ComputerOwner {
        ComputerOwner::new(
            WorkspaceId::from("workspace-one".to_string()),
            AgentId::from("agent-one".to_string()),
        )
    }

    /// The container joins its tenant's network and carries the limits
    /// and the owner labels.
    #[test]
    fn the_host_config_carries_the_tenant_network_and_the_limits() {
        let limits = ComputerLimits::default();
        let token = BindMount {
            host: std::path::PathBuf::from("/var/pagis/tokens/workspace-one/agent-one.token"),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };
        let config = host_config(&owner(), "pagis-tenant-workspace-one", &[], &token, &limits);

        assert_eq!(
            config.network_mode.as_deref(),
            Some("pagis-tenant-workspace-one")
        );
        assert_eq!(config.memory, Some(limits.memory_bytes));
        assert_eq!(config.nano_cpus, Some(limits.nano_cpus));
        assert_eq!(config.pids_limit, Some(limits.pids));
        assert_eq!(config.shm_size, Some(limits.shm_bytes));
        let ulimits = config.ulimits.expect("the container carries ulimits");
        assert_eq!(ulimits.len(), 1);
        assert_eq!(ulimits[0].name.as_deref(), Some("nofile"));
        assert_eq!(ulimits[0].hard, Some(limits.open_files));
    }

    /// No container publishes a media port (ADR-0014). The
    /// control port is the one published port, on loopback, where only
    /// the daemon reaches it; the pipeline registers outbound with the
    /// Media Relay, so the host carries no port for any Computer's media.
    #[test]
    fn the_host_config_publishes_the_control_port_on_loopback_alone() {
        let token = BindMount {
            host: std::path::PathBuf::from("/var/pagis/tokens/workspace-one/agent-one.token"),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };
        let config = host_config(
            &owner(),
            "pagis-tenant-workspace-one",
            &[],
            &token,
            &ComputerLimits::default(),
        );

        let bindings = config.port_bindings.expect("the control port is published");
        assert_eq!(bindings.len(), 1, "{bindings:?}");
        let binding = bindings[&control_key()]
            .as_ref()
            .expect("the control port has a binding");
        assert_eq!(binding[0].host_ip.as_deref(), Some("127.0.0.1"));
    }

    /// The container asks Docker to hold its writable layer to the size
    /// of the limits, so a storage driver that takes the option bounds
    /// every write outside `/data`. Limits with no layer size ask for
    /// none.
    #[test]
    fn the_host_config_carries_the_layer_size_of_the_limits() {
        let token = BindMount {
            host: std::path::PathBuf::from("/var/pagis/tokens/workspace-one/agent-one.token"),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };
        let sized = ComputerLimits {
            layer_bytes: Some(2 * 1024 * 1024 * 1024),
            ..ComputerLimits::default()
        };
        let config = host_config(&owner(), "pagis-tenant-workspace-one", &[], &token, &sized);

        assert_eq!(
            config.storage_opt,
            Some(HashMap::from([(
                "size".to_string(),
                "2147483648".to_string()
            )]))
        );

        let no_size = ComputerLimits {
            layer_bytes: None,
            ..ComputerLimits::default()
        };
        let config = host_config(
            &owner(),
            "pagis-tenant-workspace-one",
            &[],
            &token,
            &no_size,
        );

        assert_eq!(config.storage_opt, None);
    }

    /// The container reaches the relay at the Docker host: the
    /// name the offer gives the pipeline resolves to the host's gateway.
    #[test]
    fn the_host_config_maps_the_relay_host_to_the_docker_host() {
        let token = BindMount {
            host: std::path::PathBuf::from("/var/pagis/tokens/workspace-one/agent-one.token"),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };
        let config = host_config(
            &owner(),
            "pagis-tenant-workspace-one",
            &[],
            &token,
            &ComputerLimits::default(),
        );

        assert_eq!(
            config.extra_hosts,
            Some(vec!["host.docker.internal:host-gateway".to_string()])
        );
    }

    /// A Tenant Network is a bridge with IPv6 off. The egress rules on
    /// the Docker host are IPv4 rules, so a Computer with an IPv6
    /// address would reach past them.
    #[test]
    fn the_tenant_network_is_a_bridge_with_ipv6_off() {
        let labels = HashMap::from([(WORKSPACE_LABEL.to_string(), "workspace-one".to_string())]);

        let network = tenant_network("pagis-tenant-workspace-one", labels.clone());

        assert_eq!(network.name, "pagis-tenant-workspace-one");
        assert_eq!(network.driver.as_deref(), Some("bridge"));
        assert_eq!(network.enable_ipv6, Some(false));
        assert_eq!(network.labels, Some(labels));
    }

    /// The offer names the relay path the pipeline registers with:
    /// the relay host, the path's port and the path's token,
    /// beside the candidate the browser sends media to.
    #[test]
    fn the_offer_body_names_the_relay_path() {
        let path = crate::MediaPath {
            candidate: "203.0.113.10:50007".to_string(),
            port: 50007,
            token: "path-token".to_string(),
        };

        let body = offer_body("v=0 offer", &path);

        assert_eq!(
            body,
            serde_json::json!({
                "sdp": "v=0 offer",
                "candidate": "203.0.113.10:50007",
                "relay": "host.docker.internal:50007",
                "token": "path-token",
            })
        );
    }

    fn server_error(message: &str) -> bollard::errors::Error {
        bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: message.to_string(),
        }
    }

    /// Only the answer that names a quota lets the daemon make the volume
    /// without one. Every other failure is a failure: a quota
    /// dropped on any error leaves every tenant's disk unbounded on a
    /// machine that can bound it, and hides a full disk.
    #[test]
    fn a_quota_is_dropped_for_the_quota_answer_and_for_no_other() {
        for message in [
            "Filesystem does not support, or has not enabled quotas",
            "quota size requested but no quota support",
        ] {
            assert!(quota_unsupported(&server_error(message)), "{message}");
        }

        for message in [
            "volume name already in use by a different driver",
            "mkdir /var/lib/docker/volumes/x: no space left on device",
            "cannot connect to the Docker daemon",
            "invalid option key: sizze",
        ] {
            assert!(!quota_unsupported(&server_error(message)), "{message}");
        }
    }

    /// Only a refusal of the layer size lets the daemon create the
    /// container without one. Every other failure fails the wake: a
    /// size dropped on any error hides a full disk and a name conflict,
    /// and leaves the layer unbounded on a machine that holds it.
    #[test]
    fn a_layer_size_is_dropped_for_its_refusal_and_for_no_other() {
        for message in [
            // overlay2 on a filesystem other than XFS with `pquota`.
            "--storage-opt is supported only for overlay over xfs with 'pquota' mount option",
            // fuse-overlayfs, which rootless Docker uses.
            "--storage-opt is not supported",
            // A driver on Docker's own project quota control.
            "Filesystem does not support, or has not enabled quotas",
        ] {
            assert!(layer_size_refused(&server_error(message)), "{message}");
        }

        for message in [
            "Conflict. The container name \"/pagis-computer-a-b\" is already in use by \
             container \"0123\". You have to remove (or rename) that container to be able \
             to reuse that name.",
            "mkdir /var/lib/docker/overlay2/0123-init: no space left on device",
            "write /var/lib/docker/containers/0123/config.v2.json: disk quota exceeded",
            "No such image: ghcr.io/pagis-co/pagis-computer:0.17.0",
            "network pagis-tenant-workspace-one not found",
            "invalid mount config for type \"bind\": bind source path does not exist: \
             /var/lib/pagis/computer-tokens",
            "cannot connect to the Docker daemon",
        ] {
            assert!(!layer_size_refused(&server_error(message)), "{message}");
        }
    }

    fn system_info(driver: &str, status: &[[&str; 2]]) -> bollard::models::SystemInfo {
        bollard::models::SystemInfo {
            driver: Some(driver.to_string()),
            driver_status: Some(
                status
                    .iter()
                    .map(|pair| pair.iter().map(|part| part.to_string()).collect())
                    .collect(),
            ),
            ..Default::default()
        }
    }

    /// A graph driver that cannot hold the layer to its size refuses the
    /// option, so a create it takes is a bound in force. The containerd
    /// image store takes the option and gives it to no snapshotter, so a
    /// create it takes bounds nothing.
    #[test]
    fn a_taken_layer_size_is_in_force_under_a_graph_driver_alone() {
        assert_eq!(
            layer_size_in_force(&system_info(
                "overlay2",
                &[["Backing Filesystem", "xfs"], ["Supports d_type", "true"]]
            )),
            crate::Quota::Supported
        );
        assert_eq!(
            layer_size_in_force(&system_info("btrfs", &[["Build Version", "Btrfs v6.6"]])),
            crate::Quota::Supported
        );
        // The answer of Docker Engine 29 under Colima.
        assert_eq!(
            layer_size_in_force(&system_info(
                "overlayfs",
                &[["driver-type", "io.containerd.snapshotter.v1"]]
            )),
            crate::Quota::Unsupported
        );
    }

    /// A new object carries its own labels and the labels of the
    /// options, so a test's mark reaches every object the runtime
    /// creates.
    #[test]
    fn a_new_object_carries_its_owner_labels_and_the_option_labels() {
        let runtime = BollardRuntime::new(
            Arc::new(DockerDiscovery::production(None)),
            RuntimeOptions {
                limits: ComputerLimits::default(),
                tokens_dir: std::path::PathBuf::from("/nonexistent"),
                labels: vec![(crate::TEST_LABEL.to_string(), "41-00ff".to_string())],
            },
        );

        let labels = runtime.labels(owner().labels());

        assert_eq!(
            labels.get(WORKSPACE_LABEL).map(String::as_str),
            Some(owner().workspace_id.as_str())
        );
        assert_eq!(
            labels.get(crate::AGENT_LABEL).map(String::as_str),
            Some(owner().agent_id.as_str())
        );
        assert_eq!(
            labels.get(crate::TEST_LABEL).map(String::as_str),
            Some("41-00ff")
        );
        assert_eq!(labels.len(), 3);
    }

    /// The machine's answer is recorded, so the Administration Interface
    /// reports it and the log carries it once and not once per volume.
    #[test]
    fn the_volume_quota_answer_is_recorded_once() {
        let runtime = BollardRuntime::new(
            Arc::new(DockerDiscovery::production(None)),
            RuntimeOptions {
                limits: ComputerLimits::default(),
                tokens_dir: std::path::PathBuf::from("/nonexistent"),
                labels: Vec::new(),
            },
        );

        assert_eq!(*runtime.volume_quota.lock().unwrap(), crate::Quota::Unknown);
        runtime.record_volume_quota(crate::Quota::Unsupported);
        runtime.record_volume_quota(crate::Quota::Unsupported);
        assert_eq!(
            *runtime.volume_quota.lock().unwrap(),
            crate::Quota::Unsupported
        );
        runtime.record_volume_quota(crate::Quota::Supported);
        assert_eq!(
            *runtime.volume_quota.lock().unwrap(),
            crate::Quota::Supported
        );
    }

    /// The token reaches the container read-only, and the volume the
    /// binds name is the tenant's own.
    #[test]
    fn the_binds_carry_the_tenant_volume_and_the_token_read_only() {
        let token = BindMount {
            host: std::path::PathBuf::from("/var/pagis/tokens/workspace-one/agent-one.token"),
            container: TOKEN_MOUNT.to_string(),
            read_only: true,
        };
        let config = host_config(
            &owner(),
            "pagis-tenant-workspace-one",
            &[],
            &token,
            &ComputerLimits::default(),
        );

        let binds = config.binds.expect("the container carries binds");
        assert!(binds.contains(&"pagis-volume-workspace-one-agent-one:/data".to_string()));
        assert!(binds.iter().any(|bind| {
            bind.ends_with(&format!(":{TOKEN_MOUNT}:ro"))
                && bind.starts_with("/var/pagis/tokens/workspace-one/")
        }));
    }

    /// The names of two tenants never meet, and the labels say who owns
    /// what.
    #[test]
    fn two_tenants_name_and_label_their_objects_apart() {
        let one = owner();
        let two = ComputerOwner::new(
            WorkspaceId::from("workspace-two".to_string()),
            AgentId::from("agent-one".to_string()),
        );

        assert_ne!(one.container_name(), two.container_name());
        assert_ne!(one.volume_name(), two.volume_name());
        assert_ne!(
            network_name(&one.workspace_id),
            network_name(&two.workspace_id)
        );
        let labels: HashMap<String, String> = one.labels().into_iter().collect();
        assert_eq!(
            labels.get(WORKSPACE_LABEL).map(String::as_str),
            Some("workspace-one")
        );
        assert_eq!(
            labels.get(crate::AGENT_LABEL).map(String::as_str),
            Some("agent-one")
        );
    }

    /// Every start mints its own token: a token that leaks with one
    /// container does not open the next one.
    #[test]
    fn a_token_is_new_every_time_and_long() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }

    /// A stand-in screend: `/healthz` answers at once, and `/windows`
    /// lists the browser only from the `browser_after`-th read on. It
    /// counts the window reads that carry the container token.
    async fn screend(
        browser_after: usize,
    ) -> (StartedComputer, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::routing::get;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        let app = axum::Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route(
                "/windows",
                get(move |headers: axum::http::HeaderMap| {
                    let counted = Arc::clone(&counted);
                    async move {
                        use axum::response::IntoResponse as _;
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            != Some("Bearer token-one")
                        {
                            return axum::http::StatusCode::UNAUTHORIZED.into_response();
                        }
                        let read = counted.fetch_add(1, Ordering::SeqCst) + 1;
                        let mut open = vec![serde_json::json!({"title": "foot", "app_id": "foot"})];
                        if read >= browser_after {
                            open.push(serde_json::json!({"title": "about:blank - Chromium", "app_id": "chromium"}));
                        }
                        axum::Json(serde_json::Value::Array(open)).into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let control_addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let computer = StartedComputer {
            container: "container-one".to_string(),
            control_addr,
            token: "token-one".to_string(),
        };
        (computer, reads)
    }

    /// The screen the Agent sees first after a wake shows the browser,
    /// so a computer is ready only when the browser window is open.
    #[tokio::test]
    async fn a_computer_is_ready_only_when_the_browser_window_is_open() {
        let (computer, reads) = screend(3).await;

        wait_ready(&computer, TOKEN_FILE.as_ref(), Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_computer_whose_browser_never_opens_is_not_ready() {
        let (computer, _reads) = screend(usize::MAX).await;

        let error = wait_ready(&computer, TOKEN_FILE.as_ref(), Duration::from_secs(1))
            .await
            .unwrap_err();

        assert!(error.contains("browser"), "{error}");
    }

    fn volume(options: &[(&str, &str)]) -> bollard::models::Volume {
        bollard::models::Volume {
            name: "pagis-agent-one".to_string(),
            options: options
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    /// Docker answers a create for a volume that exists with that
    /// volume and applies no new option, so the answer's options decide.
    #[test]
    fn the_quota_is_in_force_only_on_a_volume_that_carries_the_size() {
        assert_eq!(
            quota_in_force(&volume(&[("size", "1073741824")]), 1_073_741_824),
            crate::Quota::Supported
        );
        // A volume an earlier start made where the daemon took no quota.
        assert_eq!(
            quota_in_force(&volume(&[]), 1_073_741_824),
            crate::Quota::Unsupported
        );
        assert_eq!(
            quota_in_force(&volume(&[("size", "2048")]), 1_073_741_824),
            crate::Quota::Unsupported
        );
    }

    const TOKEN_FILE: &str = "/private/tmp/pagis/computer-tokens/workspace-one/agent-one.token";

    /// A Docker VM that does not share the token directory, or a daemon
    /// in a container whose state directory has another path on the
    /// Docker host, mounts nothing, so screend refuses the token. The
    /// wake fails at once and names the directory and both causes, not
    /// a browser that never opened.
    #[tokio::test]
    async fn a_computer_that_refuses_its_token_names_the_unshared_directory() {
        let (computer, _reads) = screend(1).await;
        let computer = StartedComputer {
            token: "another-token".to_string(),
            ..computer
        };

        let started = tokio::time::Instant::now();
        let error = wait_ready(&computer, TOKEN_FILE.as_ref(), Duration::from_secs(10))
            .await
            .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it waited for the deadline"
        );
        assert!(error.contains("access token"), "{error}");
        assert!(
            error.contains("/private/tmp/pagis/computer-tokens/workspace-one"),
            "{error}"
        );
        assert!(error.contains("PAGIS_HOME"), "{error}");
        assert!(error.contains("same path on the Docker host"), "{error}");
    }

    /// Stderr frames without end, as a server that prints without end
    /// writes them. The counter holds how many frames the reader took.
    fn stderr_without_end(taken: Arc<std::sync::atomic::AtomicUsize>) -> ExecFrames {
        Box::pin(futures::stream::unfold(taken, |taken| async move {
            // Docker frames arrive one by one; the yield lets the test
            // look while the reader runs.
            tokio::task::yield_now().await;
            taken.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let frame = bollard::container::LogOutput::StdErr {
                message: bytes::Bytes::from_static(b"noise\n"),
            };
            Some((Ok(frame), taken))
        }))
    }

    /// A caller that does not read stderr holds the exec stream: at
    /// most one channel of chunks waits for it, and the reader takes no
    /// more frames from Docker until the caller reads again.
    #[tokio::test]
    async fn a_caller_that_does_not_read_stderr_holds_the_exec_stream() {
        use std::sync::atomic::Ordering;

        let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_stdout, mut stderr) = split_exec_output(stderr_without_end(Arc::clone(&taken)));

        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            stderr.len() <= crate::EXEC_STREAM_CAPACITY,
            "{} chunks wait for the caller",
            stderr.len()
        );
        // The reader holds one frame more while it waits for room.
        let held = taken.load(Ordering::SeqCst);
        assert!(
            held <= crate::EXEC_STREAM_CAPACITY + 1,
            "the reader took {held} frames"
        );

        // The stream waits and does not stop: one chunk read makes room
        // for one frame more.
        stderr.recv().await.expect("one chunk");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(taken.load(Ordering::SeqCst) > held);
    }

    /// A caller that drops stderr still reads stdout: a closed log
    /// never stops the protocol.
    #[tokio::test]
    async fn a_caller_that_drops_stderr_still_reads_stdout() {
        use tokio::io::AsyncBufReadExt;

        let mut frames: Vec<Result<bollard::container::LogOutput, bollard::errors::Error>> = (0
            ..crate::EXEC_STREAM_CAPACITY * 4)
            .map(|_| {
                Ok(bollard::container::LogOutput::StdErr {
                    message: bytes::Bytes::from_static(b"noise\n"),
                })
            })
            .collect();
        frames.push(Ok(bollard::container::LogOutput::StdOut {
            message: bytes::Bytes::from_static(b"hello\n"),
        }));
        let (stdout, stderr) = split_exec_output(Box::pin(futures::stream::iter(frames)));
        drop(stderr);

        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("stdout answers in time")
            .expect("stdout is readable");

        assert_eq!(line.as_deref(), Some("hello"));
    }
}
