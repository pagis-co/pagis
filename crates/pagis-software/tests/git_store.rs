//! The bare repository of one Software Package (ADR-0016).

use std::io::Read;

use pagis_core::WorkspaceId;
use pagis_software::{GitStoreError, NewVersion, PublishAuthor, SoftwareGitStore, next_version};

fn tar_of(files: &[(&str, &str, u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content.as_bytes())
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

/// The path, the mode and the content of every file in a tar.
fn files_in(tar: &[u8]) -> Vec<(String, u32, String)> {
    let mut archive = tar::Archive::new(tar);
    let mut files = Vec::new();
    for entry in archive.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().expect("path").display().to_string();
        let mode = entry.header().mode().expect("mode") & 0o777;
        let mut content = String::new();
        entry.read_to_string(&mut content).expect("content");
        files.push((path, mode, content));
    }
    files.sort();
    files
}

fn author() -> PublishAuthor {
    PublishAuthor {
        name: "Ada".to_string(),
        email: "ada@agents.pagis.local".to_string(),
    }
}

#[test]
fn the_daemon_mints_the_next_version() {
    assert_eq!(next_version(None), "v1");
    assert_eq!(next_version(Some("v1")), "v2");
    assert_eq!(next_version(Some("v9")), "v10");
}

#[tokio::test]
async fn a_version_round_trips_through_the_repository() {
    let home = tempfile::tempdir().expect("home");
    let store = SoftwareGitStore::new(home.path());
    let workspace = WorkspaceId::generate();
    let tar = tar_of(&[
        ("pagis-software.toml", "name = \"weather\"\n", 0o644),
        ("bin/forecast.py", "print(1)\n", 0o755),
    ]);

    let commit = store
        .commit_version(
            &workspace,
            NewVersion {
                package: "weather",
                version: "v1",
                previous: None,
                notes: "first",
                author: &author(),
                tar,
            },
        )
        .await
        .expect("the first version commits");
    assert_eq!(commit.len(), 40);

    let back = store
        .version_tar(&workspace, "weather", "v1")
        .await
        .expect("the version reads back");

    assert_eq!(
        files_in(&back),
        vec![
            (
                "bin/forecast.py".to_string(),
                0o755,
                "print(1)\n".to_string()
            ),
            (
                "pagis-software.toml".to_string(),
                0o644,
                "name = \"weather\"\n".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn a_publish_that_changes_nothing_is_refused() {
    let home = tempfile::tempdir().expect("home");
    let store = SoftwareGitStore::new(home.path());
    let workspace = WorkspaceId::generate();
    let tar = tar_of(&[("pagis-software.toml", "name = \"weather\"\n", 0o644)]);
    store
        .commit_version(
            &workspace,
            NewVersion {
                package: "weather",
                version: "v1",
                previous: None,
                notes: "first",
                author: &author(),
                tar: tar.clone(),
            },
        )
        .await
        .expect("the first version commits");

    let refused = store
        .commit_version(
            &workspace,
            NewVersion {
                package: "weather",
                version: "v2",
                previous: Some("v1"),
                notes: "again",
                author: &author(),
                tar,
            },
        )
        .await;

    assert_eq!(refused, Err(GitStoreError::NoChange("v1".to_string())));
}

#[tokio::test]
async fn a_second_version_keeps_the_first_one_readable() {
    let home = tempfile::tempdir().expect("home");
    let store = SoftwareGitStore::new(home.path());
    let workspace = WorkspaceId::generate();
    store
        .commit_version(
            &workspace,
            NewVersion {
                package: "weather",
                version: "v1",
                previous: None,
                notes: "first",
                author: &author(),
                tar: tar_of(&[("readme.md", "one\n", 0o644)]),
            },
        )
        .await
        .expect("v1");
    store
        .commit_version(
            &workspace,
            NewVersion {
                package: "weather",
                version: "v2",
                previous: Some("v1"),
                notes: "second",
                author: &author(),
                tar: tar_of(&[("readme.md", "two\n", 0o644)]),
            },
        )
        .await
        .expect("v2");

    let first = store
        .version_tar(&workspace, "weather", "v1")
        .await
        .expect("v1 reads");
    let second = store
        .version_tar(&workspace, "weather", "v2")
        .await
        .expect("v2 reads");

    assert_eq!(files_in(&first)[0].2, "one\n");
    assert_eq!(files_in(&second)[0].2, "two\n");
}
