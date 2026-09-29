//! The development checks, the gate, and the release build.
//! `cargo xtask dev` runs the gate steps that the changed paths select.
//! `cargo xtask full` runs every gate step, and `cargo xtask step
//! <name>...` runs the named gate steps: each job of the CI workflow runs
//! one group of them, so the local gate and CI run the same commands.
//! `cargo xtask advisories` runs the advisory checks ([`advisories`]).
//! `cargo xtask release` builds and publishes a release; see [`release`].
//!
//! Steps no-op gracefully until the parts they check exist: the Docker
//! tests skip while Docker is unreachable, the UI steps skip until
//! `ui/package.json` declares the script the step runs, and the contract
//! check with the pinned `gog` ([`gog`]) skips on a host that has no pin.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

pub mod advisories;
pub mod desktop;
pub mod desktop_linux;
pub mod emergency;
pub mod gog;
pub mod image;
pub mod pins;
pub mod release;
pub mod secrets;
pub mod server_image;
pub mod tools;
pub use desktop::{DesktopContext, DesktopPlatform, desktop_plan};
pub use image::{
    VERSION_LABEL, anonymous_pull_step, builder_step, check_pin, image_plan, labelled_version,
};
pub use release::{ReleaseContext, gog_asset_name, release_plan};
pub use server_image::{
    SERVER_IMAGE_REPOSITORY, server_image, server_image_plan, server_image_steps,
};

/// One command a step runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Variables the command runs with, on top of the environment.
    pub env: Vec<(String, String)>,
}

impl Cmd {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Cmd {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn in_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// A command that fails and prints `reason`: the plan of a step that
    /// cannot run, and that must not pass.
    pub fn refusal(reason: &str) -> Self {
        Cmd::new("sh", &["-c", "echo \"$1\" >&2; exit 1", "sh", reason])
    }

    fn display(&self) -> String {
        let mut s = String::new();
        for (key, value) in &self.env {
            s.push_str(&format!("{key}={value} "));
        }
        s.push_str(&self.program);
        for a in &self.args {
            s.push(' ');
            s.push_str(a);
        }
        s
    }
}

/// What a planned step does: run commands, or skip with a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Vec<Cmd>),
    Skip(String),
}

/// One gate step, planned against the current repository state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    pub action: Action,
}

/// The result of one executed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub name: &'static str,
    pub outcome: Outcome,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped(String),
}

/// One sequence of gate steps. The lanes of a plan run at the same
/// time; the steps of one lane run in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub name: &'static str,
    pub steps: Vec<Step>,
}

/// Plan the full gate for the workspace at `root` as three lanes that
/// run at the same time:
///
/// - `cargo`: every command that can invoke Cargo, in sequence. This
///   avoids competing Cargo build graphs in one target directory.
/// - `node`: shell, UI, and desktop checks.
/// - `secrets`: the gitleaks scan of the tracked files.
///
/// Where Docker is reachable the gate builds the Computer image from
/// `computer/` once, under the tag that the workspace pins, so the image
/// of the checked tree is what the Docker-real tests run in; the tests
/// build nothing themselves.
pub fn full_lanes(root: &Path, docker_available: bool) -> Vec<Lane> {
    // The gate keeps Cargo's default incremental setting, so a step
    // reuses the artifacts of the earlier builds.
    let cargo = |args: &[&str]| Action::Run(vec![Cmd::new("cargo", args).in_dir(root)]);
    // Compile the workspace test graph once. With Docker, include the
    // ignored Docker-real tests but exclude tests that need paid or live
    // external services, and the pinned `gog` tests, which the
    // `gog-contract` step runs.
    let tests = if docker_available {
        cargo(&[
            "nextest",
            "run",
            "--workspace",
            "--run-ignored",
            "all",
            "-E",
            "not test(/(^live_mail::|^live_migadu::|^release_evaluation::|^a_live_mailbox_logs_in_sends_to_itself|^pinned_gog::)/)",
        ])
    } else {
        cargo(&["nextest", "run", "--workspace"])
    };
    let cargo_lane = vec![
        Step {
            name: "fmt",
            action: Action::Run(vec![
                Cmd::new("cargo", &["fmt", "--all", "--", "--check"]).in_dir(root),
            ]),
        },
        Step {
            name: "clippy",
            action: cargo(&[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ]),
        },
        computer_image_step(root, docker_available),
        Step {
            name: "test",
            action: tests,
        },
        gog::contract_step(root, &target_dir(root)),
        Step {
            name: "emergency-drift",
            action: cargo(&[
                "run",
                "--package",
                "xtask",
                "--",
                "emergency-numbers",
                "--check",
            ]),
        },
        pins_step(root),
        Step {
            name: "contract-drift",
            action: npm_script_action(root, "ui", "api:check"),
        },
    ];
    let node = vec![
        Step {
            name: "pagis-apt",
            action: pagis_apt_action(root),
        },
        Step {
            name: "ui-deps",
            action: npm_deps_action(root, "ui"),
        },
        Step {
            name: "ui-typecheck",
            action: npm_script_action(root, "ui", "typecheck"),
        },
        Step {
            name: "ui-test",
            action: npm_script_action(root, "ui", "test"),
        },
        Step {
            name: "desktop-deps",
            action: npm_deps_action(root, "desktop"),
        },
        Step {
            name: "desktop-typecheck",
            action: npm_script_action(root, "desktop", "typecheck"),
        },
        Step {
            name: "desktop-test",
            action: npm_script_action(root, "desktop", "test"),
        },
    ];
    vec![
        Lane {
            name: "cargo",
            steps: cargo_lane,
        },
        Lane {
            name: "node",
            steps: node,
        },
        secrets_lane(root),
    ]
}

/// The steps of the full gate named in `names`, in the order of `names`.
/// An unknown name is an error that lists each step name of the gate.
pub fn named_steps(root: &Path, names: &[String], docker_available: bool) -> Result<Vec<Step>> {
    let steps: Vec<Step> = full_lanes(root, docker_available)
        .into_iter()
        .flat_map(|lane| lane.steps)
        .collect();
    names
        .iter()
        .map(|name| {
            steps
                .iter()
                .find(|step| step.name == name)
                .cloned()
                .ok_or_else(|| {
                    let known: Vec<&str> = steps.iter().map(|step| step.name).collect();
                    anyhow::anyhow!(
                        "the gate has no step `{name}`; its steps are: {}",
                        known.join(", ")
                    )
                })
        })
        .collect()
}

/// Build the Computer Image from `computer/` under the tag that the
/// workspace pins, which is the image that the Docker tests run. A host
/// without Docker skips the step.
///
/// The local layer cache makes an unchanged `computer/` build in seconds.
/// `--cache-from` is left out on purpose: it bypasses the local cache of
/// the intermediate stages, which hold the compiles. A CI runner has no
/// local cache, so there the build reads and writes the layer cache that
/// [`ImageCache`] names.
fn computer_image_step(root: &Path, docker_available: bool) -> Step {
    let action = if docker_available {
        Action::Run(vec![computer_image_build(
            root,
            ImageCache::from_env().as_ref(),
        )])
    } else {
        Action::Skip("Docker unavailable".into())
    };
    Step {
        name: "computer-image",
        action,
    }
}

/// The remote layer cache of the Computer Image build, from
/// `PAGIS_IMAGE_CACHE_FROM` and `PAGIS_IMAGE_CACHE_TO`: buildx cache
/// specifications such as `type=gha,scope=computer-image`. An empty
/// value is no value. CI reads the cache on every run and writes it on
/// main only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCache {
    pub from: String,
    pub to: Option<String>,
}

impl ImageCache {
    pub fn from_env() -> Option<Self> {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Some(Self {
            from: read("PAGIS_IMAGE_CACHE_FROM")?,
            to: read("PAGIS_IMAGE_CACHE_TO"),
        })
    }
}

/// The build of the Computer Image: `docker build` on the local layer
/// cache, or `docker buildx build` that loads the image into Docker and
/// reads and writes `cache`.
pub fn computer_image_build(root: &Path, cache: Option<&ImageCache>) -> Cmd {
    let Some(cache) = cache else {
        return Cmd::new(
            "docker",
            &["build", "-t", pagis_versions::COMPUTER_IMAGE, "computer"],
        )
        .in_dir(root);
    };
    let mut args = vec![
        "buildx",
        "build",
        "--load",
        "-t",
        pagis_versions::COMPUTER_IMAGE,
        "--cache-from",
        cache.from.as_str(),
    ];
    if let Some(to) = &cache.to {
        args.extend(["--cache-to", to.as_str()]);
    }
    args.push("computer");
    Cmd::new("docker", &args).in_dir(root)
}

/// The check of the pin rules of the third-party code that the release
/// workflows and the image builds run ([`pins`]).
fn pins_step(root: &Path) -> Step {
    Step {
        name: "pins",
        action: Action::Run(vec![
            Cmd::new(
                "cargo",
                &["run", "--package", "xtask", "--", "pins", "--check"],
            )
            .in_dir(root),
        ]),
    }
}

/// The lane of the secret scan. It runs no Cargo, so it runs beside the
/// other lanes.
fn secrets_lane(root: &Path) -> Lane {
    Lane {
        name: "secrets",
        steps: vec![secrets::tree_secret_scan_step(root, &target_dir(root))],
    }
}

#[derive(Debug)]
struct WorkspacePackage {
    name: String,
    path: String,
    dependencies: Vec<String>,
}

/// Plan the smallest stored check set that covers `changed_paths`.
/// Rust changes test the changed packages and all workspace packages
/// that depend on them. Shared build configuration and unknown paths
/// use the full gate. Every change runs the secret scan.
pub fn dev_lanes(
    root: &Path,
    changed_paths: &[String],
    docker_available: bool,
) -> Result<Vec<Lane>> {
    let full_trigger = changed_paths.iter().any(|path| {
        matches!(
            path.as_str(),
            "Cargo.toml" | "Cargo.lock" | "rust-toolchain.toml" | ".config/nextest.toml"
        ) || path.starts_with(".cargo/")
            || path.starts_with("xtask/")
            || (!path.starts_with("crates/")
                && !path.starts_with("ui/")
                && !path.starts_with("desktop/")
                && !path.starts_with("computer/")
                && !path.starts_with("docs/")
                && !path.ends_with(".md"))
    });
    if full_trigger {
        return Ok(full_lanes(root, docker_available));
    }

    let rust_paths: Vec<_> = changed_paths
        .iter()
        .filter(|path| path.starts_with("crates/"))
        .collect();
    let ui_changed = changed_paths.iter().any(|path| path.starts_with("ui/"));
    let desktop_changed = changed_paths
        .iter()
        .any(|path| path.starts_with("desktop/"));
    let computer_changed = changed_paths
        .iter()
        .any(|path| path.starts_with("computer/"));

    let mut cargo_steps = Vec::new();
    if !rust_paths.is_empty() || computer_changed {
        let packages = workspace_packages(root)?;
        let mut affected: Vec<String> = packages
            .iter()
            .filter(|package| {
                rust_paths
                    .iter()
                    .any(|path| path.starts_with(&format!("{}/", package.path)))
            })
            .map(|package| package.name.clone())
            .collect();
        if computer_changed
            && packages
                .iter()
                .any(|package| package.name == "pagis-computer")
        {
            affected.push("pagis-computer".into());
        }
        if affected.is_empty() {
            return Ok(full_lanes(root, docker_available));
        }
        loop {
            let before = affected.len();
            for package in &packages {
                if !affected.contains(&package.name)
                    && package
                        .dependencies
                        .iter()
                        .any(|dependency| affected.contains(dependency))
                {
                    affected.push(package.name.clone());
                }
            }
            if affected.len() == before {
                break;
            }
        }
        affected.sort();
        affected.dedup();

        cargo_steps.push(Step {
            name: "fmt",
            action: Action::Run(vec![
                Cmd::new("cargo", &["fmt", "--all", "--", "--check"]).in_dir(root),
            ]),
        });
        let package_args: Vec<String> = affected
            .iter()
            .flat_map(|package| ["-p".to_string(), package.clone()])
            .collect();
        let mut clippy_args = vec!["clippy".into(), "--all-targets".into()];
        clippy_args.extend(package_args.clone());
        clippy_args.extend(["--".into(), "-D".into(), "warnings".into()]);
        cargo_steps.push(Step {
            name: "clippy",
            action: Action::Run(vec![Cmd {
                program: "cargo".into(),
                args: clippy_args,
                cwd: Some(root.to_path_buf()),
                env: Vec::new(),
            }]),
        });
        let docker_tests = docker_available
            && (computer_changed
                || affected.iter().any(|package| {
                    matches!(
                        package.as_str(),
                        "pagis-computer" | "pagis-software" | "pagis-mail"
                    )
                }));
        if docker_tests {
            cargo_steps.push(computer_image_step(root, docker_available));
        }
        let mut test_args = vec!["nextest".into(), "run".into()];
        test_args.extend(package_args);
        if docker_tests {
            test_args.extend([
                "--run-ignored".into(),
                "all".into(),
                "-E".into(),
                "not test(/(^live_mail::|^live_migadu::|^release_evaluation::|^a_live_mailbox_logs_in_sends_to_itself|^pinned_gog::)/)".into(),
            ]);
        }
        cargo_steps.push(Step {
            name: "test",
            action: Action::Run(vec![Cmd {
                program: "cargo".into(),
                args: test_args,
                cwd: Some(root.to_path_buf()),
                env: Vec::new(),
            }]),
        });
        if affected.iter().any(|package| package == "pagis-google") {
            cargo_steps.push(gog::contract_step(root, &target_dir(root)));
        }
        // Each other file of the pin rules selects the full gate.
        if changed_paths
            .iter()
            .any(|path| pins::DOCKERFILES.contains(&path.as_str()))
        {
            cargo_steps.push(pins_step(root));
        }
        if affected.iter().any(|package| package == "pagis-server") {
            cargo_steps.push(Step {
                name: "contract-drift",
                action: npm_script_action(root, "ui", "api:check"),
            });
        }
    }

    let mut node_steps = Vec::new();
    if computer_changed {
        node_steps.push(Step {
            name: "pagis-apt",
            action: pagis_apt_action(root),
        });
    }
    if ui_changed {
        node_steps.extend([
            Step {
                name: "ui-deps",
                action: npm_deps_action(root, "ui"),
            },
            Step {
                name: "ui-typecheck",
                action: npm_script_action(root, "ui", "typecheck"),
            },
            Step {
                name: "ui-test",
                action: npm_script_action(root, "ui", "test"),
            },
        ]);
    }
    if desktop_changed {
        node_steps.extend([
            Step {
                name: "desktop-deps",
                action: npm_deps_action(root, "desktop"),
            },
            Step {
                name: "desktop-typecheck",
                action: npm_script_action(root, "desktop", "typecheck"),
            },
            Step {
                name: "desktop-test",
                action: npm_script_action(root, "desktop", "test"),
            },
        ]);
    }

    let mut lanes = Vec::new();
    if !cargo_steps.is_empty() {
        lanes.push(Lane {
            name: "cargo",
            steps: cargo_steps,
        });
    }
    if !node_steps.is_empty() {
        lanes.push(Lane {
            name: "node",
            steps: node_steps,
        });
    }
    if lanes.is_empty() {
        lanes.push(Lane {
            name: "docs",
            steps: vec![Step {
                name: "diff-check",
                action: Action::Run(vec![Cmd::new("git", &["diff", "--check"]).in_dir(root)]),
            }],
        });
    }
    if !changed_paths.is_empty() {
        lanes.push(secrets_lane(root));
    }
    Ok(lanes)
}

fn workspace_packages(root: &Path) -> Result<Vec<WorkspacePackage>> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("cannot read Cargo workspace metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let members: std::collections::HashSet<&str> = metadata["workspace_members"]
        .as_array()
        .context("cargo metadata omitted workspace_members")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let package_rows = metadata["packages"]
        .as_array()
        .context("cargo metadata omitted packages")?;
    let names_by_id: std::collections::HashMap<&str, &str> = package_rows
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package["name"].as_str()?)))
        .collect();
    let dependencies_by_id: std::collections::HashMap<&str, Vec<String>> =
        metadata["resolve"]["nodes"]
            .as_array()
            .context("cargo metadata omitted the resolved dependency graph")?
            .iter()
            .filter_map(|node| {
                let id = node["id"].as_str()?;
                let dependencies = node["deps"]
                    .as_array()?
                    .iter()
                    .filter_map(|dependency| names_by_id.get(dependency["pkg"].as_str()?))
                    .map(|name| (*name).to_string())
                    .collect();
                Some((id, dependencies))
            })
            .collect();

    package_rows
        .iter()
        .filter(|package| {
            package["id"]
                .as_str()
                .is_some_and(|id| members.contains(id))
        })
        .map(|package| {
            let id = package["id"].as_str().context("package has no id")?;
            let manifest = Path::new(
                package["manifest_path"]
                    .as_str()
                    .context("package has no manifest_path")?,
            );
            let package_root = manifest
                .parent()
                .context("package manifest has no parent")?;
            Ok(WorkspacePackage {
                name: package["name"]
                    .as_str()
                    .context("package has no name")?
                    .to_string(),
                path: package_root
                    .strip_prefix(&canonical_root)
                    .context("workspace package is outside the workspace root")?
                    .to_string_lossy()
                    .into_owned(),
                dependencies: dependencies_by_id.get(id).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

/// The Cargo target directory of the workspace at `root`: the one that
/// `CARGO_TARGET_DIR` names, or `target/` of the workspace.
pub fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

/// Run the `pagis-apt` validator tests. They are plain bash and
/// need no Docker.
fn pagis_apt_action(root: &Path) -> Action {
    const SCRIPT: &str = "computer/tests/pagis-apt-validate.sh";
    if !root.join(SCRIPT).exists() {
        return Action::Skip("computer/tests/pagis-apt-validate.sh is not present".into());
    }
    Action::Run(vec![Cmd::new("bash", &[SCRIPT]).in_dir(root)])
}

/// Install a package's npm dependencies when they are missing, so the
/// npm steps work on a fresh checkout.
fn npm_deps_action(root: &Path, package: &str) -> Action {
    let dir = root.join(package);
    if !dir.join("package.json").exists() {
        return Action::Skip(format!("{package}/ is not present yet"));
    }
    if dir.join("node_modules").exists() {
        return Action::Skip("node_modules present".into());
    }
    Action::Run(vec![Cmd::new("npm", &["ci"]).in_dir(dir)])
}

/// Run an npm script in a package when its `package.json` declares it.
fn npm_script_action(root: &Path, package: &str, script: &str) -> Action {
    let dir = root.join(package);
    let package_json = dir.join("package.json");
    if !package_json.exists() {
        return Action::Skip(format!("{package}/ is not present yet"));
    }
    let declared = std::fs::read_to_string(&package_json)
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .is_some_and(|pkg| pkg["scripts"][script].is_string());
    if declared {
        Action::Run(vec![Cmd::new("npm", &["run", script]).in_dir(dir)])
    } else {
        Action::Skip(format!("{package}/package.json has no \"{script}\" script"))
    }
}

/// The Docker label a test runtime puts on every container, volume and
/// Tenant Network it creates. It is the same key as
/// `pagis_computer::TEST_LABEL`. Its value is `<pid>-<nonce>`: the
/// process id of the test that owns the object, and a nonce that makes
/// one test's objects separate from another's in the same process.
pub const TEST_LABEL: &str = "org.pagis.test";

/// The test-labelled Docker objects to remove, by name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TestObjects {
    pub containers: Vec<String>,
    pub networks: Vec<String>,
    pub volumes: Vec<String>,
}

/// The objects in `listed` whose test process no longer runs. Each line
/// is one object: its name, a tab, and its [`TEST_LABEL`] value. A value
/// that names no process id names no owner that runs, so that object is
/// orphaned too.
pub fn orphaned_test_objects(listed: &str, running: impl Fn(u32) -> bool) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, owner)| {
            let pid = owner.split('-').next().and_then(|pid| pid.parse().ok());
            !pid.is_some_and(&running)
        })
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The commands that remove `objects`. The containers go first, because
/// Docker does not remove a network while a container is attached to it.
pub fn test_object_sweep(objects: &TestObjects) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    let mut push = |prefix: &[&str], names: &[String]| {
        if names.is_empty() {
            return;
        }
        let mut args: Vec<&str> = prefix.to_vec();
        args.extend(names.iter().map(String::as_str));
        cmds.push(Cmd::new("docker", &args));
    };
    push(&["rm", "-f"], &objects.containers);
    push(&["network", "rm"], &objects.networks);
    push(&["volume", "rm", "-f"], &objects.volumes);
    cmds
}

/// The test-labelled objects in the local Docker whose test process no
/// longer runs. An object without [`TEST_LABEL`] is never listed, so the
/// Computers and the networks of a Pagis installation on the same Docker
/// are never touched.
pub fn local_orphaned_test_objects() -> TestObjects {
    let filter = format!("label={TEST_LABEL}");
    // `docker ps` names its column `.Names`; the network and volume
    // listings name it `.Name`.
    let containers = format!("{{{{.Names}}}}\t{{{{.Label \"{TEST_LABEL}\"}}}}");
    let others = format!("{{{{.Name}}}}\t{{{{.Label \"{TEST_LABEL}\"}}}}");
    let list = |args: &[&str]| -> Vec<String> {
        match Command::new("docker").args(args).output() {
            Ok(output) if output.status.success() => {
                orphaned_test_objects(&String::from_utf8_lossy(&output.stdout), process_running)
            }
            _ => Vec::new(),
        }
    };
    TestObjects {
        containers: list(&["ps", "-a", "--filter", &filter, "--format", &containers]),
        networks: list(&["network", "ls", "--filter", &filter, "--format", &others]),
        volumes: list(&["volume", "ls", "--filter", &filter, "--format", &others]),
    }
}

/// Whether a process with this id runs on this machine.
fn process_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Check whether the Docker daemon is reachable.
pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run every lane on its own thread and report each result in plan
/// order. A failed step does not stop the later steps of its lane.
pub fn execute_lanes(lanes: &[Lane]) -> Vec<StepResult> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = lanes
            .iter()
            .map(|lane| scope.spawn(move || execute(&lane.steps)))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a lane thread panicked"))
            .collect()
    })
}

/// Run every step in order and report each result. A failed step does
/// not stop the later steps.
pub fn execute(steps: &[Step]) -> Vec<StepResult> {
    steps
        .iter()
        .map(|step| {
            let start = Instant::now();
            let outcome = match &step.action {
                Action::Skip(reason) => Outcome::Skipped(reason.clone()),
                Action::Run(cmds) => run_cmds(step.name, cmds),
            };
            StepResult {
                name: step.name,
                outcome,
                duration: start.elapsed(),
            }
        })
        .collect()
}

/// Run the steps of a publish in order, and stop at the first failed
/// step: a later step, such as the push of an image, must not run after
/// an earlier one, such as the secret scan of that image, failed. Each
/// step after the failure is reported as skipped.
pub fn execute_until_failure(steps: &[Step]) -> Vec<StepResult> {
    let mut failed = None;
    steps
        .iter()
        .map(|step| {
            if let Some(name) = failed {
                return StepResult {
                    name: step.name,
                    outcome: Outcome::Skipped(format!("{name} failed")),
                    duration: Duration::ZERO,
                };
            }
            let result = execute(std::slice::from_ref(step)).remove(0);
            if result.outcome == Outcome::Failed {
                failed = Some(step.name);
            }
            result
        })
        .collect()
}

/// Run `cmds` in order under the step name `name`; stop at the first
/// failure. Child output streams as it is produced.
pub fn run_cmds(name: &str, cmds: &[Cmd]) -> Outcome {
    for cmd in cmds {
        eprintln!("== {name}: {}", cmd.display());
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        command.envs(cmd.env.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &cmd.cwd {
            command.current_dir(cwd);
        }
        match command.status() {
            Ok(status) => {
                if !status.success() {
                    eprintln!("== {name}: {status}");
                    return Outcome::Failed;
                }
            }
            Err(err) => {
                eprintln!("== {name}: cannot start {}: {err}", cmd.program);
                return Outcome::Failed;
            }
        }
    }
    Outcome::Passed
}

/// True when no step failed. Skipped steps do not block the gate.
pub fn all_green(results: &[StepResult]) -> bool {
    results.iter().all(|r| r.outcome != Outcome::Failed)
}

/// The steps a command will run, without running them (`--dry-run`).
pub fn plan_summary(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        match &step.action {
            Action::Run(cmds) => {
                for cmd in cmds {
                    let _ = writeln!(out, "run     {}: {}", step.name, cmd.display());
                }
            }
            Action::Skip(reason) => {
                let _ = writeln!(out, "skip    {} ({reason})", step.name);
            }
        }
    }
    out
}

/// The per-step terminal summary.
pub fn summary(results: &[StepResult]) -> String {
    let mut out = String::new();
    for r in results {
        let line = match &r.outcome {
            Outcome::Passed => format!("pass    {} ({})", r.name, human_duration(r.duration)),
            Outcome::Failed => format!("FAIL    {} ({})", r.name, human_duration(r.duration)),
            Outcome::Skipped(reason) => format!("skip    {} ({reason})", r.name),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn human_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs >= 10 {
        format!("{secs}s")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}
