//! Tests for the Push Relay image (`cargo xtask relay-image`), the
//! version it carries, and the workflow of a `push-relay-v*` tag.

use std::path::Path;

use xtask::relay_image::{
    DOCKERFILE, RELAY_IMAGE_REPOSITORY, RelayImageScope, VERSION_LABEL, check_tag, publish_plan,
    relay_image, relay_image_plan, relay_version,
};
use xtask::{Action, ImagePlatform, Step, labelled_version};

use crate::support::workspace_root;

fn dockerfile() -> String {
    std::fs::read_to_string(workspace_root().join(DOCKERFILE)).expect("read the relay Dockerfile")
}

fn target() -> &'static Path {
    Path::new("/shared/pagis-target")
}

fn names(steps: &[Step]) -> Vec<&'static str> {
    steps.iter().map(|step| step.name).collect()
}

fn joined(steps: &[Step]) -> String {
    steps
        .iter()
        .flat_map(|step| match &step.action {
            Action::Run(cmds) => cmds.clone(),
            Action::Skip(reason) => panic!("step {} skipped: {reason}", step.name),
        })
        .map(|c| format!("{} {}", c.program, c.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The steps of one platform, in their order.
fn platform_steps(arch: &str) -> [String; 4] {
    [
        format!("relay-image-{arch}"),
        format!("relay-image-secret-scan-{arch}"),
        format!("relay-image-vuln-scan-{arch}"),
        format!("relay-image-push-{arch}"),
    ]
}

// --- the plan ---

/// A publish checks the advisories of the lockfile first. Then each
/// architecture is built, scanned for secrets and for known
/// vulnerabilities, and pushed by digest, and last one step joins both
/// digests under the tag.
#[test]
fn the_publish_checks_the_advisories_then_builds_scans_and_pushes_each_platform_then_joins_them() {
    let steps = relay_image_plan(
        Path::new("/repo"),
        &relay_image("1.2.3"),
        RelayImageScope::All,
        target(),
    );
    let mut expected = vec!["cargo-deny".to_string(), "builder".to_string()];
    expected.extend(platform_steps("amd64"));
    expected.extend(platform_steps("arm64"));
    expected.push("relay-image-manifest".to_string());

    assert_eq!(names(&steps), expected);
    for platform in ImagePlatform::ALL {
        let arch = platform.arch();
        let at = |name: String| steps.iter().position(|s| s.name == name).unwrap();
        let [build, secret_scan, vuln_scan, push] = platform_steps(arch).map(at);
        let export = format!("dist/image-fs/relay/linux_{arch}");

        let built = joined(&steps[build..=build]);
        assert!(
            built.contains(&format!(
                "docker buildx build --builder pagis --platform linux/{arch} \
                 --file crates/pagis-push-relay/Dockerfile --output type=tar,dest={export}.tar ."
            )),
            "{built}"
        );
        assert!(!built.contains("push=true"), "{built}");
        assert!(
            joined(&steps[secret_scan..=secret_scan]).contains("gitleaks"),
            "{}",
            joined(&steps[secret_scan..=secret_scan])
        );
        let scan = joined(&steps[vuln_scan..=vuln_scan]);
        assert!(scan.contains("trivy") && scan.contains(&export), "{scan}");
        let pushed = joined(&steps[push..=push]);
        assert!(
            pushed.contains(&format!(
                "--output type=image,name=ghcr.io/pagis-co/pagis-push-relay,push-by-digest=true,\
                 name-canonical=true,push=true --metadata-file dist/image-digests/relay-{arch}.json ."
            )),
            "{pushed}"
        );
        assert!(!pushed.contains("pagis-push-relay:1.2.3"), "{pushed}");
        assert!(pushed.ends_with(&format!("rm -rf {export}")), "{pushed}");
    }
    let manifest = joined(&steps[steps.len() - 1..]);
    assert!(
        manifest
            .contains("docker buildx imagetools create -t ghcr.io/pagis-co/pagis-push-relay:1.2.3"),
        "{manifest}"
    );
    for arch in ["amd64", "arm64"] {
        assert!(
            manifest.contains(&format!("dist/image-digests/relay-{arch}.txt")),
            "{manifest}"
        );
    }
}

/// The workflow builds each platform on a runner of that architecture,
/// so `--platform` plans the advisory check and that platform alone.
#[test]
fn one_platform_plans_the_advisories_and_that_platform_only() {
    let steps = relay_image_plan(
        Path::new("/repo"),
        &relay_image("1.2.3"),
        RelayImageScope::Platform(ImagePlatform::Arm64),
        target(),
    );
    let mut expected = vec!["cargo-deny".to_string(), "builder".to_string()];
    expected.extend(platform_steps("arm64"));

    assert_eq!(names(&steps), expected);
}

/// The manifest job joins the digests that the platform jobs pushed,
/// and builds nothing.
#[test]
fn the_manifest_plans_the_manifest_only() {
    let steps = relay_image_plan(
        Path::new("/repo"),
        &relay_image("1.2.3"),
        RelayImageScope::Manifest,
        target(),
    );

    assert_eq!(names(&steps), ["relay-image-manifest"]);
}

#[test]
fn the_flags_select_the_scope() {
    assert_eq!(
        RelayImageScope::from_flags(None, false).unwrap(),
        RelayImageScope::All
    );
    assert_eq!(
        RelayImageScope::from_flags(Some("amd64"), false).unwrap(),
        RelayImageScope::Platform(ImagePlatform::Amd64)
    );
    assert_eq!(
        RelayImageScope::from_flags(None, true).unwrap(),
        RelayImageScope::Manifest
    );
    let both = RelayImageScope::from_flags(Some("arm64"), true).unwrap_err();
    assert!(format!("{both:#}").contains("--manifest"), "{both:#}");
    let unknown = RelayImageScope::from_flags(Some("riscv64"), false).unwrap_err();
    assert!(format!("{unknown:#}").contains("riscv64"), "{unknown:#}");
}

// --- the version ---

#[test]
fn the_published_name_is_the_repository_and_the_crate_version() {
    assert_eq!(
        relay_image("1.2.3"),
        format!("{RELAY_IMAGE_REPOSITORY}:1.2.3")
    );
    assert_eq!(RELAY_IMAGE_REPOSITORY, "ghcr.io/pagis-co/pagis-push-relay");
}

/// The relay ships on its own tag, so the image carries the version of
/// its crate and not the release of the workspace.
#[test]
fn the_image_carries_the_crate_version() {
    let version = relay_version(&workspace_root()).expect("the crate version");
    let manifest: toml::Table =
        std::fs::read_to_string(workspace_root().join("crates/pagis-push-relay/Cargo.toml"))
            .expect("read the crate manifest")
            .parse()
            .expect("the manifest is TOML");

    assert_eq!(manifest["package"]["version"].as_str(), Some(&*version));
    assert_eq!(
        labelled_version(&dockerfile(), VERSION_LABEL),
        Some(version)
    );
}

/// A relay tree with the crate at `version` and a Dockerfile that labels
/// its image `label`.
fn relay_tree(version: &str, label: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let crate_dir = tmp.path().join("crates/pagis-push-relay");
    std::fs::create_dir_all(&crate_dir).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname = \"pagis-push-relay\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("Dockerfile"),
        format!("FROM scratch\nLABEL org.pagis.push-relay.version=\"{label}\"\n"),
    )
    .unwrap();
    tmp
}

#[test]
fn a_label_that_is_not_the_crate_version_refuses_the_publish_and_names_both() {
    let tree = relay_tree("1.2.3", "0.0.9");

    let refused = publish_plan(tree.path(), RelayImageScope::All, None, target()).unwrap_err();
    let refused = format!("{refused:#}");

    assert!(refused.contains("0.0.9"), "{refused}");
    assert!(refused.contains("1.2.3"), "{refused}");
    assert!(refused.contains(DOCKERFILE), "{refused}");
}

#[test]
fn a_label_that_is_the_crate_version_plans_the_publish() {
    let tree = relay_tree("1.2.3", "1.2.3");

    let (image, steps) = publish_plan(
        tree.path(),
        RelayImageScope::Manifest,
        Some("push-relay-v1.2.3"),
        target(),
    )
    .unwrap();

    assert_eq!(image, "ghcr.io/pagis-co/pagis-push-relay:1.2.3");
    assert_eq!(names(&steps), ["relay-image-manifest"]);
}

/// The tag of the workflow names the crate version, as
/// `push-relay-v1.2.3` for `1.2.3`.
#[test]
fn a_tag_that_does_not_name_the_crate_version_refuses_the_publish() {
    assert!(check_tag("push-relay-v1.2.3", "1.2.3").is_ok());
    for tag in ["push-relay-v1.2.4", "v1.2.3", "1.2.3", "push-relay-1.2.3"] {
        let refused = format!("{:#}", check_tag(tag, "1.2.3").unwrap_err());
        assert!(refused.contains(tag), "{refused}");
        assert!(refused.contains("push-relay-v1.2.3"), "{refused}");
    }
    let tree = relay_tree("1.2.3", "1.2.3");
    assert!(
        publish_plan(
            tree.path(),
            RelayImageScope::All,
            Some("push-relay-v1.2.4"),
            target()
        )
        .is_err()
    );
}

// --- the Dockerfile ---

/// The instructions of the final stage of `dockerfile`.
fn runtime_stage(dockerfile: &str) -> &str {
    &dockerfile[dockerfile.rfind("\nFROM ").expect("a final stage")..]
}

/// cargo-auditable writes the list of the crates into the executable,
/// so Trivy identifies them in the image. The build stage installs the
/// version that `pagis-versions` pins.
#[test]
fn the_image_builds_the_relay_with_the_pinned_cargo_auditable() {
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
        dockerfile.contains("cargo auditable build --release -p pagis-push-relay"),
        "{dockerfile}"
    );
    assert!(!dockerfile.contains("cargo build"), "{dockerfile}");
}

/// The image uses the same pinned bases as the Headless Server image, so
/// the pins, Dependabot and the vulnerability scan treat both the same
/// way.
#[test]
fn the_image_uses_the_bases_of_the_headless_server_image() {
    let bases = |text: &str| -> Vec<String> {
        text.lines()
            .filter_map(|line| line.strip_prefix("FROM "))
            .filter_map(|rest| rest.split_whitespace().next())
            .map(str::to_string)
            .collect()
    };
    let server = bases(
        &std::fs::read_to_string(workspace_root().join("Dockerfile"))
            .expect("read the server Dockerfile"),
    );

    let relay = bases(&dockerfile());

    assert_eq!(relay.len(), 2, "{relay:?}");
    for base in &relay {
        assert!(server.contains(base), "{base} is not a base of the server");
    }
    assert!(relay[0].starts_with("rust:1-trixie@"), "{relay:?}");
    assert!(relay[1].starts_with("debian:trixie-slim@"), "{relay:?}");
}

#[test]
fn the_relay_runs_as_a_user_that_is_not_root() {
    let dockerfile = dockerfile();
    let runtime = runtime_stage(&dockerfile);
    let user = runtime
        .lines()
        .filter_map(|line| line.trim().strip_prefix("USER "))
        .next_back()
        .expect("the final stage names a user")
        .trim();
    let name = user.split(':').next().unwrap();

    assert!(!matches!(name, "root" | "0"), "the relay runs as {user}");
}

/// Docker reads the health of the relay from its own route, on the port
/// that `PUSH_RELAY_BIND` names.
#[test]
fn the_image_checks_the_health_route() {
    let dockerfile = dockerfile();
    let runtime = runtime_stage(&dockerfile);
    let check = runtime
        .split("\nHEALTHCHECK ")
        .nth(1)
        .expect("the final stage has a HEALTHCHECK");
    let check = check.split("\n\n").next().unwrap();

    assert!(check.contains("CMD curl -fsS"), "{check}");
    assert!(check.contains("/v1/health"), "{check}");
    assert!(check.contains("PUSH_RELAY_BIND"), "{check}");
    assert!(
        runtime.contains("ENV PUSH_RELAY_BIND=0.0.0.0:8080"),
        "{runtime}"
    );
}

/// The SQLite file is in the state directory, which is a volume that the
/// user of the relay owns.
#[test]
fn the_database_is_in_the_state_directory() {
    let dockerfile = dockerfile();
    let runtime = runtime_stage(&dockerfile);

    assert!(
        runtime.contains("ENV PUSH_RELAY_DATABASE=/var/lib/pagis-push-relay/"),
        "{runtime}"
    );
    assert!(
        runtime.contains("VOLUME /var/lib/pagis-push-relay"),
        "{runtime}"
    );
    assert!(
        runtime.contains("org.opencontainers.image.source=\"https://github.com/pagis-co/pagis\""),
        "{runtime}"
    );
}

/// A fix that Debian ships reaches the image at its next build, as in
/// the Headless Server image. The relay sends to APNs, FCM and Google
/// over TLS, so the image holds the root certificates.
#[test]
fn the_runtime_stage_takes_the_debian_fixes_and_the_root_certificates() {
    let dockerfile = dockerfile();
    let runtime = runtime_stage(&dockerfile);
    let update = runtime.find("apt-get update").expect("an apt-get update");
    let upgrade = runtime
        .find("apt-get upgrade -y")
        .expect("an apt-get upgrade");
    let clean = runtime
        .find("rm -rf /var/lib/apt/lists/*")
        .expect("the package lists removed");

    assert!(update < upgrade && upgrade < clean, "{runtime}");
    assert!(runtime.contains("ca-certificates"), "{runtime}");
    assert!(runtime.contains("curl"), "{runtime}");
}

// --- the workflow ---

/// The text of each job of `workflow`, from its key to the next key at
/// the same depth.
fn job(workflow: &str, name: &str) -> String {
    let jobs = &workflow[workflow.find("\njobs:\n").expect("jobs")..];
    let mut body = String::new();
    let mut inside = false;
    for line in jobs.lines() {
        let is_key = line.starts_with("  ")
            && !line.starts_with("   ")
            && !line.trim_start().starts_with('#')
            && line.ends_with(':');
        if is_key {
            inside = line.trim() == format!("{name}:");
        } else if inside {
            body.push_str(line);
            body.push('\n');
        }
    }
    assert!(!body.is_empty(), "no job {name}");
    body
}

/// A `push-relay-v*` tag runs the gate, then the image of each platform
/// on a runner of that architecture, then the manifest and its
/// provenance. Each image job checks that the tag names the crate
/// version.
#[test]
fn the_tag_workflow_runs_the_gate_before_the_image_jobs() {
    let workflow =
        std::fs::read_to_string(workspace_root().join(".github/workflows/push-relay.yml"))
            .expect("the Push Relay workflow");

    assert!(workflow.contains("tags: [\"push-relay-v*\"]"), "{workflow}");
    assert!(job(&workflow, "gate").contains("uses: ./.github/workflows/ci.yml"));

    let image = job(&workflow, "relay-image");
    assert!(image.contains("needs: gate"), "{image}");
    assert!(
        image.contains(
            "cargo xtask relay-image --platform \"$PLATFORM\" --tag \"$GITHUB_REF_NAME\""
        ),
        "{image}"
    );
    for runner in [
        "{ platform: amd64, runner: ubuntu-24.04 }",
        "{ platform: arm64, runner: ubuntu-24.04-arm }",
    ] {
        assert!(image.contains(runner), "{image}");
    }

    let manifest = job(&workflow, "relay-manifest");
    assert!(manifest.contains("needs: relay-image"), "{manifest}");
    assert!(
        manifest.contains("cargo xtask relay-image --manifest --tag \"$GITHUB_REF_NAME\""),
        "{manifest}"
    );
    assert!(
        manifest.contains("uses: actions/attest-build-provenance@"),
        "{manifest}"
    );
    assert!(
        manifest.contains("subject-name: ghcr.io/pagis-co/pagis-push-relay"),
        "{manifest}"
    );
}
