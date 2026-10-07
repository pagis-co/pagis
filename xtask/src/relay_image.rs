//! Publishing the Push Relay image (`cargo xtask relay-image`).
//!
//! The Push Relay forwards a Web Push to APNs and FCM for the store apps
//! of the Mobile App (ADR-0030). It is not an artifact of the Release
//! Matrix: no installation runs it, and its routes start with `/v1/` and
//! stay compatible. So it has its own version, in
//! `crates/pagis-push-relay/Cargo.toml`, and its own tag,
//! `push-relay-v<version>`. `docs/PUSH-RELAY.md` holds the deployment
//! and the release.
//!
//! The image is built from `crates/pagis-push-relay/Dockerfile`, with the
//! repository root as its context, because the relay builds with the
//! lockfile of the workspace. The publish checks the advisories of that
//! lockfile first. Then, as for the Headless Server image, it exports the
//! filesystem of each architecture, scans it for secrets and for known
//! vulnerabilities, and pushes that architecture by digest only after
//! both scans pass. Last, it joins both digests under the version tag.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::advisories::{cargo_deny_step, vuln_scan_step};
use crate::image::{
    BUILDER, ImagePlatform, builder_step, check_pin, export_dir, export_step, manifest_step,
    push_by_digest_args, record_digest_cmd,
};
use crate::release::DIST_DIR;
use crate::secrets::image_secret_scan_step;
use crate::{Action, Cmd, Step};

/// The label the image carries, which says which version of the relay
/// is in it.
pub const VERSION_LABEL: &str = "org.pagis.push-relay.version";

/// The repository the image is published to. The tag is the version of
/// the relay crate.
pub const RELAY_IMAGE_REPOSITORY: &str = "ghcr.io/pagis-co/pagis-push-relay";

/// The Dockerfile of the image, relative to the repository root.
pub const DOCKERFILE: &str = "crates/pagis-push-relay/Dockerfile";

/// The manifest of the relay crate, which holds its version.
const MANIFEST: &str = "crates/pagis-push-relay/Cargo.toml";

/// The start of the tag of a relay release, as `push-relay-v0.1.0`.
pub const TAG_PREFIX: &str = "push-relay-v";

/// The name of the image in the export and digest paths.
const NAME: &str = "relay";

/// The published name of one version of the relay image.
pub fn relay_image(version: &str) -> String {
    format!("{RELAY_IMAGE_REPOSITORY}:{version}")
}

/// The version of the relay crate in the repository at `root`.
pub fn relay_version(root: &Path) -> Result<String> {
    let text = std::fs::read_to_string(root.join(MANIFEST))
        .with_context(|| format!("cannot read {MANIFEST}"))?;
    let manifest: toml::Table = text
        .parse()
        .with_context(|| format!("{MANIFEST} is not TOML"))?;
    manifest
        .get("package")
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("{MANIFEST} has no package.version"))
}

/// Refuse a tag that does not name `version`, as `push-relay-v1.2.3` for
/// `1.2.3`.
pub fn check_tag(tag: &str, version: &str) -> Result<()> {
    let expected = format!("{TAG_PREFIX}{version}");
    if tag != expected {
        bail!("tag {tag} does not name the Push Relay version; expected {expected}");
    }
    Ok(())
}

/// What one run of `cargo xtask relay-image` publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayImageScope {
    /// Each platform, then the manifest.
    All,
    /// One platform, which a runner of that architecture builds.
    Platform(ImagePlatform),
    /// The manifest of the digests that the platform runs pushed.
    Manifest,
}

impl RelayImageScope {
    /// The scope of the flags `--platform <platform>` and `--manifest`.
    pub fn from_flags(platform: Option<&str>, manifest: bool) -> Result<Self> {
        match (platform, manifest) {
            (Some(_), true) => bail!("--platform and --manifest cannot be used together"),
            (Some(name), false) => Ok(Self::Platform(ImagePlatform::parse(name)?)),
            (None, true) => Ok(Self::Manifest),
            (None, false) => Ok(Self::All),
        }
    }
}

/// The names of the build, the secret scan, the vulnerability scan and
/// the push of the relay image for `platform`.
fn step_names(platform: ImagePlatform) -> [&'static str; 4] {
    match platform {
        ImagePlatform::Amd64 => [
            "relay-image-amd64",
            "relay-image-secret-scan-amd64",
            "relay-image-vuln-scan-amd64",
            "relay-image-push-amd64",
        ],
        ImagePlatform::Arm64 => [
            "relay-image-arm64",
            "relay-image-secret-scan-arm64",
            "relay-image-vuln-scan-arm64",
            "relay-image-push-arm64",
        ],
    }
}

/// Build the image for `platform` and export its filesystem, scan the
/// export for secrets and for known vulnerabilities, and push that
/// platform to the repository of `image` by digest. The push builds
/// again on the same builder with the same inputs, so it takes each
/// layer from the build cache of the scanned build. The push step
/// removes the export after the push.
fn platform_steps(
    root: &Path,
    image: &str,
    platform: ImagePlatform,
    target_dir: &Path,
) -> Vec<Step> {
    let [build, secret_scan, vuln_scan, push] = step_names(platform);
    let export = export_dir(NAME, platform);
    let buildx = |output: &str| {
        let mut args = vec![
            "buildx",
            "build",
            "--builder",
            BUILDER,
            "--platform",
            platform.docker(),
            "--file",
            DOCKERFILE,
        ];
        args.extend(output.split(' '));
        args.push(".");
        Cmd::new("docker", &args).in_dir(root)
    };
    vec![
        export_step(root, build, &export, buildx),
        image_secret_scan_step(root, secret_scan, &export, target_dir),
        // Trivy identifies the crates of the relay and the Debian
        // packages.
        vuln_scan_step(
            root,
            vuln_scan,
            std::slice::from_ref(&export),
            target_dir,
            &[],
        ),
        Step {
            name: push,
            action: Action::Run(vec![
                Cmd::new("mkdir", &["-p", &format!("{DIST_DIR}/image-digests")]).in_dir(root),
                buildx(&push_by_digest_args(NAME, image, platform)),
                record_digest_cmd(root, NAME, platform),
                Cmd::new("rm", &["-rf", &export]).in_dir(root),
            ]),
        },
    ]
}

/// The steps of `scope`. A build of a platform starts with the advisory
/// check of the Cargo lockfile and the builder. The manifest joins the
/// digests under the tag `image`. `target_dir` keeps the downloads of
/// cargo-deny, gitleaks and Trivy.
pub fn relay_image_plan(
    root: &Path,
    image: &str,
    scope: RelayImageScope,
    target_dir: &Path,
) -> Vec<Step> {
    let manifest = || manifest_step(root, "relay-image-manifest", NAME, image);
    let platforms = match scope {
        RelayImageScope::Manifest => return vec![manifest()],
        RelayImageScope::Platform(platform) => vec![platform],
        RelayImageScope::All => ImagePlatform::ALL.to_vec(),
    };
    let mut steps = vec![cargo_deny_step(root, target_dir), builder_step(root)];
    for platform in platforms {
        steps.extend(platform_steps(root, image, platform, target_dir));
    }
    if scope == RelayImageScope::All {
        steps.push(manifest());
    }
    steps
}

/// The image and the steps of a publish of the relay at `root`. The
/// publish is refused when `tag` does not name the crate version, or
/// when the label of the Dockerfile is not the crate version: the tag,
/// the label and the crate version are one claim.
pub fn publish_plan(
    root: &Path,
    scope: RelayImageScope,
    tag: Option<&str>,
    target_dir: &Path,
) -> Result<(String, Vec<Step>)> {
    let version = relay_version(root)?;
    if let Some(tag) = tag {
        check_tag(tag, &version)?;
    }
    let dockerfile = std::fs::read_to_string(root.join(DOCKERFILE))
        .with_context(|| format!("cannot read {DOCKERFILE}"))?;
    if let Err(reason) = check_pin(&dockerfile, DOCKERFILE, VERSION_LABEL, &version) {
        bail!("the Push Relay image publish is refused: {reason}");
    }
    let image = relay_image(&version);
    let steps = relay_image_plan(root, &image, scope, target_dir);
    Ok((image, steps))
}
