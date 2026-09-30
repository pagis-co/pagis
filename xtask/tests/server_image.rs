//! Tests for the headless Linux server image
//! (`cargo xtask server-image`) and for the release number it
//! carries.

use std::path::Path;

use xtask::server_image::{SERVER_IMAGE_REPOSITORY, VERSION_LABEL};
use xtask::{
    Action, Cmd, ImagePlatform, Step, check_pin, labelled_version, server_image, server_image_plan,
    server_image_steps,
};

use crate::support::workspace_root;

const DOCKERFILE: &str = "Dockerfile";

fn dockerfile() -> String {
    std::fs::read_to_string(workspace_root().join(DOCKERFILE)).expect("read the server Dockerfile")
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

fn target() -> &'static Path {
    Path::new("/shared/pagis-target")
}

/// Each architecture is pushed by its digest only, with no tag, after
/// its own scans. Last, one step joins both digests into the release tag.
#[test]
fn the_publish_pushes_each_architecture_by_digest_then_joins_them_under_the_release_tag() {
    let steps = server_image_plan(Path::new("/repo"), &server_image("1.2.3"), target());

    for platform in ImagePlatform::ALL {
        let arch = platform.arch();
        let push = steps
            .iter()
            .position(|s| s.name == format!("server-image-push-{arch}"))
            .expect("a push step");
        let pushed = joined(&steps[push..=push]);
        assert!(
            pushed.contains(&format!(
                "--builder pagis --platform linux/{arch} --build-arg PAGIS_POSTHOG_PROJECT_ID \
                 --build-arg PAGIS_POSTHOG_TOKEN --output \
                 type=image,name=ghcr.io/pagis-co/pagis-server,push-by-digest=true,name-canonical=true,push=true \
                 --metadata-file dist/image-digests/server-{arch}.json ."
            )),
            "{pushed}"
        );
        assert!(!pushed.contains("pagis-server:1.2.3"), "{pushed}");
        assert!(
            pushed.contains(&format!("dist/image-digests/server-{arch}.txt")),
            "{pushed}"
        );
        assert!(
            pushed.ends_with(&format!("rm -rf dist/image-fs/server/linux_{arch}")),
            "{pushed}"
        );
    }
    let manifest = steps.last().unwrap();
    assert_eq!(manifest.name, "server-image-manifest");
    let manifest = joined(std::slice::from_ref(manifest));
    assert!(
        manifest.contains("docker buildx imagetools create -t ghcr.io/pagis-co/pagis-server:1.2.3"),
        "{manifest}"
    );
    for arch in ["amd64", "arm64"] {
        assert!(
            manifest.contains(&format!("dist/image-digests/server-{arch}.txt")),
            "{manifest}"
        );
    }
    assert_eq!(
        commands(&steps[1])[0].cwd.as_deref(),
        Some(Path::new("/repo"))
    );
}

/// The default `docker` driver cannot export an image and push it by
/// digest, so the plan makes the container driver's builder first, as
/// the Computer image publish does.
#[test]
fn the_publish_runs_on_a_builder_that_can_export_and_push() {
    let steps = server_image_plan(Path::new("/repo"), &server_image("1.2.3"), target());

    assert_eq!(steps[0].name, "builder");
    assert!(joined(&steps[..1]).contains("buildx create --name pagis"));
}

/// A release builds the server against the digest it resolved, not
/// against a tag that may move.
#[test]
fn a_release_builds_against_the_immutable_computer_image() {
    let steps = server_image_steps(
        Path::new("/repo"),
        &server_image("1.2.3"),
        ImagePlatform::Arm64,
        Some("dist/computer-image.txt"),
        target(),
    );
    let plan = joined(&steps);

    assert!(plan.contains("dist/computer-image.txt"), "{plan}");
    assert!(plan.contains("--build-arg PAGIS_COMPUTER_IMAGE"), "{plan}");
}

/// A build outside a release keeps the tag `pagis-versions` pins, so
/// nothing reads a digest file that is not there.
#[test]
fn a_plain_build_keeps_the_pinned_computer_image() {
    let plan = joined(&server_image_plan(
        Path::new("/repo"),
        &server_image("1.2.3"),
        target(),
    ));

    assert!(!plan.contains("PAGIS_COMPUTER_IMAGE"), "{plan}");
}

/// The PostHog project reaches the build from the environment of the
/// release and never from the repository (ADR-0026), in a release and
/// in a plain publish alike.
#[test]
fn the_build_takes_the_posthog_project_from_the_environment() {
    for steps in [
        server_image_plan(Path::new("/repo"), &server_image("1.2.3"), target()),
        server_image_steps(
            Path::new("/repo"),
            &server_image("1.2.3"),
            ImagePlatform::Amd64,
            Some("dist/computer-image.txt"),
            target(),
        ),
    ] {
        let plan = joined(&steps);
        assert!(
            plan.contains("--build-arg PAGIS_POSTHOG_PROJECT_ID --build-arg PAGIS_POSTHOG_TOKEN"),
            "{plan}"
        );
        assert!(!plan.contains("PAGIS_POSTHOG_TOKEN="), "{plan}");
    }
    let dockerfile = dockerfile();
    assert!(dockerfile.contains("ARG PAGIS_POSTHOG_PROJECT_ID=\"\""));
    assert!(dockerfile.contains("ARG PAGIS_POSTHOG_TOKEN=\"\""));
}

/// A local `.env` holds the secrets of a deployment, so the build context
/// of the image leaves it out at every depth.
#[test]
fn the_build_context_leaves_out_env_files() {
    let ignored = std::fs::read_to_string(workspace_root().join(".dockerignore"))
        .expect("read .dockerignore");
    let lines: Vec<&str> = ignored.lines().map(str::trim).collect();

    for pattern in [".env", "**/.env"] {
        assert!(
            lines.contains(&pattern),
            "{pattern} is not in .dockerignore"
        );
    }
}

/// cargo-auditable writes the list of the crates of `pagis` into the
/// executable, so Trivy identifies them in the image. The build stage
/// installs the version that `pagis-versions` pins, and builds `pagis`
/// with it and with no plain `cargo build`.
#[test]
fn the_image_builds_pagis_with_the_pinned_cargo_auditable() {
    let dockerfile = dockerfile();
    let pinned = dockerfile
        .lines()
        .find_map(|line| line.trim().strip_prefix("ARG CARGO_AUDITABLE_VERSION="));

    assert_eq!(pinned, Some(pagis_versions::CARGO_AUDITABLE_VERSION));
    assert!(
        dockerfile.contains(
            "cargo install --locked --version \"$CARGO_AUDITABLE_VERSION\" cargo-auditable"
        ),
        "{dockerfile}"
    );
    assert!(
        dockerfile.contains("cargo auditable build --release -p pagis"),
        "{dockerfile}"
    );
    assert!(!dockerfile.contains("cargo build"), "{dockerfile}");
}

// --- the release number ---

#[test]
fn the_image_carries_the_release_that_is_in_it() {
    let version = std::fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("read the workspace manifest")
        .parse::<toml::Table>()
        .expect("the manifest is TOML")["workspace"]["package"]["version"]
        .as_str()
        .expect("a workspace version")
        .to_string();

    assert_eq!(
        labelled_version(&dockerfile(), VERSION_LABEL),
        Some(version)
    );
}

#[test]
fn the_published_name_is_the_repository_and_the_release() {
    assert_eq!(
        server_image("1.2.3"),
        format!("{SERVER_IMAGE_REPOSITORY}:1.2.3")
    );
}

#[test]
fn a_publish_whose_label_is_not_the_release_names_both() {
    let file = "LABEL org.pagis.server.version=\"0.0.9\"\n";

    let reason = check_pin(file, DOCKERFILE, VERSION_LABEL, "1.2.3").unwrap_err();

    assert!(reason.contains("0.0.9"), "{reason}");
    assert!(reason.contains("1.2.3"), "{reason}");
}

/// The package is public, so the publish scans the exported filesystem
/// of each architecture for secrets and pushes it only after a clean
/// scan.
#[test]
fn the_publish_scans_each_architecture_for_secrets_before_it_pushes() {
    let steps = server_image_plan(&workspace_root(), &server_image("1.2.3"), target());
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();

    assert_eq!(
        names,
        [
            "builder",
            "server-image-amd64",
            "server-image-secret-scan-amd64",
            "server-image-vuln-scan-amd64",
            "server-image-push-amd64",
            "server-image-arm64",
            "server-image-secret-scan-arm64",
            "server-image-vuln-scan-arm64",
            "server-image-push-arm64",
            "server-image-manifest",
        ]
    );
    for (at, arch) in [(1, "amd64"), (5, "arm64")] {
        let build = joined(&steps[at..=at]);
        assert!(!build.contains("push=true"), "{build}");
        assert!(
            build.contains(&format!("--platform linux/{arch} ")),
            "{build}"
        );
        let export = format!("dist/image-fs/server/linux_{arch}");
        assert!(
            build.contains(&format!("type=tar,dest={export}.tar ")),
            "{build}"
        );
        assert!(
            build.contains(&format!("tar -xf {export}.tar -C {export}")),
            "{build}"
        );
        assert!(!build.contains("type=local"), "{build}");
        assert!(
            joined(&steps[at + 1..=at + 1]).contains(&format!("dist/image-fs/server/linux_{arch}"))
        );
    }
}

/// After the secret scan and before the push, Trivy scans the export of
/// each architecture for known vulnerabilities. The image holds `pagis`,
/// `gog` and the Debian packages, and Trivy identifies each of them, so the
/// scan prints no component for a check by hand.
#[test]
fn the_publish_scans_each_architecture_for_vulnerabilities_before_it_pushes() {
    let steps = server_image_plan(&workspace_root(), &server_image("1.2.3"), target());

    for (at, arch) in [(3, "amd64"), (7, "arm64")] {
        let scan = joined(&steps[at..=at]);
        assert_eq!(steps[at].name, format!("server-image-vuln-scan-{arch}"));
        assert!(scan.contains("trivy"), "{scan}");
        assert!(scan.contains("--scanners vuln"), "{scan}");
        assert!(
            scan.contains(&format!("dist/image-fs/server/linux_{arch}")),
            "{scan}"
        );
        assert!(!scan.contains("by hand"), "{scan}");
    }
}
