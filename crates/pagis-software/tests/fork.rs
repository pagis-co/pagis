//! `software_fork` and the origin of a Fork (ADR-0016): the
//! checkout into the forker's Computer, the rename in the manifest,
//! the refusals, and the origin the first publish records.

use crate::support;

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use pagis_core::{Agent, AgentId, AgentStatus, RunId, SoftwareStore, WorkspaceId, now_ms};
use pagis_software::{ORIGIN_FILE, Origin, SoftwareGitStore, SoftwareList, SoftwareListDeps};
use pagis_testkit::MemorySoftwareStore;

use support::{MemoryAgents, RecordingBus, RecordingManifests, RecordingNotes};

const MANIFEST: &str = r#"
# The forecast package.
[package]
name = "weather"
description = "Forecasts for any city."
keywords = ["weather", "forecast"]

[[tool]]
name = "get_weather"
description = "The forecast of one city."
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
"#;

const SCHEMA: &str = r#"{"type": "object", "properties": {"city": {"type": "string"}}}"#;

/// The tar the Docker archive endpoint answers with: every entry under
/// the exported directory's own name.
pub fn container_tar(root: &str, files: &[(&str, &str, u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{root}/{path}"), content.as_bytes())
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

fn working_copy(body: &str) -> Vec<u8> {
    container_tar(
        "weather",
        &[
            ("pagis-software.toml", MANIFEST, 0o644),
            ("bin/forecast.py", body, 0o755),
            ("schemas/forecast.json", SCHEMA, 0o644),
        ],
    )
}

/// Every file of one tar, by path.
pub fn files_of(tar: &[u8]) -> BTreeMap<String, String> {
    let mut archive = tar::Archive::new(tar);
    let mut files = BTreeMap::new();
    for entry in archive.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().expect("path").display().to_string();
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("text");
        files.insert(path, text);
    }
    files
}

pub struct Fixture {
    pub list: Arc<SoftwareList>,
    pub notes: Arc<RecordingNotes>,
    pub packages: Arc<MemorySoftwareStore>,
    pub git: Arc<SoftwareGitStore>,
    pub runtime: Arc<pagis_computer::fake::FakeComputerRuntime>,
    pub author: AgentId,
    pub forker: AgentId,
    pub workspace_id: WorkspaceId,
    _home: tempfile::TempDir,
}

pub async fn fixture() -> Fixture {
    let harness = support::harness().await;
    // The Software half resolves the Computer manager from the Workspace
    // of the caller, so these bodies act as the tenant whose
    // Computer the harness woke.
    let workspace_id = harness.workspace_id.clone();
    let forker = AgentId::generate();
    support::wake(&harness.manager, &forker).await;
    let agents = vec![
        agent(&workspace_id, &harness.agent_id, "Ada"),
        agent(&workspace_id, &forker, "Bo"),
    ];
    let home = tempfile::tempdir().expect("home");
    let packages = Arc::new(MemorySoftwareStore::default());
    let git = Arc::new(SoftwareGitStore::new(home.path()));
    let notes = Arc::new(RecordingNotes::default());
    let list = Arc::new(SoftwareList::new(SoftwareListDeps {
        packages: Arc::clone(&packages) as _,
        agents: MemoryAgents::holding(agents) as _,
        git: Arc::clone(&git),
        computers: Arc::clone(&harness.managers),
        manifests: Arc::new(RecordingManifests::default()) as _,
        bus: Arc::new(RecordingBus::default()) as _,
        notes: Arc::clone(&notes) as _,
    }));
    Fixture {
        list,
        packages,
        git,
        notes,
        runtime: Arc::clone(&harness.runtime),
        author: harness.agent_id.clone(),
        forker,
        workspace_id,
        _home: home,
    }
}

pub fn agent(workspace_id: &WorkspaceId, id: &AgentId, name: &str) -> Agent {
    let now = now_ms();
    Agent {
        id: id.clone(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
        job: "general assistant".to_string(),
        description: String::new(),
        personality: "warm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now,
        updated_at: now,
    }
}

/// Publish `weather` as its author, so a fork has something to copy.
pub async fn publish_weather(fixture: &Fixture, body: &str) {
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy(body));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "a version",
        )
        .await
        .expect("the publish succeeds");
}

/// The tar `software_fork` wrote into `~/software`.
pub fn forked_tar(fixture: &Fixture) -> Vec<u8> {
    fixture
        .runtime
        .uploads()
        .into_iter()
        .rfind(|(path, _)| path == "/data/agent/software")
        .expect("the fork was uploaded")
        .1
}

#[tokio::test]
async fn a_fork_copies_the_version_and_renames_the_manifest() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let text = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "weather-bo",
        )
        .await
        .expect("the fork succeeds");

    assert_eq!(
        text,
        "Forked weather v1 into ~/software/weather-bo. Change it, then software_publish it as \
         weather-bo."
    );
    let files = files_of(&forked_tar(&fixture));
    assert_eq!(
        files.keys().cloned().collect::<Vec<_>>(),
        vec![
            "weather-bo/.pagis-origin.toml".to_string(),
            "weather-bo/bin/forecast.py".to_string(),
            "weather-bo/pagis-software.toml".to_string(),
            "weather-bo/schemas/forecast.json".to_string(),
        ]
    );
    // The fork is written into the forker's own memory.
    assert!(
        fixture.notes.lines().contains(&format!(
            "{}: forked weather as weather-bo from v1",
            fixture.forker
        )),
        "{:?}",
        fixture.notes.lines()
    );
    let manifest = &files["weather-bo/pagis-software.toml"];
    assert!(manifest.contains("name = \"weather-bo\""), "{manifest}");
    // Only the name changes; the rest of the manifest is the author's.
    assert!(manifest.contains("# The forecast package."), "{manifest}");
    assert!(manifest.contains("name = \"get_weather\""), "{manifest}");
    assert_eq!(
        Origin::parse(&files["weather-bo/.pagis-origin.toml"]).expect("the marker parses"),
        Origin {
            package: "weather".to_string(),
            version: "v1".to_string(),
        }
    );
    assert_eq!(files["weather-bo/bin/forecast.py"], "print(1)\n");
}

#[tokio::test]
async fn a_fork_takes_the_version_it_names() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    publish_weather(&fixture, "print(2)\n").await;

    fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather@v1",
            "weather-bo",
        )
        .await
        .expect("the fork succeeds");

    let files = files_of(&forked_tar(&fixture));
    assert_eq!(files["weather-bo/bin/forecast.py"], "print(1)\n");
    assert_eq!(
        Origin::parse(&files["weather-bo/.pagis-origin.toml"])
            .expect("the marker parses")
            .version,
        "v1"
    );
}

#[tokio::test]
async fn a_taken_fork_name_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let refused = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "weather",
        )
        .await;

    assert_eq!(
        refused,
        Err("weather is taken in this workspace; pick another fork name".to_string())
    );
}

#[tokio::test]
async fn an_invalid_fork_name_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let refused = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "weather bo!",
        )
        .await
        .expect_err("the fork is refused");

    assert!(
        refused.starts_with("the fork name \"weather bo!\" is not"),
        "{refused}"
    );
    assert!(fixture.runtime.uploads().is_empty());
}

#[tokio::test]
async fn a_reserved_fork_name_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let refused = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "core",
        )
        .await;

    assert_eq!(
        refused,
        Err("the fork name \"core\" is reserved".to_string())
    );
}

#[tokio::test]
async fn an_unknown_source_is_refused() {
    let fixture = fixture().await;

    let refused = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "weather-bo",
        )
        .await;

    assert_eq!(refused, Err("there is no package weather".to_string()));
}

#[tokio::test]
async fn an_unknown_version_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let refused = fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather@v7",
            "weather-bo",
        )
        .await;

    assert_eq!(refused, Err("weather has no version v7".to_string()));
}

#[tokio::test]
async fn the_first_publish_of_a_fork_records_its_origin_and_drops_the_marker() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fixture
        .list
        .fork(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "weather-bo",
        )
        .await
        .expect("the fork succeeds");
    fixture
        .runtime
        .set_download("/data/agent/software/weather-bo", forked_tar(&fixture));

    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather-bo",
            "the fork",
        )
        .await
        .expect("the publish succeeds");

    let origin = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the origin package");
    let fork = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather-bo")
        .await
        .unwrap()
        .expect("the fork package");
    assert_eq!(fork.author_agent_id, fixture.forker);
    assert_eq!(fork.origin_package_id, Some(origin.id));
    assert_eq!(fork.origin_version, Some("v1".to_string()));

    // The marker is the daemon's, not the package's: no Version holds
    // it.
    let tar = fixture
        .git
        .version_tar(&fixture.workspace_id, "weather-bo", "v1")
        .await
        .expect("the version tar");
    assert!(!files_of(&tar).contains_key(ORIGIN_FILE));
}
