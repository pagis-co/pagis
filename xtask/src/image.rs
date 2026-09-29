//! Publishing the agent computer image (`cargo xtask image`).
//!
//! The daemon pulls `pagis_versions::COMPUTER_IMAGE` from GHCR the
//! first time an agent wakes, so an installation of Pagis works only
//! while that exact tag is on the registry. This module plans the
//! commands that put it there, for both architectures Pagis runs on,
//! and the release build reuses the same plan.
//!
//! The package is public, and a pushed layer of it cannot be recalled.
//! So the publish builds the image and exports its filesystem first,
//! scans the export for secrets and for known vulnerabilities, and pushes
//! only after both scans pass.

use std::path::Path;

use crate::advisories::computer_vuln_scan_step;
use crate::release::DIST_DIR;
use crate::secrets::image_secret_scan_step;
use crate::{Action, Cmd, Step};

/// The label the image carries and the daemon compares with its own
/// pin before it boots a container.
pub const VERSION_LABEL: &str = "org.pagis.computer.version";

/// The buildx builder the publish runs on. The default `docker` driver
/// builds one architecture only; the container driver builds both.
pub const BUILDER: &str = "pagis";

/// The architectures a release runs on: Linux amd64 and arm64. A macOS
/// host runs the image under its own Linux virtual machine, so it
/// needs the arm64 one.
pub const PLATFORMS: &str = "linux/amd64,linux/arm64";

/// Make the multi-architecture builder, or find the one that is already
/// there. Every image publish starts with it, so it is one step shared
/// by the Computer image and the headless server image.
pub fn builder_step(root: &Path) -> Step {
    Step {
        name: "builder",
        action: Action::Run(vec![
            Cmd::new("sh", &["-c", &builder_script()]).in_dir(root),
        ]),
    }
}

/// The shell command that makes the builder, or finds it.
pub(crate) fn builder_script() -> String {
    format!(
        "docker buildx inspect {BUILDER} >/dev/null 2>&1 || \
         docker buildx create --name {BUILDER} --driver docker-container"
    )
}

/// The directory a publish exports the filesystem of the image `name`
/// to, one subdirectory for each platform. The secret scan reads it.
pub fn export_dir(name: &str) -> String {
    format!("{DIST_DIR}/image-fs/{name}")
}

/// The filesystem of each platform in `export`. A build for more than one
/// platform exports each one to a subdirectory named after it, as
/// `linux_amd64`.
pub fn platform_dirs(export: &str) -> Vec<String> {
    PLATFORMS
        .split(',')
        .map(|platform| format!("{export}/{}", platform.replace('/', "_")))
        .collect()
}

/// Build the image under `computer/` for both architectures, export its
/// filesystem, scan the export for secrets and for known
/// vulnerabilities, and push the image to the registry under `image`.
/// The push builds again on the same builder with the same inputs, so it
/// takes each layer from the build cache of the scanned build. The
/// export holds gigabytes, so the push step removes it after the push.
/// `target_dir` keeps the downloads of gitleaks and Trivy.
///
/// The push needs a Docker login to GHCR with `write:packages`.
pub fn image_plan(root: &Path, image: &str, target_dir: &Path) -> Vec<Step> {
    let mut steps = vec![builder_step(root)];
    steps.extend(computer_image_steps(root, image, target_dir));
    steps
}

/// The steps of [`image_plan`] after the builder: the build, both scans
/// and the push.
pub fn computer_image_steps(root: &Path, image: &str, target_dir: &Path) -> Vec<Step> {
    let export = export_dir("computer");
    let buildx = |output: &[&str]| {
        let mut args = vec![
            "buildx",
            "build",
            "--builder",
            BUILDER,
            "--platform",
            PLATFORMS,
        ];
        args.extend(output);
        args.push("computer");
        Cmd::new("docker", &args).in_dir(root)
    };
    vec![
        Step {
            name: "image",
            action: Action::Run(vec![
                Cmd::new("rm", &["-rf", &export]).in_dir(root),
                buildx(&["--output", &format!("type=local,dest={export}")]),
            ]),
        },
        image_secret_scan_step(root, "image-secret-scan", &export, target_dir),
        computer_vuln_scan_step(root, "image-vuln-scan", &platform_dirs(&export), target_dir),
        Step {
            name: "image-push",
            action: Action::Run(vec![
                buildx(&["-t", image, "--push"]),
                Cmd::new("rm", &["-rf", &export]).in_dir(root),
            ]),
        },
    ]
}

/// The shell function `anonymous_pull <image>`: pull the manifest of the
/// image the way a new person does, with an anonymous token from the
/// registry and no stored login. A registry that refuses stops the
/// script and names the image, because an installation that cannot pull
/// the image of its release cannot start a Computer or a server.
///
/// The image is `<registry>/<repository>:<tag>` or
/// `<registry>/<repository>@<digest>`, on a registry that serves the token
/// endpoint of GHCR at `https://<registry>/token`.
pub const ANONYMOUS_PULL_FN: &str = r#"anonymous_pull() {
  ref=$1
  case "$ref" in
    *@*) name=${ref%@*}; reference=${ref#*@} ;;
    *) name=${ref%:*}; reference=${ref##*:} ;;
  esac
  registry=${name%%/*}
  repository=${name#*/}
  token=$(curl -fsS "https://$registry/token?scope=repository:$repository:pull" \
    | sed -n 's/.*"token":"\([^"]*\)".*/\1/p') || token=''
  if [ -z "$token" ] || ! curl -fsS -o /dev/null \
    -H "Authorization: Bearer $token" \
    -H 'Accept: application/vnd.oci.image.index.v1+json' \
    -H 'Accept: application/vnd.docker.distribution.manifest.list.v2+json' \
    "https://$registry/v2/$repository/manifests/$reference"; then
    echo "$ref does not pull with no credentials; make the package public on the registry" >&2
    exit 1
  fi
}
"#;

/// Pull the manifest of each image with [`ANONYMOUS_PULL_FN`].
pub fn anonymous_pull_step(root: &Path, images: &[&str]) -> Step {
    let mut script = format!("set -eu\n{ANONYMOUS_PULL_FN}");
    for image in images {
        script.push_str(&format!("anonymous_pull '{image}'\n"));
    }
    Step {
        name: "anonymous-pull",
        action: Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)]),
    }
}

/// Whether the registry holds `image`, from `docker buildx imagetools
/// inspect`. A registry that answers "not found" does not hold it. Any
/// other failure, such as a network error or a refused login, is an
/// error, because an answer of `false` would push the image again over a
/// published version.
pub fn image_published(image: &str) -> anyhow::Result<bool> {
    let output = std::process::Command::new("docker")
        .args(["buildx", "imagetools", "inspect", image])
        .output()?;
    inspect_answer(
        image,
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// The answer of [`image_published`] from the exit status and the error
/// output of the inspect command.
pub fn inspect_answer(image: &str, success: bool, stderr: &str) -> anyhow::Result<bool> {
    if success {
        return Ok(true);
    }
    if stderr.to_ascii_lowercase().contains("not found") {
        return Ok(false);
    }
    anyhow::bail!(
        "cannot tell whether the registry holds {image}: {}",
        stderr.trim()
    )
}

/// The version a Dockerfile labels its image with, under `label`.
pub fn labelled_version(dockerfile: &str, label: &str) -> Option<String> {
    dockerfile.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("LABEL ")?;
        let value = rest.trim().strip_prefix(label)?.strip_prefix('=')?;
        Some(value.trim().trim_matches('"').to_string())
    })
}

/// Refuse a publish whose image would carry the wrong version. The
/// daemon compares the label of the Computer image it pulled with its
/// own pin and boots nothing on a mismatch, and the headless server
/// image carries the release it holds, so in both cases the
/// label and the version must agree before anything reaches GHCR.
pub fn check_pin(dockerfile: &str, file: &str, label: &str, pinned: &str) -> Result<(), String> {
    match labelled_version(dockerfile, label).as_deref() {
        Some(found) if found == pinned => Ok(()),
        Some(found) => Err(format!("{file} labels {found}; {pinned} is expected")),
        None => Err(format!("{file} carries no {label} label")),
    }
}
