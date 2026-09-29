//! Backup and restore of a whole installation.
//!
//! An installation is two things that have to be captured together: the
//! **state directory**, which holds the sealed secrets, the memory
//! repositories, the artifacts, the recordings, the Plugins and the
//! Software, and the **database**, which is SQLite inside that
//! directory or a Postgres a server runs beside it. A copy of
//! one without the other restores nothing: the records name files that
//! are not there, or the files belong to people who are not there.
//!
//! Four things are deliberately outside a backup, and
//! https://docs.pagis.co/server/backup says so where an administrator reads
//! it:
//!
//! - **The Installation Key.** It seals `secrets.enc` and never sits
//!   beside it (ADR-0013), so a backup that carried it would put
//!   the lock and the key in one archive. The deployment keeps the key
//!   where it keeps its other secrets.
//! - **The Client Credential.** It is a live way into a local
//!   installation: the file, and nothing else, trades for a Session of
//!   the administrator. An archive is copied, shipped and kept, so
//!   the credential stays with the machine that made it and the next boot
//!   of a restored directory writes a fresh one.
//! - **The Computer tokens.** `computer-tokens/` holds the screend token
//!   of each Computer. A token is live while its Computer runs, and the
//!   daemon stops for a backup while its Computers run on. The next
//!   start of each Computer writes a new token, and a running Computer
//!   with no token on the disk is not adopted but started again.
//! - **The Computer volumes.** `pagis-volume-<workspace_id>-<agent_id>`
//!   belongs to the Docker host, not to this process, so
//!   `deploy/backup.sh` captures them beside the archive this module
//!   writes.
//!
//! **Quiesce is enforced, not advised.** A file copied while the daemon
//! writes it is not a backup, so the backup takes the same instance
//! lock the daemon holds for its whole life (`pagis.lock`). A running
//! daemon fails the backup with the reason instead of producing an
//! archive that looks fine.
//!
//! **Only the owner reads an archive.** Every directory that the backup
//! and the restore make is mode 700, and every file keeps the owner's
//! bits of its source and gets no bits for a group or other users, from
//! the open that makes it.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// The archive layout. It is a directory and not a tar file: the
/// deployment's own tooling already packs, ships and encrypts
/// directories, and a directory is what a restore can be checked
/// against before it runs.
pub const MANIFEST_FILE: &str = "manifest.json";
pub const STATE_DIR: &str = "state";
pub const DATABASE_DUMP: &str = "database.dump";

/// The one archive layout this release reads and writes. A restore
/// refuses another, because a layout it does not know is a restore it
/// cannot check.
pub const SCHEMA: u32 = 1;

/// What the state directory never carries into an archive: the log
/// files, which are not data, the instance lock, which belongs to the
/// process that holds it, the Client Credential, which is a live way
/// into a local installation and belongs to the machine the daemon runs
/// on, the Key File a local installation generates, which is the
/// Installation Key, and the Computer tokens, which are live while their
/// Computers run. A restore writes a fresh credential at its next boot,
/// and each Computer gets a new token at its next start.
const EXCLUDED: [&str; 5] = [
    "logs",
    "pagis.lock",
    crate::CLIENT_CREDENTIAL_FILE,
    crate::secrets::GENERATED_KEY_FILE,
    crate::boot::COMPUTER_TOKENS_DIR,
];

/// Which backend held the records when the archive was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseKind {
    /// SQLite, which lives inside the state directory, so the state
    /// copy is the database copy.
    Sqlite,
    /// Postgres, dumped beside the state copy with `pg_dump`.
    Postgres,
}

/// What an archive says about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// The server release that wrote the state. A restore onto an older
    /// server is refused by the release marker at the next boot
    /// (ADR-0025), and this says which release to install before then.
    pub release: String,
    pub database: DatabaseKind,
    pub created_at: pagis_core::UnixMillis,
}

/// The Postgres programs the backup runs. A deployment runs the ones in
/// the server image, whose major version matches the Postgres it runs.
#[derive(Debug, Clone)]
pub struct Tools {
    pub pg_dump: PathBuf,
    pub pg_restore: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            pg_dump: PathBuf::from("pg_dump"),
            pg_restore: PathBuf::from("pg_restore"),
        }
    }
}

/// One installation, as the backup and the restore address it.
#[derive(Debug, Clone)]
pub struct Installation {
    /// The state directory.
    pub home: PathBuf,
    /// The Postgres URL `pg_dump` and `pg_restore` connect to. `None`
    /// selects SQLite inside the state directory, which is what the
    /// local installation runs.
    pub database: Option<String>,
    pub tools: Tools,
}

impl Installation {
    /// The installation a state directory describes: its own
    /// `config.toml` names the database, as it does for the daemon.
    pub fn at(home: &Path) -> Result<Self> {
        Ok(Self {
            home: home.to_path_buf(),
            database: configured_database(&home.join("config.toml"))?,
            tools: Tools::default(),
        })
    }

    /// The installation an archive restores into. The target directory
    /// is empty, so the database is the one the archived `config.toml`
    /// names; a host that moved the database names the new URL itself.
    pub fn from_archive(archive: &Path, home: &Path) -> Result<Self> {
        Ok(Self {
            home: home.to_path_buf(),
            database: configured_database(&archive.join(STATE_DIR).join("config.toml"))?,
            tools: Tools::default(),
        })
    }

    /// Write an archive of this installation into `out`, which must not
    /// already hold one.
    ///
    /// The daemon must be stopped: this takes its instance lock and
    /// fails while another process holds it.
    pub fn back_up(&self, out: &Path) -> Result<Manifest> {
        let _lock = self.quiesce()?;
        empty_directory(out)?;
        copy_tree(&self.home, &out.join(STATE_DIR), &EXCLUDED)?;
        let database = match &self.database {
            Some(url) => {
                pg_dump(&self.tools.pg_dump, url, &out.join(DATABASE_DUMP))?;
                DatabaseKind::Postgres
            }
            None => DatabaseKind::Sqlite,
        };
        let manifest = Manifest {
            schema: SCHEMA,
            release: self.release()?,
            database,
            created_at: pagis_core::now_ms(),
        };
        let json = serde_json::to_vec_pretty(&manifest)?;
        let path = out.join(MANIFEST_FILE);
        new_file(&path, 0o600)
            .and_then(|mut file| file.write_all(&json))
            .with_context(|| format!("write {}", path.display()))?;
        Ok(manifest)
    }

    /// Put an archive back into this installation, which must be a
    /// fresh state directory and, on Postgres, an empty database.
    pub fn restore(&self, archive: &Path) -> Result<Manifest> {
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(archive.join(MANIFEST_FILE))
                .with_context(|| format!("read {}/{MANIFEST_FILE}", archive.display()))?,
        )
        .context("read the archive manifest")?;
        if manifest.schema != SCHEMA {
            bail!(
                "the archive is layout {} and this Pagis reads layout {SCHEMA}",
                manifest.schema
            );
        }
        if manifest.database == DatabaseKind::Postgres && self.database.is_none() {
            bail!(
                "the archive holds a Postgres dump and this installation names no \
                 `[database] url`; name the database before the restore"
            );
        }
        if manifest.database == DatabaseKind::Sqlite && self.database.is_some() {
            bail!(
                "the archive holds a SQLite installation and this one names a \
                 `[database] url`; restore it onto SQLite, which is what it was taken from"
            );
        }
        empty_directory(&self.home)?;
        copy_tree(&archive.join(STATE_DIR), &self.home, &[])?;
        if let Some(url) = &self.database {
            pg_restore(&self.tools.pg_restore, url, &archive.join(DATABASE_DUMP))?;
        }
        Ok(manifest)
    }

    /// The release that wrote this state. The marker is the server's own
    /// high-water mark (ADR-0025) and it is what says which release can
    /// open the restored data.
    fn release(&self) -> Result<String> {
        let marker = self.home.join("runtime-release");
        match std::fs::read_to_string(&marker) {
            Ok(text) => Ok(text.trim().to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                bail!(
                    "{} holds no runtime-release marker, so it is not an installation Pagis \
                     has booted",
                    self.home.display()
                )
            }
            Err(error) => Err(error).with_context(|| format!("read {}", marker.display())),
        }
    }

    /// Hold the daemon's instance lock for the length of the backup, so
    /// nothing writes the files while they are copied.
    fn quiesce(&self) -> Result<std::fs::File> {
        let path = self.home.join("pagis.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        match lock.try_lock() {
            Ok(()) => Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => bail!(
                "pagis is running against {} — stop the daemon first; a copy taken while it \
                 writes is not a backup",
                self.home.display()
            ),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(error).context("take the pagis instance lock")
            }
        }
    }
}

/// The `[database] url` of one configuration file, read without
/// writing a default file beside it.
///
/// The `PAGIS_*` overrides apply, because the daemon reads them too: a
/// container deployment states its database in its `.env` file and the
/// file in the state directory names none.
fn configured_database(path: &Path) -> Result<Option<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut config: Config = toml::from_str(&text)
        .with_context(|| format!("read the settings in {}", path.display()))?;
    config.apply_environment(&|key| std::env::var(key).ok())?;
    Ok(config.database.url().map(str::to_string))
}

fn pg_dump(program: &Path, url: &str, out: &Path) -> Result<()> {
    let file = new_file(out, 0o600).with_context(|| format!("create {}", out.display()))?;
    // The custom format, because `pg_restore` reads it selectively and
    // compresses it. No owner and no privileges: the restored host runs
    // its own roles.
    run(
        std::process::Command::new(program)
            .args(["--format=custom", "--no-owner", "--no-privileges", url])
            .stdout(file),
        program,
    )
}

fn pg_restore(program: &Path, url: &str, dump: &Path) -> Result<()> {
    let file = std::fs::File::open(dump).with_context(|| format!("read {}", dump.display()))?;
    // The dump arrives on standard input rather than as a path, so the
    // program that reads it needs no access to this filesystem. That is
    // what lets the client run in the database's own container, where
    // its major version matches the server's.
    run(
        std::process::Command::new(program)
            .args([
                "--no-owner",
                "--no-privileges",
                "--exit-on-error",
                "--dbname",
            ])
            .arg(url)
            .stdin(file),
        program,
    )
}

fn run(command: &mut std::process::Command, program: &Path) -> Result<()> {
    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "{} is not on the PATH; the server image carries it and a backup taken \
                 elsewhere needs a PostgreSQL client",
                program.display()
            )
        } else {
            anyhow::Error::from(error)
        }
    })?;
    if !status.success() {
        bail!("{} failed with {status}", program.display());
    }
    Ok(())
}

/// Make `path` and require that nothing is in it. A restore onto a
/// directory that already holds an installation would mix two
/// installations into one, and a backup into a used directory would mix
/// two archives.
fn empty_directory(path: &Path) -> Result<()> {
    private_directory(path)?;
    if std::fs::read_dir(path)?.next().is_some() {
        bail!("{} is not empty", path.display());
    }
    Ok(())
}

/// Make `path`, and each missing directory above it, with mode 700.
fn private_directory(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("create {}", path.display()))
}

/// Make a new file with `mode` from its open. It never replaces a file.
fn new_file(path: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

/// Copy the content of one file. The copy keeps the owner's bits of the
/// source, so an executable stays one, and gets no bits for a group or
/// other users, whatever mode the source has.
fn copy_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut reader = std::fs::File::open(source)?;
    let mode = reader.metadata()?.permissions().mode() & 0o700;
    std::io::copy(&mut reader, &mut new_file(target, mode)?)?;
    Ok(())
}

/// Copy a directory tree, skipping the named entries of its root.
///
/// A symbolic link stops the copy: a backup follows nothing out of the
/// state directory, and a restore writes nothing outside the one it is
/// given.
fn copy_tree(from: &Path, to: &Path, excluded: &[&str]) -> Result<()> {
    private_directory(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if excluded.iter().any(|skip| *skip == name) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!(
                "{} is a symbolic link, which a backup does not follow",
                source.display()
            );
        }
        if kind.is_dir() {
            copy_tree(&source, &target, &[])?;
        } else {
            copy_file(&source, &target)
                .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation(home: &Path) -> Installation {
        std::fs::create_dir_all(home.join("logs")).unwrap();
        std::fs::write(home.join("logs/pagis.log"), "noise").unwrap();
        std::fs::write(home.join("runtime-release"), "0.1.0\n").unwrap();
        std::fs::write(home.join("secrets.enc"), "sealed").unwrap();
        std::fs::write(home.join(crate::CLIENT_CREDENTIAL_FILE), "a".repeat(64)).unwrap();
        std::fs::create_dir_all(home.join("memory/w1")).unwrap();
        std::fs::write(home.join("memory/w1/note.md"), "remembered").unwrap();
        Installation {
            home: home.to_path_buf(),
            database: None,
            tools: Tools::default(),
        }
    }

    #[test]
    fn an_archive_carries_the_state_and_leaves_the_logs_behind() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");

        let manifest = installation(&home).back_up(&out).unwrap();

        assert_eq!(manifest.schema, SCHEMA);
        assert_eq!(manifest.release, "0.1.0");
        assert_eq!(manifest.database, DatabaseKind::Sqlite);
        assert_eq!(
            std::fs::read_to_string(out.join("state/memory/w1/note.md")).unwrap(),
            "remembered"
        );
        assert!(out.join("state/secrets.enc").exists());
        assert!(!out.join("state/logs").exists());
        assert!(!out.join(DATABASE_DUMP).exists());
    }

    /// The Key File a local installation generated is the Installation
    /// Key, and a backup never puts the key beside the file it seals.
    #[test]
    fn an_archive_carries_no_generated_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");
        let installation = installation(&home);
        std::fs::write(
            home.join(crate::secrets::GENERATED_KEY_FILE),
            "a".repeat(64),
        )
        .unwrap();

        installation.back_up(&out).unwrap();

        assert!(out.join("state/secrets.enc").exists());
        assert!(
            !out.join("state")
                .join(crate::secrets::GENERATED_KEY_FILE)
                .exists(),
            "the archive carries the Installation Key"
        );
    }

    /// The Client Credential is a live way into a local installation: the
    /// file alone trades for a Session of the administrator. An
    /// archive is copied, shipped and kept, so it carries none, and a
    /// restored directory writes a fresh one at its next boot.
    #[test]
    fn an_archive_carries_no_client_credential() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");

        installation(&home).back_up(&out).unwrap();

        assert!(home.join(crate::CLIENT_CREDENTIAL_FILE).exists());
        assert!(
            !out.join("state")
                .join(crate::CLIENT_CREDENTIAL_FILE)
                .exists(),
            "the archive carries the Client Credential"
        );

        // And a restore leaves none behind either.
        let fresh = dir.path().join("fresh");
        Installation {
            home: fresh.clone(),
            database: None,
            tools: Tools::default(),
        }
        .restore(&out)
        .unwrap();
        assert!(!fresh.join(crate::CLIENT_CREDENTIAL_FILE).exists());
    }

    #[test]
    fn a_restore_puts_the_state_back_into_a_fresh_directory() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");
        installation(&home).back_up(&out).unwrap();
        let fresh = dir.path().join("fresh");

        let manifest = Installation {
            home: fresh.clone(),
            database: None,
            tools: Tools::default(),
        }
        .restore(&out)
        .unwrap();

        assert_eq!(manifest.release, "0.1.0");
        assert_eq!(
            std::fs::read_to_string(fresh.join("memory/w1/note.md")).unwrap(),
            "remembered"
        );
    }

    /// A running daemon holds the instance lock for its whole life, so
    /// the backup that cannot take it says the daemon is running rather
    /// than writing an archive of files in motion.
    #[test]
    fn a_running_daemon_stops_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let installation = installation(&home);
        let daemon = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(home.join("pagis.lock"))
            .unwrap();
        daemon.try_lock().unwrap();

        let error = installation
            .back_up(&dir.path().join("archive"))
            .expect_err("a live daemon stops the backup");

        assert!(error.to_string().contains("stop the daemon first"));
    }

    #[test]
    fn a_restore_refuses_a_directory_that_already_holds_an_installation() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");
        installation(&home).back_up(&out).unwrap();

        let error = Installation {
            home: home.clone(),
            database: None,
            tools: Tools::default(),
        }
        .restore(&out)
        .expect_err("a used state directory stops the restore");

        assert!(error.to_string().contains("is not empty"));
    }

    /// The two halves belong together: a Postgres archive restored onto
    /// an installation that names no database would come back with the
    /// files of one installation and the records of none.
    #[test]
    fn an_archive_of_the_other_backend_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let out = dir.path().join("archive");
        installation(&home).back_up(&out).unwrap();

        let error = Installation {
            home: dir.path().join("fresh"),
            database: Some("postgres://pagis@db/pagis".to_string()),
            tools: Tools::default(),
        }
        .restore(&out)
        .expect_err("the backends have to match");

        assert!(error.to_string().contains("restore it onto SQLite"));
    }
}
