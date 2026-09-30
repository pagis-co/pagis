//! Tests for the advisory checks: cargo-deny over the Cargo lockfiles,
//! npm audit over the npm lockfiles, the Trivy scans, the pins of the
//! tools, the reviewed exceptions and the daily workflow.
//!
//! The tests do not run the checks. A check reads the advisories that
//! are published today, so its result changes without a change to the
//! tree.

use std::path::Path;

use pagis_versions::{CARGO_DENY_SHA256, CARGO_DENY_VERSION, TRIVY_SHA256, TRIVY_VERSION};
use xtask::advisories::{
    DENY_CONFIG, TRIVY_IGNORE, advisory_lane, computer_image_components, published_advisory_lane,
};
use xtask::{Action, Lane, Step};

use crate::support::workspace_root;

fn target() -> &'static Path {
    Path::new("/shared/pagis-target")
}

fn computer_dockerfile() -> String {
    std::fs::read_to_string(workspace_root().join("computer/Dockerfile"))
        .expect("read computer/Dockerfile")
}

fn step<'a>(lane: &'a Lane, name: &str) -> &'a Step {
    lane.steps
        .iter()
        .find(|step| step.name == name)
        .unwrap_or_else(|| panic!("no step {name} in lane {}", lane.name))
}

fn script(step: &Step) -> String {
    let Action::Run(cmds) = &step.action else {
        panic!("step {} must run, got {:?}", step.name, step.action);
    };
    cmds.iter()
        .map(|cmd| format!("{} {}", cmd.program, cmd.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn write(dir: &Path, path: &str, body: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// The Trivy options that every scan uses.
fn assert_trivy_scan(script: &str) {
    assert!(
        script.contains(&format!(
            "https://github.com/aquasecurity/trivy/releases/download/v{TRIVY_VERSION}/trivy_{TRIVY_VERSION}_"
        )),
        "{script}"
    );
    assert!(
        TRIVY_SHA256
            .iter()
            .any(|(_, sha256)| script.contains(sha256)),
        "{script}"
    );
    assert!(script.contains("shasum -a 256 -c"), "{script}");
    assert!(
        script.contains("/shared/pagis-target/trivy"),
        "the download is kept in the shared target directory: {script}"
    );
    for option in [
        "--scanners vuln",
        "--severity HIGH,CRITICAL",
        "--ignore-unfixed",
        "--ignorefile .trivyignore",
        "--exit-code 1",
    ] {
        assert!(script.contains(option), "{option} is missing: {script}");
    }
}

// --- the lanes ---

/// The release lane checks the lockfiles only. Each computer-image job of
/// the release scans the image of its own platform before the push.
#[test]
fn the_advisory_lane_checks_the_lockfiles() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = advisory_lane(tmp.path(), target());
    let names: Vec<&str> = lane.steps.iter().map(|step| step.name).collect();

    assert_eq!(lane.name, "advisories");
    assert_eq!(
        names,
        [
            "cargo-deny",
            "ui-npm-audit",
            "desktop-npm-audit",
            "docs-site-npm-audit",
        ]
    );
}

/// The daily workflow scans what people run: the images of the latest
/// release.
#[test]
fn the_published_advisory_lane_scans_the_images_of_the_latest_release() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = published_advisory_lane(tmp.path(), target());
    let names: Vec<&str> = lane.steps.iter().map(|step| step.name).collect();
    assert_eq!(
        names,
        [
            "cargo-deny",
            "ui-npm-audit",
            "desktop-npm-audit",
            "docs-site-npm-audit",
            "release-image-scan"
        ]
    );

    let scan = script(step(&lane, "release-image-scan"));
    assert_trivy_scan(&scan);
    assert!(
        scan.contains("repos/pagis-co/pagis/releases/latest"),
        "{scan}"
    );
    assert!(scan.contains("runtime-lock-linux-x64.json"), "{scan}");
    assert!(scan.contains("computer_image"), "{scan}");
    assert!(scan.contains("ghcr.io/pagis-co/pagis-server:"), "{scan}");
    assert!(scan.contains("--image-src remote"), "{scan}");
    assert!(scan.contains("linux/amd64 linux/arm64"), "{scan}");
    assert!(
        scan.contains("HTTP 404"),
        "a repository without a release has no image to scan: {scan}"
    );
}

// --- cargo-deny ---

#[test]
fn cargo_deny_checks_the_two_lockfiles_with_the_pinned_binary() {
    let tree = tempfile::tempdir().unwrap();
    let lane = advisory_lane(tree.path(), target());
    let deny = step(&lane, "cargo-deny");
    let script = script(deny);

    assert!(
        script.contains(&format!(
            "https://github.com/EmbarkStudios/cargo-deny/releases/download/{CARGO_DENY_VERSION}/cargo-deny-{CARGO_DENY_VERSION}-"
        )),
        "{script}"
    );
    assert!(
        CARGO_DENY_SHA256
            .iter()
            .any(|(_, sha256)| script.contains(sha256)),
        "{script}"
    );
    assert!(script.contains("shasum -a 256 -c"), "{script}");
    assert!(
        script.contains("/shared/pagis-target/cargo-deny"),
        "the download is kept in the shared target directory: {script}"
    );
    for manifest in [
        "--manifest-path Cargo.toml",
        "--manifest-path computer/screend/Cargo.toml",
    ] {
        assert!(script.contains(manifest), "{manifest}: {script}");
    }
    assert!(
        script.contains(&format!("--config {DENY_CONFIG} check")),
        "{script}"
    );
    assert!(script.contains("check advisories"), "{script}");
    let Action::Run(cmds) = &deny.action else {
        unreachable!()
    };
    for cmd in cmds {
        assert_eq!(cmd.cwd.as_deref(), Some(tree.path()));
    }
}

// --- npm audit ---

#[test]
fn npm_audit_reads_each_lockfile_at_the_high_level() {
    let tmp = tempfile::tempdir().unwrap();
    for package in ["ui", "desktop", "docs-site"] {
        write(tmp.path(), &format!("{package}/package.json"), "{}");
        write(tmp.path(), &format!("{package}/package-lock.json"), "{}");
    }
    let lane = advisory_lane(tmp.path(), target());

    for (name, package) in [
        ("ui-npm-audit", "ui"),
        ("desktop-npm-audit", "desktop"),
        ("docs-site-npm-audit", "docs-site"),
    ] {
        let Action::Run(cmds) = &step(&lane, name).action else {
            panic!("{name} must run");
        };
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "npm");
        // The audit reads the lockfile only, so it needs no
        // node_modules.
        assert_eq!(
            cmds[0].args,
            ["audit", "--audit-level=high", "--package-lock-only"]
        );
        assert_eq!(
            cmds[0].cwd.as_deref(),
            Some(tmp.path().join(package).as_path())
        );
    }
}

#[test]
fn npm_audit_skips_a_package_without_a_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = advisory_lane(tmp.path(), target());
    for name in ["ui-npm-audit", "desktop-npm-audit", "docs-site-npm-audit"] {
        assert!(
            matches!(step(&lane, name).action, Action::Skip(_)),
            "{name} must skip without a lockfile"
        );
    }
}

// --- the components that Trivy does not identify ---

/// Trivy identifies the Debian packages, uv, pnpm and the npm packages
/// that Node ships. It does not identify labwc and wlroots, which the
/// image builds from source, or the Node runtime, whose executable holds
/// no package metadata. The scan prints their pinned versions for a
/// check by hand.
#[test]
fn the_components_trivy_does_not_identify_are_read_from_the_dockerfile() {
    assert_eq!(
        computer_image_components(&computer_dockerfile()),
        ["wlroots 0.19.3", "labwc 0.9.8", "node 24.20.0"]
    );
}

#[test]
fn a_source_checkout_and_the_node_pin_are_components() {
    let dockerfile = "FROM debian\n\
                      RUN git clone --depth 1 --branch v2.1 https://example.com/tools/widget.git /src/widget\n\
                      ARG UV_VERSION=0.1.0\n\
                      ARG NODE_VERSION=26.0.1\n\
                      RUN git clone https://example.com/other.git /src/other\n";

    assert_eq!(
        computer_image_components(dockerfile),
        ["widget v2.1", "node 26.0.1"]
    );
}

// --- the pins ---

/// The gate runs on macOS arm64 and on Linux amd64, and the release on
/// macOS arm64. The pins cover the platforms the release builds for.
#[test]
fn the_advisory_tools_are_pinned_for_every_host_of_the_gate_and_the_release() {
    let names = |pins: [(&'static str, &'static str); 3]| pins.map(|(name, _)| name);
    assert_eq!(
        names(CARGO_DENY_SHA256),
        [
            "aarch64-apple-darwin",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl"
        ]
    );
    assert_eq!(
        names(TRIVY_SHA256),
        ["macOS-ARM64", "Linux-64bit", "Linux-ARM64"]
    );
    for (platform, sha256) in CARGO_DENY_SHA256.into_iter().chain(TRIVY_SHA256) {
        assert!(
            sha256.len() == 64
                && sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{platform}: {sha256}"
        );
    }
}

// --- the reviewed exceptions ---

fn deny_ignores() -> Vec<toml::Table> {
    let config: toml::Table = std::fs::read_to_string(workspace_root().join(DENY_CONFIG))
        .expect("read deny.toml")
        .parse()
        .expect("deny.toml is TOML");
    config["advisories"]["ignore"]
        .as_array()
        .expect("deny.toml has an [advisories] ignore list")
        .iter()
        .map(|entry| {
            entry
                .as_table()
                .unwrap_or_else(|| panic!("an ignore entry has no reason: {entry}"))
                .clone()
        })
        .collect()
}

#[test]
fn every_ignored_advisory_holds_a_reason() {
    for entry in deny_ignores() {
        let id = entry
            .get("id")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        let reason = entry
            .get("reason")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        assert!(id.starts_with("RUSTSEC-"), "{entry:?}");
        assert!(!reason.trim().is_empty(), "{id} has no reason");
    }
}

/// The lru exception stands only while no tantivy release accepts the
/// fixed lru, so its reason says which requirement blocks the update.
#[test]
fn the_lru_exception_names_the_tantivy_requirement_that_blocks_the_update() {
    let lru = deny_ignores()
        .into_iter()
        .find(|entry| entry["id"].as_str() == Some("RUSTSEC-2026-0253"))
        .expect("an lru entry");
    let reason = lru["reason"].as_str().unwrap();

    assert!(reason.contains("tantivy"), "{reason}");
    assert!(reason.contains("0.18.2"), "{reason}");
}

/// `.trivyignore` holds one advisory ID for each line, and a comment line
/// above each one holds its reason.
#[test]
fn every_trivy_exception_holds_a_reason() {
    let ignored =
        std::fs::read_to_string(workspace_root().join(TRIVY_IGNORE)).expect("read .trivyignore");
    let lines: Vec<&str> = ignored.lines().map(str::trim).collect();

    for (at, line) in lines.iter().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let reason = at
            .checked_sub(1)
            .and_then(|above| lines[above].strip_prefix('#'))
            .unwrap_or_default();
        assert!(!reason.trim().is_empty(), "{line} has no reason above it");
    }
}

// --- the daily workflow ---

#[test]
fn the_advisory_workflow_runs_daily_and_on_request_and_reports_in_one_issue() {
    let workflow =
        std::fs::read_to_string(workspace_root().join(".github/workflows/advisories.yml"))
            .expect("read .github/workflows/advisories.yml");

    for needed in [
        "schedule:",
        "cron:",
        "workflow_dispatch:",
        "cargo xtask advisories",
        "issues: write",
        "gh issue list",
        "gh issue comment",
        "gh issue create",
    ] {
        assert!(workflow.contains(needed), "{needed} is missing");
    }
}
