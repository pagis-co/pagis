//! Docker discovery (ADR-0024). Pagis finds the Docker daemon itself,
//! for the Client App and the CLI alike: it pings a fixed list of
//! candidate endpoints in order and remembers the first that answers.
//! The fixed socket of `connect_with_local_defaults` misses every
//! engine that listens under the user's home: Docker Desktop,
//! OrbStack, Colima, Rancher Desktop, Lima, rootless Docker and Podman.
//! containerd has no Docker API, so it is not a candidate.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;

/// How long one candidate gets to answer a ping. A `tcp://` endpoint
/// that never answers must not hold the probe.
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// The read/write timeout of a bollard connection, in seconds.
const CONNECT_TIMEOUT_SECONDS: u64 = 120;

/// Where one candidate endpoint comes from. The order of the variants
/// is the order the probe tries them, after any override. The engines
/// that listen under the user's home come before the system socket,
/// and Podman, whose Docker API is a compatibility layer, comes last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerSource {
    /// The endpoint the user typed in System settings.
    Override,
    /// `DOCKER_HOST`.
    Environment,
    /// The endpoint of the current docker context.
    Context,
    /// `~/.docker/run/docker.sock` on macOS and
    /// `~/.docker/desktop/docker.sock` on Linux.
    DockerDesktop,
    /// `~/.orbstack/run/docker.sock`.
    OrbStack,
    /// `<profile>/docker.sock` under `~/.colima` or `~/.config/colima`.
    Colima,
    /// `~/.rd/docker.sock`.
    RancherDesktop,
    /// `~/.lima/<instance>/sock/docker.sock`.
    Lima,
    /// `$XDG_RUNTIME_DIR/docker.sock`.
    RootlessDocker,
    /// `/var/run/docker.sock`.
    SystemSocket,
    /// `$XDG_RUNTIME_DIR/podman/podman.sock`,
    /// `$TMPDIR/podman/<machine>-api.sock` and `/run/podman/podman.sock`.
    Podman,
}

impl DockerSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DockerSource::Override => "override",
            DockerSource::Environment => "environment",
            DockerSource::Context => "context",
            DockerSource::DockerDesktop => "docker_desktop",
            DockerSource::OrbStack => "orbstack",
            DockerSource::Colima => "colima",
            DockerSource::RancherDesktop => "rancher_desktop",
            DockerSource::Lima => "lima",
            DockerSource::RootlessDocker => "rootless_docker",
            DockerSource::SystemSocket => "system_socket",
            DockerSource::Podman => "podman",
        }
    }
}

/// One place Pagis looks for the Docker daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerCandidate {
    pub source: DockerSource,
    pub endpoint: String,
}

/// What one candidate answered on the last probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerCandidateResult {
    pub candidate: DockerCandidate,
    /// `None` when the candidate answered the ping.
    pub error: Option<String>,
}

impl DockerCandidateResult {
    pub fn reachable(&self) -> bool {
        self.error.is_none()
    }
}

/// One probe of every candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerReport {
    /// The first candidate that answered, and the endpoint the runtime
    /// now uses. `None` when Docker is not installed or not running.
    pub endpoint: Option<String>,
    pub candidates: Vec<DockerCandidateResult>,
}

/// The ping seam. Production pings over bollard; a test answers from a
/// script, so the probe order is a stored test and no daemon is needed.
#[async_trait]
pub trait DockerPing: Send + Sync {
    /// `Ok(())` when the Docker daemon at `endpoint` answers.
    async fn ping(&self, endpoint: &str) -> Result<(), String>;
}

/// The production ping: a bollard connection and one `/_ping`.
pub struct BollardPing;

#[async_trait]
impl DockerPing for BollardPing {
    async fn ping(&self, endpoint: &str) -> Result<(), String> {
        let docker = connect(endpoint)?;
        match tokio::time::timeout(PING_TIMEOUT, docker.ping()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!("no answer in {} seconds", PING_TIMEOUT.as_secs())),
        }
    }
}

/// Open a bollard client on one endpoint. The scheme picks the
/// transport: a unix socket or named pipe, or HTTP for `tcp://`.
pub fn connect(endpoint: &str) -> Result<Docker, String> {
    let client = if endpoint.starts_with("tcp://") || endpoint.starts_with("http://") {
        Docker::connect_with_http(
            endpoint,
            CONNECT_TIMEOUT_SECONDS,
            bollard::API_DEFAULT_VERSION,
        )
    } else {
        Docker::connect_with_socket(
            endpoint,
            CONNECT_TIMEOUT_SECONDS,
            bollard::API_DEFAULT_VERSION,
        )
    };
    client.map_err(|error| format!("cannot reach Docker at {endpoint}: {error}"))
}

/// Read one endpoint the user typed. A bare absolute path is a socket
/// path; every other form keeps its scheme.
pub fn parse_endpoint(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("the endpoint is empty".to_string());
    }
    const SCHEMES: [&str; 5] = ["unix://", "npipe://", "tcp://", "http://", "https://"];
    if SCHEMES.iter().any(|scheme| raw.starts_with(scheme)) {
        return Ok(raw.to_string());
    }
    if raw.starts_with('/') {
        return Ok(format!("unix://{raw}"));
    }
    Err(format!(
        "{raw} is neither a socket path nor a tcp:// endpoint"
    ))
}

/// The places discovery looks. [`DockerSearch::from_env`] reads the
/// running machine; a test points every path at a temporary directory.
#[derive(Debug, Clone)]
pub struct DockerSearch {
    pub docker_host: Option<String>,
    /// `DOCKER_CONTEXT`. It wins over `currentContext` in
    /// `~/.docker/config.json`, as the docker CLI reads it.
    pub docker_context: Option<String>,
    /// `~/.docker`, which holds `config.json`, `contexts/` and the
    /// Docker Desktop sockets.
    pub docker_config_dir: PathBuf,
    /// The user's home, which holds the sockets of OrbStack, Colima,
    /// Rancher Desktop and Lima.
    pub home: PathBuf,
    /// `XDG_RUNTIME_DIR`, where rootless Docker and rootless Podman
    /// listen on Linux.
    pub runtime_dir: Option<PathBuf>,
    /// `TMPDIR`, or `/tmp` without it, where a Podman machine on macOS
    /// puts its API socket.
    pub temp_dir: PathBuf,
    /// `/`, which holds the system sockets of Docker Engine and Podman.
    pub root: PathBuf,
}

impl DockerSearch {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            docker_host: std::env::var("DOCKER_HOST").ok().filter(|v| !v.is_empty()),
            docker_context: std::env::var("DOCKER_CONTEXT")
                .ok()
                .filter(|v| !v.is_empty()),
            docker_config_dir: home.join(".docker"),
            home,
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            temp_dir: std::env::var_os("TMPDIR")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from),
            root: PathBuf::from("/"),
        }
    }

    /// The candidates in probe order, the override first. A socket
    /// path is listed only when the file is there; an endpoint the
    /// user or the environment named is always listed, so an endpoint
    /// that does not answer says so instead of disappearing.
    pub fn candidates(&self, override_endpoint: Option<&str>) -> Vec<DockerCandidate> {
        let mut candidates = Vec::new();
        let mut push = |source: DockerSource, endpoint: String| {
            if !candidates
                .iter()
                .any(|c: &DockerCandidate| c.endpoint == endpoint)
            {
                candidates.push(DockerCandidate { source, endpoint });
            }
        };

        if let Some(endpoint) = override_endpoint
            && let Ok(endpoint) = parse_endpoint(endpoint)
        {
            push(DockerSource::Override, endpoint);
        }
        if let Some(host) = &self.docker_host
            && let Ok(endpoint) = parse_endpoint(host)
        {
            push(DockerSource::Environment, endpoint);
        }
        if let Some(endpoint) = self.context_endpoint() {
            push(DockerSource::Context, endpoint);
        }
        for (source, path) in self.sockets() {
            if let Some(endpoint) = socket_endpoint(&path) {
                push(source, endpoint);
            }
        }
        candidates
    }

    /// The socket path of every engine, in probe order. Colima and Lima
    /// keep one socket for each profile or instance, so each directory
    /// under their home is a path.
    fn sockets(&self) -> Vec<(DockerSource, PathBuf)> {
        let home = &self.home;
        let mut sockets = vec![
            (
                DockerSource::DockerDesktop,
                self.docker_config_dir.join("run/docker.sock"),
            ),
            (
                DockerSource::DockerDesktop,
                self.docker_config_dir.join("desktop/docker.sock"),
            ),
            (
                DockerSource::OrbStack,
                home.join(".orbstack/run/docker.sock"),
            ),
        ];
        for colima in [home.join(".colima"), home.join(".config/colima")] {
            for profile in entries(&colima) {
                sockets.push((DockerSource::Colima, profile.join("docker.sock")));
            }
        }
        sockets.push((DockerSource::RancherDesktop, home.join(".rd/docker.sock")));
        for instance in entries(&home.join(".lima")) {
            sockets.push((DockerSource::Lima, instance.join("sock/docker.sock")));
        }
        if let Some(runtime) = &self.runtime_dir {
            sockets.push((DockerSource::RootlessDocker, runtime.join("docker.sock")));
        }
        sockets.push((
            DockerSource::SystemSocket,
            self.root.join("var/run/docker.sock"),
        ));
        if let Some(runtime) = &self.runtime_dir {
            sockets.push((DockerSource::Podman, runtime.join("podman/podman.sock")));
        }
        for machine in entries(&self.temp_dir.join("podman")) {
            if machine
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("-api.sock"))
            {
                sockets.push((DockerSource::Podman, machine));
            }
        }
        sockets.push((
            DockerSource::Podman,
            self.root.join("run/podman/podman.sock"),
        ));
        sockets
    }

    /// The docker endpoint of the current context. The name comes from
    /// `DOCKER_CONTEXT` or `~/.docker/config.json`, and the endpoint
    /// from the `meta.json` under `~/.docker/contexts/meta` that
    /// carries that name.
    fn context_endpoint(&self) -> Option<String> {
        let name = match &self.docker_context {
            Some(name) => name.clone(),
            None => {
                let config = std::fs::read_to_string(self.docker_config_dir.join("config.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())?;
                config.get("currentContext")?.as_str()?.to_string()
            }
        };
        if name == "default" {
            return None;
        }
        let metas = std::fs::read_dir(self.docker_config_dir.join("contexts/meta")).ok()?;
        for entry in metas.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path().join("meta.json")) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if meta.get("Name").and_then(|n| n.as_str()) != Some(name.as_str()) {
                continue;
            }
            if let Some(host) = meta
                .get("Endpoints")
                .and_then(|e| e.get("docker"))
                .and_then(|d| d.get("Host"))
                .and_then(|h| h.as_str())
                && let Ok(endpoint) = parse_endpoint(host)
            {
                return Some(endpoint);
            }
        }
        None
    }
}

/// The entries of a directory, in name order, so the candidates do not
/// depend on how the file system enumerates it. A directory that is
/// not there has none.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    entries
}

fn socket_endpoint(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    Some(format!("unix://{}", path.display()))
}

#[derive(Debug, Default)]
struct DiscoveryState {
    override_endpoint: Option<String>,
    /// The last endpoint that answered. Only a success is remembered,
    /// so Docker that starts later is still found.
    remembered: Option<String>,
}

/// The Docker endpoint in use, and the probe behind it. One instance
/// serves the runtime, the onboarding status and the System tab, so
/// they always name the same endpoint.
pub struct DockerDiscovery {
    search: DockerSearch,
    ping: Arc<dyn DockerPing>,
    state: Mutex<DiscoveryState>,
}

impl DockerDiscovery {
    pub fn new(
        search: DockerSearch,
        ping: Arc<dyn DockerPing>,
        override_endpoint: Option<String>,
    ) -> Self {
        Self {
            search,
            ping,
            state: Mutex::new(DiscoveryState {
                override_endpoint,
                remembered: None,
            }),
        }
    }

    /// Discovery against this machine, over bollard.
    pub fn production(override_endpoint: Option<String>) -> Self {
        Self::new(
            DockerSearch::from_env(),
            Arc::new(BollardPing),
            override_endpoint,
        )
    }

    pub fn override_endpoint(&self) -> Option<String> {
        self.lock().override_endpoint.clone()
    }

    /// Take the endpoint the user typed, or clear it. The remembered
    /// endpoint goes with it, so the next connect uses the new one
    /// without a restart (ADR-0024).
    pub fn set_override(&self, endpoint: Option<String>) {
        let mut state = self.lock();
        state.override_endpoint = endpoint;
        state.remembered = None;
    }

    /// Ping one endpoint. The System tab validates an override this
    /// way before it saves it (ADR-0024).
    pub async fn ping(&self, endpoint: &str) -> Result<(), String> {
        self.ping.ping(endpoint).await
    }

    /// The endpoint to connect on. The remembered one when it still
    /// answers, else a fresh probe. It pings, so Docker that stopped
    /// is noticed and a later start is found.
    pub async fn endpoint(&self) -> Option<String> {
        let remembered = self.lock().remembered.clone();
        if let Some(endpoint) = remembered {
            if self.ping.ping(&endpoint).await.is_ok() {
                return Some(endpoint);
            }
            self.lock().remembered = None;
        }
        self.probe().await.endpoint
    }

    /// Ping every candidate and remember the first that answers. Every
    /// candidate is pinged, because the System tab shows the whole
    /// table with each result.
    pub async fn probe(&self) -> DockerReport {
        let override_endpoint = self.lock().override_endpoint.clone();
        let mut endpoint = None;
        let mut candidates = Vec::new();
        for candidate in self.search.candidates(override_endpoint.as_deref()) {
            let error = self.ping.ping(&candidate.endpoint).await.err();
            if error.is_none() && endpoint.is_none() {
                endpoint = Some(candidate.endpoint.clone());
            }
            candidates.push(DockerCandidateResult { candidate, error });
        }
        self.lock().remembered = endpoint.clone();
        DockerReport {
            endpoint,
            candidates,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DiscoveryState> {
        self.state.lock().expect("docker discovery state")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// A ping that answers for a named set of endpoints and records the
    /// order it was asked in.
    struct ScriptedPing {
        answering: Vec<String>,
        asked: StdMutex<Vec<String>>,
    }

    impl ScriptedPing {
        fn new(answering: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                answering: answering.iter().map(|e| e.to_string()).collect(),
                asked: StdMutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DockerPing for ScriptedPing {
        async fn ping(&self, endpoint: &str) -> Result<(), String> {
            self.asked.lock().unwrap().push(endpoint.to_string());
            if self.answering.iter().any(|e| e == endpoint) {
                Ok(())
            } else {
                Err("connection refused".to_string())
            }
        }
    }

    /// The socket files of every engine discovery knows, under one
    /// temporary root. A ping never opens them.
    const SOCKETS: &[&str] = &[
        "home/.docker/run/docker.sock",
        "home/.docker/desktop/docker.sock",
        "home/.orbstack/run/docker.sock",
        "home/.colima/default/docker.sock",
        "home/.colima/work/docker.sock",
        "home/.config/colima/default/docker.sock",
        "home/.rd/docker.sock",
        "home/.lima/docker/sock/docker.sock",
        "run/user/501/docker.sock",
        "var/run/docker.sock",
        "run/user/501/podman/podman.sock",
        "tmp/podman/podman-machine-default-api.sock",
        "tmp/podman/podman-machine-default-gvproxy.sock",
        "run/podman/podman.sock",
    ];

    /// A search rooted at an empty temporary directory.
    fn search_under(root: &Path) -> DockerSearch {
        DockerSearch {
            docker_host: None,
            docker_context: None,
            docker_config_dir: root.join("home/.docker"),
            home: root.join("home"),
            runtime_dir: Some(root.join("run/user/501")),
            temp_dir: root.join("tmp"),
            root: root.to_path_buf(),
        }
    }

    /// A machine with a socket for every engine discovery knows.
    fn fake_home() -> (tempfile::TempDir, DockerSearch) {
        let dir = tempfile::tempdir().unwrap();
        for socket in SOCKETS {
            let path = dir.path().join(socket);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let search = search_under(dir.path());
        (dir, search)
    }

    fn socket(dir: &tempfile::TempDir, path: &str) -> String {
        format!("unix://{}/{path}", dir.path().display())
    }

    fn endpoints(candidates: &[DockerCandidate]) -> Vec<(&'static str, String)> {
        candidates
            .iter()
            .map(|c| (c.source.as_str(), c.endpoint.clone()))
            .collect()
    }

    #[test]
    fn candidates_follow_the_fixed_order() {
        let (dir, mut search) = fake_home();
        search.docker_host = Some("tcp://127.0.0.1:2375".to_string());

        let candidates = search.candidates(None);

        assert_eq!(
            endpoints(&candidates),
            vec![
                ("environment", "tcp://127.0.0.1:2375".to_string()),
                (
                    "docker_desktop",
                    socket(&dir, "home/.docker/run/docker.sock")
                ),
                (
                    "docker_desktop",
                    socket(&dir, "home/.docker/desktop/docker.sock")
                ),
                ("orbstack", socket(&dir, "home/.orbstack/run/docker.sock")),
                ("colima", socket(&dir, "home/.colima/default/docker.sock")),
                ("colima", socket(&dir, "home/.colima/work/docker.sock")),
                (
                    "colima",
                    socket(&dir, "home/.config/colima/default/docker.sock")
                ),
                ("rancher_desktop", socket(&dir, "home/.rd/docker.sock")),
                ("lima", socket(&dir, "home/.lima/docker/sock/docker.sock")),
                ("rootless_docker", socket(&dir, "run/user/501/docker.sock")),
                ("system_socket", socket(&dir, "var/run/docker.sock")),
                ("podman", socket(&dir, "run/user/501/podman/podman.sock")),
                (
                    "podman",
                    socket(&dir, "tmp/podman/podman-machine-default-api.sock")
                ),
                ("podman", socket(&dir, "run/podman/podman.sock")),
            ]
        );
    }

    #[test]
    fn a_socket_that_is_not_there_is_not_a_candidate() {
        let dir = tempfile::tempdir().unwrap();

        assert!(search_under(dir.path()).candidates(None).is_empty());
    }

    #[test]
    fn a_machine_without_a_runtime_directory_has_no_rootless_candidates() {
        let (_dir, mut search) = fake_home();
        search.runtime_dir = None;

        let sources: Vec<_> = search.candidates(None).iter().map(|c| c.source).collect();

        assert!(!sources.contains(&DockerSource::RootlessDocker));
        assert!(sources.contains(&DockerSource::Podman));
    }

    #[test]
    fn the_override_comes_first_and_is_listed_once() {
        let (dir, search) = fake_home();
        let colima = format!("{}/home/.colima/work/docker.sock", dir.path().display());

        let candidates = search.candidates(Some(&colima));

        assert_eq!(candidates[0].source, DockerSource::Override);
        assert_eq!(candidates[0].endpoint, format!("unix://{colima}"));
        assert_eq!(
            candidates
                .iter()
                .filter(|c| c.endpoint == format!("unix://{colima}"))
                .count(),
            1
        );
    }

    #[test]
    fn the_current_context_endpoint_is_a_candidate() {
        let (dir, search) = fake_home();
        let docker = dir.path().join("home/.docker");
        std::fs::write(docker.join("config.json"), r#"{"currentContext":"colima"}"#).unwrap();
        let meta = docker.join("contexts/meta/abc123");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(
            meta.join("meta.json"),
            r#"{"Name":"colima","Endpoints":{"docker":{"Host":"unix:///tmp/colima.sock"}}}"#,
        )
        .unwrap();

        let candidates = search.candidates(None);

        assert_eq!(
            candidates[0],
            DockerCandidate {
                source: DockerSource::Context,
                endpoint: "unix:///tmp/colima.sock".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn the_probe_asks_in_order_and_keeps_the_first_that_answers() {
        let (dir, search) = fake_home();
        let colima = socket(&dir, "home/.colima/default/docker.sock");
        let ping = ScriptedPing::new(&[&colima]);
        let discovery = DockerDiscovery::new(search, Arc::clone(&ping) as _, None);

        let report = discovery.probe().await;

        assert_eq!(report.endpoint, Some(colima.clone()));
        let colima_at = ping.asked().iter().position(|e| *e == colima).unwrap();
        assert_eq!(
            ping.asked()[0],
            socket(&dir, "home/.docker/run/docker.sock")
        );
        // Every candidate is pinged, so the table has a result for each.
        assert_eq!(report.candidates.len(), ping.asked().len());
        // The Podman gvproxy socket is not an API socket.
        assert_eq!(report.candidates.len(), SOCKETS.len() - 1);
        assert!(!report.candidates[0].reachable());
        assert!(report.candidates[colima_at].reachable());
        assert_eq!(
            report.candidates[0].error.as_deref(),
            Some("connection refused")
        );
    }

    #[tokio::test]
    async fn no_endpoint_answers_and_the_report_says_so() {
        let (_dir, search) = fake_home();
        let ping = ScriptedPing::new(&[]);
        let discovery = DockerDiscovery::new(search, ping as _, None);

        let report = discovery.probe().await;

        assert_eq!(report.endpoint, None);
        assert!(report.candidates.iter().all(|c| !c.reachable()));
        assert_eq!(discovery.endpoint().await, None);
    }

    #[tokio::test]
    async fn the_remembered_endpoint_serves_the_next_connect() {
        let (dir, search) = fake_home();
        let colima = socket(&dir, "home/.colima/default/docker.sock");
        let ping = ScriptedPing::new(&[&colima]);
        let discovery = DockerDiscovery::new(search, Arc::clone(&ping) as _, None);

        assert_eq!(discovery.endpoint().await, Some(colima.clone()));
        let after_first = ping.asked().len();
        assert_eq!(discovery.endpoint().await, Some(colima.clone()));

        // The remembered endpoint is confirmed with one ping, not a
        // whole probe.
        assert_eq!(ping.asked().len(), after_first + 1);
    }

    #[tokio::test]
    async fn a_new_override_reconnects_on_the_next_use() {
        let (dir, search) = fake_home();
        let default = socket(&dir, "home/.colima/default/docker.sock");
        let work = socket(&dir, "home/.colima/work/docker.sock");
        let ping = ScriptedPing::new(&[&default, &work]);
        let discovery = DockerDiscovery::new(search, ping as _, None);
        assert_eq!(discovery.endpoint().await, Some(default));

        discovery.set_override(Some(work.clone()));

        assert_eq!(discovery.override_endpoint(), Some(work.clone()));
        assert_eq!(discovery.endpoint().await, Some(work));
    }

    #[test]
    fn an_endpoint_is_a_socket_path_or_a_scheme() {
        assert_eq!(
            parse_endpoint("/var/run/docker.sock").unwrap(),
            "unix:///var/run/docker.sock"
        );
        assert_eq!(
            parse_endpoint(" tcp://127.0.0.1:2375 ").unwrap(),
            "tcp://127.0.0.1:2375"
        );
        assert!(parse_endpoint("").is_err());
        assert!(parse_endpoint("docker.sock").is_err());
    }
}
