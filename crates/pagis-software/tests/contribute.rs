//! `software_contribute`, `contribution_view` and
//! `contribution_close` (ADR-0016): the preconditions, the
//! patch, the base of a second Contribution, the message the author
//! reads, and the outcome the forker reads.

use crate::support;

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use pagis_core::{
    Agent, AgentId, AgentStatus, ContributionStatus, ContributionStore, RunId, SoftwareStore,
    WorkspaceId, now_ms,
};
use pagis_software::{
    Contributions, ContributionsDeps, SoftwareGitStore, SoftwareList, SoftwareListDeps,
};
use pagis_testkit::{MemoryContributionStore, MemorySoftwareStore};

use support::{MemoryAgents, RecordingBus, RecordingManifests, RecordingMessenger, RecordingNotes};

const MANIFEST: &str = r#"
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

fn container_tar(root: &str, files: &[(&str, &str, u32)]) -> Vec<u8> {
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

fn weather_copy(body: &str) -> Vec<u8> {
    container_tar(
        "weather",
        &[
            ("pagis-software.toml", MANIFEST, 0o644),
            ("bin/forecast.py", body, 0o755),
            ("schemas/forecast.json", SCHEMA, 0o644),
        ],
    )
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
        entry.read_to_string(&mut text).expect("text");
        files.insert(path, text);
    }
    files
}

/// The tar of the forked working copy, with one file replaced. It is
/// what the forker's Computer answers a publish with.
fn changed_fork(forked: &[u8], path: &str, body: &str) -> Vec<u8> {
    let files = files_of(forked);
    let mut builder = tar::Builder::new(Vec::new());
    for (held, text) in &files {
        let content = if held == path { body } else { text.as_str() };
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(if held.ends_with(".py") { 0o755 } else { 0o644 });
        header.set_cksum();
        builder
            .append_data(&mut header, held, content.as_bytes())
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

struct Fixture {
    list: Arc<SoftwareList>,
    notes: Arc<RecordingNotes>,
    bus: Arc<RecordingBus>,
    contributions: Arc<Contributions>,
    records: Arc<MemoryContributionStore>,
    packages: Arc<MemorySoftwareStore>,
    messenger: Arc<RecordingMessenger>,
    runtime: Arc<pagis_computer::fake::FakeComputerRuntime>,
    author: AgentId,
    forker: AgentId,
    workspace_id: WorkspaceId,
    _home: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let harness = support::harness().await;
    // The Software half resolves the Computer manager from the Workspace
    // of the caller, so these bodies act as the tenant whose
    // Computer the harness woke.
    let workspace_id = harness.workspace_id.clone();
    let forker = AgentId::generate();
    support::wake(&harness.manager, &forker).await;
    let agents = MemoryAgents::holding(vec![
        agent(&workspace_id, &harness.agent_id, "Ada"),
        agent(&workspace_id, &forker, "Bo"),
    ]);
    let home = tempfile::tempdir().expect("home");
    let packages = Arc::new(MemorySoftwareStore::default());
    let git = Arc::new(SoftwareGitStore::new(home.path()));
    let records = Arc::new(MemoryContributionStore::default());
    let messenger = Arc::new(RecordingMessenger::default());
    let notes = Arc::new(RecordingNotes::default());
    let bus = Arc::new(RecordingBus::default());
    let list = Arc::new(SoftwareList::new(SoftwareListDeps {
        packages: Arc::clone(&packages) as _,
        agents: Arc::clone(&agents) as _,
        git: Arc::clone(&git),
        computers: Arc::clone(&harness.managers),
        manifests: Arc::new(RecordingManifests::default()) as _,
        bus: Arc::clone(&bus) as _,
        notes: Arc::clone(&notes) as _,
    }));
    let contributions = Arc::new(Contributions::new(ContributionsDeps {
        packages: Arc::clone(&packages) as _,
        contributions: Arc::clone(&records) as _,
        agents: Arc::clone(&agents) as _,
        git: Arc::clone(&git),
        messenger: Arc::clone(&messenger) as _,
        bus: Arc::clone(&bus) as _,
        notes: Arc::clone(&notes) as _,
    }));
    Fixture {
        list,
        notes,
        bus,
        contributions,
        records,
        packages,
        messenger,
        runtime: Arc::clone(&harness.runtime),
        author: harness.agent_id.clone(),
        forker,
        workspace_id,
        _home: home,
    }
}

fn agent(workspace_id: &WorkspaceId, id: &AgentId, name: &str) -> Agent {
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

async fn publish_weather(fixture: &Fixture, body: &str) {
    fixture
        .runtime
        .set_download("/data/agent/software/weather", weather_copy(body));
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

fn forked_tar(fixture: &Fixture) -> Vec<u8> {
    fixture
        .runtime
        .uploads()
        .into_iter()
        .rfind(|(path, _)| path == "/data/agent/software")
        .expect("the fork was uploaded")
        .1
}

/// Fork `weather`, change one line, and publish the fork.
async fn fork_and_publish(fixture: &Fixture, body: &str) {
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
    let changed = changed_fork(&forked_tar(fixture), "weather-bo/bin/forecast.py", body);
    publish_fork(fixture, changed).await;
}

async fn publish_fork(fixture: &Fixture, tar: Vec<u8>) {
    fixture
        .runtime
        .set_download("/data/agent/software/weather-bo", tar);
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
        .expect("the fork publish succeeds");
}

#[tokio::test]
async fn a_contribution_carries_the_patch_and_wakes_the_author() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;

    let text = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "print two, not one",
        )
        .await
        .expect("the contribution opens");

    let records = fixture
        .records
        .list_by_fork(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather-bo")
                .await
                .unwrap()
                .expect("the fork")
                .id,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.status, ContributionStatus::Open);
    assert_eq!(record.base_version, "v1");
    assert_eq!(record.latest_at_open, "v1");
    assert_eq!(record.fork_version, "v1");
    assert_eq!(record.summary, "print two, not one");
    assert!(record.patch.contains("-print(1)"), "{}", record.patch);
    assert!(record.patch.contains("+print(2)"), "{}", record.patch);
    assert!(
        text.starts_with(&format!("Contribution {} is open", record.id)),
        "{text}"
    );

    // The author reads it as a message from the forker (ADR-0016).
    let posted = fixture.messenger.posted();
    assert_eq!(posted.len(), 1);
    let (from, to, message) = &posted[0];
    assert_eq!(from, &fixture.forker);
    assert_eq!(to, &fixture.author);
    assert!(
        message.starts_with(&format!(
            "Contribution `{}` to `weather` from `weather-bo`: print two, not one",
            record.id
        )),
        "{message}"
    );
    assert!(message.contains("Base weather v1"), "{message}");
    assert!(message.contains("```diff"), "{message}");
    assert!(message.contains("+print(2)"), "{message}");

    // The desk hears about it, and the forker's memory keeps it.
    let updated = fixture
        .bus
        .events()
        .into_iter()
        .find(|event| event.event_type == "contribution.updated")
        .expect("the desk is told");
    assert_eq!(updated.payload["package"], "weather");
    assert_eq!(updated.payload["id"], record.id.as_str());
    assert_eq!(updated.payload["status"], "open");
    assert!(
        fixture.notes.lines().contains(&format!(
            "{}: contribution {} to weather opened: print two, not one",
            fixture.forker, record.id
        )),
        "{:?}",
        fixture.notes.lines()
    );
}

#[tokio::test]
async fn a_large_patch_is_announced_by_its_files() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    let long: String = (0..4000)
        .map(|line| format!("print({line})\n"))
        .collect::<Vec<_>>()
        .join("");
    fork_and_publish(&fixture, &long).await;

    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a rewrite",
        )
        .await
        .expect("the contribution opens");

    let message = &fixture.messenger.posted()[0].2;
    assert!(!message.contains("```diff"), "{message}");
    assert!(message.contains("too large for a message"), "{message}");
    assert!(message.contains("- bin/forecast.py"), "{message}");
    assert!(message.contains("contribution_view"), "{message}");
}

#[tokio::test]
async fn a_forker_without_a_published_fork_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    // The fork is written into the Computer, and never published.
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

    let refused = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await;

    assert_eq!(
        refused,
        Err(
            "you have no published fork of weather; software_fork it, change it, and \
             software_publish it first"
                .to_string()
        )
    );
}

#[tokio::test]
async fn the_author_of_the_package_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;

    let refused = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await;

    assert_eq!(
        refused,
        Err(
            "you are the author of weather; publish a new version instead of contributing to \
             yourself"
                .to_string()
        )
    );
}

#[tokio::test]
async fn an_unknown_package_is_refused() {
    let fixture = fixture().await;

    let refused = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await;

    assert_eq!(refused, Err("there is no package weather".to_string()));
}

#[tokio::test]
async fn a_second_open_contribution_from_one_fork_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "the first",
        )
        .await
        .expect("the first contribution opens");

    let refused = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "the second",
        )
        .await
        .expect_err("the second is refused");

    assert!(refused.contains("is still open"), "{refused}");
}

#[tokio::test]
async fn an_empty_patch_is_refused() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    // The fork publishes the copy as it stands, so the manifest name is
    // its one change. Publish the same body and then contribute after a
    // merge, when the trees are equal.
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
    let forked = forked_tar(&fixture);
    publish_fork(&fixture, changed_fork(&forked, "none", "")).await;
    let fork = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather-bo")
        .await
        .unwrap()
        .expect("the fork");
    let origin = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the origin");
    // A merged Contribution moves the base to the Fork Version it
    // carried, so a second one with no new change has an empty patch.
    fixture
        .records
        .create(&pagis_core::Contribution {
            id: pagis_core::ContributionId::generate(),
            workspace_id: fixture.workspace_id.clone(),
            package_id: origin.id.clone(),
            base_version: "v1".to_string(),
            latest_at_open: "v1".to_string(),
            fork_package_id: fork.id.clone(),
            fork_version: "v1".to_string(),
            patch: "a patch".to_string(),
            summary: "the first".to_string(),
            status: ContributionStatus::Merged,
            outcome_reason: Some("taken".to_string()),
            created_at: now_ms(),
            closed_at: Some(now_ms()),
            run_id: RunId::generate(),
        })
        .await
        .unwrap();

    let refused = fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "again",
        )
        .await;

    assert_eq!(
        refused,
        Err(
            "weather-bo v1 is the same as weather-bo v1; there is nothing to contribute"
                .to_string()
        )
    );
}

#[tokio::test]
async fn a_second_contribution_starts_at_the_merged_fork_version() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "the first",
        )
        .await
        .expect("the first contribution opens");
    // The author merges it: a new Version of the origin, then a close.
    publish_weather(&fixture, "print(2)\n").await;
    let origin = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the origin");
    let first = fixture
        .records
        .list_by_package(&fixture.workspace_id, &origin.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the record");
    fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            first.id.as_str(),
            "merged",
            "taken as it stands",
        )
        .await
        .expect("the close succeeds");
    // The forker changes one more line and publishes v2 of the fork.
    let forked = forked_tar(&fixture);
    publish_fork(
        &fixture,
        changed_fork(&forked, "weather-bo/bin/forecast.py", "print(3)\n"),
    )
    .await;

    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "the second",
        )
        .await
        .expect("the second contribution opens");

    let second = fixture
        .records
        .list_by_fork(&fixture.workspace_id, &first.fork_package_id)
        .await
        .unwrap()
        .into_iter()
        .find(|held| held.status == ContributionStatus::Open)
        .expect("the second record");
    // It carries only the new change: the base is the Fork Version the
    // merged Contribution carried, not the origin Version again.
    assert_eq!(second.base_version, "v1");
    assert_eq!(second.fork_version, "v2");
    assert!(second.patch.contains("-print(2)"), "{}", second.patch);
    assert!(second.patch.contains("+print(3)"), "{}", second.patch);
    assert!(!second.patch.contains("print(1)"), "{}", second.patch);
    assert_eq!(second.latest_at_open, "v2");
}

#[tokio::test]
async fn a_view_reads_the_record_and_one_file_of_the_patch() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await
        .expect("the contribution opens");
    let record = fixture
        .records
        .list_by_fork(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather-bo")
                .await
                .unwrap()
                .expect("the fork")
                .id,
        )
        .await
        .unwrap()
        .remove(0);

    let whole = fixture
        .contributions
        .view(
            &fixture.workspace_id,
            &fixture.author,
            record.id.as_str(),
            None,
        )
        .await
        .expect("the author reads it");
    let one = fixture
        .contributions
        .view(
            &fixture.workspace_id,
            &fixture.forker,
            record.id.as_str(),
            Some("bin/forecast.py"),
        )
        .await
        .expect("the forker reads it");
    let missing = fixture
        .contributions
        .view(
            &fixture.workspace_id,
            &fixture.author,
            record.id.as_str(),
            Some("README.md"),
        )
        .await;

    assert!(whole.contains("status: open"), "{whole}");
    assert!(whole.contains("summary: a change"), "{whole}");
    assert!(whole.contains("+print(2)"), "{whole}");
    assert!(one.contains("diff --git a/bin/forecast.py"), "{one}");
    assert!(one.contains("+print(2)"), "{one}");
    assert!(!one.contains("pagis-software.toml"), "{one}");
    assert_eq!(
        missing,
        Err("the patch does not change README.md".to_string())
    );
}

#[tokio::test]
async fn a_close_as_merged_needs_a_publish_after_the_record_opened() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await
        .expect("the contribution opens");
    let record = fixture
        .records
        .list_by_package(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather")
                .await
                .unwrap()
                .expect("the origin")
                .id,
        )
        .await
        .unwrap()
        .remove(0);

    let refused = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            record.id.as_str(),
            "merged",
            "taken",
        )
        .await
        .expect_err("the close is refused");

    assert!(
        refused.contains("has published no version since"),
        "{refused}"
    );
    assert_eq!(
        fixture
            .records
            .get(&fixture.workspace_id, &record.id)
            .await
            .unwrap()
            .expect("the record")
            .status,
        ContributionStatus::Open
    );
    // Only one message so far: the Contribution itself.
    assert_eq!(fixture.messenger.posted().len(), 1);
}

#[tokio::test]
async fn a_merged_close_names_the_version_and_wakes_the_forker() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await
        .expect("the contribution opens");
    publish_weather(&fixture, "print(2)\n").await;
    let record = fixture
        .records
        .list_by_package(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather")
                .await
                .unwrap()
                .expect("the origin")
                .id,
        )
        .await
        .unwrap()
        .remove(0);

    let outcome = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            record.id.as_str(),
            "merged",
            "taken as it stands",
        )
        .await
        .expect("the close succeeds");

    assert_eq!(
        outcome,
        format!(
            "Contribution {}: merged in weather v2. taken as it stands",
            record.id
        )
    );
    let closed = fixture
        .records
        .get(&fixture.workspace_id, &record.id)
        .await
        .unwrap()
        .expect("the record");
    assert_eq!(closed.status, ContributionStatus::Merged);
    assert_eq!(closed.outcome_reason.as_deref(), Some("taken as it stands"));
    assert!(closed.closed_at.is_some());
    let posted = fixture.messenger.posted();
    assert_eq!(posted.len(), 2);
    assert_eq!(posted[1].0, fixture.author);
    assert_eq!(posted[1].1, fixture.forker);
    assert_eq!(posted[1].2, outcome);

    // The desk hears about the close, and the author's memory keeps it.
    let updated = fixture
        .bus
        .events()
        .into_iter()
        .rfind(|event| event.event_type == "contribution.updated")
        .expect("the desk is told");
    assert_eq!(updated.payload["id"], record.id.as_str());
    assert_eq!(updated.payload["status"], "merged");
    assert!(
        fixture.notes.lines().contains(&format!(
            "{}: contribution {} to weather merged in v2: taken as it stands",
            fixture.author, record.id
        )),
        "{:?}",
        fixture.notes.lines()
    );
}

#[tokio::test]
async fn a_declined_close_needs_no_publish_and_says_why() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await
        .expect("the contribution opens");
    let record = fixture
        .records
        .list_by_package(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather")
                .await
                .unwrap()
                .expect("the origin")
                .id,
        )
        .await
        .unwrap()
        .remove(0);

    let outcome = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            record.id.as_str(),
            "declined",
            "the package prints one on purpose",
        )
        .await
        .expect("the close succeeds");

    assert_eq!(
        outcome,
        format!(
            "Contribution {}: declined: the package prints one on purpose",
            record.id
        )
    );
    assert_eq!(
        fixture
            .records
            .get(&fixture.workspace_id, &record.id)
            .await
            .unwrap()
            .expect("the record")
            .status,
        ContributionStatus::Declined
    );
    assert_eq!(fixture.messenger.posted()[1].2, outcome);
}

#[tokio::test]
async fn only_the_author_closes_a_contribution() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "a change",
        )
        .await
        .expect("the contribution opens");
    let record = fixture
        .records
        .list_by_package(
            &fixture.workspace_id,
            &fixture
                .packages
                .get_by_name(&fixture.workspace_id, "weather")
                .await
                .unwrap()
                .expect("the origin")
                .id,
        )
        .await
        .unwrap()
        .remove(0);

    let refused = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            record.id.as_str(),
            "merged",
            "mine",
        )
        .await
        .expect_err("the close is refused");

    assert!(refused.contains("only its author closes it"), "{refused}");
}

// --- The round trip ---
//
// The tests above take one rule each. These two take the whole
// journey in one go, as the two agents live it: publish, fork,
// publish, contribute, view, publish again, close.

/// The record of the one Contribution the fixture holds.
async fn only_record(fixture: &Fixture) -> pagis_core::Contribution {
    let origin = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the origin");
    let mut records = fixture
        .records
        .list_by_package(&fixture.workspace_id, &origin.id)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    records.remove(0)
}

#[tokio::test]
async fn a_contribution_goes_from_a_fork_to_a_merged_version() {
    let fixture = fixture().await;

    // A publishes v1. B forks it, changes one line and publishes the
    // fork.
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;

    // B contributes: the record opens and A reads the patch as a
    // message in the A-B channel.
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "print two, not one",
        )
        .await
        .expect("the contribution opens");
    let record = only_record(&fixture).await;
    let offered = fixture.messenger.posted();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].0, fixture.forker);
    assert_eq!(offered[0].1, fixture.author);
    assert!(
        offered[0].2.contains(&format!(
            "Contribution `{}` to `weather` from `weather-bo`: print two, not one",
            record.id
        )),
        "{}",
        offered[0].2
    );
    assert!(offered[0].2.contains("+print(2)"), "{}", offered[0].2);

    // A views the record, applies the change in its own working copy,
    // and publishes v2 from it.
    let view = fixture
        .contributions
        .view(
            &fixture.workspace_id,
            &fixture.author,
            record.id.as_str(),
            None,
        )
        .await
        .expect("the author reads it");
    assert!(view.contains("status: open"), "{view}");
    assert!(view.contains("+print(2)"), "{view}");
    publish_weather(&fixture, "print(2)\n").await;

    // A closes it as merged.
    let outcome = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            record.id.as_str(),
            "merged",
            "taken as it stands",
        )
        .await
        .expect("the close succeeds");

    assert_eq!(
        outcome,
        format!(
            "Contribution {}: merged in weather v2. taken as it stands",
            record.id
        )
    );
    let closed = fixture
        .records
        .get(&fixture.workspace_id, &record.id)
        .await
        .unwrap()
        .expect("the record");
    assert_eq!(closed.status, ContributionStatus::Merged);
    assert_eq!(closed.outcome_reason.as_deref(), Some("taken as it stands"));
    // B reads the outcome in the same channel.
    let posted = fixture.messenger.posted();
    assert_eq!(posted.len(), 2);
    assert_eq!(posted[1].0, fixture.author);
    assert_eq!(posted[1].1, fixture.forker);
    assert_eq!(posted[1].2, outcome);
    // The origin package stands at the merged Version.
    assert_eq!(
        fixture
            .packages
            .get_by_name(&fixture.workspace_id, "weather")
            .await
            .unwrap()
            .expect("the origin")
            .latest_version,
        "v2"
    );
    // B keeps its Fork: a merge takes the change, not the package.
    assert_eq!(
        fixture
            .packages
            .get_by_name(&fixture.workspace_id, "weather-bo")
            .await
            .unwrap()
            .expect("the fork")
            .latest_version,
        "v1"
    );

    // Each action wrote one line of `software.md` in the acting
    // Agent's own private memory, so the next run reads it.
    let notes = fixture.notes.lines();
    assert_eq!(
        notes,
        vec![
            format!("{}: published weather v1: a version", fixture.author),
            format!("{}: forked weather as weather-bo from v1", fixture.forker),
            format!("{}: published weather-bo v1: the fork", fixture.forker),
            format!(
                "{}: contribution {} to weather opened: print two, not one",
                fixture.forker, record.id
            ),
            format!("{}: published weather v2: a version", fixture.author),
            format!(
                "{}: contribution {} to weather merged in v2: taken as it stands",
                fixture.author, record.id
            ),
        ],
        "the author keeps its own publishes and the close; the forker keeps its fork and the \
         contribution"
    );
}

#[tokio::test]
async fn a_declined_round_trip_leaves_the_origin_where_it_was() {
    let fixture = fixture().await;
    publish_weather(&fixture, "print(1)\n").await;
    fork_and_publish(&fixture, "print(2)\n").await;
    fixture
        .contributions
        .contribute(
            &fixture.workspace_id,
            &fixture.forker,
            &RunId::generate(),
            "weather",
            "print two, not one",
        )
        .await
        .expect("the contribution opens");
    let record = only_record(&fixture).await;

    // A declines it without publishing anything.
    let outcome = fixture
        .contributions
        .close(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            record.id.as_str(),
            "declined",
            "the package prints one on purpose",
        )
        .await
        .expect("the close succeeds");

    assert_eq!(
        outcome,
        format!(
            "Contribution {}: declined: the package prints one on purpose",
            record.id
        )
    );
    let closed = fixture
        .records
        .get(&fixture.workspace_id, &record.id)
        .await
        .unwrap()
        .expect("the record");
    assert_eq!(closed.status, ContributionStatus::Declined);
    let posted = fixture.messenger.posted();
    assert_eq!(posted.len(), 2);
    assert_eq!(posted[1].1, fixture.forker);
    assert_eq!(posted[1].2, outcome);
    assert_eq!(
        fixture
            .packages
            .get_by_name(&fixture.workspace_id, "weather")
            .await
            .unwrap()
            .expect("the origin")
            .latest_version,
        "v1"
    );
    // B still reads the record, with the reason it was declined.
    let view = fixture
        .contributions
        .view(
            &fixture.workspace_id,
            &fixture.forker,
            record.id.as_str(),
            None,
        )
        .await
        .expect("the forker reads it");
    assert!(view.contains("status: declined"), "{view}");
    assert!(
        view.contains("outcome: the package prints one on purpose"),
        "{view}"
    );

    // The decline is written down too, in the author's own memory,
    // and no publish joins it.
    assert_eq!(
        fixture.notes.lines(),
        vec![
            format!("{}: published weather v1: a version", fixture.author),
            format!("{}: forked weather as weather-bo from v1", fixture.forker),
            format!("{}: published weather-bo v1: the fork", fixture.forker),
            format!(
                "{}: contribution {} to weather opened: print two, not one",
                fixture.forker, record.id
            ),
            format!(
                "{}: contribution {} to weather declined: the package prints one on purpose",
                fixture.author, record.id
            ),
        ]
    );
}
