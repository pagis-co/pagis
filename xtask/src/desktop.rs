//! The desktop packaging (`cargo xtask desktop`): the steps the GitHub
//! Actions workflow runs for the Client App of each platform. This module
//! holds the macOS plan, and `desktop_linux` holds the Linux one. A
//! routine run packs the app unsigned and smoke tests it. A tag prepares
//! exact signed bytes. A separate command publishes those same bytes
//! after a maintainer approves the release.
//!
//! Signing credentials never enter the plan. Electron Builder and
//! `@electron/notarize` read them from the environment or keychain.
//! The plan only records whether the required inputs exist.

use std::path::Path;

use anyhow::{Result, bail};

use crate::image::ANONYMOUS_PULL_FN;
use crate::release::{NOTARIZE_FN, REPO};
use crate::{Action, Cmd, Step};

/// The platform a Client App is packed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopPlatform {
    /// macOS arm64: a signed, notarized DMG, and a ZIP of the same app
    /// with the Update feed that names it.
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
    /// Build the signed client and stop before publication.
    pub prepare_only: bool,
    /// Publish the exact prepared package.
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

/// The ZIP of the same app, which Squirrel.Mac installs as an Update
/// (ADR-0027). electron-builder writes its blockmap beside it.
pub fn zip_name(version: &str) -> String {
    format!("Pagis-{version}-arm64.zip")
}

/// The Update feed that electron-updater reads on macOS. It names the ZIP.
const MAC_FEED: &str = "latest-mac.yml";

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
                Action::Skip("publishing the exact package that was prepared".into())
            } else {
                Action::Run(vec![Cmd::new("npm", &["ci"]).in_dir(desktop.clone())])
            },
        },
        Step {
            name: "pack",
            action: if cx.publish_existing {
                Action::Skip("publishing the exact package that was prepared".into())
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
        Action::Skip("the release checks the app from the exact DMG and the exact ZIP".into())
    } else {
        Action::Run(vec![command])
    }
}

fn finalize_dmg_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() {
        return Action::Skip("no release tag: the DMG is an unsigned smoke artifact".into());
    }
    if cx.publish_existing {
        return Action::Skip("the exact DMG was finalized when it was prepared".into());
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
    if cx.publish_existing {
        return Action::Skip("the exact DMG was checked when it was prepared".into());
    }
    let dmg = format!("desktop/release/{}", dmg_name(&cx.version));
    let zip = format!("desktop/release/{}", zip_name(&cx.version));
    let version = &cx.version;
    // `check_app` holds the checks of each copy of the app that the release
    // publishes: the app in the DMG, which a Person installs, and the app
    // in the ZIP, which Squirrel.Mac installs as an Update.
    let script = format!(
        r#"set -eu
dmg='{dmg}'
zip='{zip}'
mount=$(mktemp -d)
copy=$(mktemp -d)
unzipped=$(mktemp -d)
mounted=0
cleanup() {{
  if [ "$mounted" -eq 1 ]; then /usr/bin/hdiutil detach "$mount" >/dev/null || true; fi
  rm -rf "$mount" "$copy" "$unzipped"
}}
trap cleanup EXIT HUP INT TERM
locked_team=$(node -e "process.stdout.write(require('./dist/runtime-lock-darwin-arm64.json').asset.team_id)")
check_app() {{
  cmp dist/runtime-lock-darwin-arm64.json "$1/Contents/Resources/runtime-lock.json"
  node desktop/scripts/check-package.mjs "$1" "$2"
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$1"
  /usr/bin/codesign -dv --verbose=4 "$1" 2>&1 | grep '^Authority=Developer ID Application:' >/dev/null
  team=$(/usr/bin/codesign -dv --verbose=4 "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p')
  [ "$team" = "$locked_team" ] || {{ echo 'the client and server signing teams differ' >&2; exit 1; }}
  /usr/bin/xcrun stapler validate "$1"
  /usr/sbin/spctl --assess --type execute --verbose=2 "$1"
}}
/usr/bin/hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$dmg" >/dev/null
mounted=1
app="$mount/Pagis.app"
[ -d "$app" ] || {{ echo 'the exact DMG has no Pagis.app' >&2; exit 1; }}
check_app "$app" "$dmg"
node desktop/scripts/check-packaged-runtime.mjs "$app"
/usr/bin/codesign --verify --strict --verbose=2 "$dmg"
/usr/bin/xcrun stapler validate "$dmg"
/usr/sbin/spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
/usr/bin/ditto "$app" "$copy/Pagis.app"
(cd "$copy" && ./Pagis.app/Contents/MacOS/Pagis --smoke)
/usr/bin/ditto -x -k "$zip" "$unzipped"
[ -d "$unzipped/Pagis.app" ] || {{ echo 'the exact ZIP has no Pagis.app' >&2; exit 1; }}
check_app "$unzipped/Pagis.app" "$zip"
[ -f "$zip.blockmap" ] || {{ echo 'the ZIP has no blockmap' >&2; exit 1; }}
node desktop/scripts/check-update-feed.mjs desktop/release/{MAC_FEED} "$zip" {version}
"#
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

fn released_tuple_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: no published server tuple is required".into());
    };
    if cx.prepare_only {
        return Action::Skip("the prepared package awaits publication".into());
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
    "version=\"$(node -p \"require('./package.json').version\")\"\n",
    "dmg=\"release/Pagis-$version-arm64.dmg\"\n",
    "zip=\"release/Pagis-$version-arm64.zip\"\n",
    "node scripts/check-package.mjs \"$app\" \"$dmg\" \"$zip\"\n",
    "node scripts/check-update-feed.mjs release/latest-mac.yml \"$zip\" \"$version\"\n",
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

/// Attach the DMG, the ZIP with its blockmap, and the feed that names the
/// ZIP to the draft release of the tag, beside the server packages it
/// holds. `gh release upload` without `--clobber` refuses a file that the
/// draft holds. An unsigned DMG is never published: macOS would refuse to
/// open it.
fn publish_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: the app is packed and smoke tested only".into());
    };
    if cx.prepare_only {
        return Action::Skip("the prepared package awaits publication".into());
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
    let zip = format!("desktop/release/{}", zip_name(&cx.version));
    let blockmap = format!("{zip}.blockmap");
    let feed = format!("desktop/release/{MAC_FEED}");
    Action::Run(vec![
        Cmd::new(
            "gh",
            &[
                "release", "upload", tag, &dmg, &zip, &blockmap, &feed, "--repo", REPO,
            ],
        )
        .in_dir(root),
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
