//! The advisory checks. cargo-deny checks the Cargo lockfiles and npm
//! audit checks the npm lockfiles against the published advisories.
//! Trivy scans the filesystem of each image and of each Server Package
//! for known vulnerabilities before a publish pushes it. `deny.toml` and
//! `.trivyignore` hold each reviewed exception with its reason.
//!
//! The gate does not run these checks: `cargo xtask dev`, `cargo xtask
//! full` and the CI workflow leave them out, so a newly published
//! advisory does not block an unrelated pull request.
//! `cargo xtask advisories` runs the dependency checks and scans the
//! images of the latest release, and a daily workflow runs it.
//! `cargo xtask release` runs the dependency checks and the scan of the
//! Computer Image that the tree builds after the gate.
//!
//! Trivy runs its vulnerability scanner only, because gitleaks is the
//! secret scan. It reports the findings of high and critical severity
//! that have a fixed version, as npm audit checks at the high level. A
//! finding without a fixed version has no update to check, so it is not
//! an exception to review.

use std::path::Path;

use crate::image::{BUILDER, PLATFORMS, builder_script};
use crate::release::REPO;
use crate::server_image::SERVER_IMAGE_REPOSITORY;
use crate::{Action, Cmd, Lane, Step, tools};

/// The configuration of cargo-deny: the targets it checks, and each
/// ignored advisory with its reason.
pub const DENY_CONFIG: &str = "deny.toml";

/// The reviewed exceptions of the Trivy scans: one advisory ID on each
/// line, with a comment line above it that holds its reason.
pub const TRIVY_IGNORE: &str = ".trivyignore";

/// The options of every Trivy scan.
fn trivy_options() -> String {
    format!(
        "--scanners vuln --severity HIGH,CRITICAL --ignore-unfixed --ignorefile {TRIVY_IGNORE} \
         --exit-code 1 --no-progress"
    )
}

/// The advisory lane of `cargo xtask release`: the dependency checks, and
/// the scan of the Computer Image that this tree builds.
pub fn advisory_lane(root: &Path, target_dir: &Path, docker_available: bool) -> Lane {
    let mut steps = dependency_steps(root, target_dir);
    steps.push(image_scan_step(root, target_dir, docker_available));
    Lane {
        name: "advisories",
        steps,
    }
}

/// `cargo xtask advisories`, which the daily workflow runs: the
/// dependency checks, and the scan of the Computer Image and the Headless
/// Server image of the latest release, which is what people run.
pub fn published_advisory_lane(root: &Path, target_dir: &Path) -> Lane {
    let mut steps = dependency_steps(root, target_dir);
    steps.push(release_image_scan_step(root, target_dir));
    Lane {
        name: "advisories",
        steps,
    }
}

/// cargo-deny over the two Cargo lockfiles, and npm audit over the two
/// npm lockfiles.
fn dependency_steps(root: &Path, target_dir: &Path) -> Vec<Step> {
    vec![
        cargo_deny_step(root, target_dir),
        npm_audit_step(root, "ui", "ui-npm-audit"),
        npm_audit_step(root, "desktop", "desktop-npm-audit"),
    ]
}

/// Check the advisories of the lockfiles of the workspace and
/// `computer/screend` with the pinned cargo-deny. The check continues
/// after a lockfile fails, so one run reports every advisory.
///
/// One ignore list serves the two lockfiles. The check of the workspace
/// reports an ignored advisory that its lockfile does not hold, so a
/// stale exception shows. The lockfile of `computer/screend` holds only
/// some of the ignored advisories, so its check allows the others.
fn cargo_deny_step(root: &Path, target_dir: &Path) -> Step {
    // cargo-deny falls back to its default configuration when the file
    // is missing, and that has no reviewed exception.
    let body = format!(
        "[ -f {DENY_CONFIG} ] || {{ echo '{DENY_CONFIG} is missing' >&2; exit 1; }}\n\
         status=0\n\
         \"$work/cargo-deny\" --manifest-path Cargo.toml --config {DENY_CONFIG} check advisories || status=1\n\
         \"$work/cargo-deny\" --manifest-path computer/screend/Cargo.toml --config {DENY_CONFIG} \
         check --allow advisory-not-detected advisories || status=1\n\
         exit $status\n"
    );
    Step {
        name: "cargo-deny",
        action: Action::Run(vec![tools::command(
            root,
            target_dir,
            tools::cargo_deny(),
            &body,
            &[],
        )]),
    }
}

/// Check the advisories of the npm lockfile of `package` at the high
/// level. The audit reads the lockfile only, so it needs no
/// `node_modules`.
fn npm_audit_step(root: &Path, package: &str, name: &'static str) -> Step {
    let dir = root.join(package);
    let action = if dir.join("package-lock.json").exists() {
        Action::Run(vec![
            Cmd::new(
                "npm",
                &["audit", "--audit-level=high", "--package-lock-only"],
            )
            .in_dir(dir),
        ])
    } else {
        Action::Skip(format!("{package}/package-lock.json is not present"))
    };
    Step { name, action }
}

/// Scan each of `dirs`, the filesystem of an image or a Server Package,
/// with the pinned Trivy. The scan continues after a directory fails, so
/// one run reports every finding. Then print `manual`, the components
/// that Trivy does not identify, for a check by hand.
pub(crate) fn vuln_scan_step(
    root: &Path,
    name: &'static str,
    dirs: &[String],
    target_dir: &Path,
    manual: &[String],
) -> Step {
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    Step {
        name,
        action: Action::Run(vec![tools::command(
            root,
            target_dir,
            tools::trivy(),
            &rootfs_scan(manual),
            &dirs,
        )]),
    }
}

/// [`vuln_scan_step`] for the Computer Image, with the components of
/// `computer/Dockerfile` that Trivy does not identify.
pub(crate) fn computer_vuln_scan_step(
    root: &Path,
    name: &'static str,
    dirs: &[String],
    target_dir: &Path,
) -> Step {
    vuln_scan_step(root, name, dirs, target_dir, &computer_components(root))
}

/// The components of the Computer Image that Trivy does not identify, as
/// `<name> <version>`, from `dockerfile`, the text of
/// `computer/Dockerfile`: each component that it builds from a tagged
/// source checkout (`git clone --branch <tag> <url>/<name>.git`), and the
/// Node runtime (`ARG NODE_VERSION`), whose executable holds no package
/// metadata. Trivy identifies the npm packages that Node ships.
pub fn computer_image_components(dockerfile: &str) -> Vec<String> {
    let plain = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    };
    let mut components = Vec::new();
    for line in dockerfile.lines() {
        if let Some(version) = line.trim().strip_prefix("ARG NODE_VERSION=") {
            if plain(version) {
                components.push(format!("node {version}"));
            }
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if !words.windows(2).any(|pair| pair == ["git", "clone"]) {
            continue;
        }
        let Some(at) = words.iter().position(|word| *word == "--branch") else {
            continue;
        };
        let (Some(tag), Some(url)) = (words.get(at + 1), words.get(at + 2)) else {
            continue;
        };
        let name = url.rsplit('/').next().unwrap_or_default();
        let name = name.strip_suffix(".git").unwrap_or(name);
        if plain(name) && plain(tag) {
            components.push(format!("{name} {tag}"));
        }
    }
    components
}

fn computer_components(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("computer/Dockerfile"))
        .map(|dockerfile| computer_image_components(&dockerfile))
        .unwrap_or_default()
}

/// The body of a Trivy scan of each directory in `$@`.
fn rootfs_scan(manual: &[String]) -> String {
    let mut body = format!(
        "status=0\n\
         for dir in \"$@\"; do\n\
           \"$work/trivy\" rootfs --cache-dir \"$cache/db\" {options} \"$dir\" || status=1\n\
         done\n",
        options = trivy_options()
    );
    if !manual.is_empty() {
        body.push_str(
            "echo 'Trivy does not identify these components. Check each one by hand against its upstream advisories:'\n",
        );
        for component in manual {
            body.push_str(&format!("echo '  {component}'\n"));
        }
    }
    body.push_str("exit $status\n");
    body
}

/// Build the Computer Image from `computer/` for the platform of this
/// host, on the builder of the release, and scan its filesystem. The
/// builder takes the base images from the registry, as the release does,
/// so the scan reports what a release of this tree ships. The export is
/// a temporary directory that the step removes.
fn image_scan_step(root: &Path, target_dir: &Path, docker_available: bool) -> Step {
    if !docker_available {
        return Step {
            name: "image-scan",
            action: Action::Skip("Docker unavailable".into()),
        };
    }
    let body = format!(
        "docker buildx build --builder {BUILDER} --output type=local,dest=\"$work/computer\" computer\n\
         set -- \"$work/computer\"\n\
         {}",
        rootfs_scan(&computer_components(root))
    );
    Step {
        name: "image-scan",
        action: Action::Run(vec![
            Cmd::new("sh", &["-c", &builder_script()]).in_dir(root),
            tools::command(root, target_dir, tools::trivy(), &body, &[]),
        ]),
    }
}

/// Scan the Computer Image and the Headless Server image of the latest
/// release on the registry, for both architectures. The Runtime Lock of
/// the release names the digest of its Computer Image. A repository
/// without a release has no image to scan.
fn release_image_scan_step(root: &Path, target_dir: &Path) -> Step {
    let platforms = PLATFORMS.replace(',', " ");
    let body = format!(
        "if ! tag=$(gh api repos/{REPO}/releases/latest --jq .tag_name 2>\"$work/error\"); then\n\
           if grep -q 'HTTP 404' \"$work/error\"; then\n\
             echo '{REPO} has no release, so no published image is scanned'\n\
             exit 0\n\
           fi\n\
           cat \"$work/error\" >&2\n\
           exit 1\n\
         fi\n\
         computer=$(gh release download \"$tag\" --repo {REPO} --pattern runtime-lock-linux-x64.json --output - \
         | sed -n 's/.*\"computer_image\": *\"\\([^\"]*\\)\".*/\\1/p')\n\
         [ -n \"$computer\" ] || {{ echo \"the Runtime Lock of $tag names no Computer Image\" >&2; exit 1; }}\n\
         status=0\n\
         for image in \"$computer\" \"{SERVER_IMAGE_REPOSITORY}:${{tag#v}}\"; do\n\
           for platform in {platforms}; do\n\
             \"$work/trivy\" image --image-src remote --platform \"$platform\" --cache-dir \"$cache/db\" \
         {options} \"$image\" || status=1\n\
           done\n\
         done\n\
         exit $status\n",
        options = trivy_options()
    );
    Step {
        name: "release-image-scan",
        action: Action::Run(vec![tools::command(
            root,
            target_dir,
            tools::trivy(),
            &body,
            &[],
        )]),
    }
}
