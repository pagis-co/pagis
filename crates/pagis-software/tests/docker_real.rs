//! Docker-real Software tests, behind `#[ignore]`: the gate
//! runs them where Docker is reachable
//! (`cargo nextest run --workspace --run-ignored only`) in the pinned
//! image its `computer-image` step builds from `computer/`.
//!
//! Every test here runs offline inside the Computer: the tools use
//! only what the image holds.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_computer::fake::FakeWorkspaces;
use pagis_computer::{
    BollardRuntime, ComputerManager, ComputerState, DockerDiscovery, IMAGE, ShellCommand,
    test_docker::TestDocker,
};
use pagis_core::{
    AgentId, Event, EventBus, EventId, EventScope, EventStream, NewEvent, StoreError, WorkspaceId,
    now_ms,
};
use pagis_software::manifest::PackageVersion;
use pagis_software::{Manifest, Materializer, SoftwareRunner, VersionSource, pack};

/// The agent's home inside the Computer.
const HOME: &str = "/data/agent";

/// The gate's `computer-image` step builds the pinned image from
/// `computer/` before these tests run; a test builds nothing, so the
/// build happens once and not once per test process.
fn require_image() {
    let present = Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .output()
        .expect("docker image inspect runs")
        .status
        .success();
    assert!(
        present,
        "image {IMAGE} is not built; run `docker build -t {IMAGE} computer` at the workspace root"
    );
}

/// A bus that keeps nothing: these tests assert against Docker, not
/// the audit trail.
struct SilentBus;

#[async_trait]
impl EventBus for SilentBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        Ok(Event {
            id: EventId::generate(),
            seq: 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// One version in memory, so a run needs no version store.
struct MemorySource {
    version: PackageVersion,
    tar: Vec<u8>,
}

#[async_trait]
impl VersionSource for MemorySource {
    async fn version(
        &self,
        _workspace_id: &WorkspaceId,
        _package: &str,
        _version: &str,
    ) -> Result<PackageVersion, String> {
        Ok(self.version.clone())
    }

    async fn tar(
        &self,
        _workspace_id: &WorkspaceId,
        _package: &str,
        _version: &str,
    ) -> Result<Vec<u8>, String> {
        Ok(self.tar.clone())
    }

    async fn sandbox_csp(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _package: &str,
        _version: &str,
        _widget: &str,
    ) -> Option<pagis_software::WidgetCsp> {
        None
    }

    async fn file(
        &self,
        _workspace_id: &WorkspaceId,
        _package: &str,
        _version: &str,
        path: &str,
    ) -> Result<Vec<u8>, String> {
        Err(format!("no such file {path}"))
    }
}

/// One awake Computer over the real runtime. The tempdir holds the
/// stored screenshots and must outlive the manager. The drop removes
/// every container, volume and Tenant Network the runtime created, also
/// when the test fails.
struct Live {
    /// Every tenant's manager. The Software half resolves the
    /// manager from the Workspace of the caller.
    managers: Arc<pagis_computer::ComputerManagers>,
    manager: Arc<ComputerManager>,
    agent_id: AgentId,
    workspace_id: WorkspaceId,
    _screens: tempfile::TempDir,
    _docker: TestDocker,
}

async fn live() -> Live {
    require_image();
    let docker = TestDocker::new();
    let runtime = Arc::new(BollardRuntime::new(
        Arc::new(DockerDiscovery::production(None)),
        pagis_computer::RuntimeOptions {
            limits: pagis_computer::ComputerLimits::default(),
            tokens_dir: std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("screend-tokens"),
            labels: docker.labels(),
        },
    ));
    let screens = tempfile::tempdir().expect("screens dir");
    let workspace_id = WorkspaceId::generate();
    let managers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: runtime as _,
        skills: Arc::new(pagis_core::NoSkills) as _,
        workspaces: Arc::new(FakeWorkspaces::with_timezone(&workspace_id, "UTC")) as _,
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()) as _,
        bus: Arc::new(SilentBus) as _,
        screens_dir: screens.path().to_path_buf(),
        idle_stop: Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        caps: pagis_computer::AwakeCaps::default(),
        cancel: tokio_util::sync::CancellationToken::new(),
        exit: None,
    });
    let manager = managers.get(&workspace_id);
    let agent_id = AgentId::generate();
    manager.wake(&agent_id).await.expect("wake");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while manager.state(&agent_id).await != ComputerState::Awake {
        assert!(tokio::time::Instant::now() < deadline, "never woke");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Live {
        managers,
        manager,
        agent_id,
        workspace_id,
        _screens: screens,
        _docker: docker,
    }
}

impl Live {
    /// One shell command in the agent's Computer, as the agent.
    async fn shell(&self, command: &str) -> pagis_computer::ExecOutcome {
        self.manager
            .shell(
                &self.agent_id,
                ShellCommand {
                    command: command.to_string(),
                    timeout: Duration::from_secs(120),
                    cwd: None,
                    stdin: None,
                    output_cap: None,
                },
            )
            .await
            .expect("the shell command runs")
    }

    /// The same, and it must succeed.
    async fn must(&self, command: &str) -> String {
        let outcome = self.shell(command).await;
        assert_eq!(outcome.exit_code, 0, "{command} failed: {}", outcome.stderr);
        outcome.stdout
    }
}

/// A tar of one package, with the ownership a published version
/// carries: the files came out of a Computer, where the agent owns
/// them.
fn package_tar(files: &[(&str, &str, u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    // A tar out of a Computer carries its directories, and the agent
    // owns them. Without them the extraction makes each one as root,
    // and the agent cannot freeze the tree.
    let mut directories: Vec<String> = files
        .iter()
        .filter_map(|(path, _, _)| path.rsplit_once('/').map(|(parent, _)| parent.to_string()))
        .collect();
    directories.sort();
    directories.dedup();
    for directory in &directories {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_uid(1000);
        header.set_gid(1000);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{directory}/"), &[][..])
            .expect("append");
    }
    for (path, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(*mode);
        header.set_uid(1000);
        header.set_gid(1000);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content.as_bytes())
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

fn files_of(tar: &[u8]) -> BTreeMap<String, String> {
    let mut archive = tar::Archive::new(tar);
    let mut files = BTreeMap::new();
    for entry in archive.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().expect("path").display().to_string();
        let mut text = String::new();
        // A package file of these tests is text.
        entry.read_to_string(&mut text).expect("text");
        files.insert(path, text);
    }
    files
}

/// The archive round trip between the daemon and a Computer.
///
/// An upload lands the package as the agent, not as root: the agent's
/// own shell must read what a materialized version holds.
///
/// A publish reads the working copy out of the Computer and drops the
/// dependencies: `.git`, `node_modules`, `.venv` and whatever
/// `.gitignore` names. The links a setup leaves in them stop nothing.
///
/// The archive endpoint writes a symbolic link and a second path of one
/// file in the package as link entries, and the pack refuses both by
/// path.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_archive_round_trip_keeps_the_agent_owner_and_refuses_links() {
    let live = live().await;

    let target = format!("{HOME}/.pagis/software/demo/v1");
    live.must(&format!("mkdir -p {target}")).await;
    live.manager
        .upload_archive(
            &live.agent_id,
            &target,
            package_tar(&[
                ("pagis-software.toml", "# a package\n", 0o644),
                ("bin/tool.py", "print(1)\n", 0o755),
            ]),
        )
        .await
        .expect("the upload succeeds");
    let owners = live
        .must(&format!(
            "stat -c '%U %a %n' {target}/pagis-software.toml {target}/bin/tool.py"
        ))
        .await;
    assert_eq!(
        owners,
        format!("agent 644 {target}/pagis-software.toml\nagent 755 {target}/bin/tool.py\n")
    );

    let root = format!("{HOME}/software/demo");
    live.must(&format!(
        "mkdir -p {root}/bin {root}/.git {root}/node_modules/left-pad {root}/.venv/bin \
         {root}/build && \
         printf 'print(1)\\n' > {root}/bin/tool.py && \
         printf 'build/\\n' > {root}/.gitignore && \
         printf 'x\\n' > {root}/.git/config && \
         printf 'x\\n' > {root}/node_modules/left-pad/index.js && \
         ln -s index.js {root}/node_modules/left-pad/main.js && \
         ln {root}/node_modules/left-pad/index.js {root}/node_modules/left-pad/copy.js && \
         printf 'x\\n' > {root}/.venv/pyvenv.cfg && \
         ln -s /usr/bin/python3 {root}/.venv/bin/python && \
         printf 'x\\n' > {root}/build/out.o && \
         ln -s /etc {root}/build/etc"
    ))
    .await;
    let downloaded = live
        .manager
        .download_archive(&live.agent_id, &root)
        .await
        .expect("the download succeeds");
    let version = pack(downloaded).expect("the pack");
    let files: Vec<String> = files_of(&version.tar).into_keys().collect();
    assert_eq!(files, [".gitignore", "bin/tool.py"]);

    let linked = format!("{HOME}/software/linked");
    live.must(&format!(
        "mkdir -p {linked}/bin && \
         printf 'print(1)\\n' > {linked}/bin/tool.py && \
         ln {linked}/bin/tool.py {linked}/bin/copy.py && \
         ln -s /etc/hostname {linked}/pagis-software.toml"
    ))
    .await;
    let downloaded = live
        .manager
        .download_archive(&live.agent_id, &linked)
        .await
        .expect("the download succeeds");
    let Err(refused) = pack(downloaded) else {
        panic!("the pack is not refused");
    };
    assert!(
        refused.contains("\"pagis-software.toml\" is a symbolic link"),
        "{refused}"
    );
    // The archive keeps the first path it meets as the file, so either
    // path can be the hard link.
    assert!(
        refused.contains("\"bin/copy.py\" is a hard link")
            || refused.contains("\"bin/tool.py\" is a hard link"),
        "{refused}"
    );
}

/// A `setup` runs once in the Computer, offline, and the tree it
/// leaves is read only.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_setup_runs_offline_and_leaves_a_read_only_tree() {
    let live = live().await;
    let manifest = "[package]\nname = \"demo\"\ndescription = \"A demo package.\"\nsetup = \
                    \"python3 -c 'open(\\\"ready.txt\\\", \\\"w\\\").write(\\\"ready\\\")'\"\n";
    let source = Arc::new(MemorySource {
        version: PackageVersion {
            manifest: Manifest::parse(manifest).expect("the manifest parses"),
            schemas: BTreeMap::new(),
            widget_schemas: BTreeMap::new(),
        },
        tar: package_tar(&[("pagis-software.toml", manifest, 0o644)]),
    });
    let materializer = Materializer::new(Arc::clone(&live.managers), Arc::clone(&source) as _);

    let root = materializer
        .ensure(
            &live.workspace_id,
            &live.agent_id,
            "demo",
            "v1",
            Some(
                &Manifest::parse(manifest)
                    .expect("the manifest parses")
                    .package
                    .setup
                    .expect("a setup"),
            ),
        )
        .await
        .expect("the version materializes");

    assert_eq!(root, format!("{HOME}/.pagis/software/demo/v1"));
    assert_eq!(live.must(&format!("cat {root}/ready.txt")).await, "ready");
    // The frozen tree refuses a write, so no run changes a version.
    let write = live.shell(&format!("touch {root}/late.txt")).await;
    assert_ne!(write.exit_code, 0, "the materialized tree is writable");
}

/// One real tool call through `uv run` and one through `node`: the
/// arguments arrive on stdin as JSON, and the result is what the tool
/// prints. Both run offline.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_python_tool_and_a_node_tool_answer_with_json() {
    let live = live().await;
    let manifest = r#"
[package]
name = "demo"
description = "Two tools, two runtimes."

[[tool]]
name = "add_python"
description = "Add one, with uv."
entry = "bin/add.py"
schema = "schemas/add.json"

[[tool]]
name = "add_node"
description = "Add one, with node."
entry = "bin/add.js"
schema = "schemas/add.json"
"#;
    // `--no-project` keeps uv out of the network: it runs the script
    // with the interpreter the image holds.
    let python = "#!/usr/bin/env -S uv run --no-project --script\nimport json, os, \
                  sys\nnumbers = json.load(sys.stdin)\nprint(json.dumps({\"sum\": \
                  numbers[\"left\"] + numbers[\"right\"], \"tool\": \
                  os.environ[\"PAGIS_TOOL\"]}))\n";
    let node = "#!/usr/bin/env node\nlet text = \"\";\nprocess.stdin.on(\"data\", chunk => text \
                += chunk);\nprocess.stdin.on(\"end\", () => {\n  const numbers = \
                JSON.parse(text);\n  console.log(JSON.stringify({ sum: numbers.left + \
                numbers.right, tool: process.env.PAGIS_TOOL }));\n});\n";
    let source = Arc::new(MemorySource {
        version: PackageVersion {
            manifest: Manifest::parse(manifest).expect("the manifest parses"),
            schemas: BTreeMap::new(),
            widget_schemas: BTreeMap::new(),
        },
        tar: package_tar(&[
            ("pagis-software.toml", manifest, 0o644),
            ("bin/add.py", python, 0o755),
            ("bin/add.js", node, 0o755),
            ("schemas/add.json", "{\"type\": \"object\"}", 0o644),
        ]),
    });
    let materializer = Arc::new(Materializer::new(
        Arc::clone(&live.managers),
        Arc::clone(&source) as _,
    ));
    let runner = SoftwareRunner::new(
        Arc::clone(&live.managers),
        materializer,
        Arc::clone(&source) as _,
    );
    let arguments = serde_json::json!({"left": 2, "right": 3});

    for tool in ["add_python", "add_node"] {
        let result = runner
            .run(
                &live.workspace_id,
                &live.agent_id,
                "demo",
                "v1",
                tool,
                &arguments,
            )
            .await;

        assert!(!result.is_error, "{tool}: {}", result.content);
        let answer: serde_json::Value =
            serde_json::from_str(result.content.trim()).expect("the tool printed JSON");
        assert_eq!(answer["sum"], 5);
        assert_eq!(answer["tool"], tool);
    }
}

/// `pagis-apt` as the agent: the wrapper owns every option apt
/// sees, so an option, a local `.deb` and any subcommand but `install`
/// are refused, and no refusal runs anything as root.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn pagis_apt_refuses_options_paths_and_other_subcommands() {
    let live = live().await;

    for refused in [
        "sudo pagis-apt install -o 'DPkg::Post-Invoke::=touch /tmp/owned' jq",
        "sudo pagis-apt install ./x.deb",
        "sudo pagis-apt install /tmp/x.deb",
        "sudo pagis-apt remove jq",
    ] {
        let outcome = live.shell(refused).await;

        assert_eq!(outcome.exit_code, 2, "{refused} was not refused");
        assert!(
            outcome.stderr.contains("pagis-apt:"),
            "{refused}: {}",
            outcome.stderr
        );
    }
    // No refusal ran anything as root.
    assert_ne!(live.shell("test -e /tmp/owned").await.exit_code, 0);
}
