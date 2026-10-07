//! Tests for `cargo xtask pins --check`: the pin rules of the third-party
//! code that the release workflows and the image builds run. An action
//! is pinned to a full commit ID, a checkout keeps no credentials, and a
//! base image or a Compose image is pinned by digest. The tests also hold
//! the commit check of the source checkouts of the Computer Image and the
//! Dependabot configuration that updates the pins.

use std::path::{Path, PathBuf};

use xtask::pins::{
    COMPOSE, DOCKERFILES, Violation, compose_violations, dockerfile_violations, violations,
    workflow_violations,
};

const COMMIT: &str = "fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09";
const DIGEST: &str = "sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a";

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

fn read(name: &str) -> String {
    let path = root().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn write(dir: &Path, path: &str, body: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A workflow with one job whose steps are `steps`, indented as a step
/// list under `steps:`.
fn workflow(steps: &str) -> String {
    format!(
        "name: CI\n\
         on:\n  workflow_dispatch:\n\
         jobs:\n  gate:\n    runs-on: ubuntu-latest\n    steps:\n{steps}"
    )
}

/// The line numbers of `found`, to compare with the lines a test expects.
fn lines(found: &[Violation]) -> Vec<usize> {
    found.iter().map(|violation| violation.line).collect()
}

fn listed(found: &[Violation]) -> String {
    found
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

// --- the workflows ---

#[test]
fn an_action_pinned_to_a_commit_with_its_tag_passes() {
    let text = workflow(&format!(
        "      - uses: actions/checkout@{COMMIT} # v5.1.0\n\
         \x20       with:\n\
         \x20         persist-credentials: false\n\
         \n\
         \x20     - uses: \"dtolnay/rust-toolchain@{COMMIT}\" # master\n\
         \x20       with:\n\
         \x20         toolchain: stable\n"
    ));

    let found = workflow_violations(".github/workflows/ci.yml", &text);

    assert!(found.is_empty(), "{}", listed(&found));
}

#[test]
fn an_action_referred_to_by_a_tag_a_branch_or_a_short_commit_fails() {
    for reference in ["v5", "v5.1.0", "stable", "nextest", "fbc6f39"] {
        let text = workflow(&format!(
            "      - uses: Swatinem/rust-cache@{reference} # v2.9.2\n"
        ));

        let found = workflow_violations(".github/workflows/ci.yml", &text);

        assert_eq!(lines(&found), [8], "{reference}: {}", listed(&found));
        assert!(
            found[0].reason.contains("40-character commit ID"),
            "{reference}: {}",
            found[0]
        );
        assert!(
            found[0]
                .to_string()
                .starts_with(".github/workflows/ci.yml:8: "),
            "{}",
            found[0]
        );
    }
}

#[test]
fn a_pinned_action_without_its_release_tag_in_a_comment_fails() {
    let text = workflow(&format!("      - uses: Swatinem/rust-cache@{COMMIT}\n"));

    let found = workflow_violations(".github/workflows/ci.yml", &text);

    assert_eq!(lines(&found), [8], "{}", listed(&found));
    assert!(found[0].reason.contains("comment"), "{}", found[0]);
}

#[test]
fn a_reusable_workflow_is_held_to_the_same_rule() {
    let text = "jobs:\n  call:\n    uses: octo-org/example/.github/workflows/build.yml@main\n";

    let found = workflow_violations(".github/workflows/call.yml", text);

    assert_eq!(lines(&found), [3], "{}", listed(&found));
}

/// A workflow of this repository, named by its path, runs the code of the
/// same commit, which cannot move. So it needs no pin.
#[test]
fn a_workflow_of_this_repository_needs_no_pin() {
    let text = "jobs:\n  gate:\n    uses: ./.github/workflows/ci.yml\n";

    let found = workflow_violations(".github/workflows/release.yml", text);

    assert!(found.is_empty(), "{}", listed(&found));
}

#[test]
fn a_checkout_step_that_keeps_the_credentials_fails() {
    for with in [
        "",
        "        with:\n          fetch-depth: 0\n",
        "        with:\n          persist-credentials: true\n",
    ] {
        let text = workflow(&format!(
            "      - uses: actions/checkout@{COMMIT} # v5.1.0\n{with}\
             \x20     - uses: Swatinem/rust-cache@{COMMIT} # v2.9.2\n"
        ));

        let found = workflow_violations(".github/workflows/ci.yml", &text);

        assert_eq!(lines(&found), [8], "{with:?}: {}", listed(&found));
        assert!(
            found[0].reason.contains("persist-credentials: false"),
            "{}",
            found[0]
        );
    }
}

#[test]
fn a_checkout_step_that_sets_persist_credentials_false_passes() {
    for step in [
        format!(
            "      - uses: actions/checkout@{COMMIT} # v5.1.0\n\
             \x20       with:\n\
             \x20         fetch-depth: 0\n\
             \x20         persist-credentials: false\n"
        ),
        format!(
            "      - name: check out\n\
             \x20       with:\n\
             \x20         persist-credentials: \"false\" # no step pushes\n\
             \x20       uses: actions/checkout@{COMMIT} # v5.1.0\n"
        ),
    ] {
        let found = workflow_violations(".github/workflows/ci.yml", &workflow(&step));

        assert!(found.is_empty(), "{step}: {}", listed(&found));
    }
}

/// The input belongs to the checkout step only when it is in the `with:`
/// of that step.
#[test]
fn persist_credentials_of_another_step_does_not_count() {
    let text = workflow(&format!(
        "      - uses: actions/checkout@{COMMIT} # v5.1.0\n\
         \x20     - uses: example/other@{COMMIT} # v1.0.0\n\
         \x20       with:\n\
         \x20         persist-credentials: false\n\
         \x20     # persist-credentials: false\n"
    ));

    let found = workflow_violations(".github/workflows/ci.yml", &text);

    assert_eq!(lines(&found), [8], "{}", listed(&found));
}

// --- the Dockerfiles ---

#[test]
fn a_base_image_pinned_by_tag_and_digest_passes() {
    let text = format!(
        "# A comment that names FROM debian:trixie-slim is not an instruction.\n\
         FROM debian:trixie-slim@{DIGEST} AS compositor\n\
         FROM --platform=$BUILDPLATFORM debian:trixie-slim@{DIGEST} AS blocker\n\
         from ghcr.io/example/tool:1.2@{DIGEST}\n"
    );

    let found = dockerfile_violations("computer/Dockerfile", &text);

    assert!(found.is_empty(), "{}", listed(&found));
}

#[test]
fn a_base_image_without_a_digest_fails() {
    let text = "FROM rust:1-trixie AS build\n\
                FROM --platform=$BUILDPLATFORM debian:trixie-slim\n\
                from node:26-slim as ui\n\
                FROM debian@sha256:abc AS short\n";

    let found = dockerfile_violations("Dockerfile", text);

    assert_eq!(lines(&found), [1, 2, 3, 4], "{}", listed(&found));
    assert!(found[0].reason.contains("@sha256:"), "{}", found[0]);
}

/// The tag shows the version to a reader, and Dependabot uses it to find
/// the next digest.
#[test]
fn a_digest_without_its_tag_fails() {
    let text = format!("FROM debian@{DIGEST}\nFROM ghcr.io/example/tool@{DIGEST}\n");

    let found = dockerfile_violations("Dockerfile", &text);

    assert_eq!(lines(&found), [1, 2], "{}", listed(&found));
    assert!(found[0].reason.contains("tag"), "{}", found[0]);
}

#[test]
fn a_stage_of_the_same_file_and_scratch_are_not_images() {
    let text = format!(
        "FROM debian:trixie-slim@{DIGEST} AS base\n\
         FROM base AS final\n\
         FROM Base\n\
         FROM scratch\n"
    );

    let found = dockerfile_violations("Dockerfile", &text);

    assert!(found.is_empty(), "{}", listed(&found));
}

#[test]
fn a_line_that_continues_an_instruction_is_not_a_from() {
    let text = format!(
        "FROM debian:trixie-slim@{DIGEST}\n\
         RUN echo one \\\n\
         \x20   # a comment inside the instruction\n\
         FROM the words of the same instruction \\\n\
         \x20   && echo two\n"
    );

    let found = dockerfile_violations("Dockerfile", &text);

    assert!(found.is_empty(), "{}", listed(&found));
}

// --- the Compose file ---

#[test]
fn a_compose_image_pinned_by_tag_and_digest_passes() {
    let text = format!(
        "services:\n  db:\n    image: postgres:18-alpine@{DIGEST}\n  proxy:\n    image: \"caddy:2-alpine@{DIGEST}\" # the proxy\n"
    );

    let found = compose_violations("deploy/compose.yaml", &text);

    assert!(found.is_empty(), "{}", listed(&found));
}

#[test]
fn a_compose_image_without_a_digest_fails() {
    let text = "services:\n  db:\n    image: postgres:18-alpine\n  proxy:\n    image: caddy:2-alpine\n  cache:\n    image: redis@sha256:abc\n";

    let found = compose_violations("deploy/compose.yaml", text);

    assert_eq!(lines(&found), [3, 5, 7], "{}", listed(&found));
    assert!(found[0].reason.contains("@sha256:"), "{}", found[0]);
}

/// The tag of the Headless Server image is the release that the
/// deployment sets in `PAGIS_VERSION`, so the check skips it.
#[test]
fn the_headless_server_image_takes_its_tag_from_the_release() {
    let text = "services:\n  pagis:\n    image: ghcr.io/pagis-co/pagis-server:${PAGIS_VERSION:?set PAGIS_VERSION in .env}\n";

    let found = compose_violations("deploy/compose.yaml", text);

    assert!(found.is_empty(), "{}", listed(&found));
}

/// The tag of the Push Relay image is the crate version that the
/// deployment sets in `PUSH_RELAY_VERSION`, so the check skips it too.
/// Another image of the same Compose file is still checked.
#[test]
fn the_push_relay_image_takes_its_tag_from_the_crate_version() {
    let text = "services:\n  relay:\n    image: ghcr.io/pagis-co/pagis-push-relay:${PUSH_RELAY_VERSION:?set PUSH_RELAY_VERSION in .env}\n  proxy:\n    image: caddy:2-alpine\n";

    let found = compose_violations("deploy/push-relay/compose.yaml", text);

    assert_eq!(lines(&found), [5], "{}", listed(&found));
}

// --- the files the check reads ---

fn write_tree(dir: &Path) {
    write(
        dir,
        ".github/workflows/a.yml",
        &workflow("      - uses: actions/setup-node@v5\n"),
    );
    write(
        dir,
        ".github/workflows/b.yaml",
        &workflow("      - uses: docker/setup-buildx-action@v3\n"),
    );
    write(
        dir,
        ".github/workflows/notes.txt",
        "uses: example/any@main\n",
    );
    write(dir, "Dockerfile", "FROM node:26-slim AS ui\n");
    write(dir, "computer/Dockerfile", "FROM debian:trixie-slim\n");
    write(
        dir,
        "crates/pagis-push-relay/Dockerfile",
        "FROM rust:1-trixie AS build\n",
    );
    write(
        dir,
        "deploy/compose.yaml",
        "services:\n  db:\n    image: postgres:18-alpine\n",
    );
    write(
        dir,
        "deploy/push-relay/compose.yaml",
        "services:\n  proxy:\n    image: caddy:2-alpine\n",
    );
}

#[test]
fn the_check_reads_each_workflow_the_three_dockerfiles_and_both_compose_files() {
    let tmp = tempfile::tempdir().unwrap();
    write_tree(tmp.path());

    let found = violations(tmp.path()).unwrap();
    let files: Vec<&str> = found
        .iter()
        .map(|violation| violation.file.as_str())
        .collect();

    assert_eq!(
        DOCKERFILES,
        [
            "Dockerfile",
            "computer/Dockerfile",
            "crates/pagis-push-relay/Dockerfile"
        ]
    );
    assert_eq!(
        COMPOSE,
        ["deploy/compose.yaml", "deploy/push-relay/compose.yaml"]
    );
    assert_eq!(
        files,
        [
            ".github/workflows/a.yml",
            ".github/workflows/b.yaml",
            "Dockerfile",
            "computer/Dockerfile",
            "crates/pagis-push-relay/Dockerfile",
            "deploy/compose.yaml",
            "deploy/push-relay/compose.yaml",
        ],
        "{}",
        listed(&found)
    );
}

/// A file that moves must not leave the check without a word.
#[test]
fn a_missing_file_is_an_error() {
    for path in [
        ".github/workflows",
        "Dockerfile",
        "computer/Dockerfile",
        "crates/pagis-push-relay/Dockerfile",
        "deploy/compose.yaml",
        "deploy/push-relay/compose.yaml",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write_tree(tmp.path());
        let path_in_tree = tmp.path().join(path);
        if path_in_tree.is_dir() {
            std::fs::remove_dir_all(path_in_tree).unwrap();
        } else {
            std::fs::remove_file(path_in_tree).unwrap();
        }

        let error = violations(tmp.path()).unwrap_err();

        assert!(format!("{error:#}").contains(path), "{path}: {error:#}");
    }
}

#[test]
fn the_repository_follows_the_pin_rules() {
    let found = violations(&root()).unwrap();

    assert!(
        found.is_empty(),
        "the release workflows and the image builds hold mutable references:\n{}",
        listed(&found)
    );
}

// --- the source checkouts of the Computer Image ---

/// The instructions of a Dockerfile, each continued line joined to the
/// line before it, with the comment lines left out.
fn instructions(dockerfile: &str) -> Vec<String> {
    let mut all = Vec::new();
    let mut current = String::new();
    for line in dockerfile.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        match line.trim_end().strip_suffix('\\') {
            Some(start) => {
                current.push_str(start);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                all.push(std::mem::take(&mut current));
            }
        }
    }
    all
}

/// A tag can move, so the build of each source checkout compares the
/// commit of the clone with a pinned commit ID and stops on a mismatch.
#[test]
fn each_source_checkout_of_the_computer_image_is_checked_against_its_pinned_commit() {
    let dockerfile = read("computer/Dockerfile");
    let instructions = instructions(&dockerfile);
    let mut names = Vec::new();
    for clone in instructions
        .iter()
        .filter(|instruction| instruction.contains("git clone"))
    {
        let words: Vec<&str> = clone.split_whitespace().collect();
        let at = words
            .iter()
            .position(|word| *word == "--branch")
            .unwrap_or_else(|| panic!("the clone takes no tag: {clone}"));
        let url = words[at + 2];
        let name = url.rsplit('/').next().unwrap().trim_end_matches(".git");
        let variable = format!("{}_COMMIT", name.to_uppercase());
        let pinned = instructions
            .iter()
            .find_map(|instruction| instruction.trim().strip_prefix(&format!("ARG {variable}=")))
            .unwrap_or_else(|| panic!("computer/Dockerfile has no ARG {variable}"));
        assert!(
            pinned.len() == 40 && pinned.bytes().all(|b| b.is_ascii_hexdigit()),
            "{variable} is not a full commit ID: {pinned}"
        );
        for part in [
            format!("git -C /src/{name} rev-parse HEAD"),
            format!("!= \"${variable}\""),
            "exit 1".to_string(),
        ] {
            assert!(clone.contains(&part), "{name}: {part} is missing: {clone}");
        }
        names.push(name.to_string());
    }

    assert_eq!(names, ["wlroots", "labwc"]);
}

/// tar that runs as root keeps the owner that an archive records. The
/// Node and uv archives record uid 1001, which is `screen` in the
/// Computer Image, and `screen` must not own the runtimes and the
/// directories of `/usr/local` that the agent and root run from
/// (ADR-0013). So each extraction gives its files to root.
#[test]
fn each_archive_of_the_computer_image_is_extracted_as_root() {
    let dockerfile = read("computer/Dockerfile");
    let mut archives = Vec::new();
    for instruction in instructions(&dockerfile) {
        let commands = instruction
            .replace("&&", ";")
            .replace("||", ";")
            .replace('|', ";");
        for command in commands.split(';') {
            let words: Vec<&str> = command.split_whitespace().collect();
            let extracts = words.first() == Some(&"tar")
                && words.iter().skip(1).any(|word| {
                    *word == "--extract"
                        || (word.starts_with('-') && !word.starts_with("--") && word.contains('x'))
                });
            if !extracts {
                continue;
            }
            assert!(
                words.contains(&"--no-same-owner"),
                "the extraction keeps the owners of the archive: {}",
                command.trim()
            );
            archives.extend(
                words
                    .iter()
                    .filter(|word| word.starts_with("/tmp/"))
                    .map(|word| word.to_string()),
            );
        }
    }

    assert_eq!(
        archives,
        [
            "/tmp/uv.tar.gz",
            "/tmp/node.tar.xz",
            "/tmp/npm.tgz",
            "/tmp/pnpm.tgz"
        ]
    );
}

// --- the updates ---

/// Dependabot opens the pull requests that move each pin: the actions,
/// the base images of the three Dockerfiles and the images of both
/// Compose files.
#[test]
fn dependabot_updates_the_actions_the_base_images_and_the_compose_images() {
    let config = read(".github/dependabot.yml");
    let entries: Vec<(String, Vec<String>)> = config
        .split("- package-ecosystem:")
        .skip(1)
        .map(|entry| {
            let ecosystem = entry.lines().next().unwrap().trim().to_string();
            let directories = entry
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    line.strip_prefix("directory:")
                        .or_else(|| line.strip_prefix("- "))
                        .map(|directory| directory.trim().trim_matches('"').to_string())
                })
                .collect();
            (ecosystem, directories)
        })
        .collect();

    assert!(config.contains("version: 2"), "{config}");
    assert_eq!(
        entries,
        [
            ("github-actions".to_string(), vec!["/".to_string()]),
            (
                "docker".to_string(),
                vec![
                    "/".to_string(),
                    "/computer".to_string(),
                    "/crates/pagis-push-relay".to_string()
                ]
            ),
            (
                "docker-compose".to_string(),
                vec!["/deploy".to_string(), "/deploy/push-relay".to_string()]
            ),
        ]
    );
}
