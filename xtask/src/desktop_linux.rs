//! The Linux Client App packaging (`cargo xtask desktop` on a Linux host,
//! or with `--linux`). It builds an AppImage and a deb package for amd64
//! and arm64 with electron-builder, checks what they embed, and smoke
//! tests the package of the host's architecture.
//!
//! Linux has no platform notary. A tagged release prepares the exact
//! packages, waits for the distribution proof from a clean Linux
//! machine, and then publishes those bytes with a checksum list that the
//! Pagis release key signs (ADR-0025). The key never enters the plan:
//! `gpg` reads it from the agent of the person who publishes.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::desktop::DesktopContext;
use crate::image::ANONYMOUS_PULL_FN;
use crate::release::{ClientPlatform, REPO};
use crate::{Action, Cmd, Step};

/// The public half of the Pagis release key, in the repository. A
/// person checks the checksum signature with it, and publication checks
/// the signature against this exact file.
pub const RELEASE_KEY: &str = "docs/release-key.asc";

/// The AppImage electron-builder writes for one architecture. It names
/// amd64 `x86_64`, as AppImage tools do.
pub fn appimage_name(version: &str, platform: ClientPlatform) -> String {
    let arch = match platform {
        ClientPlatform::LinuxX64 => "x86_64",
        _ => "arm64",
    };
    format!("Pagis-{version}-{arch}.AppImage")
}

/// The deb package electron-builder writes for one architecture, with
/// Debian's architecture name.
pub fn deb_name(version: &str, platform: ClientPlatform) -> String {
    let arch = match platform {
        ClientPlatform::LinuxX64 => "amd64",
        _ => "arm64",
    };
    format!("Pagis-{version}-{arch}.deb")
}

/// The checksum list of the four Linux packages. Its detached signature
/// is this name with `.asc`.
pub fn checksums_name(version: &str) -> String {
    format!("Pagis-{version}-linux.SHA256SUMS")
}

/// The directory electron-builder unpacks one architecture into. The
/// default architecture, amd64, carries no suffix.
fn unpacked_dir(platform: ClientPlatform) -> &'static str {
    match platform {
        ClientPlatform::LinuxX64 => "release/linux-unpacked",
        _ => "release/linux-arm64-unpacked",
    }
}

/// The name of the Client App's executable in the package. It is not
/// `pagis`, which is the server's own command.
pub const EXECUTABLE: &str = "pagis-client";

/// The inputs a Linux publication needs that the environment does not
/// carry: the fingerprint of the release key in the local `gpg` agent.
pub fn missing_signing_inputs(is_set: &dyn Fn(&str) -> bool) -> Vec<String> {
    if is_set("PAGIS_RELEASE_GPG_KEY") {
        Vec::new()
    } else {
        vec!["PAGIS_RELEASE_GPG_KEY".into()]
    }
}

pub fn linux_plan(root: &Path, cx: &DesktopContext) -> Vec<Step> {
    let desktop = root.join("desktop");
    let reuse = "publishing the exact packages that already passed external proof";
    let validate_locks = ClientPlatform::LINUX
        .into_iter()
        .map(|platform| {
            Cmd::new(
                "cargo",
                &[
                    "run",
                    "--quiet",
                    "-p",
                    "xtask",
                    "--",
                    "runtime-lock",
                    "validate-metadata",
                    &format!("dist/{}", platform.lock_file()),
                    &cx.version,
                    &platform.name(),
                ],
            )
            .in_dir(root)
        })
        .collect();
    let build = |command: Cmd| {
        if cx.publish_existing {
            Action::Skip(reuse.into())
        } else {
            Action::Run(vec![command])
        }
    };
    vec![
        Step {
            name: "credentials",
            action: credentials_action(root, cx),
        },
        Step {
            name: "runtime-locks",
            action: Action::Run(validate_locks),
        },
        Step {
            name: "deps",
            action: build(Cmd::new("npm", &["ci"]).in_dir(desktop.clone())),
        },
        Step {
            name: "pack",
            action: build(Cmd::new("npm", &["run", "pack:linux"]).in_dir(desktop.clone())),
        },
        Step {
            name: "inventory",
            action: build(Cmd::new("sh", &["-c", &inventory_script(&cx.version)]).in_dir(&desktop)),
        },
        Step {
            name: "packaged-runtime",
            action: build(
                Cmd::new(
                    "node",
                    &[
                        "scripts/check-packaged-runtime.mjs",
                        unpacked_dir(ClientPlatform::LinuxX64),
                    ],
                )
                .in_dir(&desktop),
            ),
        },
        Step {
            name: "smoke",
            action: build(Cmd::new("sh", &["-c", SMOKE_SCRIPT]).in_dir(&desktop)),
        },
        Step {
            name: "checksums",
            action: build(Cmd::new("sh", &["-c", &checksums_script(&cx.version)]).in_dir(&desktop)),
        },
        Step {
            name: "distribution-proof",
            action: distribution_proof_action(root, cx),
        },
        Step {
            name: "released-tuple",
            action: released_tuple_action(root, cx),
        },
        Step {
            name: "sign-checksums",
            action: sign_action(root, cx),
        },
        Step {
            name: "publish",
            action: publish_action(root, cx),
        },
    ]
}

/// Check both architectures: the unpacked tree carries the client alone
/// and the lock of its architecture, and the finished deb carries the
/// same lock. The deb is checked from its exact bytes, which `dpkg-deb`
/// can read on any architecture.
fn inventory_script(version: &str) -> String {
    let mut script = "set -eu\n".to_string();
    for platform in ClientPlatform::LINUX {
        let unpacked = unpacked_dir(platform);
        let lock = format!("../dist/{}", platform.lock_file());
        let appimage = format!("release/{}", appimage_name(version, platform));
        let deb = format!("release/{}", deb_name(version, platform));
        script.push_str(&format!(
            "cmp {lock} {unpacked}/resources/runtime-lock.json\n\
             node scripts/check-package.mjs {unpacked} {appimage} {deb}\n\
             extracted=$(mktemp -d)\n\
             dpkg-deb -x {deb} \"$extracted\"\n\
             cmp {lock} \"$extracted/opt/Pagis/resources/runtime-lock.json\"\n\
             test -x \"$extracted/opt/Pagis/{EXECUTABLE}\"\n\
             test -f \"$extracted/opt/Pagis/resources/apparmor-profile\"\n\
             rm -rf \"$extracted\"\n",
        ));
    }
    script
}

/// Start the client of the host's architecture with no installed server.
/// It must open its local setup page. A machine with no display runs it
/// under a virtual one.
const SMOKE_SCRIPT: &str = concat!(
    "set -eu\n",
    "case \"$(uname -m)\" in\n",
    "  x86_64) app=release/linux-unpacked ;;\n",
    "  aarch64|arm64) app=release/linux-arm64-unpacked ;;\n",
    "  *) echo \"no Linux client for $(uname -m)\" >&2; exit 1 ;;\n",
    "esac\n",
    "[ -x \"$app/pagis-client\" ] || { echo \"the packaged client is missing from $app\" >&2; exit 1; }\n",
    "if [ -z \"${DISPLAY:-}\" ] && [ -z \"${WAYLAND_DISPLAY:-}\" ]; then\n",
    "  xvfb-run -a \"$app/pagis-client\" --smoke\n",
    "else\n",
    "  \"$app/pagis-client\" --smoke\n",
    "fi\n",
);

/// The checksum list of the four packages, beside them. A tag signs it
/// at publication; a routine build keeps it as the record of what it
/// built.
fn checksums_script(version: &str) -> String {
    let mut files = Vec::new();
    for platform in ClientPlatform::LINUX {
        files.push(appimage_name(version, platform));
        files.push(deb_name(version, platform));
    }
    format!(
        "set -eu\ncd release\nsha256sum {} > {}\n",
        files.join(" "),
        checksums_name(version)
    )
}

fn credentials_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() || !cx.publish_existing {
        return Action::Skip("only publication signs, with the release key".into());
    }
    if cx.missing_credentials.is_empty() {
        return Action::Run(vec![Cmd::new("sh", &["-c", "true"]).in_dir(root)]);
    }
    fail_action(
        root,
        &format!(
            "Linux publication needs {}",
            cx.missing_credentials.join("; ")
        ),
    )
}

fn distribution_proof_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: distribution proof is not required".into());
    }
    if cx.prepare_only {
        return Action::Skip("prepared packages await external distribution proof".into());
    }
    let commands = ClientPlatform::LINUX
        .into_iter()
        .map(|platform| {
            Cmd::new(
                "cargo",
                &[
                    "run",
                    "--quiet",
                    "-p",
                    "xtask",
                    "--",
                    "distribution-proof",
                    "validate-linux",
                    &format!("dist/distribution-proof-{}.json", platform.name()),
                    &format!("dist/{}", platform.lock_file()),
                    &format!("desktop/release/{}", appimage_name(&cx.version, platform)),
                    &format!("desktop/release/{}", deb_name(&cx.version, platform)),
                    &cx.version,
                ],
            )
            .in_dir(root)
        })
        .collect();
    Action::Run(commands)
}

/// The published server tuple must equal the locks the packages embed:
/// the lock files, the archives they name and the Computer image.
fn released_tuple_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: no published server tuple is required".into());
    };
    if cx.prepare_only {
        return Action::Skip("prepared packages await external distribution proof".into());
    }
    let mut script =
        format!("set -eu\n{ANONYMOUS_PULL_FN}tmp=$(mktemp -d)\ntrap 'rm -rf \"$tmp\"' EXIT\n");
    for platform in ClientPlatform::LINUX {
        let lock = platform.lock_file();
        let archive = platform.server_package(&cx.version);
        script.push_str(&format!(
            "gh release download {tag} --repo {REPO} --dir \"$tmp\" --pattern {lock} --pattern {archive}\n\
             cmp dist/{lock} \"$tmp/{lock}\"\n\
             locked=$(node -e \"process.stdout.write(require('./dist/{lock}').asset.sha256)\")\n\
             actual=$(shasum -a 256 \"$tmp/{archive}\" | awk '{{print $1}}')\n\
             [ \"$actual\" = \"$locked\" ] || {{ echo 'the published {archive} does not match its Runtime Lock' >&2; exit 1; }}\n\
             image=$(node -e \"process.stdout.write(require('./dist/{lock}').computer_image)\")\n\
             anonymous_pull \"$image\"\n",
        ));
    }
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// Sign the checksum list with the release key, then prove the signature
/// with the key the repository publishes and prove each listed hash.
fn sign_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(_) = &cx.tag else {
        return Action::Skip("no release tag: nothing is signed".into());
    };
    if cx.prepare_only {
        return Action::Skip("prepared packages await external distribution proof".into());
    }
    let sums = format!("desktop/release/{}", checksums_name(&cx.version));
    let script = format!(
        "set -eu\n\
         test -f {RELEASE_KEY} || {{ echo 'the release key {RELEASE_KEY} is missing; publication needs it' >&2; exit 1; }}\n\
         : \"${{PAGIS_RELEASE_GPG_KEY:?PAGIS_RELEASE_GPG_KEY names the release key}}\"\n\
         (cd desktop/release && sha256sum -c {name})\n\
         rm -f {sums}.asc\n\
         gpg --batch --yes --armor --detach-sign --local-user \"$PAGIS_RELEASE_GPG_KEY\" --output {sums}.asc {sums}\n\
         keyring=$(mktemp)\n\
         trap 'rm -f \"$keyring\"' EXIT\n\
         gpg --dearmor < {RELEASE_KEY} > \"$keyring\"\n\
         gpgv --keyring \"$keyring\" {sums}.asc {sums}\n",
        name = checksums_name(&cx.version),
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

fn publish_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: the packages are built and smoke tested only".into());
    };
    if cx.prepare_only {
        return Action::Skip("prepared packages await external distribution proof".into());
    }
    if !cx.missing_credentials.is_empty() {
        return fail_action(
            root,
            &format!(
                "Linux publication needs {}",
                cx.missing_credentials.join("; ")
            ),
        );
    }
    let mut files = Vec::new();
    for platform in ClientPlatform::LINUX {
        files.push(format!(
            "desktop/release/{}",
            appimage_name(&cx.version, platform)
        ));
        files.push(format!(
            "desktop/release/{}",
            deb_name(&cx.version, platform)
        ));
    }
    let sums = format!("desktop/release/{}", checksums_name(&cx.version));
    files.push(sums.clone());
    files.push(format!("{sums}.asc"));
    let mut args = vec!["release", "upload", tag.as_str()];
    args.extend(files.iter().map(String::as_str));
    args.extend(["--repo", REPO]);
    Action::Run(vec![Cmd::new("gh", &args).in_dir(root)])
}

fn fail_action(root: &Path, reason: &str) -> Action {
    let script = format!("echo '{}' >&2; exit 1", reason.replace('\'', "'\\''"));
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinuxDistributionProof {
    schema: u8,
    release: String,
    platform: String,
    runtime_lock_sha256: String,
    server_archive_sha256: String,
    appimage_sha256: String,
    deb_sha256: String,
    previous_client_sha256: String,
    distribution: String,
    test_account: String,
    checks: LinuxDistributionChecks,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinuxDistributionChecks {
    deb_installed: bool,
    appimage_launched: bool,
    initial_client_owned_launch: bool,
    authenticated_health: bool,
    product_app_opened: bool,
    secret_service_key_created_and_read: bool,
    key_file_without_secret_service: bool,
    offline_client_owned_launch: bool,
    forward_update_read_same_key: bool,
    no_docker_setup: bool,
    computer_screen_ready: bool,
    computer_shell_ready: bool,
    workspace_reused_after_update: bool,
    unsafe_downgrade_refused: bool,
}

#[derive(Deserialize)]
struct ProofLock {
    release: String,
    platform: String,
    arch: String,
    asset: ProofAsset,
}

#[derive(Deserialize)]
struct ProofAsset {
    sha256: String,
}

/// Validate the evidence recorded on a clean Linux machine of one
/// architecture. The proof names the exact lock, server archive, AppImage
/// and deb bytes. A failed or omitted check blocks publication.
pub fn validate_linux_distribution_proof(
    proof_path: &Path,
    lock_path: &Path,
    appimage: &Path,
    deb: &Path,
    release: &str,
) -> Result<()> {
    let proof: LinuxDistributionProof = serde_json::from_slice(
        &fs::read(proof_path).with_context(|| format!("read {}", proof_path.display()))?,
    )
    .context("parse Linux distribution proof")?;
    let lock_bytes =
        fs::read(lock_path).with_context(|| format!("read {}", lock_path.display()))?;
    let lock: ProofLock =
        serde_json::from_slice(&lock_bytes).context("parse the runtime lock for proof")?;
    let platform = format!("{}-{}", lock.platform, lock.arch);
    if proof.schema != 1 || proof.release != release || lock.release != release {
        bail!("the Linux distribution proof does not name release {release}");
    }
    if lock.platform != "linux" || proof.platform != platform {
        bail!(
            "the distribution proof names {}, and the runtime lock names {platform}",
            proof.platform
        );
    }
    require_hash(
        "runtime lock",
        &proof.runtime_lock_sha256,
        &hex::encode(Sha256::digest(&lock_bytes)),
    )?;
    require_hash(
        "server archive",
        &proof.server_archive_sha256,
        &lock.asset.sha256,
    )?;
    require_hash("AppImage", &proof.appimage_sha256, &hash_file(appimage)?)?;
    require_hash("deb", &proof.deb_sha256, &hash_file(deb)?)?;
    valid_hash("previous client", &proof.previous_client_sha256)?;
    for (name, value) in [
        ("distribution", proof.distribution.as_str()),
        ("test_account", proof.test_account.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("the Linux distribution proof has no {name}");
        }
    }
    let c = &proof.checks;
    for (name, passed) in [
        ("deb_installed", c.deb_installed),
        ("appimage_launched", c.appimage_launched),
        ("initial_client_owned_launch", c.initial_client_owned_launch),
        ("authenticated_health", c.authenticated_health),
        ("product_app_opened", c.product_app_opened),
        (
            "secret_service_key_created_and_read",
            c.secret_service_key_created_and_read,
        ),
        (
            "key_file_without_secret_service",
            c.key_file_without_secret_service,
        ),
        ("offline_client_owned_launch", c.offline_client_owned_launch),
        (
            "forward_update_read_same_key",
            c.forward_update_read_same_key,
        ),
        ("no_docker_setup", c.no_docker_setup),
        ("computer_screen_ready", c.computer_screen_ready),
        ("computer_shell_ready", c.computer_shell_ready),
        (
            "workspace_reused_after_update",
            c.workspace_reused_after_update,
        ),
        ("unsafe_downgrade_refused", c.unsafe_downgrade_refused),
    ] {
        if !passed {
            bail!("Linux distribution proof check {name} did not pass");
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn valid_hash(name: &str, hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("the Linux distribution proof has an invalid {name} hash");
    }
    Ok(())
}

fn require_hash(name: &str, recorded: &str, actual: &str) -> Result<()> {
    valid_hash(name, recorded)?;
    if recorded != actual {
        bail!("the Linux distribution proof {name} hash does not match the release artifact");
    }
    Ok(())
}
