//! Publishing the agent computer image (`cargo xtask image`).
//!
//! The daemon pulls `pagis_versions::COMPUTER_IMAGE` from GHCR when it
//! starts or when an agent wakes, if the image is absent, so an
//! installation of Pagis works only while that exact tag is on the
//! registry. This module plans the
//! commands that put it there, for both architectures Pagis runs on,
//! and the release build reuses the same plan.
//!
//! The package is public, and a pushed layer of it cannot be recalled.
//! So the publish builds each architecture and exports its filesystem
//! first, scans the export for secrets and for known vulnerabilities,
//! and pushes that architecture only after both scans pass. The push is
//! by digest only, with no tag. Last, the manifest step joins the digest
//! of each architecture into one index under the tag. The release runs
//! each architecture on a runner of that architecture, so no build runs
//! under emulation.

use std::path::Path;

use anyhow::{Context, Result};

use crate::advisories::computer_vuln_scan_step;
use crate::release::DIST_DIR;
use crate::secrets::image_secret_scan_step;
use crate::{Action, Cmd, Step};

/// The label the image carries and the daemon compares with its own
/// pin before it boots a container.
pub const VERSION_LABEL: &str = "org.pagis.computer.version";

/// The buildx builder the publish runs on. The default `docker` driver
/// cannot export an image and push it by digest; the container driver
/// can.
pub const BUILDER: &str = "pagis";

/// An architecture that a release runs on: Linux amd64 and arm64. A
/// macOS host runs the image under its own Linux virtual machine, so it
/// needs the arm64 one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImagePlatform {
    Amd64,
    Arm64,
}

impl ImagePlatform {
    pub const ALL: [ImagePlatform; 2] = [ImagePlatform::Amd64, ImagePlatform::Arm64];

    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|platform| platform.arch() == name)
            .with_context(|| format!("{name} is not an image platform; use amd64 or arm64"))
    }

    /// The architecture, as `amd64`.
    pub fn arch(self) -> &'static str {
        match self {
            ImagePlatform::Amd64 => "amd64",
            ImagePlatform::Arm64 => "arm64",
        }
    }

    /// The platform as Docker names it, as `linux/amd64`.
    pub fn docker(self) -> &'static str {
        match self {
            ImagePlatform::Amd64 => "linux/amd64",
            ImagePlatform::Arm64 => "linux/arm64",
        }
    }
}

/// Make the builder, or find the one that is already there. Every image
/// publish starts with it, so it is one step shared by the Computer
/// image and the headless server image.
pub fn builder_step(root: &Path) -> Step {
    Step {
        name: "builder",
        action: Action::Run(vec![
            Cmd::new("sh", &["-c", &builder_script()]).in_dir(root),
        ]),
    }
}

/// The shell command that makes the builder, or finds it.
fn builder_script() -> String {
    format!(
        "docker buildx inspect {BUILDER} >/dev/null 2>&1 || \
         docker buildx create --name {BUILDER} --driver docker-container"
    )
}

/// The directory a publish exports the filesystem of the image `name`
/// for `platform` to, as `dist/image-fs/computer/linux_amd64`. The scans
/// read it.
pub fn export_dir(name: &str, platform: ImagePlatform) -> String {
    format!(
        "{DIST_DIR}/image-fs/{name}/{}",
        platform.docker().replace('/', "_")
    )
}

/// The build step `name` that exports the filesystem of an image to
/// `export`. `buildx` makes the build command from its `--output`
/// arguments. The build writes one tar archive, and the step unpacks it.
/// BuildKit sends a tar archive as one stream. Its `local` exporter sends
/// each file on its own stream, and that copy can deadlock with no
/// progress (moby/buildkit#2950).
pub(crate) fn export_step(
    root: &Path,
    name: &'static str,
    export: &str,
    buildx: impl Fn(&str) -> Cmd,
) -> Step {
    let archive = format!("{export}.tar");
    Step {
        name,
        action: Action::Run(vec![
            Cmd::new("rm", &["-rf", export, &archive]).in_dir(root),
            buildx(&format!("--output type=tar,dest={archive}")),
            Cmd::new("mkdir", &["-p", export]).in_dir(root),
            Cmd::new("tar", &["-xf", &archive, "-C", export]).in_dir(root),
            Cmd::new("rm", &["-f", &archive]).in_dir(root),
        ]),
    }
}

/// The file that holds the digest of the image `name` that the push of
/// `platform` wrote, as `dist/image-digests/computer-amd64.txt`.
pub fn digest_file(name: &str, platform: ImagePlatform) -> String {
    format!("{DIST_DIR}/image-digests/{name}-{}.txt", platform.arch())
}

/// The file that buildx writes the metadata of the push of `platform`
/// to. The digest of the push is in it.
fn metadata_file(name: &str, platform: ImagePlatform) -> String {
    format!("{DIST_DIR}/image-digests/{name}-{}.json", platform.arch())
}

/// The repository of `image`, with no tag.
fn repository(image: &str) -> &str {
    match image.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => repository,
        _ => image,
    }
}

/// The buildx arguments that push the image `name` for `platform` to the
/// repository of `image` by digest only, with no tag, and write the
/// metadata of the push.
pub(crate) fn push_by_digest_args(name: &str, image: &str, platform: ImagePlatform) -> String {
    format!(
        "--output type=image,name={},push-by-digest=true,name-canonical=true,push=true \
         --metadata-file {}",
        repository(image),
        metadata_file(name, platform)
    )
}

/// Read the digest of the push of `platform` from the metadata file of
/// buildx and write it to [`digest_file`]. The manifest step reads it.
pub(crate) fn record_digest_cmd(root: &Path, name: &str, platform: ImagePlatform) -> Cmd {
    let script = format!(
        "set -eu\n\
         digest=$(sed -n 's/.*\"containerimage.digest\": *\"\\([^\"]*\\)\".*/\\1/p' {metadata})\n\
         case \"$digest\" in sha256:[0-9a-f][0-9a-f]*) ;; *) echo '{metadata} holds no SHA-256 digest' >&2; exit 1 ;; esac\n\
         printf '%s\\n' \"$digest\" > {digest}\n",
        metadata = metadata_file(name, platform),
        digest = digest_file(name, platform),
    );
    Cmd::new("sh", &["-c", &script]).in_dir(root)
}

/// Join the digest of each platform of the image `name` into one index
/// under the tag `image`. A missing digest file stops the step and names
/// the file.
pub fn manifest_step(root: &Path, step: &'static str, name: &str, image: &str) -> Step {
    let mut script = String::from("set -eu\n");
    let mut sources = String::new();
    for platform in ImagePlatform::ALL {
        let file = digest_file(name, platform);
        script.push_str(&format!(
            "[ -s {file} ] || {{ echo '{file} is missing; push the {} image first' >&2; exit 1; }}\n",
            platform.docker()
        ));
        sources.push_str(&format!(" \"{}@$(cat {file})\"", repository(image)));
    }
    script.push_str(&format!(
        "docker buildx imagetools create -t {image}{sources}\n"
    ));
    Step {
        name: step,
        action: Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)]),
    }
}

/// Build the image under `computer/` for each architecture, export its
/// filesystem, scan the export for secrets and for known
/// vulnerabilities, and push that architecture by digest. Then join
/// both digests under `image`. `target_dir` keeps the downloads of
/// gitleaks and Trivy.
///
/// The push needs a Docker login to GHCR with `write:packages`.
pub fn image_plan(root: &Path, image: &str, target_dir: &Path) -> Vec<Step> {
    let mut steps = vec![builder_step(root)];
    for platform in ImagePlatform::ALL {
        steps.extend(computer_image_steps(root, image, platform, target_dir));
    }
    steps.push(computer_manifest_step(root, image));
    steps
}

/// The names of the build, the secret scan, the vulnerability scan and
/// the push of the Computer image for `platform`.
fn computer_step_names(platform: ImagePlatform) -> [&'static str; 4] {
    match platform {
        ImagePlatform::Amd64 => [
            "image-amd64",
            "image-secret-scan-amd64",
            "image-vuln-scan-amd64",
            "image-push-amd64",
        ],
        ImagePlatform::Arm64 => [
            "image-arm64",
            "image-secret-scan-arm64",
            "image-vuln-scan-arm64",
            "image-push-arm64",
        ],
    }
}

/// The steps of [`image_plan`] for one platform: the build, both scans
/// and the push by digest. The push builds again on the same builder
/// with the same inputs, so it takes each layer from the build cache of
/// the scanned build. The export holds gigabytes, so the push step
/// removes it after the push.
pub fn computer_image_steps(
    root: &Path,
    image: &str,
    platform: ImagePlatform,
    target_dir: &Path,
) -> Vec<Step> {
    let [build, secret_scan, vuln_scan, push] = computer_step_names(platform);
    let export = export_dir("computer", platform);
    let buildx = |output: &str| {
        let mut args = vec![
            "buildx",
            "build",
            "--builder",
            BUILDER,
            "--platform",
            platform.docker(),
        ];
        args.extend(output.split(' '));
        args.push("computer");
        Cmd::new("docker", &args).in_dir(root)
    };
    vec![
        export_step(root, build, &export, buildx),
        image_secret_scan_step(root, secret_scan, &export, target_dir),
        computer_vuln_scan_step(root, vuln_scan, std::slice::from_ref(&export), target_dir),
        Step {
            name: push,
            action: Action::Run(vec![
                Cmd::new("mkdir", &["-p", &format!("{DIST_DIR}/image-digests")]).in_dir(root),
                buildx(&push_by_digest_args("computer", image, platform)),
                record_digest_cmd(root, "computer", platform),
                Cmd::new("rm", &["-rf", &export]).in_dir(root),
            ]),
        },
    ]
}

/// Join the digests of the Computer image under its tag `image`.
pub fn computer_manifest_step(root: &Path, image: &str) -> Step {
    manifest_step(root, "image-manifest", "computer", image)
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
