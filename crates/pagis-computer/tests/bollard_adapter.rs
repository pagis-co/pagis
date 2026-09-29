use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, Response, StatusCode, header};
use axum::routing::any;
use pagis_computer::docker::{DockerPing, DockerSearch};
use pagis_computer::{
    BollardRuntime, ComputerOwner, ComputerRuntime, DockerDiscovery, IMAGE, IMAGE_VERSION, Quota,
    RuntimeOptions, VERSION_LABEL,
};
use pagis_core::{AgentId, WorkspaceId};
use serde_json::json;
use tracing::instrument::WithSubscriber;

const INDEX_DIGEST: &str = "sha256:index-digest";
const PLATFORM_MANIFEST_DIGEST: &str = "sha256:platform-manifest-digest";
const PLATFORM_CONFIG_ID: &str = "sha256:platform-config-id";
/// What `overlay2` answers a layer size on a filesystem other than XFS
/// with `pquota`.
const LAYER_SIZE_REFUSAL: &str =
    "--storage-opt is supported only for overlay over xfs with 'pquota' mount option";

struct AlwaysReachable;

#[async_trait]
impl DockerPing for AlwaysReachable {
    async fn ping(&self, _endpoint: &str) -> Result<(), String> {
        Ok(())
    }
}

/// What the fake Docker answers a container create.
#[derive(Clone, Copy)]
enum CreateAnswer {
    /// Docker creates every container that it is asked for.
    Accept,
    /// The storage driver refuses a layer size and takes a create
    /// without one.
    RefuseLayerSize,
    /// Every create fails on a full disk.
    DiskFull,
}

#[derive(Clone)]
struct DockerFixture {
    port: u16,
    container_image: &'static str,
    pull_error: bool,
    health_ready: bool,
    create: CreateAnswer,
    /// Whether `GET /info` names the containerd image store.
    containerd_store: bool,
    /// The body of every container create, in order.
    creates: Arc<Mutex<Vec<serde_json::Value>>>,
}

/// A Docker that runs the pinned image, under a graph driver, and
/// takes every create.
fn fixture() -> DockerFixture {
    DockerFixture {
        port: 0,
        container_image: PLATFORM_CONFIG_ID,
        pull_error: false,
        health_ready: true,
        create: CreateAnswer::Accept,
        containerd_store: false,
        creates: Arc::default(),
    }
}

async fn docker_response(State(fixture): State<DockerFixture>, request: Request) -> Response<Body> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("request body");
    if path == "/healthz" {
        return Response::builder()
            .status(if fixture.health_ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            })
            .body(Body::empty())
            .expect("health response");
    }
    // screend's window list, with the browser open.
    if path == "/windows" {
        return json_response(
            StatusCode::OK,
            json!([{ "app_id": "chromium" }]).to_string(),
        );
    }
    if path.ends_with("/images/create") {
        let item = if fixture.pull_error {
            json!({
                "error": "manifest rejected",
                "errorDetail": { "code": 500, "message": "registry denied the manifest" }
            })
        } else {
            json!({ "status": "downloaded" })
        };
        return json_response(StatusCode::OK, format!("{item}\n"));
    }
    if path.ends_with("/containers/create") {
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("a container create carries JSON");
        let sized = body["HostConfig"]["StorageOpt"]["size"].is_string();
        fixture.creates.lock().expect("the creates").push(body);
        let refusal = match fixture.create {
            CreateAnswer::Accept => None,
            CreateAnswer::RefuseLayerSize => sized.then_some(LAYER_SIZE_REFUSAL),
            CreateAnswer::DiskFull => {
                Some("mkdir /var/lib/docker/overlay2/0123-init: no space left on device")
            }
        };
        return match refusal {
            Some(message) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "message": message }).to_string(),
            ),
            None => json_response(
                StatusCode::CREATED,
                json!({ "Id": "container-id", "Warnings": [] }).to_string(),
            ),
        };
    }
    if path.contains("/containers/") && (method == Method::DELETE || path.ends_with("/start")) {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .expect("empty response");
    }
    if path.ends_with("/volumes/create") {
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("a volume create carries JSON");
        return json_response(
            StatusCode::CREATED,
            json!({
                "Name": body["Name"],
                "Driver": "local",
                "Mountpoint": "/var/lib/docker/volumes/data",
                "Labels": body["Labels"],
                "Options": body["DriverOpts"],
                "Scope": "local"
            })
            .to_string(),
        );
    }
    if path.contains("/networks/") {
        return json_response(
            StatusCode::OK,
            json!({ "Name": "tenant-network", "Id": "network-id" }).to_string(),
        );
    }
    if path.ends_with("/info") {
        let info = if fixture.containerd_store {
            json!({
                "Driver": "overlayfs",
                "DriverStatus": [["driver-type", "io.containerd.snapshotter.v1"]]
            })
        } else {
            json!({
                "Driver": "overlay2",
                "DriverStatus": [["Backing Filesystem", "xfs"], ["Supports d_type", "true"]]
            })
        };
        return json_response(StatusCode::OK, info.to_string());
    }
    let decoded_path = path
        .replace("%2F", "/")
        .replace("%3A", ":")
        .replace("%40", "@");
    if decoded_path.ends_with(&format!("/images/{IMAGE}/json")) {
        return json_response(
            StatusCode::OK,
            json!({
                "Id": PLATFORM_CONFIG_ID,
                "RepoDigests": [format!("pagis/computer@{INDEX_DIGEST}")],
                "Descriptor": { "digest": PLATFORM_MANIFEST_DIGEST },
                "Config": { "Labels": { VERSION_LABEL: IMAGE_VERSION } }
            })
            .to_string(),
        );
    }
    if path.contains("/containers/") && path.ends_with("/json") {
        return json_response(
            StatusCode::OK,
            json!({
                "Id": "container-id",
                "Image": fixture.container_image,
                "State": { "Running": true },
                "Config": { "Labels": { VERSION_LABEL: IMAGE_VERSION } },
                "NetworkSettings": { "Ports": {
                    "7900/tcp": [{ "HostIp": "127.0.0.1", "HostPort": fixture.port.to_string() }],
                    "7901/udp": [{ "HostIp": "127.0.0.1", "HostPort": "47901" }]
                }}
            })
            .to_string(),
        );
    }
    json_response(StatusCode::NOT_FOUND, "{}".to_string())
}

fn json_response(status: StatusCode, body: String) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("fixture response")
}

/// What `run` gives, and every line the daemon logs at warning level
/// while it runs.
async fn logged<T>(run: impl std::future::Future<Output = T>) -> (T, String) {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&buffer);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || Log(Arc::clone(&writer)))
        .finish();
    let value = run.with_subscriber(subscriber).await;
    let log = String::from_utf8(buffer.lock().expect("the log").clone()).expect("the log is text");
    (value, log)
}

struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("the log").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The runtime over a fake Docker endpoint, and the owner whose
/// container the fixture answers for. The owner's screend token is
/// written first: a container with no token on this disk is not
/// adopted, which is what the daemon wants and not what these tests
/// measure.
async fn runtime(fixture: DockerFixture) -> (BollardRuntime, ComputerOwner) {
    let (runtime, owner, _token_file) = runtime_with_token(fixture, true).await;
    (runtime, owner)
}

/// The same runtime, with or without the owner's screend token on this
/// disk, and the path of that token file.
async fn runtime_with_token(
    fixture: DockerFixture,
    token: bool,
) -> (BollardRuntime, ComputerOwner, std::path::PathBuf) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Docker endpoint");
    let address = listener.local_addr().expect("fake Docker address");
    let fixture = DockerFixture {
        port: address.port(),
        ..fixture
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .fallback(any(docker_response))
                .with_state(fixture),
        )
        .await
        .expect("serve fake Docker endpoint");
    });
    let temp = tempfile::tempdir().expect("Docker search root");
    let search = DockerSearch {
        docker_host: Some(format!("http://{address}")),
        docker_context: None,
        docker_config_dir: temp.path().join("docker"),
        home: temp.path().join("home"),
        system_socket: temp.path().join("docker.sock"),
    };
    let owner = ComputerOwner::new(WorkspaceId::generate(), AgentId::generate());
    let tokens_dir = temp.path().join("tokens");
    let token_file = tokens_dir
        .join(owner.workspace_id.to_string())
        .join(format!("{}.token", owner.agent_id));
    if token {
        std::fs::create_dir_all(token_file.parent().expect("token directory"))
            .expect("token directory");
        std::fs::write(&token_file, "token-of-the-fixture").expect("token file");
    }
    let runtime = BollardRuntime::new(
        Arc::new(DockerDiscovery::new(
            search,
            Arc::new(AlwaysReachable),
            None,
        )),
        RuntimeOptions {
            limits: pagis_computer::ComputerLimits::default(),
            tokens_dir,
            labels: Vec::new(),
        },
    );
    // The search paths and the token file must outlive the runtime.
    std::mem::forget(temp);
    (runtime, owner, token_file)
}

/// A restored State Directory holds no Computer token, because a Backup
/// leaves `computer-tokens/` out. A running container with no token on
/// this disk is not adopted and is no error, and the next start
/// replaces the container and writes a new token.
#[tokio::test]
async fn a_running_computer_with_no_token_on_this_disk_is_started_again() {
    let (runtime, owner, token_file) = runtime_with_token(fixture(), false).await;

    let adopted = runtime
        .running(&owner)
        .await
        .expect("a missing token is no error");
    assert!(adopted.is_none(), "a container with no token is adopted");

    runtime
        .start(&owner, &[], &[])
        .await
        .expect("the next wake starts the computer");
    let token = std::fs::read_to_string(&token_file).expect("the start writes a token");
    assert!(!token.trim().is_empty(), "the new token is empty");
    assert!(
        runtime
            .running(&owner)
            .await
            .expect("inspect the new container")
            .is_some(),
        "the started computer is adopted with its new token"
    );
}

#[tokio::test]
async fn running_compares_the_container_with_dockers_resolved_platform_config() {
    let (matching, matching_owner) = runtime(fixture()).await;
    assert!(
        matching
            .running(&matching_owner)
            .await
            .expect("inspect matching container")
            .expect("healthy running container")
            .image_matches
    );

    let (mismatching, mismatching_owner) = runtime(DockerFixture {
        container_image: PLATFORM_MANIFEST_DIGEST,
        ..fixture()
    })
    .await;
    assert!(
        !mismatching
            .running(&mismatching_owner)
            .await
            .expect("inspect mismatching container")
            .expect("healthy running container")
            .image_matches
    );
}

#[tokio::test]
async fn running_does_not_adopt_a_container_before_its_control_endpoint_is_ready() {
    let (runtime, owner) = runtime(DockerFixture {
        health_ready: false,
        ..fixture()
    })
    .await;

    assert!(
        runtime
            .running(&owner)
            .await
            .expect("inspect booting container")
            .is_none()
    );
}

#[tokio::test]
async fn pull_propagates_a_docker_stream_error_without_false_completion() {
    let (runtime, _owner) = runtime(DockerFixture {
        pull_error: true,
        ..fixture()
    })
    .await;
    let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();

    let error = runtime
        .pull_image(progress)
        .await
        .expect_err("embedded Docker pull error must fail");

    assert!(error.contains("manifest rejected"), "{error}");
    assert!(
        updates.try_recv().is_err(),
        "failed pull reported completion"
    );
}

/// A storage driver that refuses the layer size does not stop a wake:
/// the daemon creates the container again without the size, logs one
/// warning for the daemon and not one for each wake, and the Health
/// view reports the layer as `unsupported`.
#[tokio::test]
async fn a_refused_layer_size_still_wakes_the_computer_and_warns_once() {
    let docker = DockerFixture {
        create: CreateAnswer::RefuseLayerSize,
        ..fixture()
    };
    let creates = Arc::clone(&docker.creates);
    let (runtime, owner) = runtime(docker).await;
    assert_eq!(runtime.container_quota().await, Quota::Unknown);

    let ((first, second), log) = logged(async {
        (
            runtime.start(&owner, &[], &[]).await,
            runtime.start(&owner, &[], &[]).await,
        )
    })
    .await;

    first.expect("the first wake starts the computer");
    second.expect("the second wake starts the computer");
    let creates = creates.lock().expect("the creates").clone();
    assert_eq!(
        creates.len(),
        4,
        "each wake asks with the size, then without"
    );
    assert_eq!(
        creates[0]["HostConfig"]["StorageOpt"]["size"],
        json!((10u64 * 1024 * 1024 * 1024).to_string())
    );
    let second_size = &creates[1]["HostConfig"]["StorageOpt"];
    assert!(second_size.is_null(), "{second_size}");
    // The second create keeps every other part of the container.
    assert_eq!(
        creates[1]["HostConfig"]["Memory"],
        creates[0]["HostConfig"]["Memory"]
    );
    let warnings: Vec<&str> = log.lines().filter(|line| line.contains("WARN")).collect();
    assert_eq!(warnings.len(), 1, "{log}");
    assert!(warnings[0].contains("writable layer"), "{log}");
    assert_eq!(runtime.container_quota().await, Quota::Unsupported);
    assert_eq!(runtime.container_quota().await.as_str(), "unsupported");
}

/// Only the refusal of the layer size lets the daemon create the
/// container without it. A full disk fails the wake, and the daemon
/// does not try again without the size.
#[tokio::test]
async fn a_create_failure_that_is_not_the_layer_size_fails_the_wake() {
    let docker = DockerFixture {
        create: CreateAnswer::DiskFull,
        ..fixture()
    };
    let creates = Arc::clone(&docker.creates);
    let (runtime, owner) = runtime(docker).await;

    let error = runtime
        .start(&owner, &[], &[])
        .await
        .expect_err("a full disk fails the wake");

    assert!(error.contains("no space left on device"), "{error}");
    assert_eq!(creates.lock().expect("the creates").len(), 1);
    assert_eq!(runtime.container_quota().await, Quota::Unknown);
}

/// A graph driver that takes the layer size holds the layer to it. The
/// containerd image store takes the size and holds the layer to
/// nothing, so there the answer is `unsupported`, with one warning.
#[tokio::test]
async fn a_taken_layer_size_is_supported_under_a_graph_driver_alone() {
    for (containerd_store, answer, warnings) in
        [(false, Quota::Supported, 0), (true, Quota::Unsupported, 1)]
    {
        let docker = DockerFixture {
            containerd_store,
            ..fixture()
        };
        let creates = Arc::clone(&docker.creates);
        let (runtime, owner) = runtime(docker).await;

        let (woken, log) = logged(runtime.start(&owner, &[], &[])).await;

        woken.expect("the wake starts the computer");
        let creates = creates.lock().expect("the creates").clone();
        assert_eq!(creates.len(), 1);
        let size = &creates[0]["HostConfig"]["StorageOpt"];
        assert!(size["size"].is_string(), "{size}");
        assert_eq!(runtime.container_quota().await, answer);
        assert_eq!(
            log.lines().filter(|line| line.contains("WARN")).count(),
            warnings,
            "{log}"
        );
    }
}
