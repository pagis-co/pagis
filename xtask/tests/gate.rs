//! Tests for the gate: the development check (`cargo xtask dev`), the
//! full gate (`cargo xtask full`), the named steps (`cargo xtask step`)
//! and the CI workflow that runs them.

use std::path::Path;
use std::time::Duration;

use pagis_versions::COMPUTER_IMAGE;
use xtask::mobile::Toolchains;
use xtask::{
    Action, Cmd, Lane, Outcome, StepResult, TestObjects, all_green, dev_lanes, execute,
    execute_lanes, execute_until_failure, full_lanes, named_steps, orphaned_test_objects,
    test_object_sweep,
};

use crate::support::workspace_root;

/// The steps of the advisory checks.
const ADVISORY_STEPS: [&str; 6] = [
    "cargo-deny",
    "ui-npm-audit",
    "desktop-npm-audit",
    "docs-site-npm-audit",
    "mobile-npm-audit",
    "image-scan",
];

/// A host with no native toolchain of the Mobile App.
const NO_TOOLCHAINS: Toolchains = Toolchains {
    android: false,
    ios: false,
};

/// A host with the JDK 21, the Android SDK and Xcode.
const EVERY_TOOLCHAIN: Toolchains = Toolchains {
    android: true,
    ios: true,
};

fn step_names(lanes: &[Lane]) -> Vec<&'static str> {
    lanes
        .iter()
        .flat_map(|lane| lane.steps.iter().map(|step| step.name))
        .collect()
}

fn plan(root: &Path, docker_available: bool) -> Vec<xtask::Step> {
    full_lanes(root, docker_available, NO_TOOLCHAINS)
        .into_iter()
        .flat_map(|lane| lane.steps)
        .collect()
}

fn write_package_json(root: &Path, package: &str, scripts: &[&str]) {
    let scripts_json: Vec<String> = scripts
        .iter()
        .map(|s| format!("\"{s}\": \"echo {s}\""))
        .collect();
    let body = format!("{{\"scripts\": {{{}}}}}", scripts_json.join(", "));
    std::fs::create_dir_all(root.join(package)).unwrap();
    std::fs::write(root.join(package).join("package.json"), body).unwrap();
}

fn write_ui_package_json(root: &Path, scripts: &[&str]) {
    write_package_json(root, "ui", scripts);
}

fn write_rust_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/base\", \"crates/app\", \"crates/other\"]\n",
    )
    .unwrap();
    for (path, name, dependency) in [
        ("crates/base", "base", ""),
        (
            "crates/app",
            "app",
            "base_alias = { package = \"base\", path = \"../base\" }",
        ),
        ("crates/other", "other", ""),
    ] {
        std::fs::create_dir_all(root.join(path).join("src")).unwrap();
        std::fs::write(root.join(path).join("src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join(path).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n[dependencies]\n{dependency}\n"
            ),
        )
        .unwrap();
    }
}

/// A workspace with the package that the Computer checks select.
fn write_computer_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/pagis-computer\"]\n",
    )
    .unwrap();
    let package = root.join("crates/pagis-computer");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(package.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"pagis-computer\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
}

// --- plan ---

#[test]
fn dev_plan_tests_a_changed_crate_and_its_reverse_dependants() {
    let tmp = tempfile::tempdir().unwrap();
    write_rust_workspace(tmp.path());
    let lanes = dev_lanes(
        tmp.path(),
        &["crates/base/src/lib.rs".into()],
        false,
        NO_TOOLCHAINS,
    )
    .unwrap();
    let steps: Vec<_> = lanes.into_iter().flat_map(|lane| lane.steps).collect();
    let test = steps.iter().find(|step| step.name == "test").unwrap();
    let Action::Run(cmds) = &test.action else {
        panic!("tests must run");
    };
    assert!(cmds[0].args.windows(2).any(|pair| pair == ["-p", "base"]));
    assert!(cmds[0].args.windows(2).any(|pair| pair == ["-p", "app"]));
    assert!(!cmds[0].args.iter().any(|arg| arg == "other"));
}

#[test]
fn dev_plan_runs_the_ui_checks_and_the_secret_scan_for_a_ui_change() {
    let tmp = tempfile::tempdir().unwrap();
    write_ui_package_json(tmp.path(), &["typecheck", "test"]);
    let lanes = dev_lanes(tmp.path(), &["ui/src/App.tsx".into()], false, NO_TOOLCHAINS).unwrap();
    let names: Vec<_> = lanes
        .iter()
        .flat_map(|lane| lane.steps.iter().map(|step| step.name))
        .collect();
    assert_eq!(names, ["ui-deps", "ui-typecheck", "ui-test", "secret-scan"]);
}

/// A page of the documentation site is MDX that the build compiles, so a
/// change to the content runs the checks and the build of the site.
#[test]
fn dev_plan_runs_the_docs_site_checks_and_the_secret_scan_for_a_docs_site_change() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(
        tmp.path(),
        "docs-site",
        &["typecheck", "test", "build", "test:export"],
    );
    let lanes = dev_lanes(
        tmp.path(),
        &["docs-site/content/index.mdx".into()],
        false,
        NO_TOOLCHAINS,
    )
    .unwrap();
    assert_eq!(
        step_names(&lanes),
        [
            "docs-site-deps",
            "docs-site-typecheck",
            "docs-site-test",
            "docs-site-build",
            "docs-site-export",
            "secret-scan"
        ]
    );
}

/// The steps of the Mobile App, in the order of the gate.
const MOBILE_STEPS: [&str; 5] = [
    "mobile-deps",
    "mobile-typecheck",
    "mobile-test",
    "mobile-android-test",
    "mobile-ios-test",
];

fn mobile_step_names(lanes: &[Lane]) -> Vec<&'static str> {
    step_names(lanes)
        .into_iter()
        .filter(|name| name.starts_with("mobile-"))
        .collect()
}

/// A change to the web part of the Mobile App runs each mobile step and
/// the secret scan, and not the full gate.
#[test]
fn dev_plan_runs_the_mobile_checks_and_the_secret_scan_for_a_mobile_change() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "mobile", &["build", "typecheck", "test"]);
    let lanes = dev_lanes(
        tmp.path(),
        &["mobile/src/connect.ts".into()],
        false,
        NO_TOOLCHAINS,
    )
    .unwrap();
    let mut expected = MOBILE_STEPS.to_vec();
    expected.push("secret-scan");
    assert_eq!(step_names(&lanes), expected);
    assert_ne!(lanes, full_lanes(tmp.path(), false, NO_TOOLCHAINS));
}

/// A change to one native project runs the native tests of that platform
/// alone.
#[test]
fn dev_plan_runs_the_native_tests_of_the_changed_platform_only() {
    let tmp = tempfile::tempdir().unwrap();
    let names = |paths: &[&str]| {
        let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        mobile_step_names(&dev_lanes(tmp.path(), &paths, false, NO_TOOLCHAINS).unwrap())
    };
    assert_eq!(
        names(&["mobile/android/app/src/main/java/co/pagis/mobile/MainActivity.java"]),
        ["mobile-deps", "mobile-android-test"]
    );
    assert_eq!(
        names(&["mobile/ios/App/App/PagisViewController.swift"]),
        ["mobile-deps", "mobile-ios-test"]
    );
    assert_eq!(
        names(&[
            "mobile/android/app/build.gradle",
            "mobile/ios/App/App/Info.plist"
        ]),
        ["mobile-deps", "mobile-android-test", "mobile-ios-test"]
    );
    assert_eq!(
        names(&["mobile/ios/App/App/Info.plist", "mobile/package.json"]),
        MOBILE_STEPS
    );
}

#[test]
fn dev_plan_runs_no_mobile_step_for_a_ui_change() {
    let tmp = tempfile::tempdir().unwrap();
    write_ui_package_json(tmp.path(), &["typecheck", "test"]);
    let lanes = dev_lanes(tmp.path(), &["ui/src/App.tsx".into()], false, NO_TOOLCHAINS).unwrap();
    assert!(mobile_step_names(&lanes).is_empty(), "{lanes:?}");
}

/// The mobile lane runs no Cargo, so it runs beside the other lanes.
#[test]
fn the_mobile_steps_run_no_cargo() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "mobile", &["build", "typecheck", "test"]);
    let lanes = full_lanes(tmp.path(), true, EVERY_TOOLCHAIN);
    let mobile = lanes.iter().find(|lane| lane.name == "mobile").unwrap();
    for step in &mobile.steps {
        let Action::Run(cmds) = &step.action else {
            panic!("{} must run, got {:?}", step.name, step.action);
        };
        assert!(
            cmds.iter().all(|cmd| cmd.program != "cargo"),
            "{}: {cmds:?}",
            step.name
        );
    }
}

/// The web checks of the Mobile App are its npm scripts, in `mobile/`.
#[test]
fn plan_runs_the_mobile_web_checks_in_the_mobile_package() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "mobile", &["build", "typecheck", "test"]);
    let steps = plan(tmp.path(), false);
    let mobile = tmp.path().join("mobile");
    for (name, args) in [
        ("mobile-deps", vec!["ci"]),
        ("mobile-typecheck", vec!["run", "typecheck"]),
        ("mobile-test", vec!["run", "test"]),
    ] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        let Action::Run(cmds) = &step.action else {
            panic!("{name} must run, got {:?}", step.action);
        };
        assert_eq!(cmds.len(), 1, "{name}");
        assert_eq!(cmds[0].program, "npm", "{name}");
        assert_eq!(cmds[0].args, args, "{name}");
        assert_eq!(cmds[0].cwd.as_deref(), Some(mobile.as_path()), "{name}");
    }
}

/// A native step builds the web part and syncs it into the native project
/// before the tests, so no test runs against stale generated files.
#[test]
fn a_native_step_syncs_the_native_project_before_its_tests() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "mobile", &["build", "typecheck", "test"]);
    let steps: Vec<_> = full_lanes(tmp.path(), false, EVERY_TOOLCHAIN)
        .into_iter()
        .flat_map(|lane| lane.steps)
        .collect();
    let mobile = tmp.path().join("mobile");
    let commands = |name: &str| {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        let Action::Run(cmds) = &step.action else {
            panic!("{name} must run, got {:?}", step.action);
        };
        cmds.clone()
    };

    let android = commands("mobile-android-test");
    assert_eq!(android.len(), 3, "{android:?}");
    assert_eq!(
        (android[0].program.as_str(), &android[0].args[..]),
        ("npm", &["run".to_string(), "build".into()][..])
    );
    assert_eq!(android[1].program, "npx");
    assert_eq!(android[1].args, ["cap", "sync", "android"]);
    assert_eq!(android[2].program, "./gradlew");
    assert_eq!(android[2].args, ["testDebugUnitTest"]);
    assert_eq!(android[0].cwd.as_deref(), Some(mobile.as_path()));
    assert_eq!(android[1].cwd.as_deref(), Some(mobile.as_path()));
    assert_eq!(
        android[2].cwd.as_deref(),
        Some(mobile.join("android").as_path())
    );

    let ios = commands("mobile-ios-test");
    assert_eq!(ios.len(), 3, "{ios:?}");
    assert_eq!(ios[0].args, ["run", "build"]);
    assert_eq!(ios[1].args, ["cap", "sync", "ios"]);
    let test = ios[2].args.join(" ");
    assert!(
        test.contains("xcrun simctl list devices available --json"),
        "{test}"
    );
    assert!(test.contains("iPhone"), "{test}");
    assert!(
        test.contains("xcodebuild test -project App/App.xcodeproj -scheme App"),
        "{test}"
    );
    assert_eq!(ios[2].cwd.as_deref(), Some(mobile.join("ios").as_path()));
}

/// The skip message of each native step in `full`, and the refusal of
/// `cargo xtask step` with the same message. A CI job names its steps,
/// so CI never passes a native step that did not run.
#[test]
fn a_native_step_without_its_toolchain_skips_in_the_gate_and_fails_when_named() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "mobile", &["build", "typecheck", "test"]);
    let android = "no JDK 21 (java) and Android SDK (ANDROID_HOME)";
    let ios = "no Xcode (xcodebuild) on a macOS host";

    let steps = plan(tmp.path(), false);
    for (name, reason) in [("mobile-android-test", android), ("mobile-ios-test", ios)] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert_eq!(step.action, Action::Skip(reason.into()), "{name}");

        let named = named_steps(tmp.path(), &[name.into()], false, NO_TOOLCHAINS).unwrap();
        assert_eq!(
            named[0].action,
            Action::Run(vec![Cmd::refusal(&format!("{name}: {reason}"))]),
            "{name}"
        );
        let results = execute(&named);
        assert_eq!(results[0].outcome, Outcome::Failed, "{name}");
    }

    // With its toolchain, a named native step is the step of the gate.
    let named = named_steps(
        tmp.path(),
        &["mobile-android-test".into(), "mobile-ios-test".into()],
        false,
        EVERY_TOOLCHAIN,
    )
    .unwrap();
    let gate: Vec<_> = full_lanes(tmp.path(), false, EVERY_TOOLCHAIN)
        .into_iter()
        .flat_map(|lane| lane.steps)
        .collect();
    for step in &named {
        assert_eq!(Some(step), gate.iter().find(|s| s.name == step.name));
    }
}

/// The probe reads the major version from the first line of
/// `java -version`, which a JDK prints on stderr.
#[test]
fn the_android_probe_reads_the_major_version_of_java() {
    use xtask::mobile::java_major_version;
    assert_eq!(
        java_major_version(
            "openjdk version \"21.0.8\" 2025-07-15 LTS\nOpenJDK Runtime Environment"
        ),
        Some(21)
    );
    assert_eq!(
        java_major_version("openjdk version \"25\" 2025-09-16"),
        Some(25)
    );
    assert_eq!(java_major_version("java version \"1.8.0_461\""), Some(8));
    assert_eq!(
        java_major_version("java version \"17.0.16\" 2025-07-15 LTS"),
        Some(17)
    );
    assert_eq!(
        java_major_version("The operation couldn’t be completed. Unable to locate a Java Runtime."),
        None
    );
    assert_eq!(java_major_version(""), None);
}

#[test]
fn dev_plan_falls_back_to_full_for_shared_build_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    write_rust_workspace(tmp.path());
    let dev = dev_lanes(tmp.path(), &["Cargo.toml".into()], false, NO_TOOLCHAINS).unwrap();
    assert_eq!(dev, full_lanes(tmp.path(), false, NO_TOOLCHAINS));
}

/// The other files of the pin rules select the full gate. A change to
/// `computer/Dockerfile` selects the Computer checks, and the pin check
/// with them.
#[test]
fn dev_plan_checks_the_pins_for_a_change_to_the_computer_dockerfile() {
    let tmp = tempfile::tempdir().unwrap();
    write_computer_workspace(tmp.path());
    let names = |path: &str| {
        step_names(&dev_lanes(tmp.path(), &[path.into()], false, NO_TOOLCHAINS).unwrap())
            .into_iter()
            .filter(|name| *name == "pins")
            .count()
    };
    assert_eq!(names("computer/Dockerfile"), 1);
    assert_eq!(names("computer/entrypoint.sh"), 0);
    for path in [
        ".github/workflows/ci.yml",
        ".github/dependabot.yml",
        "Dockerfile",
        "deploy/compose.yaml",
    ] {
        assert_eq!(
            dev_lanes(tmp.path(), &[path.into()], false, NO_TOOLCHAINS).unwrap(),
            full_lanes(tmp.path(), false, NO_TOOLCHAINS),
            "{path}"
        );
    }
}

/// A change to the Dockerfile of the Push Relay image checks the relay
/// crate and the pins with it. Another change to the relay does not
/// check the pins.
#[test]
fn dev_plan_checks_the_pins_for_a_change_to_the_push_relay_dockerfile() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/pagis-push-relay\"]\n",
    )
    .unwrap();
    let package = tmp.path().join("crates/pagis-push-relay");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(package.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"pagis-push-relay\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let names = |path: &str| {
        step_names(&dev_lanes(tmp.path(), &[path.into()], false, NO_TOOLCHAINS).unwrap())
    };

    let dockerfile = names("crates/pagis-push-relay/Dockerfile");
    assert_eq!(
        dockerfile.iter().filter(|name| **name == "pins").count(),
        1,
        "{dockerfile:?}"
    );
    assert!(dockerfile.contains(&"test"), "{dockerfile:?}");
    let source = names("crates/pagis-push-relay/src/lib.rs");
    assert!(!source.contains(&"pins"), "{source:?}");
    assert_eq!(
        dev_lanes(
            tmp.path(),
            &["deploy/push-relay/compose.yaml".into()],
            false,
            NO_TOOLCHAINS
        )
        .unwrap(),
        full_lanes(tmp.path(), false, NO_TOOLCHAINS),
        "the Compose file of the relay selects the full gate"
    );
}

/// A change to screend, or to the Dockerfile that pins its builder image,
/// runs the tests of screend after the Computer Image build. Another
/// change to the Computer does not.
#[test]
fn dev_plan_tests_screend_for_a_change_to_screend_or_its_dockerfile() {
    let tmp = tempfile::tempdir().unwrap();
    write_computer_workspace(tmp.path());
    let names = |path: &str| {
        step_names(&dev_lanes(tmp.path(), &[path.into()], true, NO_TOOLCHAINS).unwrap())
    };

    for path in ["computer/screend/src/exit.rs", "computer/Dockerfile"] {
        let steps = names(path);
        let image = steps.iter().position(|name| *name == "computer-image");
        let screend = steps.iter().position(|name| *name == "screend-test");
        assert!(
            image.is_some() && screend == image.map(|at| at + 1),
            "{path}: {steps:?}"
        );
    }
    assert!(!names("computer/entrypoint.sh").contains(&"screend-test"));
    assert!(!names("crates/pagis-computer/src/lib.rs").contains(&"screend-test"));
}

/// A workspace with the Google adapter, a package that the adapter
/// depends on, a package that depends on the adapter, and one apart.
fn write_google_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/versions\", \"crates/pagis-google\", \"crates/app\", \"crates/other\"]\n",
    )
    .unwrap();
    for (path, name, dependency) in [
        ("crates/versions", "versions", ""),
        (
            "crates/pagis-google",
            "pagis-google",
            "versions = { path = \"../versions\" }",
        ),
        (
            "crates/app",
            "app",
            "pagis-google = { path = \"../pagis-google\" }",
        ),
        ("crates/other", "other", ""),
    ] {
        std::fs::create_dir_all(root.join(path).join("src")).unwrap();
        std::fs::write(root.join(path).join("src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join(path).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n[dependencies]\n{dependency}\n"
            ),
        )
        .unwrap();
    }
}

/// A change to the Google adapter, or to a package that it depends on,
/// runs the contract check with the pinned `gog` after the tests. Another
/// change does not.
#[test]
fn dev_plan_checks_the_google_adapter_against_the_pinned_gog() {
    let tmp = tempfile::tempdir().unwrap();
    write_google_workspace(tmp.path());
    let names = |path: &str| {
        step_names(&dev_lanes(tmp.path(), &[path.into()], false, NO_TOOLCHAINS).unwrap())
    };

    for path in [
        "crates/pagis-google/src/lib.rs",
        "crates/versions/src/lib.rs",
    ] {
        let steps = names(path);
        let test = steps.iter().position(|name| *name == "test").unwrap();
        assert_eq!(steps[test + 1], "gog-contract", "{path}: {steps:?}");
    }
    assert!(!names("crates/other/src/lib.rs").contains(&"gog-contract"));
    assert!(!names("crates/app/src/lib.rs").contains(&"gog-contract"));
}

/// The test step runs the Docker tests of the changed packages, but not
/// the pinned `gog` tests: they need `PAGIS_GOG`, which the contract
/// check supplies.
#[test]
fn dev_plan_leaves_the_pinned_gog_tests_to_the_contract_check() {
    let tmp = tempfile::tempdir().unwrap();
    write_computer_workspace(tmp.path());
    let lanes = dev_lanes(
        tmp.path(),
        &["crates/pagis-computer/src/lib.rs".into()],
        true,
        NO_TOOLCHAINS,
    )
    .unwrap();
    let test = lanes
        .iter()
        .flat_map(|lane| lane.steps.iter())
        .find(|step| step.name == "test")
        .unwrap();
    let Action::Run(cmds) = &test.action else {
        panic!("tests must run");
    };
    let filter = cmds[0].args.last().unwrap();
    assert!(filter.starts_with("not test("), "{filter}");
    assert!(filter.contains("^pinned_gog::"), "{filter}");
}

#[test]
fn dev_plan_uses_a_cheap_whitespace_check_for_docs_only() {
    let tmp = tempfile::tempdir().unwrap();
    let lanes = dev_lanes(tmp.path(), &["docs/guide.md".into()], false, NO_TOOLCHAINS).unwrap();
    let names: Vec<(&str, Vec<&str>)> = lanes
        .iter()
        .map(|l| (l.name, l.steps.iter().map(|s| s.name).collect()))
        .collect();
    assert_eq!(
        names,
        [
            ("docs", vec!["diff-check"]),
            ("secrets", vec!["secret-scan"])
        ]
    );
    let Action::Run(cmds) = &lanes[0].steps[0].action else {
        panic!("diff check must run");
    };
    assert_eq!(cmds[0].program, "git");
    assert_eq!(cmds[0].args, ["diff", "--check"]);
}

/// Every change to the tree can carry a secret, so the development check
/// scans the tracked files whenever a path changed.
#[test]
fn dev_plan_scans_for_secrets_when_a_path_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let scans = |paths: &[String]| {
        dev_lanes(tmp.path(), paths, false, NO_TOOLCHAINS)
            .unwrap()
            .iter()
            .flat_map(|lane| lane.steps.iter().map(|step| step.name))
            .filter(|name| *name == "secret-scan")
            .count()
    };
    assert_eq!(scans(&["docs/guide.md".into()]), 1);
    assert_eq!(scans(&["ui/src/App.tsx".into()]), 1);
    assert_eq!(scans(&["deploy/.env.example".into()]), 1);
    assert_eq!(scans(&[]), 0);
}

/// A newly published advisory must not block an unrelated pull request,
/// so the full gate runs no advisory check.
#[test]
fn the_full_gate_runs_no_advisory_check() {
    let tmp = tempfile::tempdir().unwrap();
    for docker_available in [true, false] {
        let names = step_names(&full_lanes(tmp.path(), docker_available, NO_TOOLCHAINS));
        for advisory in ADVISORY_STEPS {
            assert!(!names.contains(&advisory), "the gate runs {advisory}");
        }
    }
}

/// `cargo xtask dev` runs no advisory check, also when a shared
/// configuration change selects the full gate.
#[test]
fn a_shared_configuration_change_runs_no_advisory_check() {
    let tmp = tempfile::tempdir().unwrap();
    write_rust_workspace(tmp.path());
    for path in ["Cargo.toml", "Cargo.lock", "deny.toml", ".trivyignore"] {
        let lanes = dev_lanes(tmp.path(), &[path.into()], true, NO_TOOLCHAINS).unwrap();
        assert_eq!(lanes, full_lanes(tmp.path(), true, NO_TOOLCHAINS), "{path}");
        let names = step_names(&lanes);
        for advisory in ADVISORY_STEPS {
            assert!(!names.contains(&advisory), "{path} runs {advisory}");
        }
    }
}

#[test]
fn plan_lists_every_gate_step_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "fmt",
            "clippy",
            "computer-image",
            "screend-test",
            "test",
            "gog-contract",
            "emergency-drift",
            "pins",
            "contract-drift",
            "pagis-apt",
            "ui-deps",
            "ui-typecheck",
            "ui-test",
            "desktop-deps",
            "desktop-typecheck",
            "desktop-test",
            "docs-site-deps",
            "docs-site-typecheck",
            "docs-site-test",
            "docs-site-build",
            "docs-site-export",
            "mobile-deps",
            "mobile-typecheck",
            "mobile-test",
            "mobile-android-test",
            "mobile-ios-test",
            "secret-scan",
        ]
    );
}

/// The tree that becomes public is the tracked tree, so the gate scans
/// it for secrets with or without Docker, in a lane of its own.
#[test]
fn the_gate_scans_the_tracked_tree_for_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    for docker_available in [true, false] {
        let lanes = full_lanes(tmp.path(), docker_available, NO_TOOLCHAINS);
        let lane = lanes
            .iter()
            .find(|lane| lane.name == "secrets")
            .expect("a secrets lane");
        assert_eq!(lane.steps.len(), 1);
        assert_eq!(lane.steps[0].name, "secret-scan");
        let Action::Run(cmds) = &lane.steps[0].action else {
            panic!("the secret scan must run, got {:?}", lane.steps[0].action);
        };
        let script = cmds
            .iter()
            .map(|cmd| cmd.args.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(script.contains("gitleaks"), "{script}");
        assert!(script.contains("git add --update"), "{script}");
        for cmd in cmds {
            assert_eq!(cmd.cwd.as_deref(), Some(tmp.path()));
        }
    }
}

#[test]
fn plan_runs_cargo_steps_unconditionally() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    for name in [
        "fmt",
        "clippy",
        "computer-image",
        "test",
        "emergency-drift",
        "pins",
    ] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert!(
            matches!(step.action, Action::Run(_)),
            "step {name} must run, got {:?}",
            step.action
        );
    }
}

/// The pin rules are a stored check, so a change that adds a mutable
/// reference fails the gate also when no workflow runs.
#[test]
fn plan_checks_the_pins_of_the_third_party_code() {
    let tmp = tempfile::tempdir().unwrap();
    for docker_available in [true, false] {
        let steps = plan(tmp.path(), docker_available);
        let step = steps.iter().find(|s| s.name == "pins").unwrap();
        let Action::Run(cmds) = &step.action else {
            panic!("expected a run, got {:?}", step.action);
        };
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "cargo");
        assert_eq!(
            cmds[0].args,
            ["run", "--package", "xtask", "--", "pins", "--check"]
        );
        assert_eq!(cmds[0].cwd.as_deref(), Some(tmp.path()));
    }
}

#[test]
fn plan_runs_all_hermetic_and_docker_tests_in_one_nextest_command() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    let step = steps.iter().find(|s| s.name == "test").unwrap();
    let Action::Run(cmds) = &step.action else {
        panic!("expected a run, got {:?}", step.action);
    };
    assert_eq!(cmds[0].program, "cargo");
    assert_eq!(
        cmds[0].args,
        [
            "nextest",
            "run",
            "--workspace",
            "--run-ignored",
            "all",
            "-E",
            "not test(/(^live_mail::|^live_migadu::|^release_evaluation::|^a_live_mailbox_logs_in_sends_to_itself|^pinned_gog::)/)"
        ]
    );
}

#[test]
fn plan_compiles_incrementally() {
    // The gate reuses the artifacts of the earlier builds, so it must not
    // turn incremental compilation off.
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    for name in ["clippy", "test", "emergency-drift", "pins"] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        let Action::Run(cmds) = &step.action else {
            panic!("expected a run, got {:?}", step.action);
        };
        assert!(
            !cmds[0]
                .env
                .iter()
                .any(|(key, _)| key == "CARGO_INCREMENTAL"),
            "step {name} must not set CARGO_INCREMENTAL, got {:?}",
            cmds[0].env
        );
    }
}

#[test]
fn plan_runs_only_hermetic_tests_without_docker() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), false);
    let step = steps.iter().find(|s| s.name == "test").unwrap();
    let Action::Run(cmds) = &step.action else {
        panic!("expected a run, got {:?}", step.action);
    };
    assert_eq!(cmds[0].args, ["nextest", "run", "--workspace"]);
}

#[test]
fn plan_builds_the_computer_image_once_before_the_combined_tests() {
    // Tests that built the image themselves would build it once per
    // process, which under nextest is once per test.
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    let step = steps.iter().find(|s| s.name == "computer-image").unwrap();
    let Action::Run(cmds) = &step.action else {
        panic!("expected a run, got {:?}", step.action);
    };
    // CI sets the layer cache of the build, so the plan reads it from
    // the environment the test runs in.
    assert_eq!(
        cmds[..],
        [xtask::computer_image_build(
            tmp.path(),
            xtask::ImageCache::from_env().as_ref()
        )]
    );

    let local = xtask::computer_image_build(tmp.path(), None);
    assert_eq!(local.program, "docker");
    assert_eq!(
        local.args,
        ["build", "-t", COMPUTER_IMAGE, "computer"],
        "the tag is the one the daemon pins"
    );
    assert_eq!(local.cwd.as_deref(), Some(tmp.path()));
}

#[test]
fn a_ci_build_of_the_computer_image_reads_and_writes_its_layer_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let on_main = xtask::ImageCache {
        from: "type=gha,scope=computer-image".into(),
        to: Some("type=gha,scope=computer-image,mode=max".into()),
    };
    let build = xtask::computer_image_build(tmp.path(), Some(&on_main));
    assert_eq!(build.program, "docker");
    assert_eq!(
        build.args,
        [
            "buildx",
            "build",
            "--load",
            "-t",
            COMPUTER_IMAGE,
            "--cache-from",
            "type=gha,scope=computer-image",
            "--cache-to",
            "type=gha,scope=computer-image,mode=max",
            "computer",
        ]
    );

    let on_a_pull_request = xtask::ImageCache {
        to: None,
        ..on_main
    };
    let build = xtask::computer_image_build(tmp.path(), Some(&on_a_pull_request));
    assert!(!build.args.iter().any(|arg| arg == "--cache-to"));
}

#[test]
fn plan_skips_the_image_build_without_docker() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), false);
    let step = steps.iter().find(|s| s.name == "computer-image").unwrap();
    assert_eq!(step.action, Action::Skip("Docker unavailable".into()));
}

#[test]
fn plan_runs_the_pagis_apt_shell_test_when_it_exists() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("computer/tests")).unwrap();
    std::fs::write(
        tmp.path().join("computer/tests/pagis-apt-validate.sh"),
        "exit 0\n",
    )
    .unwrap();
    let steps = plan(tmp.path(), true);
    let step = steps.iter().find(|s| s.name == "pagis-apt").unwrap();
    let Action::Run(cmds) = &step.action else {
        panic!("expected a run, got {:?}", step.action);
    };
    assert_eq!(cmds[0].program, "bash");
    assert_eq!(cmds[0].args, ["computer/tests/pagis-apt-validate.sh"]);
}

#[test]
fn plan_skips_the_pagis_apt_shell_test_when_it_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    let step = steps.iter().find(|s| s.name == "pagis-apt").unwrap();
    assert!(matches!(step.action, Action::Skip(_)));
}

#[test]
fn plan_skips_ui_steps_when_ui_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    for name in ["ui-deps", "contract-drift", "ui-typecheck", "ui-test"] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert!(
            matches!(step.action, Action::Skip(_)),
            "step {name} must skip without ui/"
        );
    }
}

#[test]
fn plan_skips_desktop_steps_when_desktop_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    for name in ["desktop-deps", "desktop-typecheck", "desktop-test"] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert!(
            matches!(step.action, Action::Skip(_)),
            "step {name} must skip without desktop/"
        );
    }
}

#[test]
fn plan_skips_docs_site_steps_when_docs_site_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let steps = plan(tmp.path(), true);
    for name in [
        "docs-site-deps",
        "docs-site-typecheck",
        "docs-site-test",
        "docs-site-build",
        "docs-site-export",
    ] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert!(
            matches!(step.action, Action::Skip(_)),
            "step {name} must skip without docs-site/"
        );
    }
}

#[test]
fn plan_runs_the_docs_site_steps_whose_scripts_are_declared() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(
        tmp.path(),
        "docs-site",
        &["typecheck", "test", "build", "test:export"],
    );
    let steps = plan(tmp.path(), true);
    for (name, script) in [
        ("docs-site-typecheck", "typecheck"),
        ("docs-site-test", "test"),
        ("docs-site-build", "build"),
        ("docs-site-export", "test:export"),
    ] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        let Action::Run(cmds) = &step.action else {
            panic!("{name} must run");
        };
        assert_eq!(cmds[0].args, ["run", script]);
        assert_eq!(
            cmds[0].cwd.as_deref(),
            Some(tmp.path().join("docs-site").as_path())
        );
    }
}

#[test]
fn plan_runs_the_desktop_steps_whose_scripts_are_declared() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "desktop", &["typecheck", "test"]);
    let steps = plan(tmp.path(), true);
    for (name, script) in [("desktop-typecheck", "typecheck"), ("desktop-test", "test")] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        match &step.action {
            Action::Run(cmds) => {
                assert_eq!(cmds[0].program, "npm");
                assert_eq!(cmds[0].args, ["run", script]);
                assert_eq!(
                    cmds[0].cwd.as_deref(),
                    Some(tmp.path().join("desktop")).as_deref()
                );
            }
            other => panic!("step {name} must run, got {other:?}"),
        }
    }
}

#[test]
fn plan_installs_ui_deps_only_while_node_modules_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    write_ui_package_json(tmp.path(), &[]);
    let steps = plan(tmp.path(), true);
    let deps = steps.iter().find(|s| s.name == "ui-deps").unwrap();
    match &deps.action {
        Action::Run(cmds) => {
            assert_eq!(cmds[0].program, "npm");
            assert_eq!(cmds[0].args, ["ci"]);
        }
        other => panic!("ui-deps must run without node_modules, got {other:?}"),
    }

    std::fs::create_dir_all(tmp.path().join("ui/node_modules")).unwrap();
    let steps = plan(tmp.path(), true);
    let deps = steps.iter().find(|s| s.name == "ui-deps").unwrap();
    assert!(matches!(deps.action, Action::Skip(_)));
}

#[test]
fn plan_runs_ui_steps_whose_scripts_are_declared() {
    let tmp = tempfile::tempdir().unwrap();
    write_ui_package_json(tmp.path(), &["api:check", "typecheck", "test"]);
    let steps = plan(tmp.path(), true);
    for (name, script) in [
        ("contract-drift", "api:check"),
        ("ui-typecheck", "typecheck"),
        ("ui-test", "test"),
    ] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        match &step.action {
            Action::Run(cmds) => {
                assert_eq!(cmds.len(), 1);
                assert_eq!(cmds[0].program, "npm");
                assert_eq!(cmds[0].args, ["run", script]);
                assert_eq!(
                    cmds[0].cwd.as_deref(),
                    Some(tmp.path().join("ui")).as_deref()
                );
            }
            other => panic!("step {name} must run, got {other:?}"),
        }
    }
}

#[test]
fn plan_skips_ui_steps_whose_scripts_are_missing() {
    let tmp = tempfile::tempdir().unwrap();
    write_ui_package_json(tmp.path(), &["typecheck"]);
    let steps = plan(tmp.path(), true);
    let typecheck = steps.iter().find(|s| s.name == "ui-typecheck").unwrap();
    assert!(matches!(typecheck.action, Action::Run(_)));
    for name in ["contract-drift", "ui-test"] {
        let step = steps.iter().find(|s| s.name == name).unwrap();
        assert!(
            matches!(step.action, Action::Skip(_)),
            "step {name} must skip when its script is not declared"
        );
    }
}

// --- execute ---

fn step(name: &'static str, action: Action) -> xtask::Step {
    xtask::Step { name, action }
}

#[test]
fn execute_reports_pass_fail_and_skip() {
    let steps = vec![
        step("ok", Action::Run(vec![Cmd::new("true", &[])])),
        step("bad", Action::Run(vec![Cmd::new("false", &[])])),
        step("off", Action::Skip("not present yet".into())),
    ];
    let results = execute(&steps);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].outcome, Outcome::Passed);
    assert_eq!(results[1].outcome, Outcome::Failed);
    assert_eq!(
        results[2].outcome,
        Outcome::Skipped("not present yet".into())
    );
}

#[test]
fn execute_runs_every_step_after_a_failure() {
    let steps = vec![
        step("bad", Action::Run(vec![Cmd::new("false", &[])])),
        step("ok", Action::Run(vec![Cmd::new("true", &[])])),
    ];
    let results = execute(&steps);
    assert_eq!(results[1].outcome, Outcome::Passed);
}

/// A publish stops at the first failed step, so a push never runs after
/// the secret scan of its image failed.
#[test]
fn execute_until_failure_runs_no_step_after_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let pushed = tmp.path().join("pushed");
    let steps = vec![
        step("build", Action::Run(vec![Cmd::new("true", &[])])),
        step("scan", Action::Run(vec![Cmd::new("false", &[])])),
        step(
            "push",
            Action::Run(vec![Cmd::new("touch", &[pushed.to_str().unwrap()])]),
        ),
    ];
    let results = execute_until_failure(&steps);
    let outcomes: Vec<(&str, Outcome)> = results
        .iter()
        .map(|r| (r.name, r.outcome.clone()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("build", Outcome::Passed),
            ("scan", Outcome::Failed),
            ("push", Outcome::Skipped("scan failed".into())),
        ]
    );
    assert!(!pushed.exists(), "the push ran after the failed scan");
    assert!(!all_green(&results));
}

#[test]
fn execute_until_failure_runs_every_step_while_they_pass() {
    let steps = vec![
        step("one", Action::Run(vec![Cmd::new("true", &[])])),
        step("off", Action::Skip("not on this host".into())),
        step("two", Action::Run(vec![Cmd::new("true", &[])])),
    ];
    let results = execute_until_failure(&steps);
    assert_eq!(results.len(), 3);
    assert_eq!(results[2].outcome, Outcome::Passed);
    assert!(all_green(&results));
}

#[test]
fn execute_fails_a_step_with_a_missing_program() {
    let steps = vec![step(
        "ghost",
        Action::Run(vec![Cmd::new("xtask-no-such-program", &[])]),
    )];
    let results = execute(&steps);
    assert_eq!(results[0].outcome, Outcome::Failed);
}

#[test]
fn all_green_accepts_skips_and_rejects_failures() {
    let passed = StepResult {
        name: "a",
        outcome: Outcome::Passed,
        duration: Duration::ZERO,
    };
    let skipped = StepResult {
        name: "b",
        outcome: Outcome::Skipped("later".into()),
        duration: Duration::ZERO,
    };
    let failed = StepResult {
        name: "c",
        outcome: Outcome::Failed,
        duration: Duration::ZERO,
    };
    assert!(all_green(&[passed.clone(), skipped.clone()]));
    assert!(!all_green(&[passed, skipped, failed]));
}

// --- test objects ---

#[test]
fn a_test_object_whose_test_process_ended_is_orphaned() {
    let listed = "pagis-tenant-a\t4100-1f2e\n\
                  pagis-tenant-b\t4200-3c4d\n\
                  pagis-tenant-c\tnot-a-pid\n";
    let running = |pid: u32| pid == 4200;
    assert_eq!(
        orphaned_test_objects(listed, running),
        ["pagis-tenant-a", "pagis-tenant-c"]
    );
}

#[test]
fn a_test_object_of_a_running_test_is_kept() {
    let listed = "pagis-computer-a\t4200-3c4d\n";
    assert!(orphaned_test_objects(listed, |_| true).is_empty());
    assert!(orphaned_test_objects("", |_| false).is_empty());
}

#[test]
fn the_test_object_sweep_removes_containers_before_their_networks() {
    let cmds = test_object_sweep(&TestObjects {
        containers: vec!["pagis-computer-a".into(), "pagis-computer-b".into()],
        networks: vec!["pagis-tenant-a".into()],
        volumes: vec!["pagis-volume-a".into()],
    });
    let args: Vec<Vec<String>> = cmds.iter().map(|cmd| cmd.args.clone()).collect();
    assert!(cmds.iter().all(|cmd| cmd.program == "docker"));
    assert_eq!(
        args,
        [
            vec!["rm", "-f", "pagis-computer-a", "pagis-computer-b"],
            vec!["network", "rm", "pagis-tenant-a"],
            vec!["volume", "rm", "-f", "pagis-volume-a"],
        ]
    );
}

#[test]
fn the_test_object_sweep_of_nothing_runs_nothing() {
    assert!(test_object_sweep(&TestObjects::default()).is_empty());
    let cmds = test_object_sweep(&TestObjects {
        networks: vec!["pagis-tenant-a".into()],
        ..TestObjects::default()
    });
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].args, ["network", "rm", "pagis-tenant-a"]);
}

// --- lanes ---

#[test]
fn full_lanes_keep_every_cargo_consumer_in_one_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let lanes = full_lanes(tmp.path(), true, NO_TOOLCHAINS);
    let names: Vec<(&str, Vec<&str>)> = lanes
        .iter()
        .map(|l| (l.name, l.steps.iter().map(|s| s.name).collect()))
        .collect();
    assert_eq!(
        names,
        [
            (
                "cargo",
                vec![
                    "fmt",
                    "clippy",
                    "computer-image",
                    "screend-test",
                    "test",
                    "gog-contract",
                    "emergency-drift",
                    "pins",
                    "contract-drift",
                ]
            ),
            (
                "node",
                vec![
                    "pagis-apt",
                    "ui-deps",
                    "ui-typecheck",
                    "ui-test",
                    "desktop-deps",
                    "desktop-typecheck",
                    "desktop-test",
                    "docs-site-deps",
                    "docs-site-typecheck",
                    "docs-site-test",
                    "docs-site-build",
                    "docs-site-export",
                ]
            ),
            (
                "mobile",
                vec![
                    "mobile-deps",
                    "mobile-typecheck",
                    "mobile-test",
                    "mobile-android-test",
                    "mobile-ios-test",
                ]
            ),
            ("secrets", vec!["secret-scan"]),
        ]
    );
}

#[test]
fn execute_lanes_runs_the_lanes_at_the_same_time() {
    // The waiter only ends when the other lane writes the flag, so the
    // test passes only when both lanes run at once.
    let tmp = tempfile::tempdir().unwrap();
    let flag = tmp.path().join("flag with spaces");
    let wait = "for ((i=0; i<100; i++)); do [ -f \"$1\" ] && exit 0; sleep 0.1; done; exit 1";
    let lanes = vec![
        Lane {
            name: "waiter",
            steps: vec![step(
                "wait",
                Action::Run(vec![Cmd::new(
                    "bash",
                    &["-c", wait, "wait", flag.to_str().unwrap()],
                )]),
            )],
        },
        Lane {
            name: "flagger",
            steps: vec![
                step("first", Action::Run(vec![Cmd::new("true", &[])])),
                step(
                    "flag",
                    Action::Run(vec![Cmd::new("touch", &[flag.to_str().unwrap()])]),
                ),
            ],
        },
    ];
    let results = execute_lanes(&lanes);
    let names: Vec<&str> = results.iter().map(|r| r.name).collect();
    assert_eq!(
        names,
        ["wait", "first", "flag"],
        "results keep the plan order"
    );
    assert!(all_green(&results), "{results:?}");
}

#[test]
fn execute_lanes_keeps_a_failed_lane_going_and_reports_it() {
    let lanes = vec![Lane {
        name: "one",
        steps: vec![
            step("bad", Action::Run(vec![Cmd::new("false", &[])])),
            step("ok", Action::Run(vec![Cmd::new("true", &[])])),
        ],
    }];
    let results = execute_lanes(&lanes);
    assert_eq!(results[0].outcome, Outcome::Failed);
    assert_eq!(results[1].outcome, Outcome::Passed);
}

#[test]
fn contract_check_shares_the_cargo_lane_and_compiles_incrementally() {
    let tmp = tempfile::tempdir().unwrap();
    write_package_json(tmp.path(), "ui", &["api:check"]);
    let lanes = full_lanes(tmp.path(), false, NO_TOOLCHAINS);
    let cargo = lanes.iter().find(|lane| lane.name == "cargo").unwrap();
    let contract = cargo.steps.last().unwrap();
    assert_eq!(contract.name, "contract-drift");
    let Action::Run(cmds) = &contract.action else {
        panic!("contract check must run");
    };
    assert!(
        !cmds[0]
            .env
            .iter()
            .any(|(key, _)| key == "CARGO_INCREMENTAL")
    );
}

// --- named steps ---

#[test]
fn named_steps_are_the_gate_steps_in_the_order_of_the_names() {
    let tmp = tempfile::tempdir().unwrap();
    let names: Vec<String> = ["test", "computer-image", "fmt"].map(String::from).into();
    let steps = named_steps(tmp.path(), &names, true, NO_TOOLCHAINS).unwrap();
    let gate = plan(tmp.path(), true);
    for (step, name) in steps.iter().zip(&names) {
        assert_eq!(step.name, name);
        assert_eq!(Some(step), gate.iter().find(|s| s.name == name));
    }
    assert_eq!(steps.len(), 3);
}

#[test]
fn a_name_that_is_not_a_gate_step_is_refused_with_the_step_names() {
    let tmp = tempfile::tempdir().unwrap();
    let error = named_steps(
        tmp.path(),
        &["fmt".into(), "lint".into()],
        false,
        NO_TOOLCHAINS,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("no step `lint`"), "{error}");
    assert!(
        error.contains("fmt, clippy, computer-image, screend-test, test"),
        "{error}"
    );
}

// --- the CI workflow ---

/// The step names of each `cargo xtask step` command in `workflow`.
fn ci_step_runs(workflow: &str) -> Vec<Vec<String>> {
    workflow
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("- ")
                .unwrap_or(line)
                .strip_prefix("run: cargo xtask step ")
        })
        .map(|names| names.split_whitespace().map(String::from).collect())
        .collect()
}

/// CI is the merge signal, so its jobs run each step of the full gate,
/// and they run the steps through `cargo xtask step`, which is the code
/// that `cargo xtask full` runs.
#[test]
fn the_ci_workflow_runs_each_gate_step() {
    let workflow = std::fs::read_to_string(workspace_root().join(".github/workflows/ci.yml"))
        .expect("read .github/workflows/ci.yml");
    let mut run: Vec<String> = ci_step_runs(&workflow).into_iter().flatten().collect();
    let tmp = tempfile::tempdir().unwrap();
    let mut gate: Vec<String> = step_names(&full_lanes(tmp.path(), true, NO_TOOLCHAINS))
        .into_iter()
        .map(String::from)
        .collect();
    run.sort();
    run.dedup();
    gate.sort();
    assert_eq!(run, gate);
}

/// Docker Hub refuses the anonymous pulls of a shared runner address
/// with `429 Too Many Requests`. The test job pulls its base images and
/// its test images through mirror.gcr.io: the Docker daemon for `docker
/// run`, and the buildx builder for the image build.
#[test]
fn the_ci_test_job_pulls_docker_hub_images_through_the_mirror() {
    let workflow = std::fs::read_to_string(workspace_root().join(".github/workflows/ci.yml"))
        .expect("read .github/workflows/ci.yml");
    let start = workflow.find("\n  test:\n").expect("a test job");
    let end = workflow[start + 1..]
        .find("\n  contract:\n")
        .map_or(workflow.len(), |at| start + 1 + at);
    let job = &workflow[start..end];

    let daemon = job
        .find("run: .github/scripts/docker-hub-mirror.sh")
        .expect("the job sets the mirror of the Docker daemon");
    let builder = job
        .find("docker/setup-buildx-action")
        .expect("the job makes a buildx builder");
    assert!(
        daemon < builder,
        "the daemon restart must come before the builder starts"
    );
    assert!(job[builder..].contains("buildkitd-config: buildkitd.toml"));

    let script =
        std::fs::read_to_string(workspace_root().join(".github/scripts/docker-hub-mirror.sh"))
            .expect("read .github/scripts/docker-hub-mirror.sh");
    assert!(script.contains("https://mirror.gcr.io"));
    let buildkitd = std::fs::read_to_string(workspace_root().join("buildkitd.toml"))
        .expect("read buildkitd.toml");
    assert!(buildkitd.contains("[registry.\"docker.io\"]"));
    assert!(buildkitd.contains("mirrors = [\"mirror.gcr.io\"]"));
}

/// Branch protection and the merge queue require one job, which needs
/// each other job and fails when one of them did not pass.
#[test]
fn the_ci_workflow_ends_in_one_job_that_needs_every_other_job() {
    let workflow = std::fs::read_to_string(workspace_root().join(".github/workflows/ci.yml"))
        .expect("read .github/workflows/ci.yml");
    let jobs_at = workflow.find("\njobs:\n").expect("a jobs block");
    let jobs: Vec<&str> = workflow[jobs_at..]
        .lines()
        .filter_map(|line| {
            let name = line.strip_prefix("  ")?.strip_suffix(':')?;
            (!name.starts_with(' ') && !name.starts_with('#')).then_some(name)
        })
        .collect();
    assert_eq!(jobs.last(), Some(&"ci-success"), "{jobs:?}");
    let needs_at = workflow.find("  ci-success:").unwrap();
    let aggregate = &workflow[needs_at..];
    for job in &jobs[..jobs.len() - 1] {
        assert!(
            aggregate.contains(&format!("      - {job}\n")),
            "ci-success does not need {job}"
        );
    }
    for trigger in ["pull_request:", "merge_group:", "workflow_dispatch:"] {
        assert!(workflow.contains(trigger), "{trigger} is missing");
    }
}

// --- the release workflow of the Mobile App ---

/// Each job of `workflow`, by its id, with the lines of its block.
fn workflow_jobs(workflow: &str) -> Vec<(String, String)> {
    let jobs_at = workflow.find("\njobs:\n").expect("a jobs block");
    let mut jobs: Vec<(String, String)> = Vec::new();
    for line in workflow[jobs_at + "\njobs:\n".len()..].lines() {
        let top = line
            .strip_prefix("  ")
            .filter(|rest| !rest.starts_with(' ') && !rest.starts_with('#'))
            .and_then(|rest| rest.strip_suffix(':'));
        match (top, jobs.last_mut()) {
            (Some(id), _) => jobs.push((id.to_string(), String::new())),
            (None, Some((_, block))) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, None) => {}
        }
    }
    jobs
}

/// The ids in the `needs:` of a job block, in the list or the one-line
/// form.
fn job_needs(block: &str) -> Vec<String> {
    let Some(at) = block.find("    needs:") else {
        return Vec::new();
    };
    let rest = &block[at + "    needs:".len()..];
    let first = rest.lines().next().unwrap_or_default().trim();
    if !first.is_empty() {
        return first
            .trim_matches(['[', ']'])
            .split(',')
            .map(|id| id.trim().to_string())
            .collect();
    }
    rest.lines()
        .skip(1)
        .map_while(|line| line.trim().strip_prefix("- "))
        .map(str::to_string)
        .collect()
}

/// A `mobile-v*` tag releases the iOS app. The tagged commit passes the
/// gate, and the lockfile of the Mobile App passes npm audit, before the
/// prepare job builds anything. The publication waits for a maintainer
/// in the `release` environment.
#[test]
fn the_mobile_release_runs_the_gate_and_the_npm_audit_before_the_prepare_job() {
    let workflow =
        std::fs::read_to_string(workspace_root().join(".github/workflows/mobile-release.yml"))
            .expect("read .github/workflows/mobile-release.yml");
    assert!(
        workflow.contains("on:\n  push:\n    tags: [\"mobile-v*\"]\n"),
        "{workflow}"
    );
    let jobs = workflow_jobs(&workflow);
    let ids: Vec<&str> = jobs.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "gate",
            "mobile-npm-audit",
            "mobile-ios",
            "publish-mobile-ios"
        ]
    );
    let job = |id: &str| &jobs.iter().find(|(job, _)| job == id).unwrap().1;

    assert!(
        job("gate").contains("    uses: ./.github/workflows/ci.yml\n"),
        "{}",
        job("gate")
    );

    let audit = job("mobile-npm-audit");
    assert_eq!(job_needs(audit), ["gate"]);
    assert!(
        audit.contains("run: cargo xtask advisories mobile-npm-audit\n"),
        "{audit}"
    );

    let prepare = job("mobile-ios");
    assert_eq!(job_needs(prepare), ["gate", "mobile-npm-audit"]);
    assert!(prepare.contains("    runs-on: macos-latest\n"), "{prepare}");
    assert!(
        prepare.contains("PUSH_RELAY_ORIGIN: https://push.pagis.co\n"),
        "{prepare}"
    );
    assert!(
        prepare.contains("run: cargo xtask mobile --ios --tag \"$GITHUB_REF_NAME\" --prepare\n"),
        "{prepare}"
    );
    assert!(
        prepare.contains("actions/attest-build-provenance@")
            && prepare.contains("name: mobile-ios\n")
            && prepare.contains("mobile/release/Pagis.ipa"),
        "the prepare job attests the .ipa and keeps it as the artifact mobile-ios: {prepare}"
    );

    let publish = job("publish-mobile-ios");
    assert_eq!(job_needs(publish), ["mobile-ios"]);
    assert!(publish.contains("    environment: release\n"), "{publish}");
    assert!(
        publish.contains(
            "run: cargo xtask mobile --ios --tag \"$GITHUB_REF_NAME\" --publish-existing\n"
        ),
        "{publish}"
    );
    assert!(publish.contains("name: mobile-ios\n"), "{publish}");
}

/// The `v*` release does not ship the Mobile App (ADR-0025).
#[test]
fn the_server_release_does_not_ship_the_mobile_app() {
    let workflow = std::fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read .github/workflows/release.yml");
    assert!(
        !workflow.contains("cargo xtask mobile"),
        "release.yml ships the Mobile App"
    );
}
