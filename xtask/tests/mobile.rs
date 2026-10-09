//! Tests for the build configuration of the Mobile App in `mobile/`.

use std::process::Command;

use crate::support::workspace_root;

/// A build of the iOS project takes `PUSH_RELAY_ORIGIN` from
/// `mobile/ios/app.xcconfig`, which holds the placeholder, and from the
/// local file `mobile/ios/local.xcconfig`, which overrides it. A value in
/// **Build Settings** in Xcode goes into `project.pbxproj`. It then
/// overrides each xcconfig file and puts the origin in the repository.
#[test]
fn the_ios_push_relay_origin_of_a_local_build_stays_out_of_the_repository() {
    let root = workspace_root();
    let project =
        std::fs::read_to_string(root.join("mobile/ios/App/App.xcodeproj/project.pbxproj")).unwrap();
    assert!(
        !project.contains("PUSH_RELAY_ORIGIN = "),
        "project.pbxproj sets PUSH_RELAY_ORIGIN, which overrides \
         mobile/ios/local.xcconfig; set it in the xcconfig files instead"
    );

    let ignored = Command::new("git")
        .args(["check-ignore", "--quiet", "mobile/ios/local.xcconfig"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(
        ignored.success(),
        "git does not ignore mobile/ios/local.xcconfig"
    );
}

// --- the iOS release (`cargo xtask mobile --ios`) ---

use xtask::mobile::{IosPhase, ios_release_plan};
use xtask::{Action, Cmd, Step, plan_summary};

/// The values of the environment of a release job. The secret values
/// are text that the plan must never hold.
fn release_env(name: &str) -> Option<String> {
    match name {
        "PUSH_RELAY_ORIGIN" => Some("https://push.pagis.co".into()),
        "GITHUB_RUN_NUMBER" => Some("42".into()),
        "APPLE_API_KEY" => Some("/runner/temp/SECRET-KEY-PATH.p8".into()),
        "APPLE_API_KEY_ID" => Some("SECRET-KEY-ID".into()),
        "APPLE_API_ISSUER" => Some("SECRET-ISSUER-ID".into()),
        _ => None,
    }
}

/// A checkout whose `mobile/package.json` is version `version`.
fn checkout(version: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("mobile")).unwrap();
    std::fs::write(
        tmp.path().join("mobile/package.json"),
        format!("{{\"name\": \"pagis-mobile\", \"version\": \"{version}\"}}"),
    )
    .unwrap();
    tmp
}

fn plan(phase: IosPhase, env: &dyn Fn(&str) -> Option<String>) -> (tempfile::TempDir, Vec<Step>) {
    let tmp = checkout("1.2.3");
    let steps = ios_release_plan(tmp.path(), "mobile-v1.2.3", phase, env).unwrap();
    (tmp, steps)
}

fn refusal(phase: IosPhase, env: &dyn Fn(&str) -> Option<String>) -> String {
    let tmp = checkout("1.2.3");
    ios_release_plan(tmp.path(), "mobile-v1.2.3", phase, env)
        .expect_err("the plan must stop")
        .to_string()
}

/// `release_env` with `name` set to `value`, or unset for `None`.
fn env_with(name: &'static str, value: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
    move |key: &str| {
        if key == name {
            value.map(str::to_string)
        } else {
            release_env(key)
        }
    }
}

fn step<'a>(steps: &'a [Step], name: &str) -> &'a Step {
    steps
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no step {name}"))
}

fn commands(step: &Step) -> &[Cmd] {
    match &step.action {
        Action::Run(cmds) => cmds,
        Action::Skip(reason) => panic!("step {} skipped: {reason}", step.name),
    }
}

fn skip_reason(step: &Step) -> &str {
    match &step.action {
        Action::Skip(reason) => reason,
        Action::Run(_) => panic!("step {} runs; a skip was expected", step.name),
    }
}

fn joined(step: &Step) -> String {
    commands(step)
        .iter()
        .map(|c| format!("{} {}", c.program, c.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn the_ios_release_lists_each_step_in_order() {
    let (_tmp, steps) = plan(IosPhase::Prepare, &release_env);
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "deps",
            "build",
            "sync",
            "archive",
            "export",
            "check-ipa",
            "upload"
        ]
    );
}

#[test]
fn the_prepare_phase_builds_the_connect_screen_and_syncs_the_ios_project() {
    let (tmp, steps) = plan(IosPhase::Prepare, &release_env);
    let mobile = tmp.path().join("mobile");
    for (name, command) in [
        ("deps", "npm ci"),
        ("build", "npm run build"),
        ("sync", "npx cap sync ios"),
    ] {
        assert_eq!(joined(step(&steps, name)), command);
        assert_eq!(
            commands(step(&steps, name))[0].cwd.as_deref(),
            Some(mobile.as_path()),
            "step {name} runs in mobile/"
        );
    }
}

#[test]
fn the_archive_takes_its_versions_and_the_push_relay_origin_on_the_command_line() {
    let (tmp, steps) = plan(IosPhase::Prepare, &release_env);
    let archive = step(&steps, "archive");
    assert_eq!(
        commands(archive)[0].cwd.as_deref(),
        Some(tmp.path().join("mobile/ios").as_path())
    );
    let script = joined(archive);
    for expected in [
        "xcodebuild archive",
        "-project App/App.xcodeproj",
        "-scheme App",
        "-configuration Release",
        "-destination generic/platform=iOS",
        "-archivePath ../release/Pagis.xcarchive",
        // The marketing version of mobile/package.json and the number of
        // the workflow run go into CFBundleShortVersionString and
        // CFBundleVersion of the app and of the extension.
        "MARKETING_VERSION=1.2.3",
        "CURRENT_PROJECT_VERSION=42",
        // A command-line setting comes before each xcconfig file.
        "PUSH_RELAY_ORIGIN=https://push.pagis.co",
    ] {
        assert!(script.contains(expected), "no {expected}: {script}");
    }
}

/// Xcode automatic signing with the App Store Connect API key makes the
/// profiles of the app and the extension, and signs the export with a
/// cloud-managed distribution certificate.
#[test]
fn the_archive_and_the_export_sign_with_the_api_key_from_the_environment() {
    let (_tmp, steps) = plan(IosPhase::Prepare, &release_env);
    for name in ["archive", "export"] {
        let script = joined(step(&steps, name));
        for expected in [
            "-allowProvisioningUpdates",
            "-authenticationKeyPath \"$APPLE_API_KEY\"",
            "-authenticationKeyID \"$APPLE_API_KEY_ID\"",
            "-authenticationKeyIssuerID \"$APPLE_API_ISSUER\"",
        ] {
            assert!(
                script.contains(expected),
                "{name} has no {expected}: {script}"
            );
        }
    }
}

#[test]
fn the_export_writes_the_ipa_for_app_store_connect() {
    let (tmp, steps) = plan(IosPhase::Prepare, &release_env);
    let export = step(&steps, "export");
    assert_eq!(
        commands(export)[0].cwd.as_deref(),
        Some(tmp.path().join("mobile/ios").as_path())
    );
    let script = joined(export);
    for expected in [
        "xcodebuild -exportArchive",
        "-archivePath ../release/Pagis.xcarchive",
        "-exportOptionsPlist ExportOptions.plist",
        "../release/Pagis.ipa",
    ] {
        assert!(script.contains(expected), "no {expected}: {script}");
    }
}

/// The value of `key` in the top dictionary of the plist `text`.
fn plist_value(text: &str, key: &str) -> Option<String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options).unwrap();
    let dict = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("dict"))
        .unwrap();
    let mut nodes = dict.children().filter(|n| n.is_element());
    while let Some(node) = nodes.next() {
        if node.has_tag_name("key") && node.text() == Some(key) {
            let value = nodes.next()?;
            return Some(match value.tag_name().name() {
                "true" | "false" => value.tag_name().name().to_string(),
                _ => value.text().unwrap_or_default().to_string(),
            });
        }
    }
    None
}

/// The export makes an App Store Connect build with automatic signing,
/// and it keeps the versions of the archive: App Store Connect does not
/// change the build number.
#[test]
fn the_export_options_name_app_store_connect_and_automatic_signing() {
    let options =
        std::fs::read_to_string(workspace_root().join("mobile/ios/ExportOptions.plist")).unwrap();
    for (key, value) in [
        ("method", "app-store-connect"),
        ("signingStyle", "automatic"),
        ("destination", "export"),
        ("manageAppVersionAndBuildNumber", "false"),
    ] {
        assert_eq!(
            plist_value(&options, key).as_deref(),
            Some(value),
            "{key} in ExportOptions.plist"
        );
    }
}

/// The prepare phase checks the exact .ipa that it uploads later: the
/// signatures, the versions, the relay origin, the privacy manifests and
/// the production APNs environment.
#[test]
fn the_prepare_phase_checks_the_exact_ipa() {
    let (tmp, steps) = plan(IosPhase::Prepare, &release_env);
    let check = step(&steps, "check-ipa");
    assert_eq!(commands(check)[0].cwd.as_deref(), Some(tmp.path()));
    let script = joined(check);
    for expected in [
        "ipa='mobile/release/Pagis.ipa'",
        "Payload/App.app",
        "PlugIns/PagisNotificationService.appex",
        "codesign --verify --strict",
        "CFBundleShortVersionString",
        "'1.2.3'",
        "CFBundleVersion",
        "'42'",
        "CFBundleIdentifier",
        "'co.pagis.mobile'",
        "PagisPushRelayOrigin",
        "'https://push.pagis.co'",
        "PrivacyInfo.xcprivacy",
        "aps-environment",
        "production",
    ] {
        assert!(script.contains(expected), "no {expected}: {script}");
    }
}

#[test]
fn the_prepared_ipa_waits_for_publication() {
    let (_tmp, steps) = plan(IosPhase::Prepare, &release_env);
    assert!(skip_reason(step(&steps, "upload")).contains("awaits"));
}

/// The publication uploads the bytes that the prepare phase built and
/// checked. It does not build or sign again.
#[test]
fn the_publish_phase_uploads_the_prepared_ipa_with_altool_only() {
    let (tmp, steps) = plan(IosPhase::PublishExisting, &release_env);
    for name in ["deps", "build", "sync", "archive", "export", "check-ipa"] {
        assert!(
            skip_reason(step(&steps, name)).contains("prepared"),
            "step {name} must not run again"
        );
    }
    let upload = step(&steps, "upload");
    assert_eq!(commands(upload)[0].cwd.as_deref(), Some(tmp.path()));
    let script = joined(upload);
    for expected in [
        "xcrun altool --upload-app -f mobile/release/Pagis.ipa -t ios",
        "--apiKey \"$APPLE_API_KEY_ID\"",
        "--apiIssuer \"$APPLE_API_ISSUER\"",
        // altool reads the key from AuthKey_<key id>.p8 in a directory.
        "AuthKey_$APPLE_API_KEY_ID.p8",
        "API_PRIVATE_KEYS_DIR=",
    ] {
        assert!(script.contains(expected), "no {expected}: {script}");
    }
    assert!(!script.contains("xcodebuild"), "{script}");
}

/// The publication needs only the tag and the API key: the prepared
/// .ipa holds the relay origin and the build number.
#[test]
fn the_publish_phase_needs_no_relay_origin_and_no_run_number() {
    let env = |name: &str| match name {
        "PUSH_RELAY_ORIGIN" | "GITHUB_RUN_NUMBER" => None,
        _ => release_env(name),
    };
    let tmp = checkout("1.2.3");
    assert!(ios_release_plan(tmp.path(), "mobile-v1.2.3", IosPhase::PublishExisting, &env).is_ok());
}

/// The plan names its inputs. The shell reads each secret from the
/// environment when the step runs.
#[test]
fn the_plan_names_its_inputs_and_holds_no_secret_value() {
    for phase in [IosPhase::Prepare, IosPhase::PublishExisting] {
        let (_tmp, steps) = plan(phase, &release_env);
        let mut text = plan_summary(&steps);
        for step in &steps {
            if let Action::Run(cmds) = &step.action {
                for cmd in cmds {
                    text.push_str(&format!("{:?}", cmd));
                }
            }
        }
        for secret in ["SECRET-KEY-PATH", "SECRET-KEY-ID", "SECRET-ISSUER-ID"] {
            assert!(!text.contains(secret), "{phase:?} holds {secret}: {text}");
        }
        for input in ["$APPLE_API_KEY_ID", "$APPLE_API_ISSUER"] {
            assert!(text.contains(input), "{phase:?} does not name {input}");
        }
    }
}

#[test]
fn a_tag_that_does_not_name_the_version_of_package_json_stops_the_plan() {
    let tmp = checkout("1.2.3");
    for tag in ["mobile-v1.2.4", "v1.2.3", "push-relay-v1.2.3", "1.2.3"] {
        for phase in [IosPhase::Prepare, IosPhase::PublishExisting] {
            let error = ios_release_plan(tmp.path(), tag, phase, &release_env)
                .err()
                .unwrap_or_else(|| panic!("{tag} must stop the plan"))
                .to_string();
            assert!(error.contains(tag), "{error}");
            assert!(error.contains("mobile-v1.2.3"), "{error}");
            assert!(error.contains("mobile/package.json"), "{error}");
        }
    }
}

#[test]
fn a_missing_or_wrong_push_relay_origin_stops_the_prepare_phase() {
    for value in [
        None,
        Some(""),
        Some("http://push.pagis.co"),
        Some("push.pagis.co"),
        Some("https://"),
        Some("https://push.pagis.co/"),
        Some("https://push.pagis.co/relay"),
        Some("https://push.pagis.co?x=1"),
        Some("https://user@push.pagis.co"),
        // The placeholder of mobile/ios/app.xcconfig never resolves.
        Some("https://push-relay.invalid"),
    ] {
        let error = refusal(IosPhase::Prepare, &env_with("PUSH_RELAY_ORIGIN", value));
        assert!(error.contains("PUSH_RELAY_ORIGIN"), "{value:?}: {error}");
    }
}

#[test]
fn an_https_origin_with_a_port_is_a_push_relay_origin() {
    let (_tmp, steps) = plan(
        IosPhase::Prepare,
        &env_with("PUSH_RELAY_ORIGIN", Some("https://relay.example.com:8443")),
    );
    assert!(
        joined(step(&steps, "archive"))
            .contains("PUSH_RELAY_ORIGIN=https://relay.example.com:8443")
    );
}

#[test]
fn a_missing_or_wrong_run_number_stops_the_prepare_phase() {
    for value in [None, Some("0"), Some("12a"), Some("-3")] {
        let error = refusal(IosPhase::Prepare, &env_with("GITHUB_RUN_NUMBER", value));
        assert!(error.contains("GITHUB_RUN_NUMBER"), "{value:?}: {error}");
    }
}

#[test]
fn a_missing_api_key_input_stops_each_phase_and_is_named() {
    for name in ["APPLE_API_KEY", "APPLE_API_KEY_ID", "APPLE_API_ISSUER"] {
        for phase in [IosPhase::Prepare, IosPhase::PublishExisting] {
            let error = refusal(phase, &env_with(name, None));
            assert!(error.contains(name), "{phase:?} {name}: {error}");
        }
    }
}

#[test]
fn the_ios_release_shell_steps_are_valid_shell() {
    for phase in [IosPhase::Prepare, IosPhase::PublishExisting] {
        let (_tmp, steps) = plan(phase, &release_env);
        for step in steps {
            let Action::Run(commands) = step.action else {
                continue;
            };
            for command in commands.into_iter().filter(|c| c.program == "sh") {
                let status = Command::new("sh")
                    .args(["-n", "-c", &command.args[1]])
                    .status()
                    .unwrap();
                assert!(status.success(), "invalid shell in step {}", step.name);
            }
        }
    }
}

/// The prepared bytes stay out of the repository.
#[test]
fn git_ignores_the_prepared_release() {
    let ignored = Command::new("git")
        .args(["check-ignore", "--quiet", "mobile/release/Pagis.ipa"])
        .current_dir(workspace_root())
        .status()
        .unwrap();
    assert!(ignored.success(), "git does not ignore mobile/release/");
}

#[test]
fn the_ios_release_reads_the_version_of_the_checkout() {
    let root = workspace_root();
    let manifest = std::fs::read_to_string(root.join("mobile/package.json")).unwrap();
    let version = serde_json::from_str::<serde_json::Value>(&manifest).unwrap()["version"]
        .as_str()
        .unwrap()
        .to_string();
    let steps = ios_release_plan(
        &root,
        &format!("mobile-v{version}"),
        IosPhase::Prepare,
        &release_env,
    )
    .unwrap();
    assert!(joined(step(&steps, "archive")).contains(&format!("MARKETING_VERSION={version}")));
}

/// App Store Connect takes a `CFBundleShortVersionString` of one to
/// three whole numbers with periods between them.
#[test]
fn a_version_that_app_store_connect_refuses_stops_the_plan() {
    for version in ["1.2.3-rc.1", "1.2.3.4", "1..2", "v1.2.3", "1.2;true"] {
        let tmp = checkout(version);
        let error = ios_release_plan(
            tmp.path(),
            &format!("mobile-v{version}"),
            IosPhase::Prepare,
            &release_env,
        )
        .err()
        .unwrap_or_else(|| panic!("{version} must stop the plan"))
        .to_string();
        assert!(error.contains(version), "{error}");
        assert!(error.contains("mobile/package.json"), "{error}");
    }
    let tmp = checkout("2.1");
    assert!(ios_release_plan(tmp.path(), "mobile-v2.1", IosPhase::Prepare, &release_env).is_ok());
}
