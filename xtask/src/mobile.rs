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

use std::path::Path;
use std::process::Command;

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
