use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Context;
use pagis_core::{User, UserRole, now_ms};
use rand::RngCore;

use crate::config::Config;

/// The name of the one Org of an installation. An administrator
/// renames it later.
const DEFAULT_ORG_NAME: &str = "Org";
/// The file that holds the Client Credential of a local installation.
/// The Client App reads it and trades it for a Session;
/// it is never handed to a browser.
pub const CLIENT_CREDENTIAL_FILE: &str = "client-credential";
/// The directory of the screend token of each Computer. The daemon
/// writes a new token at each start of a Computer.
pub const COMPUTER_TOKENS_DIR: &str = "computer-tokens";

/// The directory of the Plugin stderr logs: one directory for each
/// Workspace, and one file for each Plugin in it (ADR-0017). It is in
/// the `logs` directory, so only the daemon's OS user enters it and a
/// Backup does not carry it.
pub fn plugin_logs_directory(home: &Path) -> PathBuf {
    home.join("logs").join("plugins")
}

/// Which kind of installation this process runs (ADR-0025). The
/// process that starts the daemon says it: the Client App, and a person
/// at a terminal, pass `--local`; the headless image and a service
/// manager do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installation {
    /// An installation on the owner's own machine. It holds a Client
    /// Credential, whatever its Public Origin, so the owner's Client App
    /// opens signed in even where other People reach the installation
    /// through the owner's proxy.
    Local,
    /// A server that nobody sits at. It holds no Client Credential, and
    /// People sign in with an address and a password.
    Server,
}

#[derive(Debug)]
pub struct Booted {
    pub home: PathBuf,
    pub config: Config,
    /// The Client Credential of this local installation, or `None` on
    /// a server. This is the one record of which kind of installation
    /// the process runs, and every other site reads it: the first-run
    /// flow, the credential trade, the sign-in link, the runtime
    /// identity handshake and the start banner. A server writes no
    /// credential, because nobody sits at its machine and a file there
    /// would be a second way in for anybody with a shell on the host.
    pub client_credential: Option<String>,
    /// Every repository, behind its trait. The backend is the one the
    /// configuration names, and nothing here knows which it is.
    pub stores: pagis_core::Stores,
    /// True when this boot created the workspace: the first run on an
    /// empty data directory, which opens the browser on it.
    pub first_run: bool,
    /// The held instance lock (below), released by the OS when this
    /// file closes at process exit. Never read, only kept open.
    _instance_lock: std::fs::File,
}

impl Booted {
    /// The kind of installation this process runs, read from the one
    /// record of it: a local installation holds a Client Credential.
    pub fn installation(&self) -> Installation {
        match self.client_credential {
            Some(_) => Installation::Local,
            None => Installation::Server,
        }
    }
}

/// Bring the data directory to a runnable state: layout, config, the
/// Client Credential, the migrated database, and the seed records.
/// Idempotent.
pub async fn boot(home: &Path, installation: Installation) -> anyhow::Result<Booted> {
    create_state_directory(home)?;
    make_state_directory_private(home)?;

    // One daemon per data directory: recovery below fails every
    // unfinished run and expires pending requests, which is safe for
    // the daemon that actually owns them but corrupts a live sibling's
    // in-memory runs if a second `pagis` starts against the same home
    // (it would run recovery, then fail to bind the port and exit,
    // having already yanked state out from under the real daemon).
    // Fail fast here instead, before recovery or migrations run.
    let instance_lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // The file is a lock handle, never read: keep whatever it holds.
        .truncate(false)
        .open(home.join("pagis.lock"))?;
    wait_for_instance_lock(&instance_lock, home).await?;

    enforce_runtime_release(home, pagis_server::VERSION)?;

    let config = Config::load_or_init(&home.join("config.toml"))?;
    config.check_server_origin(installation, &|key| std::env::var(key).ok())?;
    config.check_database(installation)?;
    // The kind of installation decides whether it holds a Client
    // Credential, and every other site reads the answer from here
    // (ADR-0025). A server removes a file that a local run left, so the
    // file on disk says the same as this record.
    let credential_file = home.join(CLIENT_CREDENTIAL_FILE);
    let client_credential = match installation {
        Installation::Local => Some(load_or_create_client_credential(&credential_file)?),
        Installation::Server => {
            remove_client_credential(&credential_file)?;
            None
        }
    };

    let stores = open_stores(home, &config).await?;
    let first_run = seed(&stores).await?;
    // The Sessions that expired are of no use to anybody. One sweep a
    // boot keeps the table the size of the live sessions.
    let expired = stores.sessions.delete_expired(now_ms()).await?;
    if expired > 0 {
        tracing::info!(expired, "dropped the sessions that expired");
    }
    ensure_well_known_aliases(&stores).await?;
    recover(&stores).await?;

    Ok(Booted {
        home: home.to_path_buf(),
        config,
        client_credential,
        stores,
        first_run,
        _instance_lock: instance_lock,
    })
}

/// Create the State Directory and its `logs` directory where they are
/// not there. Only the OS user that runs the daemon can enter them.
pub fn create_state_directory(home: &Path) -> anyhow::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home.join("logs"))
        .with_context(|| format!("create the state directory {}", home.display()))
}

/// Set a State Directory that a group or other users can enter to mode
/// 700. The Client App can make the directory with a looser mode, and
/// it becomes private with no step from the owner.
fn make_state_directory_private(home: &Path) -> anyhow::Result<()> {
    let mode = std::fs::metadata(home)
        .with_context(|| format!("read the state directory {}", home.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 == 0 {
        return Ok(());
    }
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("set the state directory {} to mode 700", home.display()))?;
    tracing::info!(
        home = %home.display(),
        mode = format!("{mode:o}"),
        "other users could enter the state directory; it is mode 700 now"
    );
    Ok(())
}

/// Open the backend the configuration names and apply its migrations.
///
/// [`Config::check_database`] has already matched the backend to the
/// kind of installation: a server names Postgres in `[database] url`,
/// and a local installation names nothing and runs on SQLite in the
/// state directory.
async fn open_stores(home: &Path, config: &Config) -> anyhow::Result<pagis_core::Stores> {
    match config.database.url() {
        Some(url) => Ok(pagis_storage_postgres::open(url).await?),
        None => Ok(pagis_storage_sqlite::open(&home.join("pagis.db")).await?),
    }
}

/// Refuse to open data that a newer server may already have migrated. The
/// marker is server-owned, so clearing the Client App state cannot lower it.
fn enforce_runtime_release(home: &Path, current: &str) -> anyhow::Result<()> {
    let current = semver::Version::parse(current).context("parse this server release")?;
    let marker = home.join("runtime-release");
    if marker.exists() {
        let text = std::fs::read_to_string(&marker)
            .with_context(|| format!("read {}", marker.display()))?;
        let recorded = semver::Version::parse(text.trim())
            .with_context(|| format!("{} has an invalid server release", marker.display()))?;
        match recorded.cmp_precedence(&current) {
            std::cmp::Ordering::Greater => anyhow::bail!(
                "Pagis {current} cannot open data already opened by newer Pagis {recorded}"
            ),
            std::cmp::Ordering::Equal => return Ok(()),
            std::cmp::Ordering::Less => {}
        }
    }

    let temporary = home.join("runtime-release.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    writeln!(file, "{current}")?;
    file.sync_all()?;
    std::fs::rename(&temporary, &marker)?;
    Ok(())
}

/// How long a boot waits for the instance lock before it reports a
/// running daemon. A daemon that is on its way out can hold the lock
/// for a moment after its owner let go: a child it forked keeps the
/// file description open until it execs. A restart that follows at
/// once must not read that as a live sibling.
const INSTANCE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
const INSTANCE_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(25);

async fn wait_for_instance_lock(lock: &std::fs::File, home: &Path) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + INSTANCE_LOCK_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(INSTANCE_LOCK_POLL).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!(
                    "pagis is already running against {}; stop it first",
                    home.display()
                );
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(err).context("acquiring the pagis instance lock");
            }
        }
    }
}

/// Mint a one-time sign-in link for the administrator of the
/// installation. The link starts at `origin`, which is the local origin
/// of the daemon: it answers a browser on this machine alone
/// (ADR-0025).
pub async fn sign_in_link(
    stores: &pagis_core::Stores,
    public_origin: &str,
) -> anyhow::Result<String> {
    let user = administrator(stores)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the installation has no administrator"))?;
    Ok(pagis_server::mint_sign_in_link(
        stores.sign_in_links.as_ref(),
        &user.id,
        public_origin,
        now_ms(),
    )
    .await?)
}

/// The administrator of the one Org, or `None` before the seed runs.
async fn administrator(stores: &pagis_core::Stores) -> anyhow::Result<Option<User>> {
    let orgs = stores.orgs.list().await?;
    let Some(org) = orgs.into_iter().next() else {
        return Ok(None);
    };
    Ok(stores
        .users
        .list_by_org(&org.id)
        .await?
        .into_iter()
        .find(|user| user.role == UserRole::Administrator))
}

/// Read the Client Credential, or write one. It is 32 random bytes in
/// hexadecimal, readable by the OS user alone.
fn load_or_create_client_credential(path: &Path) -> anyhow::Result<String> {
    match std::fs::metadata(path) {
        Ok(metadata) => read_client_credential(path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => write_client_credential(path),
        Err(error) => {
            Err(error).with_context(|| format!("read the client credential {}", path.display()))
        }
    }
}

/// The Client Credential in a file that is there. The credential gives
/// a Session of the Administrator, so a file that a group or another
/// user can read is refused, as a Key File is. A file that holds no
/// credential, such as the empty file of a write that stopped, is
/// refused too: an empty credential matches an empty one in the trade.
fn read_client_credential(path: &Path, metadata: &std::fs::Metadata) -> anyhow::Result<String> {
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!(
            "the client credential at {} is mode {mode:o}: a group or another user can read \
             the credential that signs in as the Administrator. Remove the file, and the next \
             start writes a new one",
            path.display()
        );
    }
    let credential = std::fs::read_to_string(path)
        .with_context(|| format!("read the client credential {}", path.display()))?
        .trim()
        .to_string();
    if credential.len() != 64 || !credential.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!(
            "the client credential at {} does not hold 64 hexadecimal characters. Remove the \
             file, and the next start writes a new one",
            path.display()
        );
    }
    Ok(credential)
}

/// Write a new Client Credential with mode 600 from the open that makes
/// the file, as the Key File is written. It never replaces a file.
fn write_client_credential(path: &Path) -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let credential: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create the client credential {}", path.display()))?;
    file.write_all(credential.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("write the client credential {}", path.display()))?;
    Ok(credential)
}

/// Remove a Client Credential that a local run left behind.
fn remove_client_credential(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            tracing::info!(path = %path.display(), "removed the client credential of a local run");
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("remove the client credential {}", path.display()))
        }
    }
}

/// Boot recovery: a restart fails every unfinished run,
/// fails every orphaned streaming message, and expires the pending
/// requests of parked runs.
///
/// It is server-wide, and on a server that serves many people it stays
/// server-wide. One daemon owns every run of the installation and
/// holds each one's mid-turn state in its own memory, so a restart ends
/// the work of everybody, not of the person who asked for it. Scoping
/// recovery to one tenant would leave another tenant's run marked
/// `running` with no process behind it, which is worse than a failure
/// that says what happened. Every failed run carries its own reason, so
/// each person reads why their run ended;
/// `docs/DEPLOYING-A-SERVER.md` says who a restart reaches.
async fn recover(stores: &pagis_core::Stores) -> anyhow::Result<()> {
    let failed = pagis_agent::fail_unfinished_runs(
        stores.runs.as_ref(),
        stores.messages.as_ref(),
        stores.requests.as_ref(),
        stores.events.as_ref(),
    )
    .await?;
    if failed > 0 {
        tracing::info!(failed, "marked unfinished runs failed after restart");
    }
    Ok(())
}

/// Seed the Org, its administrator, their Workspace, the default
/// assistant, and its DM channel on an empty database. Returns true when
/// this call seeded.
///
/// The Workspace itself comes from the one seed an Administrator's
/// account creation uses as well, so a local first run and a new
/// account on a server give a person the same starting point.
///
/// The seeded person is the administrator and has no password: a local
/// installation signs in with the Client Credential, and the person may
/// set a password later.
async fn seed(stores: &pagis_core::Stores) -> anyhow::Result<bool> {
    if !stores.workspaces.list().await?.is_empty() {
        return Ok(false);
    }

    let now = now_ms();
    let user = pagis_core::seed_org_and_administrator(
        stores.orgs.as_ref(),
        stores.users.as_ref(),
        DEFAULT_ORG_NAME,
        now,
    )
    .await?;

    pagis_server::provisioning::WorkspaceSeed::from(stores)
        .run(
            &user.id,
            pagis_server::provisioning::DEFAULT_WORKSPACE_NAME,
            &iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
            // The boot does not know yet whether this installation is a
            // server, so the seed keeps the wizard. A server's own setup
            // answers its questions when it runs.
            pagis_server::provisioning::Onboarding::Wizard,
            now,
        )
        .await?;
    Ok(true)
}

/// Add required product aliases without replacing user choices. This
/// runs on every boot because a new binary can add a well-known alias to
/// a Workspace that already exists.
async fn ensure_well_known_aliases(stores: &pagis_core::Stores) -> anyhow::Result<()> {
    let seed = pagis_server::provisioning::WorkspaceSeed::from(stores);
    for workspace in stores.workspaces.list().await? {
        seed.ensure_aliases(&workspace.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod runtime_release_tests {
    use super::enforce_runtime_release;

    #[test]
    fn release_marker_allows_the_same_or_newer_server_and_refuses_a_downgrade() {
        let home = tempfile::tempdir().unwrap();
        enforce_runtime_release(home.path(), "1.2.3").unwrap();
        enforce_runtime_release(home.path(), "1.2.3").unwrap();
        enforce_runtime_release(home.path(), "1.3.0").unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("runtime-release")).unwrap(),
            "1.3.0\n"
        );
        let error = enforce_runtime_release(home.path(), "1.2.9")
            .unwrap_err()
            .to_string();
        assert!(error.contains("newer Pagis 1.3.0"), "{error}");
    }

    #[test]
    fn invalid_release_marker_fails_closed() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("runtime-release"), "latest\n").unwrap();
        let error = enforce_runtime_release(home.path(), "1.2.3")
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid server release"), "{error}");
    }
}
