//! `software_publish` end to end (ADR-0016): the export
//! out of the Computer, the checks, the commit and tag, the records,
//! the Capability Manifest and the audit fact.

use crate::support;

use std::sync::Arc;

use pagis_core::{Agent, AgentId, AgentStatus, RunId, SoftwareStore, WorkspaceId, now_ms};
use pagis_software::{SoftwareGitStore, SoftwareList, SoftwareListDeps, VersionSource};
use pagis_testkit::MemorySoftwareStore;

use support::{MemoryAgents, RecordingBus, RecordingManifests, RecordingNotes};

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

/// The files of the sound package other than its manifest.
const PACKAGE_FILES: [(&str, &str, u32); 2] = [
    ("bin/forecast.py", "print(1)\n", 0o755),
    ("schemas/forecast.json", SCHEMA, 0o644),
];

/// A text that no package holds. A refusal that carries it has read a
/// file on the daemon host.
const MARKER: &str = "MARKER-from-the-daemon-host";

/// The tar the Docker archive endpoint answers with: every entry under
/// the exported directory's own name.
fn container_tar(files: &[(&str, &str, u32)]) -> Vec<u8> {
    container_tar_with(files, &[])
}

/// A container tar of `files` and of `others`, the entries that are
/// not regular files: each one is a path, a type and a link target. A
/// FIFO has no link target.
fn container_tar_with(
    files: &[(&str, &str, u32)],
    others: &[(&str, tar::EntryType, &str)],
) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("weather/{path}"), content.as_bytes())
            .expect("append");
    }
    for (path, kind, target) in others {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_size(0);
        header.set_mode(0o644);
        let path = format!("weather/{path}");
        if target.is_empty() {
            builder
                .append_data(&mut header, path, std::io::empty())
                .expect("append");
        } else {
            builder
                .append_link(&mut header, path, target)
                .expect("append");
        }
    }
    builder.into_inner().expect("tar")
}

/// A relative link target that reaches `file` from the scratch
/// directory a publish unpacks into. The scratch directory is one level
/// below the temp directory, so one `..` for each component of the temp
/// directory climbs to the file system root. The rest walks down to the
/// file.
fn climbing_target(file: &std::path::Path) -> String {
    let depth = std::env::temp_dir()
        .canonicalize()
        .expect("the temp directory")
        .components()
        .count();
    format!(
        "{}{}",
        "../".repeat(depth),
        file.strip_prefix("/").expect("an absolute path").display()
    )
}

/// The start of a long refusal, so a failed assertion stays readable.
fn preview(text: &str) -> String {
    text.chars().take(300).collect()
}

fn working_copy(body: &str) -> Vec<u8> {
    container_tar(&[
        ("pagis-software.toml", MANIFEST, 0o644),
        ("bin/forecast.py", body, 0o755),
        ("schemas/forecast.json", SCHEMA, 0o644),
    ])
}

struct Fixture {
    list: Arc<SoftwareList>,
    packages: Arc<MemorySoftwareStore>,
    manifests: Arc<RecordingManifests>,
    bus: Arc<RecordingBus>,
    notes: Arc<RecordingNotes>,
    runtime: Arc<pagis_computer::fake::FakeComputerRuntime>,
    author: AgentId,
    workspace_id: WorkspaceId,
    _home: tempfile::TempDir,
}

async fn fixture(others: &[(AgentId, &str)]) -> Fixture {
    let harness = support::harness().await;
    // The Software half resolves the Computer manager from the Workspace
    // of the caller, so these bodies act as the tenant whose
    // Computer the harness woke.
    let workspace_id = harness.workspace_id.clone();
    let mut agents = vec![agent(&workspace_id, &harness.agent_id, "Ada")];
    for (id, name) in others {
        agents.push(agent(&workspace_id, id, name));
    }
    let home = tempfile::tempdir().expect("home");
    let packages = Arc::new(MemorySoftwareStore::default());
    let manifests = Arc::new(RecordingManifests::default());
    let bus = Arc::new(RecordingBus::default());
    let notes = Arc::new(RecordingNotes::default());
    let list = Arc::new(SoftwareList::new(SoftwareListDeps {
        packages: Arc::clone(&packages) as _,
        agents: MemoryAgents::holding(agents) as _,
        git: Arc::new(SoftwareGitStore::new(home.path())),
        computers: Arc::clone(&harness.managers),
        manifests: Arc::clone(&manifests) as _,
        bus: Arc::clone(&bus) as _,
        notes: Arc::clone(&notes) as _,
    }));
    Fixture {
        list,
        packages,
        manifests,
        bus,
        notes,
        runtime: Arc::clone(&harness.runtime),
        author: harness.agent_id.clone(),
        workspace_id,
        _home: home,
    }
}

/// Publish `~/software/weather` as the fixture's author.
async fn publish_weather(fixture: &Fixture) -> Result<String, String> {
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
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

#[tokio::test]
async fn a_first_publish_claims_the_name_and_mints_v1() {
    let fixture = fixture(&[]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    let run = RunId::generate();

    let text = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &run,
            "weather",
            "first cut",
        )
        .await
        .expect("the publish succeeds");

    assert_eq!(
        text,
        "Published weather v1. Its tools reach other runs from their next start."
    );
    assert_eq!(
        fixture.runtime.downloaded(),
        vec!["/data/agent/software/weather".to_string()]
    );

    let package = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the package is claimed");
    assert_eq!(package.author_agent_id, fixture.author);
    assert_eq!(package.latest_version, "v1");
    assert_eq!(package.description, "Forecasts for any city.");
    assert_eq!(package.keywords, vec!["weather", "forecast"]);

    let versions = fixture
        .packages
        .list_versions(&fixture.workspace_id, &package.id)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].notes, "first cut");
    assert_eq!(versions[0].run_id, run);
    assert_eq!(versions[0].commit_id.len(), 40);

    // The tools of the Version reach the broker under the package
    // namespace, at the tag the daemon minted.
    let installed = fixture.manifests.installed();
    let last = installed.last().expect("a manifest is installed");
    assert_eq!(last.namespace, "weather");
    assert_eq!(last.source_version, "v1");
    assert_eq!(last.tools[0].definition.name, "weather__get_weather");

    let audit = fixture.bus.events();
    let published = audit
        .iter()
        .find(|event| event.event_type == "software.published")
        .expect("the publish is audited");
    assert_eq!(published.payload["package"], "weather");
    assert_eq!(published.payload["version"], "v1");
    assert_eq!(published.agent_id.as_ref(), Some(&fixture.author));

    // The publish is written into the author's own memory.
    let notes = fixture.notes.lines();
    assert_eq!(
        notes,
        vec![format!(
            "{}: published weather v1: first cut",
            fixture.author
        )]
    );
}

#[tokio::test]
async fn the_published_version_reads_back_as_a_tar_and_a_manifest() {
    let fixture = fixture(&[]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first cut",
        )
        .await
        .expect("the publish succeeds");

    let version = fixture
        .list
        .version(&fixture.workspace_id, "weather", "v1")
        .await
        .expect("the manifest reads back");
    let tar = fixture
        .list
        .tar(&fixture.workspace_id, "weather", "v1")
        .await
        .expect("the tar reads back");

    assert_eq!(version.manifest.package.name, "weather");
    assert_eq!(version.schemas["get_weather"]["type"], "object");
    let mut archive = tar::Archive::new(tar.as_slice());
    let mut paths: Vec<String> = archive
        .entries()
        .expect("entries")
        .map(|entry| entry.expect("entry"))
        .filter(|entry| entry.header().entry_type().is_file())
        .map(|entry| entry.path().expect("path").display().to_string())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "bin/forecast.py".to_string(),
            "pagis-software.toml".to_string(),
            "schemas/forecast.json".to_string()
        ]
    );
}

#[tokio::test]
async fn a_second_version_follows_the_first() {
    let fixture = fixture(&[]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect("v1");
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(2)\n"));

    let text = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "second",
        )
        .await
        .expect("v2");

    assert!(text.starts_with("Published weather v2."), "{text}");
    let package = fixture
        .packages
        .get_by_name(&fixture.workspace_id, "weather")
        .await
        .unwrap()
        .expect("the package");
    assert_eq!(package.latest_version, "v2");
    assert_eq!(
        fixture
            .packages
            .list_versions(&fixture.workspace_id, &package.id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn a_publish_that_changes_nothing_is_refused() {
    let fixture = fixture(&[]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect("v1");

    let refused = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "again",
        )
        .await;

    assert_eq!(refused, Err("no change since v1".to_string()));
}

#[tokio::test]
async fn another_agent_is_told_to_fork() {
    let intruder = AgentId::generate();
    let fixture = fixture(&[(intruder.clone(), "Bo")]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect("v1");

    let refused = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &intruder,
            &RunId::generate(),
            "weather",
            "mine now",
        )
        .await;

    assert_eq!(refused, Err("weather is owned by Ada, fork it".to_string()));
}

#[tokio::test]
async fn the_manifest_name_must_equal_the_published_name() {
    let fixture = fixture(&[]).await;
    // The export is a directory called `climate`, but its manifest
    // still names `weather`.
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content, mode) in [
        ("pagis-software.toml", MANIFEST, 0o644),
        ("bin/forecast.py", "print(1)\n", 0o755),
        ("schemas/forecast.json", SCHEMA, 0o644),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("climate/{path}"), content.as_bytes())
            .expect("append");
    }
    fixture.runtime.set_download(
        "/data/agent/software/climate",
        builder.into_inner().expect("tar"),
    );

    let refused = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "climate",
            "first",
        )
        .await;

    assert_eq!(
        refused,
        Err(
            "the manifest names the package \"weather\", and you publish \"climate\"; they must \
             be the same"
                .to_string()
        )
    );
}

#[tokio::test]
async fn every_problem_of_the_working_copy_is_reported_at_once() {
    let fixture = fixture(&[]).await;
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        container_tar(&[
            ("pagis-software.toml", MANIFEST, 0o644),
            // The entry is not executable and the schema is missing.
            ("bin/forecast.py", "print(1)\n", 0o644),
        ]),
    );

    let refused = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect_err("the publish is refused");

    assert!(refused.contains("is not executable"), "{refused}");
    assert!(refused.contains("schemas/forecast.json"), "{refused}");
}

#[tokio::test]
async fn a_missing_working_copy_is_reported() {
    let fixture = fixture(&[]).await;

    let refused = fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect_err("the publish is refused");

    assert!(
        refused.starts_with("cannot read ~/software/weather"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_refresh_reinstalls_every_package_and_builds_the_index() {
    let fixture = fixture(&[]).await;
    fixture
        .runtime
        .set_download("/data/agent/software/weather", working_copy("print(1)\n"));
    fixture
        .list
        .publish(
            &fixture.workspace_id,
            &fixture.author,
            &RunId::generate(),
            "weather",
            "first",
        )
        .await
        .expect("v1");

    fixture
        .list
        .refresh(&fixture.workspace_id)
        .await
        .expect("the refresh runs");

    let index = fixture.list.index(&fixture.workspace_id);
    assert_eq!(
        index.browse(),
        vec!["weather v1 by Ada: Forecasts for any city. (tools: get_weather)".to_string()]
    );
    let hits = index.search("weather forecast").expect("the search runs");
    assert_eq!(hits[0].name, "weather");
    // The search a run calls loads the package it found, and names
    // every tool of it as the manifest describes it.
    let outcome = pagis_software::tool_search(&index, "weather forecast");
    assert_eq!(outcome.packages, ["weather"]);
    assert!(
        outcome.text.starts_with("weather__get_weather: "),
        "{}",
        outcome.text
    );
}

/// A manifest link to a file on the daemon host, in the three shapes an
/// Agent makes with `ln -s`: an absolute target, a relative target that
/// climbs out of the package, and a chain of two links.
#[tokio::test]
async fn a_manifest_link_out_of_the_package_is_refused_and_its_target_stays_unread() {
    let outside = tempfile::tempdir().expect("outside");
    let marker = outside.path().join("marker");
    std::fs::write(&marker, format!("{MARKER}\n")).expect("marker");
    let absolute = marker.display().to_string();
    let relative = climbing_target(&marker);
    let fixture = fixture(&[]).await;

    for (shape, links) in [
        (
            "an absolute link",
            vec![("pagis-software.toml", absolute.as_str())],
        ),
        (
            "a relative link",
            vec![("pagis-software.toml", relative.as_str())],
        ),
        (
            "a chain of two links",
            vec![("pagis-software.toml", "hop"), ("hop", absolute.as_str())],
        ),
    ] {
        let others: Vec<(&str, tar::EntryType, &str)> = links
            .iter()
            .map(|(path, target)| (*path, tar::EntryType::Symlink, *target))
            .collect();
        fixture.runtime.set_download(
            "/data/agent/software/weather",
            container_tar_with(&PACKAGE_FILES, &others),
        );

        let refused = publish_weather(&fixture)
            .await
            .expect_err("the publish is refused");

        assert!(!refused.contains(MARKER), "{shape}: {refused}");
        for (path, _) in &links {
            assert!(
                refused.contains(&format!("{path:?} is a symbolic link")),
                "{shape}: {refused}"
            );
        }
    }
}

/// A read of `/dev/zero` never ends, and it fills the daemon memory.
/// The refusal comes from the tar entry alone.
#[tokio::test]
async fn a_manifest_link_to_dev_zero_is_refused_before_any_read() {
    let fixture = fixture(&[]).await;
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        container_tar_with(
            &PACKAGE_FILES,
            &[("pagis-software.toml", tar::EntryType::Symlink, "/dev/zero")],
        ),
    );

    let refused = publish_weather(&fixture)
        .await
        .expect_err("the publish is refused");

    assert!(
        refused.contains("\"pagis-software.toml\" is a symbolic link"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_hard_link_and_a_fifo_are_refused_by_path() {
    let fixture = fixture(&[]).await;
    let mut files = vec![("pagis-software.toml", MANIFEST, 0o644)];
    files.extend(PACKAGE_FILES);
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        container_tar_with(
            &files,
            &[
                // The archive endpoint names the first path of the file
                // as the link target, under the exported directory.
                (
                    "bin/copy.py",
                    tar::EntryType::Link,
                    "weather/bin/forecast.py",
                ),
                ("pipe", tar::EntryType::Fifo, ""),
            ],
        ),
    );

    let refused = publish_weather(&fixture)
        .await
        .expect_err("the publish is refused");

    assert!(
        refused.contains("\"bin/copy.py\" is a hard link"),
        "{refused}"
    );
    assert!(refused.contains("\"pipe\" is a special file"), "{refused}");
}

/// Each link points at a regular file inside the package. The rule is
/// about the type of the entry, not about where it points.
#[tokio::test]
async fn a_link_at_a_tool_entry_a_schema_or_a_widget_page_is_refused() {
    let manifest = format!(
        "{MANIFEST}\n[[widget]]\nname = \"chart\"\nhtml = \"widgets/chart.html\"\nschema = \
         \"schemas/chart.json\"\n"
    );
    let html = "<!doctype html><title>chart</title>";
    let fixture = fixture(&[]).await;

    for (linked, target) in [
        ("bin/forecast.py", "../real/forecast.py"),
        ("schemas/forecast.json", "../real/forecast.json"),
        ("widgets/chart.html", "../real/chart.html"),
    ] {
        let files: Vec<(&str, &str, u32)> = [
            ("pagis-software.toml", manifest.as_str(), 0o644),
            ("bin/forecast.py", "print(1)\n", 0o755),
            ("schemas/forecast.json", SCHEMA, 0o644),
            ("schemas/chart.json", SCHEMA, 0o644),
            ("widgets/chart.html", html, 0o644),
            ("real/forecast.py", "print(1)\n", 0o755),
            ("real/forecast.json", SCHEMA, 0o644),
            ("real/chart.html", html, 0o644),
        ]
        .into_iter()
        .filter(|(path, _, _)| *path != linked)
        .collect();
        fixture.runtime.set_download(
            "/data/agent/software/weather",
            container_tar_with(&files, &[(linked, tar::EntryType::Symlink, target)]),
        );

        let outcome = publish_weather(&fixture).await;

        let refused = outcome.unwrap_err();
        assert!(
            refused.contains(&format!("{linked:?} is a symbolic link")),
            "{linked}: {refused}"
        );
    }
}

/// The manifest is not TOML at all, so a parse would fail with its own
/// message. The size refusal comes first.
#[tokio::test]
async fn a_manifest_over_the_size_limit_is_refused_before_it_parses() {
    let oversized = "x".repeat(pagis_software::MAX_MANIFEST_BYTES as usize + 1);
    let fixture = fixture(&[]).await;
    let mut files = vec![("pagis-software.toml", oversized.as_str(), 0o644)];
    files.extend(PACKAGE_FILES);
    fixture
        .runtime
        .set_download("/data/agent/software/weather", container_tar(&files));

    let refused = publish_weather(&fixture)
        .await
        .expect_err("the publish is refused");

    assert!(
        refused.contains(&format!(
            "pagis-software.toml is larger than the limit of {} bytes",
            pagis_software::MAX_MANIFEST_BYTES
        )),
        "{}",
        preview(&refused)
    );
    assert!(!refused.contains("does not parse"), "{}", preview(&refused));
}

/// A Python or a Node package that ran its setup in the Working Copy
/// holds links in `.venv` and `node_modules`. The publish drops those
/// trees, so their links do not stop it.
#[tokio::test]
async fn a_link_in_the_dependencies_does_not_stop_the_publish() {
    let fixture = fixture(&[]).await;
    let mut files = vec![("pagis-software.toml", MANIFEST, 0o644)];
    files.extend(PACKAGE_FILES);
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        container_tar_with(
            &files,
            &[
                (
                    ".venv/bin/python",
                    tar::EntryType::Symlink,
                    "/usr/bin/python3",
                ),
                (
                    "node_modules/.bin/left-pad",
                    tar::EntryType::Symlink,
                    "../left-pad/cli.js",
                ),
            ],
        ),
    );

    let text = publish_weather(&fixture)
        .await
        .expect("the publish succeeds");

    assert!(text.starts_with("Published weather v1."), "{text}");
}

/// One chunk of a synthetic download: zeros that the test binary holds
/// once. A stream of these holds no copy of what it sends.
static ZEROS: [u8; CHUNK] = [0; CHUNK];
const CHUNK: usize = 1024 * 1024;

/// The container tar of a working copy called `root`: the sound
/// package with its manifest renamed, and then `data` entries of zero
/// bytes, each one a path under `root` and a size.
fn package_with_data(root: &str, data: &[(&str, u64)]) -> Vec<u8> {
    let manifest = MANIFEST.replace("name = \"weather\"", &format!("name = \"{root}\""));
    let mut builder = tar::Builder::new(Vec::new());
    let mut files = vec![("pagis-software.toml", manifest.as_str(), 0o644)];
    files.extend(PACKAGE_FILES);
    for (path, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{root}/{path}"), content.as_bytes())
            .expect("append");
    }
    for (path, size) in data {
        let mut header = tar::Header::new_gnu();
        header.set_size(*size);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("{root}/{path}"),
                std::io::Read::take(std::io::repeat(0), *size),
            )
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

/// The stream is generated as the daemon reads it, and it is longer
/// than the raw limit. The download stops at the chunk that passes the
/// limit: the spool file holds only what the download read, so it
/// stays below the limit plus one chunk.
#[tokio::test]
async fn a_working_copy_tar_over_the_raw_limit_is_refused_before_its_end() {
    use futures::StreamExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    let fixture = fixture(&[]).await;
    let total = pagis_computer::MAX_ARCHIVE_BYTES + 8 * CHUNK as u64;
    let sent = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&sent);
    fixture
        .runtime
        .set_download_stream("/data/agent/software/weather", move || {
            let counter = Arc::clone(&counter);
            Box::pin(
                futures::stream::iter(0..total / CHUNK as u64).map(move |_| {
                    counter.fetch_add(CHUNK as u64, Ordering::SeqCst);
                    Ok(bytes::Bytes::from_static(&ZEROS))
                }),
            )
        });

    let refused = publish_weather(&fixture)
        .await
        .expect_err("the publish is refused");

    assert_eq!(
        refused,
        format!(
            "cannot read ~/software/weather: computer runtime error: the tar of \
             /data/agent/software/weather is larger than the limit of {} bytes",
            pagis_computer::MAX_ARCHIVE_BYTES
        )
    );
    let sent = sent.load(Ordering::SeqCst);
    assert!(sent < total, "the download read the whole stream");
    assert!(
        sent <= pagis_computer::MAX_ARCHIVE_BYTES + CHUNK as u64,
        "the download read {sent} bytes"
    );
    assert!(
        fixture
            .packages
            .list_packages(&fixture.workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The kept files pass the package limit at the third data file. The
/// refusal names the file where the running count passes the limit.
/// It comes from the pack, and the pack reads the tar before any file
/// reaches the disk: the scratch tree is unpacked from the version tar,
/// and a refused pack makes no version tar.
#[tokio::test]
async fn kept_files_over_the_package_limit_are_refused_at_the_running_count() {
    let fixture = fixture(&[]).await;
    let part = 20 * 1024 * 1024;
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        package_with_data(
            "weather",
            &[
                ("data/0.bin", part),
                ("data/1.bin", part),
                ("data/2.bin", part),
                ("data/3.bin", part),
            ],
        ),
    );

    let refused = publish_weather(&fixture)
        .await
        .expect_err("the publish is refused");

    assert_eq!(
        refused,
        format!(
            "the package is larger than the limit of {} bytes after exclusions; \
             \"data/2.bin\" passes it",
            pagis_software::MAX_PACKAGE_BYTES
        )
    );
    assert!(
        fixture
            .packages
            .list_packages(&fixture.workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A Node package that ran its setup in the Working Copy holds a
/// `node_modules` tree larger than the package limit. The publish drops
/// the tree, and its bytes do not count toward the limit.
#[tokio::test]
async fn a_node_modules_tree_over_the_package_limit_does_not_stop_the_publish() {
    let fixture = fixture(&[]).await;
    fixture.runtime.set_download(
        "/data/agent/software/weather",
        package_with_data(
            "weather",
            &[(
                "node_modules/heavy/blob.bin",
                pagis_software::MAX_PACKAGE_BYTES + CHUNK as u64,
            )],
        ),
    );

    let text = publish_weather(&fixture)
        .await
        .expect("the publish succeeds");

    assert!(text.starts_with("Published weather v1."), "{text}");
    let tar = fixture
        .list
        .tar(&fixture.workspace_id, "weather", "v1")
        .await
        .expect("the tar reads back");
    assert!(
        support::read_from_tar(&tar, "node_modules/heavy/blob.bin").is_err(),
        "the version holds node_modules"
    );
}

/// Three publishes start at once. Each download stays open until the
/// test lets it go, so a publish that runs holds its download open.
/// While two downloads are open, the third publish waits and opens no
/// download. At no time are more than two downloads open, and all
/// three publishes land.
#[tokio::test]
async fn at_most_two_publishes_run_at_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let fixture = fixture(&[]).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let names = ["weather", "climate", "tides"];
    for name in names {
        let tar = bytes::Bytes::from(package_with_data(name, &[]));
        let (gate, open, peak) = (Arc::clone(&gate), Arc::clone(&open), Arc::clone(&peak));
        fixture
            .runtime
            .set_download_stream(format!("/data/agent/software/{name}"), move || {
                let (gate, open, peak, tar) = (
                    Arc::clone(&gate),
                    Arc::clone(&open),
                    Arc::clone(&peak),
                    tar.clone(),
                );
                Box::pin(futures::stream::once(async move {
                    let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let pass = gate.acquire().await.expect("the gate");
                    drop(pass);
                    open.fetch_sub(1, Ordering::SeqCst);
                    Ok(tar)
                }))
            });
    }

    let publishes: Vec<_> = names
        .into_iter()
        .map(|name| {
            let list = Arc::clone(&fixture.list);
            let (workspace_id, author) = (fixture.workspace_id.clone(), fixture.author.clone());
            tokio::spawn(async move {
                list.publish(&workspace_id, &author, &RunId::generate(), name, "first")
                    .await
            })
        })
        .collect();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while open.load(Ordering::SeqCst) < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no two downloads opened"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // A third publish that runs opens its download in this time.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert_eq!(open.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.runtime.downloaded().len(), 2);
    gate.add_permits(names.len());
    for publish in publishes {
        let text = publish
            .await
            .expect("the task")
            .expect("the publish succeeds");
        assert!(text.starts_with("Published "), "{text}");
    }
    assert_eq!(fixture.runtime.downloaded().len(), 3);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}
