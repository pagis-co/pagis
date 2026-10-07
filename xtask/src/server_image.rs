//! Publishing the headless Linux server image
//! (`cargo xtask server-image`), which is the fourth release artifact.
//!
//! The client-installed server is a signed macOS disk image, and this
//! is the same daemon for a team's own VM. It carries the Product App
//! and the Administration Interface it serves, the `gog` runner beside
//! it, and the programs the daemon starts, so a VM needs Docker and
//! nothing else. `docs/RELEASING-SERVER.md` holds the release matrix
//! and https://docs.pagis.co/server holds the deployment.
//!
//! The image is built from the `Dockerfile` at the repository root,
//! with the whole repository as its context: the bundle it embeds is
//! built from `ui/` in a stage of its own.
//!
//! As for the Computer image, the publish exports the filesystem of
//! each architecture first, scans it for secrets and for known
//! vulnerabilities, and pushes that architecture by digest only after
//! both scans pass. Then it joins both digests under the release tag.

use std::path::Path;

use crate::advisories::vuln_scan_step;
use crate::image::{
    BUILDER, ImagePlatform, builder_step, export_dir, export_step, manifest_step,
    push_by_digest_args, record_digest_cmd,
};
use crate::release::DIST_DIR;
use crate::secrets::image_secret_scan_step;
use crate::{Action, Cmd, Step};

/// The label the image carries, which says which release is in it.
pub const VERSION_LABEL: &str = "co.pagis.server.version";

/// The repository the image is published to. The tag is the release, so
/// the server image and the server package of one release carry the
/// same number, and `docs/RELEASING-SERVER.md` states that rule.
pub const SERVER_IMAGE_REPOSITORY: &str = "ghcr.io/pagis-co/pagis-server";

/// The PostHog project of a release build (ADR-0026). A build argument
/// with no value takes the variable of the same name from the
/// environment, and one that the environment does not set stays empty,
/// so the daemon in the image sends no analytics.
const ANALYTICS_BUILD_ARGS: &str =
    "--build-arg PAGIS_POSTHOG_PROJECT_ID --build-arg PAGIS_POSTHOG_TOKEN";

/// The published name of one release's server image.
pub fn server_image(version: &str) -> String {
    format!("{SERVER_IMAGE_REPOSITORY}:{version}")
}

/// The names of the build, the secret scan, the vulnerability scan and
/// the push of the server image for `platform`.
fn step_names(platform: ImagePlatform) -> [&'static str; 4] {
    match platform {
        ImagePlatform::Amd64 => [
            "server-image-amd64",
            "server-image-secret-scan-amd64",
            "server-image-vuln-scan-amd64",
            "server-image-push-amd64",
        ],
        ImagePlatform::Arm64 => [
            "server-image-arm64",
            "server-image-secret-scan-arm64",
            "server-image-vuln-scan-arm64",
            "server-image-push-arm64",
        ],
    }
}

/// Build the image for `platform` and export its filesystem, scan the
/// export for secrets and for known vulnerabilities, and push that
/// platform to the repository of `image` by digest. The push builds
/// again on the same builder with the same inputs, so it takes each
/// layer from the build cache of the scanned build. The push step
/// removes the export after the push.
///
/// `computer_digest_file` names the file the release's `image-digest`
/// step wrote, which holds the immutable Computer image reference the
/// server is built against. Without it the server keeps the tag
/// `pagis-versions` pins, which is what a build outside a release
/// wants. `target_dir` keeps the downloads of gitleaks and Trivy.
pub fn server_image_steps(
    root: &Path,
    image: &str,
    platform: ImagePlatform,
    computer_digest_file: Option<&str>,
    target_dir: &Path,
) -> Vec<Step> {
    let [build, secret_scan, vuln_scan, push] = step_names(platform);
    let export = export_dir("server", platform);
    let docker_platform = platform.docker();
    let buildx = |output: &str| {
        let script = match computer_digest_file {
            Some(file) => format!(
                "set -eu\ncomputer=$(cat {file})\ndocker buildx build --builder {BUILDER} \
                 --platform {docker_platform} --build-arg PAGIS_COMPUTER_IMAGE=\"$computer\" \
                 {ANALYTICS_BUILD_ARGS} {output} .\n"
            ),
            None => format!(
                "set -eu\ndocker buildx build --builder {BUILDER} --platform {docker_platform} \
                 {ANALYTICS_BUILD_ARGS} {output} .\n"
            ),
        };
        Cmd::new("sh", &["-c", &script]).in_dir(root)
    };
    vec![
        export_step(root, build, &export, buildx),
        image_secret_scan_step(root, secret_scan, &export, target_dir),
        // Trivy identifies the crates of `pagis`, `gog` and the Debian
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
                buildx(&push_by_digest_args("server", image, platform)),
                record_digest_cmd(root, "server", platform),
                Cmd::new("rm", &["-rf", &export]).in_dir(root),
            ]),
        },
    ]
}

/// Join the digests of the server image under its tag `image`.
pub fn server_manifest_step(root: &Path, image: &str) -> Step {
    manifest_step(root, "server-image-manifest", "server", image)
}

/// `cargo xtask server-image`: the builder, the build, the scans and the
/// push of each platform, then the manifest, as `cargo xtask image` does
/// for the Computer image.
pub fn server_image_plan(root: &Path, image: &str, target_dir: &Path) -> Vec<Step> {
    let mut steps = vec![builder_step(root)];
    for platform in ImagePlatform::ALL {
        steps.extend(server_image_steps(root, image, platform, None, target_dir));
    }
    steps.push(server_manifest_step(root, image));
    steps
}
