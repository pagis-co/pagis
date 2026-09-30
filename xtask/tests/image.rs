//! Tests for publishing the agent computer image
//! (`cargo xtask image`) and for the pins the publish depends on.

use std::path::Path;

use xtask::advisories::computer_image_components;
use xtask::{Action, Cmd, Step, VERSION_LABEL, check_pin, image_plan, labelled_version};

use crate::support::workspace_root;

const DOCKERFILE: &str = "computer/Dockerfile";

fn dockerfile() -> String {
    std::fs::read_to_string(workspace_root().join(DOCKERFILE)).expect("read computer/Dockerfile")
}

fn commands(step: &Step) -> &[Cmd] {
    match &step.action {
        Action::Run(cmds) => cmds,
        Action::Skip(reason) => panic!("step {} skipped: {reason}", step.name),
    }
}

fn joined(steps: &[Step]) -> String {
    steps
        .iter()
        .flat_map(commands)
        .map(|c| format!("{} {}", c.program, c.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

// --- the plan ---

#[test]
fn the_publish_pushes_both_architectures_under_the_pinned_tag() {
    let steps = image_plan(
        Path::new("/repo"),
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        Path::new("/shared/pagis-target"),
    );
    let build = steps.last().expect("a publish has a build step");
    let cmd = &commands(build)[0];
    assert_eq!(cmd.program, "docker");
    let args = cmd.args.join(" ");
    assert!(args.contains("buildx build"), "{args}");
    assert!(args.contains("linux/amd64,linux/arm64"), "{args}");
    assert!(
        args.contains("ghcr.io/pagis-co/pagis-computer:0.3.0"),
        "{args}"
    );
    assert!(args.contains("--push"), "{args}");
    assert!(
        args.ends_with("computer"),
        "the context is computer/: {args}"
    );
    assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/repo")));
}

#[test]
fn the_publish_runs_on_a_builder_that_can_cross_build() {
    // The default `docker` driver refuses a multi-platform build, so
    // the plan makes the container driver's builder first and names it
    // on the build. Making it twice is not an error.
    let steps = image_plan(
        Path::new("/repo"),
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        Path::new("/shared/pagis-target"),
    );
    let script = joined(&steps);
    assert!(script.contains("docker-container"), "{script}");
    assert!(script.contains("--builder pagis"), "{script}");
    let build = &commands(steps.last().unwrap())[0].args.join(" ");
    assert!(
        script.find("docker-container") < script.find(build.as_str()),
        "the builder is made before the build: {script}"
    );
}

/// The package is public, so the publish scans the exported filesystem
/// of the image for secrets and pushes only after a clean scan.
#[test]
fn the_publish_scans_the_image_for_secrets_before_it_pushes() {
    let steps = image_plan(
        &workspace_root(),
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        Path::new("/shared/pagis-target"),
    );
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "builder",
            "image",
            "image-secret-scan",
            "image-vuln-scan",
            "image-push"
        ]
    );
    let build = joined(&steps[1..2]);
    assert!(!build.contains("--push"), "{build}");
    assert!(
        build.contains("type=local,dest=dist/image-fs/computer"),
        "{build}"
    );
    let scan = joined(&steps[2..3]);
    assert!(scan.contains("dist/image-fs/computer"), "{scan}");
}

/// After the secret scan and before the push, Trivy scans the export of
/// each architecture for known vulnerabilities, and the scan prints the
/// pinned components that Trivy does not identify.
#[test]
fn the_publish_scans_the_image_for_vulnerabilities_before_it_pushes() {
    let steps = image_plan(
        &workspace_root(),
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        Path::new("/shared/pagis-target"),
    );
    let scan = steps
        .iter()
        .find(|s| s.name == "image-vuln-scan")
        .expect("an image-vuln-scan step");
    let scan = joined(std::slice::from_ref(scan));

    assert!(scan.contains("trivy"), "{scan}");
    assert!(scan.contains("--scanners vuln"), "{scan}");
    assert!(scan.contains("rootfs"), "{scan}");
    for platform in ["linux_amd64", "linux_arm64"] {
        assert!(
            scan.contains(&format!("dist/image-fs/computer/{platform}")),
            "{scan}"
        );
    }
    for component in computer_image_components(&dockerfile()) {
        assert!(scan.contains(&component), "{component}: {scan}");
    }
}

#[test]
fn every_publish_command_runs_at_the_workspace_root() {
    let steps = image_plan(
        Path::new("/repo"),
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        Path::new("/shared/pagis-target"),
    );
    for cmd in steps.iter().flat_map(commands) {
        assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/repo")), "{cmd:?}");
    }
}

// --- the version label ---

#[test]
fn the_labelled_version_is_read_from_the_dockerfile() {
    let file = "FROM debian\nLABEL org.pagis.computer.version=\"9.9.9\"\nRUN true\n";
    assert_eq!(
        labelled_version(file, VERSION_LABEL).as_deref(),
        Some("9.9.9")
    );
}

#[test]
fn a_dockerfile_without_the_label_has_no_version() {
    assert_eq!(
        labelled_version("FROM debian\nRUN true\n", VERSION_LABEL),
        None
    );
}

#[test]
fn the_image_the_daemon_pins_is_the_one_the_dockerfile_labels() {
    // A push of a mismatched image is a computer the daemon refuses to
    // boot: it compares this label with its own pin.
    assert_eq!(
        labelled_version(&dockerfile(), VERSION_LABEL).as_deref(),
        Some(pagis_versions::COMPUTER_IMAGE_VERSION),
    );
}

#[test]
fn the_pinned_image_is_the_pinned_version_of_the_ghcr_package() {
    assert_eq!(
        pagis_versions::COMPUTER_IMAGE,
        format!(
            "ghcr.io/pagis-co/pagis-computer:{}",
            pagis_versions::COMPUTER_IMAGE_VERSION
        ),
    );
}

/// The image builds `pagis-screend` with the cargo-auditable version that
/// `pagis-versions` pins, which is the version of each build of `pagis`.
#[test]
fn the_image_builds_screend_with_the_pinned_cargo_auditable() {
    let dockerfile = dockerfile();
    let pinned = dockerfile
        .lines()
        .find_map(|line| line.trim().strip_prefix("ARG CARGO_AUDITABLE_VERSION="));
    assert_eq!(pinned, Some(pagis_versions::CARGO_AUDITABLE_VERSION));
    assert!(
        dockerfile.contains("cargo auditable build --release --locked"),
        "{dockerfile}"
    );
}

#[test]
fn the_image_names_the_repository_it_is_built_from() {
    // GHCR reads `org.opencontainers.image.source` to link the package
    // to the repository, which is where a reader of the package page
    // goes for the source and the license.
    assert!(
        dockerfile()
            .contains("org.opencontainers.image.source=\"https://github.com/pagis-co/pagis\""),
        "computer/Dockerfile carries no OCI source label"
    );
}

// --- the pin guard ---

#[test]
fn a_publish_of_the_pinned_version_is_allowed() {
    let file = "LABEL org.pagis.computer.version=\"0.12.0\"\n";
    assert_eq!(check_pin(file, DOCKERFILE, VERSION_LABEL, "0.12.0"), Ok(()));
}

#[test]
fn a_publish_the_daemon_would_refuse_names_both_versions() {
    let file = "LABEL org.pagis.computer.version=\"0.11.0\"\n";
    let reason = check_pin(file, DOCKERFILE, VERSION_LABEL, "0.12.0").unwrap_err();
    assert!(reason.contains("0.11.0"), "{reason}");
    assert!(reason.contains("0.12.0"), "{reason}");
}

#[test]
fn a_publish_of_an_unlabelled_image_is_refused() {
    let reason = check_pin("FROM debian\n", DOCKERFILE, VERSION_LABEL, "0.12.0").unwrap_err();
    assert!(reason.contains("org.pagis.computer.version"), "{reason}");
}
