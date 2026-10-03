//! Tests for publishing the agent computer image
//! (`cargo xtask image`) and for the pins the publish depends on.

use std::path::Path;

use xtask::advisories::computer_image_components;
use xtask::{
    Action, Cmd, ImagePlatform, Step, VERSION_LABEL, check_pin, image_plan, labelled_version,
};

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

const IMAGE: &str = "ghcr.io/pagis-co/pagis-computer:0.3.0";

fn plan(root: &Path) -> Vec<Step> {
    image_plan(root, IMAGE, Path::new("/shared/pagis-target"))
}

fn step<'a>(steps: &'a [Step], name: &str) -> &'a Step {
    steps
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no step {name}"))
}

/// Each architecture builds on its own, is scanned, and is pushed by its
/// digest only. Last, one step joins both digests into the pinned tag.
#[test]
fn the_publish_pushes_each_architecture_then_joins_them_under_the_pinned_tag() {
    let steps = plan(&workspace_root());
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "builder",
            "image-amd64",
            "image-secret-scan-amd64",
            "image-vuln-scan-amd64",
            "image-push-amd64",
            "image-arm64",
            "image-secret-scan-arm64",
            "image-vuln-scan-arm64",
            "image-push-arm64",
            "image-manifest",
        ]
    );
    let manifest = joined(&steps[9..]);
    assert!(
        manifest.contains(&format!("docker buildx imagetools create -t {IMAGE}")),
        "{manifest}"
    );
}

#[test]
fn the_publish_runs_on_a_builder_that_can_export_and_push() {
    // The default `docker` driver cannot export an image and push it by
    // digest, so the plan makes the container driver's builder first
    // and names it on each build. Making it twice is not an error.
    let steps = plan(Path::new("/repo"));
    let script = joined(&steps);
    assert!(script.contains("docker-container"), "{script}");
    let build = joined(std::slice::from_ref(step(&steps, "image-amd64")));
    assert!(build.contains("--builder pagis"), "{build}");
    assert!(
        script.find("docker-container") < script.find("--builder pagis"),
        "the builder is made before the build: {script}"
    );
}

/// The package is public, so the publish scans the exported filesystem
/// of each architecture for secrets and pushes it only after a clean
/// scan.
#[test]
fn the_publish_scans_each_architecture_for_secrets_before_it_pushes() {
    let steps = plan(&workspace_root());
    for platform in ImagePlatform::ALL {
        let arch = platform.arch();
        let export = format!("dist/image-fs/computer/linux_{arch}");
        let build = joined(std::slice::from_ref(step(&steps, &format!("image-{arch}"))));
        assert!(!build.contains("push=true"), "{build}");
        assert!(
            build.contains(&format!("--platform linux/{arch} ")),
            "{build}"
        );
        assert!(
            build.contains(&format!("type=tar,dest={export}.tar computer | ")),
            "the context is computer/: {build}"
        );
        assert!(
            build.contains(&format!("tar -xf {export}.tar -C {export}")),
            "{build}"
        );
        assert!(!build.contains("type=local"), "{build}");
        let scan = joined(std::slice::from_ref(step(
            &steps,
            &format!("image-secret-scan-{arch}"),
        )));
        assert!(scan.contains("gitleaks"), "{scan}");
        assert!(scan.contains(&export), "{scan}");
    }
}

/// After the secret scan and before the push, Trivy scans the export of
/// each architecture for known vulnerabilities, and the scan prints the
/// pinned components that Trivy does not identify.
#[test]
fn the_publish_scans_each_architecture_for_vulnerabilities_before_it_pushes() {
    let steps = plan(&workspace_root());
    for platform in ImagePlatform::ALL {
        let arch = platform.arch();
        let scan = joined(std::slice::from_ref(step(
            &steps,
            &format!("image-vuln-scan-{arch}"),
        )));
        assert!(scan.contains("trivy"), "{scan}");
        assert!(scan.contains("--scanners vuln"), "{scan}");
        assert!(scan.contains("rootfs"), "{scan}");
        assert!(
            scan.contains(&format!("dist/image-fs/computer/linux_{arch}")),
            "{scan}"
        );
        for component in computer_image_components(&dockerfile()) {
            assert!(scan.contains(&component), "{component}: {scan}");
        }
    }
}

/// Each architecture is pushed by its digest only, with no tag, after
/// its own scans. The push builds again on the same builder for the same
/// platform, so it takes each layer from the build cache of the scanned
/// build. It records the digest for the manifest and removes the export.
#[test]
fn each_platform_is_pushed_by_digest_after_its_own_scans() {
    let steps = plan(Path::new("/repo"));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    let at = |name: String| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("no step {name} in {names:?}"))
    };
    for platform in ImagePlatform::ALL {
        let arch = platform.arch();
        let push = at(format!("image-push-{arch}"));
        assert!(at(format!("image-secret-scan-{arch}")) < push, "{names:?}");
        assert!(at(format!("image-vuln-scan-{arch}")) < push, "{names:?}");

        let pushed = joined(std::slice::from_ref(&steps[push]));
        assert!(
            pushed.contains(&format!(
                "--builder pagis --platform linux/{arch} --output \
                 type=image,name=ghcr.io/pagis-co/pagis-computer,push-by-digest=true,name-canonical=true,push=true \
                 --metadata-file dist/image-digests/computer-{arch}.json computer"
            )),
            "{pushed}"
        );
        assert!(!pushed.contains(IMAGE), "no tag is pushed: {pushed}");
        assert!(
            pushed.contains(&format!("dist/image-digests/computer-{arch}.txt")),
            "{pushed}"
        );
        assert!(
            pushed.ends_with(&format!("rm -rf dist/image-fs/computer/linux_{arch}")),
            "the export is removed after the push: {pushed}"
        );
    }
}

/// A fake `docker` on the `PATH` of `cmd`, which logs its arguments to
/// `docker.log` and runs `body`.
#[cfg(unix)]
fn with_fake_docker(dir: &Path, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;

    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let docker = bin.join("docker");
    std::fs::write(
        &docker,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\n{body}",
            dir.join("docker.log").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

#[cfg(unix)]
fn run_step(step: &Step, path: &str) -> (bool, String) {
    let mut output = String::new();
    for cmd in commands(step) {
        let result = std::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(cmd.cwd.as_ref().unwrap())
            .env("PATH", path)
            .output()
            .unwrap();
        output.push_str(&String::from_utf8_lossy(&result.stderr));
        if !result.status.success() {
            return (false, output);
        }
    }
    (true, output)
}

/// The push reads the digest of the platform from the metadata file of
/// buildx and writes it to the digest file, and a push with no digest
/// fails.
#[cfg(unix)]
#[test]
fn the_push_records_the_digest_of_its_platform() {
    let digest = format!("sha256:{}", "a".repeat(64));
    // The fake build writes the metadata file that it is given.
    let body = "while [ $# -gt 0 ]; do\n  if [ \"$1\" = --metadata-file ]; then\n    \
         printf '{\\n  \"containerimage.digest\": \"%s\"\\n}\\n' \"$DIGEST\" > \"$2\"\n  fi\n  shift\n\
         done\n";

    let dir = tempfile::tempdir().unwrap();
    let path = with_fake_docker(dir.path(), &body.replace("$DIGEST", &digest));
    let steps = plan(dir.path());
    let (ok, output) = run_step(step(&steps, "image-push-arm64"), &path);
    assert!(ok, "{output}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("dist/image-digests/computer-arm64.txt")).unwrap(),
        format!("{digest}\n")
    );

    let dir = tempfile::tempdir().unwrap();
    let path = with_fake_docker(dir.path(), &body.replace("$DIGEST", "latest"));
    let steps = plan(dir.path());
    let (ok, output) = run_step(step(&steps, "image-push-arm64"), &path);
    assert!(!ok, "a push with no digest must fail");
    assert!(output.contains("SHA-256 digest"), "{output}");
}

/// The manifest joins the digest of each platform into one index under
/// the pinned tag, and it names the digest file that is missing.
#[cfg(unix)]
#[test]
fn the_manifest_joins_both_digests_into_the_tag() {
    let dir = tempfile::tempdir().unwrap();
    let path = with_fake_docker(dir.path(), "");
    let steps = plan(dir.path());
    let manifest = step(&steps, "image-manifest");

    let digests = dir.path().join("dist/image-digests");
    std::fs::create_dir_all(&digests).unwrap();
    std::fs::write(
        digests.join("computer-amd64.txt"),
        format!("sha256:{}\n", "a".repeat(64)),
    )
    .unwrap();
    let (ok, output) = run_step(manifest, &path);
    assert!(!ok, "a missing digest must fail");
    assert!(
        output.contains("dist/image-digests/computer-arm64.txt"),
        "{output}"
    );

    std::fs::write(
        digests.join("computer-arm64.txt"),
        format!("sha256:{}\n", "b".repeat(64)),
    )
    .unwrap();
    let (ok, output) = run_step(manifest, &path);
    assert!(ok, "{output}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("docker.log")).unwrap(),
        format!(
            "buildx imagetools create -t {IMAGE} \
             ghcr.io/pagis-co/pagis-computer@sha256:{} \
             ghcr.io/pagis-co/pagis-computer@sha256:{}\n",
            "a".repeat(64),
            "b".repeat(64)
        )
    );
}

#[test]
fn each_image_platform_parses_from_its_architecture() {
    for platform in ImagePlatform::ALL {
        assert_eq!(ImagePlatform::parse(platform.arch()).unwrap(), platform);
        assert_eq!(platform.docker(), format!("linux/{}", platform.arch()));
    }
    assert_eq!(
        ImagePlatform::ALL.map(ImagePlatform::arch),
        ["amd64", "arm64"]
    );
    assert!(ImagePlatform::parse("linux/amd64").is_err());
}

#[test]
fn every_publish_command_runs_at_the_workspace_root() {
    let steps = plan(Path::new("/repo"));
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

#[test]
fn the_final_stage_takes_the_debian_fixes_of_its_base() {
    let dockerfile = dockerfile();
    let image = &dockerfile[dockerfile.rfind("\nFROM ").expect("a final stage")..];
    let update = image.find("apt-get update").expect("an apt-get update");
    let upgrade = image
        .find("apt-get upgrade -y")
        .expect("an apt-get upgrade");
    let clean = image
        .find("rm -rf /var/lib/apt/lists/*")
        .expect("the package lists removed");

    assert!(update < upgrade && upgrade < clean, "{image}");
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
