//! A Postgres for the tests that run against both backends.
//!
//! One `postgres:17-alpine` container for the machine, and one schema
//! inside one database for each test, which is the isolation
//! `#[sqlx::test]` gives the SQLite side. The container is started
//! through the Docker daemon with `bollard`, which the workspace already
//! carries for the agent Computer.
//!
//! A schema, not a database: `CREATE DATABASE` copies a template, so a
//! full run of the suite would write more than a gigabyte and fill the
//! virtual machine the Docker host runs in. A schema costs what the test
//! writes into it and nothing else. The connection URL carries
//! `options=-c search_path=<schema>`, so every connection of that test —
//! the store set, the migrations and a daemon the test boots — reads and
//! writes inside it.
//!
//! The container has a fixed name and is shared, because the test
//! runner gives each test a process of its own: one container for each
//! process would be one for each test, and a machine running the suite
//! would hold tens of them. The first process to ask starts it; every
//! other process finds it by name and reuses it. It stays up between
//! runs, so a later run starts at once. A container that an older build
//! made, with another image or a smaller `/dev/shm` than `SHM_SIZE`, is
//! removed and made again. It holds only test data. Remove it, with the
//! volume that holds its data, by hand when you want the space back:
//!
//! ```text
//! docker rm -f -v pagis-test-postgres
//! ```
//!
//! The name of each schema, and of each database that [`Cluster`]
//! makes, holds the id of the process that made it. Each process that
//! makes one drops first the schemas and databases of the processes
//! that ended, so the container holds little more than the tests that
//! run now. A name whose process id is in use again stays until its
//! run is an hour old.
//!
//! When Docker is not reachable, every call answers `None` with one
//! message that says why, and the caller skips. A test never waits for a
//! daemon that is not there: every step has a deadline.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use bollard::Docker;
use bollard::models::{ContainerCreateBody, ContainerInspectResponse};
use sqlx::PgPool;
use tokio::sync::OnceCell;

/// The image the tests run. It is pinned, so a test never depends on
/// which Postgres the machine happens to hold.
const IMAGE: &str = "postgres:17-alpine";
/// The name and the label of the one container. The name is what makes
/// the container shared: a second process that asks for it finds it by
/// name and uses it.
const NAME: &str = "pagis-test-postgres";
/// The size of `/dev/shm` in the container, in bytes.
///
/// Postgres keeps its dynamic shared memory in `/dev/shm`. The largest
/// user is the cumulative statistics. They keep about 300 bytes for each
/// relation that a test uses, until the schema of the relation is
/// dropped. A test schema holds about 400 relations, so it costs about
/// 120 KB. The statistics grow in segments that double in size. With the
/// Docker default of 64 MB, Postgres cannot add the next segment at
/// about 560 schemas, and a backend crashes.
///
/// Each run drops the schemas of the processes that ended. Under nextest
/// the container then holds about one schema for each test that runs,
/// and a stress run of the store suite uses about 3 MB. The size is for
/// the cases that this rule does not decide. A gate run makes about 240
/// schemas. When one process holds all of them to the end of the run,
/// as `cargo test` does, they need about 45 MB. When the one-hour rule
/// of [`finished`] decides, an hour of stress runs makes about 1200
/// schemas, which need about 190 MB. 512 MB holds about 3000 schemas and
/// the segments of parallel queries. `/dev/shm` is a tmpfs, so it uses
/// memory only for what Postgres writes. 512 MB is one eighth of the
/// 4 GB of the Colima virtual machine.
const SHM_SIZE: i64 = 512 * 1024 * 1024;
const USER: &str = "pagis";
const PASSWORD: &str = "pagis";
/// How long a process waits for the server to accept connections. It
/// covers the first start, where the image has to initialize a cluster.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(200);
/// How many times a process reads the container again, and how long it
/// waits between two reads, while other processes make or remove the
/// container of the same name. The removal of a container deletes its
/// data volume, which takes seconds when the volume is large, and 30
/// seconds covers it.
const ENSURE_ATTEMPTS: u32 = 120;
const ENSURE_BACKOFF: Duration = Duration::from_millis(250);
/// How long a test schema or database lives, when its process id is in
/// use again, before a later run drops it. A run that takes longer than
/// this is not a run.
const STALE_AFTER: u64 = 3600;
/// The first key of the advisory lock that a process holds while it
/// drops one test name. The key is "pagi" in ASCII, and the second key
/// is a hash of the name. Advisory locks with two keys do not meet the
/// one-key locks of the migrations.
const DROP_LOCK: i32 = 0x7061_6769;
/// The one database every test schema lives in.
const DATABASE: &str = "postgres";
/// How many times a test tries to make its database before it fails,
/// and how long it waits between two attempts.
///
/// The first process of a run starts the container, and the image
/// initializes a cluster and restarts the server once before it serves.
/// Every other process reaches the port inside that window and is told
/// the system is not yet accepting connections. The Docker host also
/// forwards a published port from a virtual machine, and that forward
/// can drop for a moment under a full run. Forty attempts at half a
/// second covers both, and a failure past that is a real one.
const SETUP_ATTEMPTS: u32 = 40;
const SETUP_BACKOFF: Duration = Duration::from_millis(500);

/// The Docker client, and whether the container is up. The reason is
/// held, so a machine with no Docker answers the same thing every time.
static DOCKER: OnceCell<Result<Docker, String>> = OnceCell::const_new();
static UP: OnceCell<Result<(), String>> = OnceCell::const_new();
/// The published address, as the last inspect read it. The Docker host
/// gives the container a fresh port each time it is created, so the
/// address is read again after a connection is refused instead of being
/// held for the life of the process.
static ADDRESS: tokio::sync::RwLock<Option<String>> = tokio::sync::RwLock::const_new(None);
/// Names the databases of this process apart.
static NEXT: AtomicU32 = AtomicU32::new(0);

fn url(address: &str) -> String {
    format!("postgres://{USER}:{PASSWORD}@{address}/{DATABASE}")
}

/// The URL of one test's schema. `search_path` sends every unqualified
/// name of every statement into it, which is what makes one database
/// hold many tests.
fn schema_url(address: &str, schema: &str) -> String {
    format!("{}?options=-c%20search_path%3D{schema}", url(address))
}

/// One empty Postgres schema, with its URL and a pool on it. Every
/// connection the URL opens reads and writes inside the schema.
pub struct TestDatabase {
    pub url: String,
    pub pool: PgPool,
}

/// One empty Postgres schema with the migrations applied, or `None` when
/// Docker is not reachable.
///
/// The caller holds it for the length of one test.
pub async fn database() -> Option<TestDatabase> {
    let docker = match DOCKER.get_or_init(client).await {
        Ok(docker) => docker,
        Err(reason) => {
            skipped(reason);
            return None;
        }
    };
    if let Err(reason) = UP.get_or_init(|| start(docker)).await {
        skipped(reason);
        return None;
    }
    // Many tests set up at once, and the port forward of the Docker
    // host is not always up the moment the container is. Every step here
    // is idempotent on a name of this process's own, so the answer to a
    // refusal is to read the address again and try again.
    let mut last = String::new();
    for _ in 0..SETUP_ATTEMPTS {
        let address = match address(docker).await {
            Ok(address) => address,
            Err(reason) => {
                last = reason;
                tokio::time::sleep(SETUP_BACKOFF).await;
                continue;
            }
        };
        match make_database(&address).await {
            Ok(database) => return Some(database),
            Err(reason) => {
                last = reason;
                // A refused connection can mean the container was made
                // again with a different port, so drop the address.
                ADDRESS.write().await.take();
                tokio::time::sleep(SETUP_BACKOFF).await;
            }
        }
    }
    panic!("could not make a test database in {SETUP_ATTEMPTS} attempts: {last}");
}

fn skipped(reason: &str) {
    eprintln!(
        "SKIPPED: this test needs Postgres and Docker is not reachable: {reason}\n\
         Start Docker and set DOCKER_HOST, for example \
         `export DOCKER_HOST=unix:///Users/<you>/.colima/default/docker.sock`."
    );
}

async fn client() -> Result<Docker, String> {
    let docker = Docker::connect_with_defaults()
        .map_err(|error| format!("cannot reach the Docker daemon: {error}"))?;
    with_deadline(docker.ping(), Duration::from_secs(10))
        .await
        .map_err(|error| format!("the Docker daemon did not answer a ping: {error}"))?;
    Ok(docker)
}

/// The published address of the one container, read again when it is
/// absent.
async fn address(docker: &Docker) -> Result<String, String> {
    if let Some(address) = ADDRESS.read().await.clone() {
        return Ok(address);
    }
    let address = published_address(docker, NAME).await?;
    *ADDRESS.write().await = Some(address.clone());
    Ok(address)
}

/// The address on the host where the container `name` publishes its
/// Postgres port, as the Docker daemon reports it now.
async fn published_address(docker: &Docker, name: &str) -> Result<String, String> {
    let container = docker
        .inspect_container(
            name,
            None::<bollard::query_parameters::InspectContainerOptions>,
        )
        .await
        .map_err(|error| format!("cannot inspect the {IMAGE} container: {error}"))?;
    let port = published_port(&container).ok_or_else(|| {
        format!(
            "the container published no Postgres port: {:?}",
            container.state
        )
    })?;
    Ok(format!("127.0.0.1:{port}"))
}

async fn make_database(address: &str) -> Result<TestDatabase, String> {
    let schema = format!(
        "pagis_test_{}_{}_{}",
        started_at(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    // One connection, not a pool: this does a few statements and goes.
    let mut admin = <sqlx::PgConnection as sqlx::Connection>::connect(&url(address))
        .await
        .map_err(|error| format!("connect to the test Postgres at {address}: {error}"))?;
    drop_finished(&mut admin).await;
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut admin)
        .await
        .map_err(|error| format!("create the test schema: {error}"))?;
    let _ = sqlx::Connection::close(admin).await;
    let url = schema_url(address, &schema);
    let pool = pagis_storage_postgres::connect(&url)
        .await
        .map_err(|error| format!("connect to the test schema: {error}"))?;
    pagis_storage_postgres::MIGRATOR
        .run(&pool)
        .await
        .map_err(|error| format!("apply the migrations: {error}"))?;
    Ok(TestDatabase { url, pool })
}

/// Find the one container, or make it, and wait until it serves.
async fn start(docker: &Docker) -> Result<(), String> {
    pull(docker).await?;
    ensure(docker, NAME, &container_config()).await?;
    let address = wait_ready(docker, NAME).await?;
    *ADDRESS.write().await = Some(address);
    Ok(())
}

/// Pull the image when the Docker host does not hold it. A fresh host,
/// such as a CI runner, holds no image, and Docker makes no container
/// from an image it has not pulled.
async fn pull(docker: &Docker) -> Result<(), String> {
    use futures::StreamExt as _;

    if docker.inspect_image(IMAGE).await.is_ok() {
        return Ok(());
    }
    let options = bollard::query_parameters::CreateImageOptionsBuilder::default()
        .from_image(IMAGE)
        .build();
    let mut progress = docker.create_image(Some(options), None, None);
    while let Some(step) = progress.next().await {
        step.map_err(|error| format!("cannot pull {IMAGE}: {error}"))?;
    }
    Ok(())
}

/// Find the container `name`, or make it from `config`, and start it.
///
/// The container outlives the runs, so the one that holds the name can
/// be from an older `config`. A container that does not [fit](fits) is
/// removed and made again.
///
/// Many processes do this at the same time, and Docker decides each
/// race. A name conflict on create means that another process made the
/// container first. A removal names the container by its id, so it
/// removes only the container that does not fit, and never the one that
/// another process made in its place. After each step the process reads
/// the container again, until a container that fits holds the name.
async fn ensure(docker: &Docker, name: &str, config: &ContainerCreateBody) -> Result<(), String> {
    for _ in 0..ENSURE_ATTEMPTS {
        match docker
            .inspect_container(
                name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(found) if fits(&found, config) => return start_container(docker, name).await,
            Ok(found) => {
                let id = found
                    .id
                    .ok_or_else(|| format!("the {IMAGE} container has no id"))?;
                remove(docker, &id).await?;
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(error) => return Err(format!("cannot inspect the {IMAGE} container: {error}")),
        }
        match docker
            .create_container(
                Some(
                    bollard::query_parameters::CreateContainerOptionsBuilder::default()
                        .name(name)
                        .build(),
                ),
                config.clone(),
            )
            .await
        {
            Ok(_) => {}
            // Another process makes or removes a container of this name
            // at the same time.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => tokio::time::sleep(ENSURE_BACKOFF).await,
            Err(error) => return Err(format!("cannot create the {IMAGE} container: {error}")),
        }
    }
    Err(format!(
        "no {IMAGE} container that fits holds the name {name} after {ENSURE_ATTEMPTS} attempts"
    ))
}

/// Start the container `name`. A container that an earlier run left
/// stopped comes back up, and a container that runs already is not an
/// error.
async fn start_container(docker: &Docker, name: &str) -> Result<(), String> {
    match docker
        .start_container(
            name,
            None::<bollard::query_parameters::StartContainerOptions>,
        )
        .await
    {
        Ok(())
        | Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 304, ..
        }) => Ok(()),
        Err(error) => Err(format!("cannot start the {IMAGE} container: {error}")),
    }
}

/// Remove the container `id` with the anonymous volume that holds its
/// data. A container that another process removed first is gone all
/// the same, so that is not an error.
async fn remove(docker: &Docker, id: &str) -> Result<(), String> {
    match docker
        .remove_container(
            id,
            Some(
                bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(true)
                    .build(),
            ),
        )
        .await
    {
        Ok(())
        | Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404 | 409,
            ..
        }) => Ok(()),
        Err(error) => Err(format!("cannot remove the {IMAGE} container: {error}")),
    }
}

/// Whether a container that holds the name is the container that
/// `wanted` asks for. It has the same image and a `/dev/shm` of the
/// asked size or larger. A larger `/dev/shm` fits, so two builds that ask
/// for two sizes do not remove the container of the other in turn.
fn fits(found: &ContainerInspectResponse, wanted: &ContainerCreateBody) -> bool {
    let image = found
        .config
        .as_ref()
        .and_then(|config| config.image.as_deref());
    let shm_size = |host: Option<&bollard::models::HostConfig>| {
        host.and_then(|host| host.shm_size).unwrap_or(0)
    };
    image == wanted.image.as_deref()
        && shm_size(found.host_config.as_ref()) >= shm_size(wanted.host_config.as_ref())
}

/// Wait until the server in the container `name` accepts a connection,
/// and answer its address, or give up with the last error. A server
/// that never comes up fails the wait; it does not hang the test.
async fn wait_ready(docker: &Docker, name: &str) -> Result<String, String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last = String::from("no attempt finished");
    while Instant::now() < deadline {
        match published_address(docker, name).await {
            Ok(address) => match PgPool::connect(&url(&address)).await {
                Ok(pool) => {
                    pool.close().await;
                    return Ok(address);
                }
                Err(error) => last = error.to_string(),
            },
            Err(reason) => last = reason,
        }
        tokio::time::sleep(READY_POLL).await;
    }
    Err(format!(
        "the {IMAGE} container did not accept a connection in {}s: {last}",
        READY_TIMEOUT.as_secs()
    ))
}

fn container_config() -> ContainerCreateBody {
    let host_config = bollard::models::HostConfig {
        port_bindings: Some(HashMap::from([(
            "5432/tcp".to_string(),
            Some(vec![bollard::models::PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("0".to_string()),
            }]),
        )])),
        shm_size: Some(SHM_SIZE),
        ..Default::default()
    };
    ContainerCreateBody {
        image: Some(IMAGE.to_string()),
        // `fsync` off and no full page writes: the schemas live for one
        // run, and a crash of the container loses nothing anybody wants.
        // `max_connections` covers the tests that run at the same time,
        // each with a pool of its own.
        cmd: Some(
            [
                "postgres",
                "-c",
                "fsync=off",
                "-c",
                "full_page_writes=off",
                "-c",
                "max_connections=300",
            ]
            .iter()
            .map(|word| word.to_string())
            .collect(),
        ),
        env: Some(vec![
            format!("POSTGRES_USER={USER}"),
            format!("POSTGRES_PASSWORD={PASSWORD}"),
            "POSTGRES_DB=postgres".to_string(),
        ]),
        labels: Some(HashMap::from([(NAME.to_string(), "1".to_string())])),
        exposed_ports: Some(HashMap::from([("5432/tcp".to_string(), HashMap::new())])),
        host_config: Some(host_config),
        ..Default::default()
    }
}

/// The host port Docker gave the container's 5432.
fn published_port(container: &ContainerInspectResponse) -> Option<u16> {
    container
        .network_settings
        .as_ref()?
        .ports
        .as_ref()?
        .get("5432/tcp")?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse()
        .ok()
}

/// Drop the test schemas and the test databases whose run has
/// [finished]. The name carries all that the rule reads, so this needs
/// no bookkeeping of its own.
///
/// A drop that fails leaves the name to a later run.
async fn drop_finished(admin: &mut sqlx::PgConnection) {
    let schemas = sqlx::query_scalar::<_, String>(
        "SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 'pagis\\_test\\_%'",
    )
    .fetch_all(&mut *admin)
    .await
    .unwrap_or_default();
    let databases = sqlx::query_scalar::<_, String>(
        "SELECT datname::text FROM pg_database WHERE datname LIKE 'pagis\\_test\\_%'",
    )
    .fetch_all(&mut *admin)
    .await
    .unwrap_or_default();
    let now = unix_seconds();
    // A name that `finished` accepts holds only letters, digits and
    // underscores, so it is a valid identifier without quotes.
    for schema in schemas.iter().filter(|name| finished(name, now, running)) {
        let statement = format!("DROP SCHEMA IF EXISTS {schema} CASCADE");
        drop_unless_taken(admin, schema, &statement).await;
    }
    for database in databases.iter().filter(|name| finished(name, now, running)) {
        let statement = format!("DROP DATABASE IF EXISTS {database}");
        drop_unless_taken(admin, database, &statement).await;
    }
}

/// Run `statement`, the drop of `name`, unless another process drops
/// `name` now. Many processes start at the same time and find the same
/// names. A process holds an advisory lock on a name while it drops it,
/// and a process that does not get the lock goes on to the next name. It
/// does not wait, and it does not fail.
async fn drop_unless_taken(admin: &mut sqlx::PgConnection, name: &str, statement: &str) {
    let taken = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1, hashtext($2))")
        .bind(DROP_LOCK)
        .bind(name)
        .fetch_one(&mut *admin)
        .await;
    if !matches!(taken, Ok(true)) {
        return;
    }
    let _ = sqlx::query(statement).execute(&mut *admin).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1, hashtext($2))")
        .bind(DROP_LOCK)
        .bind(name)
        .execute(&mut *admin)
        .await;
}

/// Whether the run that named a test schema or test database has ended,
/// at the second `now`.
///
/// The name is `pagis_test_<second>_<pid>_<n>`: the second at which the
/// process made its first name, the id of that process, and a count. A
/// process that does not run has ended. A process that runs can be a new
/// process that has the id of an ended one, so the second decides: a run
/// that began more than [`STALE_AFTER`] ago has ended. A name of another
/// form stays.
fn finished(name: &str, now: u64, running: impl Fn(u32) -> bool) -> bool {
    let Some((second, pid)) = named_by(name) else {
        return false;
    };
    second < now.saturating_sub(STALE_AFTER) || !running(pid)
}

/// The second and the process id in a name
/// `pagis_test_<second>_<pid>_<n>`, or `None` for a name of another form.
fn named_by(name: &str) -> Option<(u64, u32)> {
    let parts: Vec<&str> = name.strip_prefix("pagis_test_")?.split('_').collect();
    let [second, pid, count] = parts[..] else {
        return None;
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !(digits(second) && digits(pid) && digits(count)) {
        return None;
    }
    Some((second.parse().ok()?, pid.parse().ok()?))
}

/// Whether a process with the id `pid` runs on this machine.
///
/// Each process that uses the container reaches it on a loopback
/// address, so each one runs on this machine, and its id is an id of
/// this machine.
fn running(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    // Signal 0 only asks if the process exists. A process of another
    // user answers that this process may not signal it, so it runs.
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

async fn with_deadline<F, T, E>(future: F, limit: Duration) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(limit, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("no answer in {}s", limit.as_secs())),
    }
}

/// The second this process started, as every database it makes carries
/// it. It is read once, so every database of one process shares it.
fn started_at() -> u64 {
    static STARTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *STARTED.get_or_init(unix_seconds)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One Postgres database of its own, for the backup test.
///
/// A whole database and not a schema, because `pg_dump` and
/// `pg_restore` work on a database and that is what a deployment backs
/// up. The two URLs name the same database from the two sides of the
/// container: `url` goes through the published port, and `inside` is
/// what a program running in the container uses.
pub struct OwnDatabase {
    pub url: String,
    pub inside: String,
    pub name: String,
}

/// The PostgreSQL client programs, as the backup runs them.
pub struct Client {
    pub pg_dump: std::path::PathBuf,
    pub pg_restore: std::path::PathBuf,
    /// Holds the two programs on disk for as long as the test runs.
    _dir: tempfile::TempDir,
}

/// The test Postgres, with a way to make databases in it and the client
/// programs that dump and restore them. `None` when Docker is
/// not reachable, with the reason already on stderr.
pub struct Cluster {
    address: String,
    client: Client,
}

impl Cluster {
    pub async fn start() -> Option<Self> {
        let docker = match DOCKER.get_or_init(client).await {
            Ok(docker) => docker,
            Err(reason) => {
                skipped(reason);
                return None;
            }
        };
        if let Err(reason) = UP.get_or_init(|| start(docker)).await {
            skipped(reason);
            return None;
        }
        let address = match address(docker).await {
            Ok(address) => address,
            Err(reason) => {
                skipped(&reason);
                return None;
            }
        };
        Some(Self {
            address,
            client: container_client(),
        })
    }

    /// An empty database with no migrations applied: the daemon that
    /// opens it migrates it, and a restore writes the dump's own schema.
    pub async fn database(&self) -> OwnDatabase {
        let name = format!(
            "pagis_test_{}_{}_{}",
            started_at(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut admin = <sqlx::PgConnection as sqlx::Connection>::connect(&url(&self.address))
            .await
            .expect("connect to the test Postgres");
        drop_finished(&mut admin).await;
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&mut admin)
            .await
            .expect("create the test database");
        let _ = sqlx::Connection::close(admin).await;
        OwnDatabase {
            url: format!("postgres://{USER}:{PASSWORD}@{}/{name}", self.address),
            inside: format!("postgres://{USER}:{PASSWORD}@127.0.0.1:5432/{name}"),
            name,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }
}

/// `pg_dump` and `pg_restore` as two one-line programs that run the
/// pinned container's own client. The machine running the tests then
/// needs no PostgreSQL install, and the client's major version matches
/// the server's by construction — which is the same reason
/// `deploy/backup.sh` runs the client in a container rather than on the
/// host.
fn container_client() -> Client {
    let dir = tempfile::tempdir().expect("a directory for the client programs");
    let write = |name: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nexec docker exec -i {NAME} {name} \"$@\"\n"),
        )
        .expect("write the client program");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make the client program executable");
        }
        path
    };
    let pg_dump = write("pg_dump");
    let pg_restore = write("pg_restore");
    Client {
        pg_dump,
        pg_restore,
        _dir: dir,
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::panic::AssertUnwindSafe;

    use futures::FutureExt as _;

    use super::*;

    const MIB: i64 = 1024 * 1024;

    #[test]
    fn the_container_asks_for_a_dev_shm_of_512_mib() {
        let asked = container_config()
            .host_config
            .and_then(|host| host.shm_size);
        assert_eq!(asked, Some(512 * MIB));
    }

    #[test]
    fn a_container_with_a_smaller_dev_shm_than_asked_does_not_fit() {
        assert!(!fits(&inspected(IMAGE, 64 * MIB), &asking(512 * MIB)));
    }

    #[test]
    fn a_container_of_another_image_does_not_fit() {
        assert!(!fits(
            &inspected("postgres:16-alpine", 512 * MIB),
            &asking(512 * MIB)
        ));
    }

    #[test]
    fn a_container_of_the_image_with_at_least_the_asked_dev_shm_fits() {
        assert!(fits(&inspected(IMAGE, 512 * MIB), &asking(512 * MIB)));
        assert!(fits(&inspected(IMAGE, 1024 * MIB), &asking(512 * MIB)));
    }

    #[test]
    fn a_name_is_finished_when_its_process_is_gone_or_its_run_began_over_an_hour_ago() {
        let now = 1_790_000_000;
        let live = |pid| pid == 7;
        assert!(
            finished(&format!("pagis_test_{now}_8_0"), now, live),
            "the process is gone"
        );
        assert!(
            !finished(&format!("pagis_test_{now}_7_0"), now, live),
            "the process runs"
        );
        let old = now - STALE_AFTER - 1;
        assert!(
            finished(&format!("pagis_test_{old}_7_0"), now, live),
            "a process with the id runs, but the run began over an hour ago"
        );
        assert!(
            !finished(&format!("pagis_test_{now}"), now, live),
            "the name has no process id"
        );
        assert!(
            !finished(&format!("pagis_test_{now}_8_x"), now, live),
            "the name has a part that is not a number"
        );
        assert!(
            !finished("pagis_test_", now, live),
            "the name has no second"
        );
        assert!(!finished("public", now, live), "not a test name");
    }

    #[test]
    fn a_process_that_ended_does_not_run_and_this_process_does() {
        assert!(running(std::process::id()));
        assert!(!running(ended_process()));
    }

    #[tokio::test]
    async fn a_container_with_a_smaller_dev_shm_is_made_again_and_one_that_fits_is_reused() {
        with_own_container("remade", |docker, name| async move {
            ensure(&docker, &name, &asking(64 * MIB))
                .await
                .expect("make a container with the default /dev/shm of Docker");
            let small = inspect(&docker, &name).await;
            assert_eq!(shm_size(&small), Some(64 * MIB));
            assert!(is_running(&small));

            let wanted = container_config();
            ensure(&docker, &name, &wanted)
                .await
                .expect("make the container again");
            let made = inspect(&docker, &name).await;
            assert_ne!(
                made.id, small.id,
                "the container with the smaller /dev/shm is made again"
            );
            assert_eq!(
                shm_size(&made),
                wanted.host_config.and_then(|host| host.shm_size)
            );
            assert!(is_running(&made));

            ensure(&docker, &name, &container_config())
                .await
                .expect("find the container");
            let found = inspect(&docker, &name).await;
            assert_eq!(found.id, made.id, "the container that fits is reused");
            assert!(is_running(&found));
        })
        .await;
    }

    #[tokio::test]
    async fn eight_callers_that_find_a_smaller_dev_shm_at_once_leave_one_container_that_fits() {
        with_own_container("raced", |docker, name| async move {
            ensure(&docker, &name, &asking(64 * MIB))
                .await
                .expect("make a container with the default /dev/shm of Docker");
            let small = inspect(&docker, &name).await;

            // Each call is one test process of a run that starts eight at
            // the same time.
            let wanted = container_config();
            let callers = (0..8).map(|_| ensure(&docker, &name, &wanted));
            for outcome in futures::future::join_all(callers).await {
                outcome.expect("each caller finds or makes a container that fits");
            }
            let made = inspect(&docker, &name).await;
            assert_ne!(made.id, small.id);
            assert_eq!(
                shm_size(&made),
                wanted.host_config.and_then(|host| host.shm_size)
            );
            assert!(is_running(&made));
        })
        .await;
    }

    #[tokio::test]
    async fn the_next_run_drops_what_an_ended_process_left_and_keeps_what_a_live_process_holds() {
        with_own_container("dropped", |docker, name| async move {
            ensure(&docker, &name, &container_config())
                .await
                .expect("make the container");
            let address = wait_ready(&docker, &name)
                .await
                .expect("the container serves");
            let now = started_at();
            let old = now - STALE_AFTER - 1;
            let ended = ended_process();
            let live = std::process::id();
            // The last part counts from 1000, apart from the numbers
            // that this process gives to the schemas it makes.
            let ended_schema = format!("pagis_test_{now}_{ended}_1000");
            let live_schema = format!("pagis_test_{now}_{live}_1001");
            let old_schema = format!("pagis_test_{old}_{live}_1002");
            let ended_database = format!("pagis_test_{now}_{ended}_1003");
            let live_database = format!("pagis_test_{now}_{live}_1004");
            let mut admin = <sqlx::PgConnection as sqlx::Connection>::connect(&url(&address))
                .await
                .expect("connect to the test container");
            for schema in [&ended_schema, &live_schema, &old_schema] {
                sqlx::query(&format!("CREATE SCHEMA {schema}"))
                    .execute(&mut admin)
                    .await
                    .expect("make a schema");
            }
            for database in [&ended_database, &live_database] {
                sqlx::query(&format!("CREATE DATABASE {database}"))
                    .execute(&mut admin)
                    .await
                    .expect("make a database");
            }

            let next = make_database(&address)
                .await
                .expect("the next run makes its schema");
            next.pool.close().await;

            let schemas: Vec<String> = sqlx::query_scalar("SELECT nspname::text FROM pg_namespace")
                .fetch_all(&mut admin)
                .await
                .expect("read the schemas");
            let databases: Vec<String> =
                sqlx::query_scalar("SELECT datname::text FROM pg_database")
                    .fetch_all(&mut admin)
                    .await
                    .expect("read the databases");
            let _ = sqlx::Connection::close(admin).await;
            assert!(
                !schemas.contains(&ended_schema),
                "the schema of an ended process is dropped: {schemas:?}"
            );
            assert!(
                schemas.contains(&live_schema),
                "the schema of a live process stays"
            );
            assert!(
                !schemas.contains(&old_schema),
                "a schema whose run began over an hour ago is dropped"
            );
            assert!(
                !databases.contains(&ended_database),
                "the database of an ended process is dropped: {databases:?}"
            );
            assert!(
                databases.contains(&live_database),
                "the database of a live process stays"
            );
        })
        .await;
    }

    /// The configuration of the one container, with another size of
    /// `/dev/shm`.
    fn asking(shm_size: i64) -> ContainerCreateBody {
        let mut config = container_config();
        config
            .host_config
            .get_or_insert_with(Default::default)
            .shm_size = Some(shm_size);
        config
    }

    /// What an inspect of a container of `image` with a `/dev/shm` of
    /// `shm_size` answers.
    fn inspected(image: &str, shm_size: i64) -> ContainerInspectResponse {
        ContainerInspectResponse {
            config: Some(bollard::models::ContainerConfig {
                image: Some(image.to_string()),
                ..Default::default()
            }),
            host_config: Some(bollard::models::HostConfig {
                shm_size: Some(shm_size),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// The id of a process that ran and ended.
    fn ended_process() -> u32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("start a process");
        let pid = child.id();
        child.wait().expect("the process ends");
        pid
    }

    /// Run `body` on a container of the test's own, and remove that
    /// container after it, also when `body` panics. The name holds the
    /// process id, so two runs of the test do not share a container,
    /// and the shared container of the other tests is never touched.
    async fn with_own_container<F, Fut>(role: &str, body: F)
    where
        F: FnOnce(Docker, String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let docker = match client().await {
            Ok(docker) => docker,
            Err(reason) => {
                skipped(&reason);
                return;
            }
        };
        // Nothing here pulls the image, so a machine without it skips,
        // as the tests on the shared container do.
        if let Err(error) = docker.inspect_image(IMAGE).await {
            skipped(&format!(
                "the image {IMAGE} is not on this machine: {error}"
            ));
            return;
        }
        let prefix = format!("{NAME}-{role}-");
        remove_left_behind(&docker, &prefix).await;
        let name = format!("{prefix}{}", std::process::id());
        let outcome = AssertUnwindSafe(body(docker.clone(), name.clone()))
            .catch_unwind()
            .await;
        remove(&docker, &name)
            .await
            .expect("remove the test container");
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }

    /// Remove each container `<prefix><pid>` whose process has ended. A
    /// test process that was killed did not remove its container.
    async fn remove_left_behind(docker: &Docker, prefix: &str) {
        let filters = HashMap::from([("name", vec![prefix])]);
        let Ok(containers) = docker
            .list_containers(Some(
                bollard::query_parameters::ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await
        else {
            return;
        };
        for container in containers {
            let ended = container
                .names
                .iter()
                .flatten()
                .filter_map(|name| name.strip_prefix('/')?.strip_prefix(prefix)?.parse().ok())
                .any(|pid| !running(pid));
            if let (true, Some(id)) = (ended, container.id) {
                let _ = remove(docker, &id).await;
            }
        }
    }

    async fn inspect(docker: &Docker, name: &str) -> ContainerInspectResponse {
        docker
            .inspect_container(
                name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
            .expect("inspect the test container")
    }

    fn shm_size(container: &ContainerInspectResponse) -> Option<i64> {
        container.host_config.as_ref()?.shm_size
    }

    fn is_running(container: &ContainerInspectResponse) -> bool {
        container.state.as_ref().and_then(|state| state.running) == Some(true)
    }
}
