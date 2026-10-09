//! The gate steps of the Mobile App in `mobile/`.
//!
//! - `mobile-deps`, `mobile-typecheck` and `mobile-test` check the web
//!   part with its npm scripts.
//! - `mobile-android-test` and `mobile-ios-test` build the web part, sync
//!   it into the native project with `npx cap sync`, and run the JUnit
//!   tests or the XCTest tests. The sync puts the current web assets and
//!   the current plugin list in the native project, so no test runs
//!   against stale generated files.
//!
//! A native step needs its toolchain: a JDK 21 and the Android SDK, or
//! Xcode on a macOS host. `cargo xtask dev` and `cargo xtask full` skip a
//! native step whose toolchain is absent, as they skip the Docker steps
//! without Docker. `cargo xtask step` refuses it, so the CI job that names
//! the step proves that it ran.
//!
//! This module also holds the iOS release (`cargo xtask mobile --ios`),
//! which `.github/workflows/mobile-release.yml` runs for a `mobile-v*`
//! tag in two phases (`docs/RELEASING-MOBILE.md`). The prepare phase
//! builds, signs and checks the exact `.ipa`. The publish phase uploads
//! those bytes to App Store Connect after a maintainer approves the
//! release. The plan names its inputs, and the shell reads each secret
//! from the environment when a step runs.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::{Action, Cmd, Lane, Step, npm_deps_action, npm_script_action};

/// The directory of the Mobile App.
const PACKAGE: &str = "mobile";

const ANDROID_TEST: &str = "mobile-android-test";
const IOS_TEST: &str = "mobile-ios-test";

/// The oldest JDK that Capacitor 8 builds Android with.
const MINIMUM_JAVA: u32 = 21;

const NO_ANDROID: &str = "no JDK 21 (java) and Android SDK (ANDROID_HOME)";
const NO_IOS: &str = "no Xcode (xcodebuild) on a macOS host";

/// Run the XCTest tests of the app on the first available iPhone
/// simulator. Node reads the device list, because each npm step needs
/// Node already.
const XCODE_TEST: &str = r#"set -eu
udid=$(xcrun simctl list devices available --json | node -e '
const { devices } = JSON.parse(require("fs").readFileSync(0, "utf8"));
const phone = Object.entries(devices)
  .filter(([runtime]) => runtime.includes(".iOS-"))
  .flatMap(([, list]) => list)
  .find((device) => device.name.startsWith("iPhone"));
if (!phone) {
  console.error("no available iPhone simulator");
  process.exit(1);
}
console.log(phone.udid);
')
xcodebuild test -project App/App.xcodeproj -scheme App -destination "id=$udid"
"#;

/// The native toolchains of this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toolchains {
    /// `java -version` reports 21 or later, and `ANDROID_HOME` names a
    /// directory.
    pub android: bool,
    /// The host is macOS, and `xcodebuild -version` runs.
    pub ios: bool,
}

impl Toolchains {
    /// Probe the toolchains of this host.
    pub fn probe() -> Self {
        let android_home = std::env::var_os("ANDROID_HOME")
            .is_some_and(|home| !home.is_empty() && Path::new(&home).is_dir());
        let java = Command::new("java")
            .arg("-version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| java_major_version(&String::from_utf8_lossy(&output.stderr)));
        let xcodebuild = cfg!(target_os = "macos")
            && Command::new("xcodebuild")
                .arg("-version")
                .output()
                .is_ok_and(|output| output.status.success());
        Toolchains {
            android: android_home && java.is_some_and(|major| major >= MINIMUM_JAVA),
            ios: xcodebuild,
        }
    }
}

/// The major version in the output of `java -version`: the first quoted
/// version, as in `openjdk version "21.0.8"`. A version `1.<n>` is Java
/// `<n>`.
pub fn java_major_version(output: &str) -> Option<u32> {
    let version = output.split('"').nth(1)?;
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// What a native step does when its toolchain is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Absent {
    /// Skip, in `cargo xtask dev` and `cargo xtask full`.
    Skip,
    /// Fail, in `cargo xtask step`.
    Refuse,
}

/// The lane of the Mobile App. It runs no Cargo, so it runs beside the
/// other lanes.
pub(crate) fn lane(root: &Path, toolchains: Toolchains, absent: Absent) -> Lane {
    Lane {
        name: "mobile",
        steps: steps(root, toolchains, absent),
    }
}

/// The mobile steps that a change to `changed_paths` selects. A change
/// in one native project runs the tests of that platform. Each other
/// change in `mobile/` runs every mobile step.
pub(crate) fn dev_steps(
    root: &Path,
    changed_paths: &[String],
    toolchains: Toolchains,
) -> Vec<Step> {
    let mut android = false;
    let mut ios = false;
    let mut web = false;
    for path in changed_paths {
        let Some(path) = path.strip_prefix("mobile/") else {
            continue;
        };
        if path.starts_with("android/") {
            android = true;
        } else if path.starts_with("ios/") {
            ios = true;
        } else {
            web = true;
        }
    }
    if !(android || ios || web) {
        return Vec::new();
    }
    steps(root, toolchains, Absent::Skip)
        .into_iter()
        .filter(|step| match step.name {
            "mobile-deps" => true,
            ANDROID_TEST => web || android,
            IOS_TEST => web || ios,
            _ => web,
        })
        .collect()
}

fn steps(root: &Path, toolchains: Toolchains, absent: Absent) -> Vec<Step> {
    let mobile = root.join(PACKAGE);
    vec![
        Step {
            name: "mobile-deps",
            action: npm_deps_action(root, PACKAGE),
        },
        Step {
            name: "mobile-typecheck",
            action: npm_script_action(root, PACKAGE, "typecheck"),
        },
        Step {
            name: "mobile-test",
            action: npm_script_action(root, PACKAGE, "test"),
        },
        native_step(
            root,
            ANDROID_TEST,
            toolchains.android,
            NO_ANDROID,
            absent,
            Cmd::new("./gradlew", &["testDebugUnitTest"]).in_dir(mobile.join("android")),
            "android",
        ),
        native_step(
            root,
            IOS_TEST,
            toolchains.ios,
            NO_IOS,
            absent,
            Cmd::new("sh", &["-c", XCODE_TEST]).in_dir(mobile.join("ios")),
            "ios",
        ),
    ]
}

/// Build the web part, sync it into the native project of `platform`,
/// then run `test`.
fn native_step(
    root: &Path,
    name: &'static str,
    toolchain: bool,
    missing: &str,
    absent: Absent,
    test: Cmd,
    platform: &str,
) -> Step {
    let mobile = root.join(PACKAGE);
    let action = if !mobile.join("package.json").exists() {
        Action::Skip(format!("{PACKAGE}/ is not present yet"))
    } else if !toolchain {
        match absent {
            Absent::Skip => Action::Skip(missing.into()),
            Absent::Refuse => Action::Run(vec![Cmd::refusal(&format!("{name}: {missing}"))]),
        }
    } else {
        Action::Run(vec![
            Cmd::new("npm", &["run", "build"]).in_dir(&mobile),
            Cmd::new("npx", &["cap", "sync", platform]).in_dir(&mobile),
            test,
        ])
    };
    Step { name, action }
}

/// The phase of the iOS release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosPhase {
    /// Build, sign and check the exact `.ipa`, and stop before the upload.
    Prepare,
    /// Upload the prepared `.ipa`. Build and sign nothing.
    PublishExisting,
}

/// The `.ipa` that the prepare phase writes and the publish phase
/// uploads, from the repository root.
const IPA: &str = "mobile/release/Pagis.ipa";

/// The archive and the export, from `mobile/ios`.
const ARCHIVE: &str = "../release/Pagis.xcarchive";

/// The bundle id of the app (ADR-0032).
const BUNDLE_ID: &str = "co.pagis.mobile";

/// The environment variable that holds the `https` origin of the Push
/// Relay, which the archive gives to `xcodebuild` as a build setting.
const RELAY_ORIGIN: &str = "PUSH_RELAY_ORIGIN";

/// The environment variable that holds the number of the workflow run,
/// which is the build number of the app.
const RUN_NUMBER: &str = "GITHUB_RUN_NUMBER";

/// The App Store Connect API key: the path of its `.p8` file, its key ID
/// and its issuer ID. Automatic signing and the upload use it.
const API_KEY_INPUTS: [&str; 3] = ["APPLE_API_KEY", "APPLE_API_KEY_ID", "APPLE_API_ISSUER"];

/// The `xcodebuild` options of automatic signing with the API key.
const AUTOMATIC_SIGNING: &str = "-allowProvisioningUpdates \
    -authenticationKeyPath \"$APPLE_API_KEY\" \
    -authenticationKeyID \"$APPLE_API_KEY_ID\" \
    -authenticationKeyIssuerID \"$APPLE_API_ISSUER\"";

/// The inputs of the prepare phase, after their checks.
struct Build {
    version: String,
    run_number: String,
    relay_origin: String,
}

/// Plan the iOS release of the checkout at `root` for the tag `tag`.
/// `env` reads the environment. The plan stops when the tag does not
/// name the version of `mobile/package.json`, or when an input of the
/// phase is missing or wrong.
pub fn ios_release_plan(
    root: &Path,
    tag: &str,
    phase: IosPhase,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<Step>> {
    let version = package_version(root)?;
    let expected = format!("mobile-v{version}");
    if tag != expected {
        bail!("tag {tag} does not name the version of {PACKAGE}/package.json; expected {expected}");
    }
    let value = |name: &str| env(name).filter(|value| !value.trim().is_empty());
    let missing: Vec<&str> = API_KEY_INPUTS
        .into_iter()
        .filter(|name| value(name).is_none())
        .collect();
    if !missing.is_empty() {
        bail!(
            "the iOS release needs the App Store Connect API key in {}",
            missing.join(", ")
        );
    }
    let build = match phase {
        IosPhase::Prepare => Some(Build {
            version,
            run_number: run_number(value(RUN_NUMBER))?,
            relay_origin: relay_origin(value(RELAY_ORIGIN))?,
        }),
        // The prepared .ipa holds the build number and the relay origin.
        IosPhase::PublishExisting => None,
    };
    let mobile = root.join(PACKAGE);
    let ios = mobile.join("ios");
    let prepared = |name: &'static str, run: &dyn Fn(&Build) -> Cmd| Step {
        name,
        action: match &build {
            Some(build) => Action::Run(vec![run(build)]),
            None => Action::Skip("publishing the exact .ipa that was prepared".into()),
        },
    };
    let upload = match phase {
        IosPhase::Prepare => Action::Skip("the prepared .ipa awaits publication".into()),
        IosPhase::PublishExisting => {
            Action::Run(vec![Cmd::new("sh", &["-c", &upload_script()]).in_dir(root)])
        }
    };
    Ok(vec![
        prepared("deps", &|_| Cmd::new("npm", &["ci"]).in_dir(&mobile)),
        prepared("build", &|_| {
            Cmd::new("npm", &["run", "build"]).in_dir(&mobile)
        }),
        prepared("sync", &|_| {
            Cmd::new("npx", &["cap", "sync", "ios"]).in_dir(&mobile)
        }),
        prepared("archive", &|build| {
            Cmd::new("sh", &["-c", &archive_script(build)]).in_dir(&ios)
        }),
        prepared("export", &|_| {
            Cmd::new("sh", &["-c", &export_script()]).in_dir(&ios)
        }),
        prepared("check-ipa", &|build| {
            Cmd::new("sh", &["-c", &check_script(build)]).in_dir(root)
        }),
        Step {
            name: "upload",
            action: upload,
        },
    ])
}

/// The version of `mobile/package.json`. App Store Connect takes a
/// `CFBundleShortVersionString` of one to three whole numbers with
/// periods between them.
fn package_version(root: &Path) -> Result<String> {
    let path = root.join(PACKAGE).join("package.json");
    let manifest = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&manifest)?;
    let version = package["version"].as_str().unwrap_or_default();
    let parts: Vec<&str> = version.split('.').collect();
    let numbers = parts.len() <= 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !numbers {
        bail!(
            "{PACKAGE}/package.json is version {version}, and App Store Connect takes one to \
             three whole numbers with periods between them"
        );
    }
    Ok(version.to_string())
}

/// The build number: a positive whole number.
fn run_number(value: Option<String>) -> Result<String> {
    let Some(value) = value else {
        bail!("the iOS release needs {RUN_NUMBER}, the build number of the app");
    };
    let value = value.trim();
    if value.parse::<u64>().is_ok_and(|number| number > 0) {
        Ok(value.to_string())
    } else {
        bail!("{RUN_NUMBER} is {value}, and the build number must be a positive whole number")
    }
}

/// The `https` origin of the Push Relay: a host and an optional port,
/// with no user, path, query or fragment. The placeholder of
/// `mobile/ios/app.xcconfig` is a `.invalid` name, which never resolves.
fn relay_origin(value: Option<String>) -> Result<String> {
    let Some(value) = value else {
        bail!("the iOS release needs {RELAY_ORIGIN}, the https origin of the Push Relay");
    };
    let value = value.trim();
    let authority = value.strip_prefix("https://").unwrap_or_default();
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.parse::<u16>().is_ok() {
                host
            } else {
                ""
            }
        });
    let plain_host = !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !plain_host {
        bail!(
            "{RELAY_ORIGIN} is {value}, which is not an https origin such as https://push.pagis.co"
        );
    }
    if host.ends_with(".invalid") {
        bail!("{RELAY_ORIGIN} is {value}, the placeholder that never resolves");
    }
    Ok(value.to_string())
}

/// Archive the app and its extension. The settings on the command line
/// come before each xcconfig file, so the versions and the relay origin
/// of the release apply to each target.
fn archive_script(build: &Build) -> String {
    format!(
        "set -eu\n\
         rm -rf ../release\n\
         mkdir -p ../release\n\
         xcodebuild archive -project App/App.xcodeproj -scheme App -configuration Release \
         -destination generic/platform=iOS -archivePath {ARCHIVE} {AUTOMATIC_SIGNING} \
         MARKETING_VERSION={version} CURRENT_PROJECT_VERSION={run_number} \
         {RELAY_ORIGIN}={origin}\n",
        version = build.version,
        run_number = build.run_number,
        origin = build.relay_origin,
    )
}

/// Export the archive for App Store Connect, and name the `.ipa` after
/// the app.
fn export_script() -> String {
    format!(
        "set -eu\n\
         out=$(mktemp -d)\n\
         trap 'rm -rf \"$out\"' EXIT\n\
         xcodebuild -exportArchive -archivePath {ARCHIVE} -exportPath \"$out\" \
         -exportOptionsPlist ExportOptions.plist {AUTOMATIC_SIGNING}\n\
         set -- \"$out\"/*.ipa\n\
         [ $# -eq 1 ] && [ -f \"$1\" ] || {{ echo 'the export wrote no single .ipa' >&2; exit 1; }}\n\
         mv \"$1\" ../release/Pagis.ipa\n"
    )
}

/// Check the exact `.ipa`: the signatures of the app and the extension,
/// their versions and privacy manifests, the bundle id, the relay origin,
/// the declaration of exempt encryption, and the production APNs
/// environment of a distribution signature.
fn check_script(build: &Build) -> String {
    format!(
        r#"set -eu
ipa='{IPA}'
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
/usr/bin/ditto -x -k "$ipa" "$tmp"
app="$tmp/Payload/App.app"
appex="$app/PlugIns/PagisNotificationService.appex"
value() {{ /usr/libexec/PlistBuddy -c "Print :$2" "$1"; }}
expect() {{ [ "$2" = "$3" ] || {{ echo "$1 is $2; expected $3" >&2; exit 1; }}; }}
for bundle in "$app" "$appex"; do
  /usr/bin/codesign --verify --strict --verbose=2 "$bundle"
  expect "CFBundleShortVersionString of $bundle" "$(value "$bundle/Info.plist" CFBundleShortVersionString)" '{version}'
  expect "CFBundleVersion of $bundle" "$(value "$bundle/Info.plist" CFBundleVersion)" '{run_number}'
  [ -f "$bundle/PrivacyInfo.xcprivacy" ] || {{ echo "$bundle has no PrivacyInfo.xcprivacy" >&2; exit 1; }}
done
expect CFBundleIdentifier "$(value "$app/Info.plist" CFBundleIdentifier)" '{BUNDLE_ID}'
expect PagisPushRelayOrigin "$(value "$app/Info.plist" PagisPushRelayOrigin)" '{origin}'
expect ITSAppUsesNonExemptEncryption "$(value "$app/Info.plist" ITSAppUsesNonExemptEncryption)" false
/usr/bin/codesign -d --entitlements - --xml "$app" > "$tmp/entitlements.plist"
expect aps-environment "$(value "$tmp/entitlements.plist" aps-environment)" production
"#,
        version = build.version,
        run_number = build.run_number,
        origin = build.relay_origin,
    )
}

/// Upload the prepared `.ipa`. altool reads the key from the file
/// `AuthKey_<key id>.p8` in the directory of `API_PRIVATE_KEYS_DIR`.
fn upload_script() -> String {
    format!(
        r#"set -eu
keys=$(mktemp -d)
trap 'rm -rf "$keys"' EXIT
cp "$APPLE_API_KEY" "$keys/AuthKey_$APPLE_API_KEY_ID.p8"
API_PRIVATE_KEYS_DIR="$keys" xcrun altool --upload-app -f {IPA} -t ios --apiKey "$APPLE_API_KEY_ID" --apiIssuer "$APPLE_API_ISSUER"
"#
    )
}
