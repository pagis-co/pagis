//! Materialization of one package version inside an agent's Computer:
//! upload, `setup` once, then a read-only tree.
//!
//! A materialized copy lives at
//! `/data/agent/.pagis/software/<package>/<version>/`. The daemon
//! writes into a temporary sibling and renames it into place, so a
//! reader never sees a half-built tree. One asynchronous lock per
//! (agent, package, version) makes a second caller wait for the first
//! one instead of building the same tree twice.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_computer::{ComputerManagers, ShellCommand};
use pagis_core::AgentId;

use crate::manifest::PackageVersion;

/// Where materialized copies live in the agent's home.
pub const SOFTWARE_ROOT: &str = "/data/agent/.pagis/software";
/// How long `setup` gets. It installs dependencies over the network,
/// so it takes the longest deadline a shell command can have.
const SETUP_TIMEOUT: Duration = Duration::from_secs(600);
/// How long the small directory commands get.
const HOUSEKEEPING_TIMEOUT: Duration = Duration::from_secs(60);

/// Where one published version comes from. The store fills it;
/// tests use an in-memory one.
#[async_trait]
pub trait VersionSource: Send + Sync {
    /// The manifest and the argument schemas of one version.
    async fn version(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<PackageVersion, String>;

    /// The tar of one version, with entries relative to the package
    /// root.
    async fn tar(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, String>;

    /// The Content-Security-Policy one Widget declares (ADR-0016).
    ///
    /// The sandbox proxy is the one route that answers with no Session,
    /// because a sandboxed frame sends no cookie, and what it reads is
    /// the author's declaration and never a person's record. The
    /// Workspace therefore comes from the sandbox URL itself: a Widget of
    /// one tenant gets that tenant's declared policy, and no caller can
    /// read another tenant's installed package triples through this route.
    async fn sandbox_csp(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
        widget: &str,
    ) -> Option<crate::WidgetCsp>;

    /// The bytes of one file of one version, by a path relative to
    /// the package root. A Widget page reads through it.
    async fn file(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        package: &str,
        version: &str,
        path: &str,
    ) -> Result<Vec<u8>, String>;
}

/// The directory one materialized version lives in.
pub fn package_root(package: &str, version: &str) -> String {
    format!("{SOFTWARE_ROOT}/{package}/{version}")
}

type Key = (AgentId, String, String);

pub struct Materializer {
    /// Every tenant's Computer manager. A materialization is one
    /// tenant's work, so the manager is resolved per call from the
    /// Workspace the caller names and never held for one of them.
    computers: Arc<ComputerManagers>,
    source: Arc<dyn VersionSource>,
    locks: Mutex<HashMap<Key, Arc<tokio::sync::Mutex<()>>>>,
    ready: Mutex<HashSet<Key>>,
    /// Makes each temporary directory name its own.
    counter: AtomicU64,
}

impl Materializer {
    pub fn new(computers: Arc<ComputerManagers>, source: Arc<dyn VersionSource>) -> Self {
        Self {
            computers,
            source,
            locks: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashSet::new()),
            counter: AtomicU64::new(0),
        }
    }

    /// The root of the materialized version, built if this Computer
    /// does not hold it yet. A failed `setup` discards the tree, so the
    /// next call tries again.
    pub async fn ensure(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &AgentId,
        package: &str,
        version: &str,
        setup: Option<&str>,
    ) -> Result<String, String> {
        let key = (agent_id.clone(), package.to_string(), version.to_string());
        let root = package_root(package, version);
        if self.is_ready(&key) {
            return Ok(root);
        }
        let lock = self.lock_for(&key);
        let _guard = lock.lock().await;
        if self.is_ready(&key) {
            return Ok(root);
        }
        // A daemon restart forgets what a live container still holds.
        if self
            .exit_code(workspace_id, agent_id, &format!("test -d {}", quote(&root)))
            .await?
            == 0
        {
            self.mark_ready(key);
            return Ok(root);
        }

        let tar = self.source.tar(workspace_id, package, version).await?;
        let serial = self.counter.fetch_add(1, Ordering::Relaxed);
        let temp = format!("{SOFTWARE_ROOT}/{package}/.tmp-{version}-{serial}");
        self.run(
            workspace_id,
            agent_id,
            &format!("rm -rf {temp} && mkdir -p {temp}", temp = quote(&temp)),
            None,
            HOUSEKEEPING_TIMEOUT,
        )
        .await?;

        match self
            .build(workspace_id, agent_id, &temp, &root, tar, setup)
            .await
        {
            Ok(()) => {
                self.mark_ready(key);
                Ok(root)
            }
            Err(problem) => {
                // The tree is still writable until `chmod`, so the
                // discard always succeeds.
                let _ = self
                    .run(
                        workspace_id,
                        agent_id,
                        &format!("rm -rf {}", quote(&temp)),
                        None,
                        HOUSEKEEPING_TIMEOUT,
                    )
                    .await;
                Err(problem)
            }
        }
    }

    /// Upload, `setup`, freeze, rename. Every step happens in the
    /// temporary directory, so the named root appears complete or not
    /// at all.
    async fn build(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &AgentId,
        temp: &str,
        root: &str,
        tar: Vec<u8>,
        setup: Option<&str>,
    ) -> Result<(), String> {
        let computer = self.computers.get(workspace_id);
        computer
            .upload_archive(agent_id, temp, tar)
            .await
            .map_err(|error| format!("cannot upload the package: {error}"))?;

        if let Some(setup) = setup {
            let outcome = computer
                .shell(
                    agent_id,
                    ShellCommand {
                        command: setup.to_string(),
                        timeout: SETUP_TIMEOUT,
                        cwd: Some(temp.to_string()),
                        stdin: None,
                        output_cap: None,
                    },
                )
                .await
                .map_err(|error| format!("setup failed: {error}"))?;
            if outcome.exit_code != 0 {
                return Err(format!(
                    "setup failed with exit code: {}\nstderr:\n{}",
                    outcome.exit_code, outcome.stderr
                ));
            }
        }

        self.run(
            workspace_id,
            agent_id,
            &format!(
                "chmod -R a-w {temp} && mv {temp} {root}",
                temp = quote(temp),
                root = quote(root)
            ),
            None,
            HOUSEKEEPING_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// One housekeeping command that must succeed.
    async fn run(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &AgentId,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(), String> {
        let outcome = self
            .computers
            .get(workspace_id)
            .shell(
                agent_id,
                ShellCommand {
                    command: command.to_string(),
                    timeout,
                    cwd: None,
                    stdin,
                    output_cap: None,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        if outcome.exit_code == 0 {
            Ok(())
        } else {
            Err(format!(
                "{command} failed with exit code: {}\nstderr:\n{}",
                outcome.exit_code, outcome.stderr
            ))
        }
    }

    /// The exit code of one command that is allowed to fail.
    async fn exit_code(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &AgentId,
        command: &str,
    ) -> Result<i64, String> {
        self.computers
            .get(workspace_id)
            .shell(
                agent_id,
                ShellCommand {
                    command: command.to_string(),
                    timeout: HOUSEKEEPING_TIMEOUT,
                    cwd: None,
                    stdin: None,
                    output_cap: None,
                },
            )
            .await
            .map(|outcome| outcome.exit_code)
            .map_err(|error| error.to_string())
    }

    fn is_ready(&self, key: &Key) -> bool {
        self.ready.lock().expect("ready set").contains(key)
    }

    fn mark_ready(&self, key: Key) {
        self.ready.lock().expect("ready set").insert(key);
    }

    fn lock_for(&self, key: &Key) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.locks
                .lock()
                .expect("lock table")
                .entry(key.clone())
                .or_default(),
        )
    }
}

/// One word for `bash -c`. A single-quoted string ends only at the
/// next single quote, so an embedded one is spelled `'\''`.
pub fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}
