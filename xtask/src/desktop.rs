//! The desktop packaging (`cargo xtask desktop`): the steps the GitHub
//! Actions workflow runs for the Client App of each platform. This module
//! holds the macOS plan, and `desktop_linux` holds the Linux one. A
//! routine run packs the app unsigned and smoke tests it. A tag prepares
//! exact signed bytes. A separate command publishes those same bytes
//! after external proof.
//!
//! Signing credentials never enter the plan. Electron Builder and
//! `@electron/notarize` read them from the environment or keychain.
//! The plan only records whether the required inputs exist.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::image::ANONYMOUS_PULL_FN;
use crate::release::{NOTARIZE_FN, REPO};
use crate::{Action, Cmd, Step};

/// The platform a Client App is packed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopPlatform {
    /// macOS arm64: a signed, notarized DMG.
    Mac,
    /// Linux amd64 and arm64: an AppImage and a deb for each.
    Linux,
}

/// What the packaging is built from.
#[derive(Debug, Clone)]
pub struct DesktopContext {
    /// The platform the plan packs.
    pub platform: DesktopPlatform,
    /// The workspace version, which the app and the DMG carry.
    pub version: String,
    /// The release tag the DMG is attached to; `None` on a pull
    /// request, where nothing is published.
    pub tag: Option<String>,
    /// The signing variables the environment does not carry.
    pub missing_credentials: Vec<String>,
    /// Build the signed client and stop before distribution proof.
    pub prepare_only: bool,
    /// Reuse the exact prepared package after external proof.
    pub publish_existing: bool,
}

impl DesktopContext {
    /// True when the build can sign and notarize: it is a release and
    /// every credential is present.
    fn signs(&self) -> bool {
        self.tag.is_some() && self.missing_credentials.is_empty()
    }
}

/// The DMG electron-builder writes for one version.
pub fn dmg_name(version: &str) -> String {
    format!("Pagis-{version}-arm64.dmg")
}

/// The missing alternatives for signing and notarization. A local keychain
/// identity avoids exporting a private key. CI can still use an imported P12.
pub fn missing_signing_inputs(is_set: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut missing = Vec::new();
    let local_identity = is_set("CSC_NAME");
    let imported_identity = is_set("CSC_LINK") && is_set("CSC_KEY_PASSWORD");
    if !local_identity && !imported_identity {
        missing.push("CSC_NAME or CSC_LINK with CSC_KEY_PASSWORD".into());
    }

    let keychain_notary = is_set("APPLE_KEYCHAIN_PROFILE");
    let api_key_notary =
        is_set("APPLE_API_KEY") && is_set("APPLE_API_KEY_ID") && is_set("APPLE_API_ISSUER");
    if !keychain_notary && !api_key_notary {
        missing.push(
            "APPLE_KEYCHAIN_PROFILE or APPLE_API_KEY with APPLE_API_KEY_ID and APPLE_API_ISSUER"
                .into(),
        );
    }
    missing
}

/// Refuse an app version that has drifted from the workspace version.
/// The DMG is named after the workspace version and lands on that
/// release, so a drifted app would ship under the wrong name.
pub fn check_version(root: &Path, workspace_version: &str) -> Result<()> {
    let manifest = std::fs::read_to_string(root.join("desktop/package.json"))?;
    let package: serde_json::Value = serde_json::from_str(&manifest)?;
    let version = package["version"].as_str().unwrap_or_default();
    if version != workspace_version {
        bail!(
            "desktop/package.json is version {version}, and the workspace is {workspace_version}; \
             they must agree"
        );
    }
    Ok(())
}

/// Refuse a tag that names another version than the one being packed.
pub fn check_tag(tag: &str, version: &str) -> Result<()> {
    let expected = format!("v{version}");
    if tag != expected {
        bail!("tag {tag} does not name the version being packed; expected {expected}");
    }
    Ok(())
}

/// Plan the packaging steps for the workspace at `root`.
pub fn desktop_plan(root: &Path, cx: &DesktopContext) -> Vec<Step> {
    match cx.platform {
        DesktopPlatform::Mac => mac_plan(root, cx),
        DesktopPlatform::Linux => crate::desktop_linux::linux_plan(root, cx),
    }
}

fn mac_plan(root: &Path, cx: &DesktopContext) -> Vec<Step> {
    let desktop = root.join("desktop");
    // The signed client embeds the final lock, never a server binary. Release
    // creates it only after the server asset and Computer image are immutable.
    let runtime_lock = Action::Run(vec![
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
                "dist/runtime-lock-darwin-arm64.json",
                &cx.version,
                "darwin-arm64",
            ],
        )
        .in_dir(root),
    ]);
    let mut pack = Cmd::new("npm", &["run", "pack"]).in_dir(desktop.clone());
    if !cx.signs() {
        // Without a certificate electron-builder would search the
        // keychain and sign with whatever it finds there.
        pack = pack.env("CSC_IDENTITY_AUTO_DISCOVERY", "false");
    }
    vec![
        Step {
            name: "credentials",
            action: credentials_action(root, cx),
        },
        Step {
            name: "runtime-lock",
            action: runtime_lock,
        },
        Step {
            name: "deps",
            action: if cx.publish_existing {
                Action::Skip(
                    "publishing the exact package that already passed external proof".into(),
                )
            } else {
                Action::Run(vec![Cmd::new("npm", &["ci"]).in_dir(desktop.clone())])
            },
        },
        Step {
            name: "pack",
            action: if cx.publish_existing {
                Action::Skip(
                    "publishing the exact package that already passed external proof".into(),
                )
            } else {
                Action::Run(vec![pack])
            },
        },
        Step {
            name: "finalize-dmg",
            action: finalize_dmg_action(root, cx),
        },
        Step {
            name: "inventory",
            action: loose_app_action(
                cx,
                Cmd::new("sh", &["-c", PACKAGE_INVENTORY_SCRIPT]).in_dir(desktop.clone()),
            ),
        },
        Step {
            name: "packaged-runtime",
            action: loose_app_action(
                cx,
                Cmd::new("sh", &["-c", PACKAGED_RUNTIME_SCRIPT]).in_dir(desktop.clone()),
            ),
        },
        Step {
            name: "smoke",
            action: loose_app_action(cx, Cmd::new("sh", &["-c", SMOKE_SCRIPT]).in_dir(desktop)),
        },
        Step {
            name: "signed-client",
            action: signed_client_action(root, cx),
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
            name: "publish",
            action: publish_action(root, cx),
        },
    ]
}

fn loose_app_action(cx: &DesktopContext, command: Cmd) -> Action {
    if cx.tag.is_some() {
        Action::Skip("the release checks the mounted app from the exact DMG".into())
    } else {
        Action::Run(vec![command])
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DistributionProof {
    schema: u8,
    release: String,
    runtime_lock_sha256: String,
    server_dmg_sha256: String,
    client_dmg_sha256: String,
    previous_client_dmg_sha256: String,
    team_id: String,
    notarization_submission_id: String,
    macos_version: String,
    test_account: String,
    checks: DistributionChecks,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DistributionChecks {
    server_gatekeeper: bool,
    server_stapled: bool,
    server_designated_requirements: bool,
    client_gatekeeper: bool,
    client_stapled: bool,
    client_quarantined: bool,
    installed_runtime_quarantined: bool,
    initial_client_owned_launch: bool,
    authenticated_health: bool,
    product_app_opened: bool,
    keychain_created_and_read: bool,
    offline_client_owned_launch: bool,
    forward_update_read_same_keychain_item: bool,
    no_docker_setup: bool,
    computer_screen_ready: bool,
    computer_shell_ready: bool,
    workspace_reused_after_update: bool,
    unsafe_downgrade_refused: bool,
}

#[derive(Deserialize)]
struct ProofRuntimeLock {
    release: String,
    asset: ProofAsset,
}

#[derive(Deserialize)]
struct ProofAsset {
    sha256: String,
    team_id: String,
}

/// Validate the evidence recorded on a clean macOS account. The proof names
/// the exact lock, server DMG and client DMG bytes. A failed or omitted check
/// blocks client publication.
pub fn validate_distribution_proof(
    proof_path: &Path,
    lock_path: &Path,
    client_dmg: &Path,
    release: &str,
) -> Result<()> {
    let proof: DistributionProof = serde_json::from_slice(
        &fs::read(proof_path).with_context(|| format!("read {}", proof_path.display()))?,
    )
    .context("parse distribution proof")?;
    if proof.schema != 1 || proof.release != release {
        bail!("distribution proof does not name release {release}");
    }
    let lock_bytes =
        fs::read(lock_path).with_context(|| format!("read {}", lock_path.display()))?;
    let lock: ProofRuntimeLock =
        serde_json::from_slice(&lock_bytes).context("parse the runtime lock for proof")?;
    if lock.release != release {
        bail!("the runtime lock does not name release {release}");
    }
    require_hash(
        "runtime lock",
        &proof.runtime_lock_sha256,
        &hash_bytes(&lock_bytes),
    )?;
    require_hash("server DMG", &proof.server_dmg_sha256, &lock.asset.sha256)?;
    require_hash(
        "client DMG",
        &proof.client_dmg_sha256,
        &hash_file(client_dmg)?,
    )?;
    valid_hash("previous client DMG", &proof.previous_client_dmg_sha256)?;
    if proof.team_id != lock.asset.team_id {
        bail!("distribution proof team does not match the runtime lock");
    }
    for (name, value) in [
        (
            "notarization_submission_id",
            proof.notarization_submission_id.as_str(),
        ),
        ("macos_version", proof.macos_version.as_str()),
        ("test_account", proof.test_account.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("distribution proof has no {name}");
        }
    }
    let checks = &proof.checks;
    for (name, passed) in [
        ("server_gatekeeper", checks.server_gatekeeper),
        ("server_stapled", checks.server_stapled),
        (
            "server_designated_requirements",
            checks.server_designated_requirements,
        ),
        ("client_gatekeeper", checks.client_gatekeeper),
        ("client_stapled", checks.client_stapled),
        ("client_quarantined", checks.client_quarantined),
        (
            "installed_runtime_quarantined",
            checks.installed_runtime_quarantined,
        ),
        (
            "initial_client_owned_launch",
            checks.initial_client_owned_launch,
        ),
        ("authenticated_health", checks.authenticated_health),
        ("product_app_opened", checks.product_app_opened),
        (
            "keychain_created_and_read",
            checks.keychain_created_and_read,
        ),
        (
            "offline_client_owned_launch",
            checks.offline_client_owned_launch,
        ),
        (
            "forward_update_read_same_keychain_item",
            checks.forward_update_read_same_keychain_item,
        ),
        ("no_docker_setup", checks.no_docker_setup),
        ("computer_screen_ready", checks.computer_screen_ready),
        ("computer_shell_ready", checks.computer_shell_ready),
        (
            "workspace_reused_after_update",
            checks.workspace_reused_after_update,
        ),
        ("unsafe_downgrade_refused", checks.unsafe_downgrade_refused),
    ] {
        if !passed {
            bail!("distribution proof check {name} did not pass");
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_hash(name: &str, hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("distribution proof has an invalid {name} hash");
    }
    Ok(())
}

fn require_hash(name: &str, recorded: &str, actual: &str) -> Result<()> {
    valid_hash(name, recorded)?;
    if recorded != actual {
        bail!("distribution proof {name} hash does not match the release artifact");
    }
    Ok(())
}

fn distribution_proof_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: distribution proof is not required".into());
    }
    if cx.prepare_only {
        return Action::Skip("prepared package awaits external distribution proof".into());
    }
    let dmg = format!("desktop/release/{}", dmg_name(&cx.version));
    let version = cx.version.as_str();
    Action::Run(vec![
        Cmd::new(
            "cargo",
            &[
                "run",
                "--quiet",
                "-p",
                "xtask",
                "--",
                "distribution-proof",
                "validate",
                "dist/distribution-proof.json",
                "dist/runtime-lock-darwin-arm64.json",
                &dmg,
                version,
            ],
        )
        .in_dir(root),
    ])
}

fn finalize_dmg_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: the DMG is an unsigned smoke artifact".into());
    }
    if cx.publish_existing {
        return Action::Skip("the exact DMG was already finalized before external proof".into());
    }
    let dmg = format!("desktop/release/{}", dmg_name(&cx.version));
    let script = format!(
        "set -eu\n{NOTARIZE_FN}dmg='{dmg}'\n/usr/bin/codesign --verify --strict --verbose=2 \"$dmg\"\nnotarize \"$dmg\"\n/usr/bin/xcrun stapler staple \"$dmg\"\n/usr/bin/xcrun stapler validate \"$dmg\"\n"
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

fn signed_client_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: the package is an unsigned smoke artifact".into());
    }
    let dmg = format!("desktop/release/{}", dmg_name(&cx.version));
    let script = format!(
        "set -eu\ndmg='{dmg}'\nmount=$(mktemp -d)\ncopy=$(mktemp -d)\nmounted=0\ncleanup() {{\n  if [ \"$mounted\" -eq 1 ]; then /usr/bin/hdiutil detach \"$mount\" >/dev/null || true; fi\n  rm -rf \"$mount\" \"$copy\"\n}}\ntrap cleanup EXIT HUP INT TERM\n/usr/bin/hdiutil attach -readonly -nobrowse -mountpoint \"$mount\" \"$dmg\" >/dev/null\nmounted=1\napp=\"$mount/Pagis.app\"\n[ -d \"$app\" ] || {{ echo 'the exact DMG has no Pagis.app' >&2; exit 1; }}\ncmp dist/runtime-lock-darwin-arm64.json \"$app/Contents/Resources/runtime-lock.json\"\nnode desktop/scripts/check-package.mjs \"$app\" \"$dmg\"\nnode desktop/scripts/check-packaged-runtime.mjs \"$app\"\n/usr/bin/codesign --verify --deep --strict --verbose=2 \"$app\"\n/usr/bin/codesign -dv --verbose=4 \"$app\" 2>&1 | grep '^Authority=Developer ID Application:' >/dev/null\nteam=$(/usr/bin/codesign -dv --verbose=4 \"$app\" 2>&1 | sed -n 's/^TeamIdentifier=//p')\nlocked_team=$(node -e \"process.stdout.write(require('./dist/runtime-lock-darwin-arm64.json').asset.team_id)\")\n[ \"$team\" = \"$locked_team\" ] || {{ echo 'the client and server signing teams differ' >&2; exit 1; }}\n/usr/bin/codesign --verify --strict --verbose=2 \"$dmg\"\n/usr/bin/xcrun stapler validate \"$dmg\"\n/usr/sbin/spctl --assess --type execute --verbose=2 \"$app\"\n/usr/sbin/spctl --assess --type open --context context:primary-signature --verbose=2 \"$dmg\"\n/usr/bin/ditto \"$app\" \"$copy/Pagis.app\"\n(cd \"$copy\" && ./Pagis.app/Contents/MacOS/Pagis --smoke)\n"
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

fn released_tuple_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: no published server tuple is required".into());
    };
    if cx.prepare_only {
        return Action::Skip("prepared package awaits external distribution proof".into());
    }
    let server = crate::release::server_dmg_name(&cx.version);
    let script = format!(
        "set -eu\ntmp=$(mktemp -d)\ntrap 'rm -rf \"$tmp\"' EXIT\ngh release download {tag} --repo {REPO} --dir \"$tmp\" --pattern runtime-lock-darwin-arm64.json --pattern {server}\ncmp dist/runtime-lock-darwin-arm64.json \"$tmp/runtime-lock-darwin-arm64.json\"\ncmp dist/{server} \"$tmp/{server}\"\nlocked_server=$(node -e \"process.stdout.write(require('./dist/runtime-lock-darwin-arm64.json').asset.sha256)\")\nactual_server=$(shasum -a 256 \"$tmp/{server}\" | awk '{{print $1}}')\n[ \"$actual_server\" = \"$locked_server\" ] || {{ echo 'the published server DMG does not match the Runtime Lock' >&2; exit 1; }}\nimage=$(node -e \"process.stdout.write(require('./dist/runtime-lock-darwin-arm64.json').computer_image)\")\n{ANONYMOUS_PULL_FN}anonymous_pull \"$image\"\n"
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// Start the packaged client with no installed server. It must open its local
/// setup page, and its signed Resources must contain no bundled server.
///
/// The output directory carries the architecture only when it is not
/// the host's, so the app is found by its shape and not by one name.
const PACKAGE_INVENTORY_SCRIPT: &str = concat!(
    "set -eu\n",
    "app=\"$(find release -maxdepth 2 -type d -name Pagis.app -print -quit)\"\n",
    "[ -n \"$app\" ] || { echo 'the packaged Pagis.app is missing' >&2; exit 1; }\n",
    "dmg=\"release/$(node -p \"'Pagis-' + require('./package.json').version + '-arm64.dmg'\")\"\n",
    "node scripts/check-package.mjs \"$app\" \"$dmg\"\n",
);

const PACKAGED_RUNTIME_SCRIPT: &str = concat!(
    "set -eu\n",
    "app=\"$(find release -maxdepth 2 -type d -name Pagis.app -print -quit)\"\n",
    "[ -n \"$app\" ] || { echo 'the packaged Pagis.app is missing' >&2; exit 1; }\n",
    "node scripts/check-packaged-runtime.mjs \"$app\"\n",
);

const SMOKE_SCRIPT: &str = concat!(
    "set -eu\n",
    "app=\"$(find release -maxdepth 2 -type d -name Pagis.app -print -quit)\"\n",
    "[ -n \"$app\" ] || { echo 'the packaged Pagis.app is missing' >&2; exit 1; }\n",
    "tmp=$(mktemp -d)\n",
    "trap 'rm -rf \"$tmp\"' EXIT\n",
    "/usr/bin/ditto \"$app\" \"$tmp/Pagis.app\"\n",
    "cd \"$tmp\"\n",
    "./Pagis.app/Contents/MacOS/Pagis --smoke\n",
);

/// Attach the DMG to the draft release of the tag, beside the server
/// packages it holds. An unsigned DMG is never published: macOS would
/// refuse to open it.
fn publish_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: the app is packed and smoke tested only".into());
    };
    if cx.prepare_only {
        return Action::Skip("prepared package awaits external distribution proof".into());
    }
    if !cx.missing_credentials.is_empty() {
        return fail_action(
            root,
            &format!(
                "release signing needs {}",
                cx.missing_credentials.join("; ")
            ),
        );
    }
    let dmg = format!("desktop/release/{}", dmg_name(&cx.version));
    Action::Run(vec![
        Cmd::new("gh", &["release", "upload", tag, &dmg, "--repo", REPO]).in_dir(root),
    ])
}

fn credentials_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: signing credentials are not needed".into());
    }
    if cx.missing_credentials.is_empty() {
        return Action::Run(vec![Cmd::new("sh", &["-c", "true"]).in_dir(root)]);
    }
    fail_action(
        root,
        &format!(
            "release signing needs {}",
            cx.missing_credentials.join("; ")
        ),
    )
}

fn fail_action(root: &Path, reason: &str) -> Action {
    let script = format!("echo '{}' >&2; exit 1", reason.replace('\'', "'\\''"));
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}
