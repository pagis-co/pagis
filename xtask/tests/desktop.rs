//! Tests for the desktop packaging (`cargo xtask desktop`), the piece
//! the GitHub Actions workflow calls on macOS.

use std::path::Path;
use std::process::Command;

use xtask::desktop::{
    DesktopContext, DesktopPlatform, check_tag, check_version, desktop_plan, dmg_name,
    missing_signing_inputs,
};
use xtask::{Action, Cmd, Step};

fn context(tag: Option<&str>, missing: &[&str]) -> DesktopContext {
    DesktopContext {
        platform: DesktopPlatform::Mac,
        version: "1.2.3".into(),
        tag: tag.map(str::to_string),
        missing_credentials: missing.iter().map(|v| v.to_string()).collect(),
        prepare_only: false,
        publish_existing: false,
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

// --- plan ---

#[test]
fn plan_lists_every_desktop_step_in_order() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "credentials",
            "runtime-lock",
            "deps",
            "pack",
            "finalize-dmg",
            "inventory",
            "packaged-runtime",
            "smoke",
            "signed-client",
            "released-tuple",
            "publish"
        ]
    );
}

#[test]
fn the_final_runtime_lock_is_required_before_the_client_app() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    let lock = step(&steps, "runtime-lock");
    assert!(joined(lock).contains("dist/runtime-lock-darwin-arm64.json"));
    assert_eq!(commands(lock)[0].cwd.as_deref(), Some(Path::new("/repo")));
}

#[test]
fn the_app_packs_in_the_desktop_package() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    for name in ["deps", "pack"] {
        let cmds = commands(step(&steps, name));
        assert_eq!(
            cmds[0].cwd.as_deref(),
            Some(Path::new("/repo/desktop")),
            "step {name} runs in desktop/"
        );
    }
    assert_eq!(joined(step(&steps, "deps")), "npm ci");
    assert_eq!(joined(step(&steps, "pack")), "npm run pack");
}

#[test]
fn a_pull_request_packs_the_app_unsigned() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    assert_eq!(
        commands(step(&steps, "pack"))[0].env,
        [("CSC_IDENTITY_AUTO_DISCOVERY".to_string(), "false".into())],
        "without the certificate the pack must not look for one in the keychain"
    );
}

#[test]
fn a_tag_with_every_credential_packs_the_app_signed() {
    let steps = desktop_plan(Path::new("/repo"), &context(Some("v1.2.3"), &[]));
    assert!(
        commands(step(&steps, "pack"))[0].env.is_empty(),
        "the signing variables come from the environment, not from the plan"
    );
}

#[test]
fn a_tag_without_a_credential_packs_the_app_unsigned() {
    let steps = desktop_plan(
        Path::new("/repo"),
        &context(Some("v1.2.3"), &["APPLE_API_KEY"]),
    );
    assert_eq!(
        commands(step(&steps, "pack"))[0].env,
        [("CSC_IDENTITY_AUTO_DISCOVERY".to_string(), "false".into())]
    );
}

#[test]
fn the_smoke_runs_the_packaged_app_and_asks_it_for_the_health_answer() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    let cmds = commands(step(&steps, "smoke"));
    assert_eq!(cmds[0].program, "sh");
    let script = &cmds[0].args[1];
    assert!(script.contains("Pagis.app"), "script: {script}");
    assert!(script.contains("--smoke"), "script: {script}");
    assert!(
        script.contains("ditto"),
        "the app must run outside the checkout: {script}"
    );
    assert_eq!(cmds[0].cwd.as_deref(), Some(Path::new("/repo/desktop")));
}

#[test]
fn routine_packaging_checks_the_client_inventory_and_compiled_runtime_boundary() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    let inventory = joined(step(&steps, "inventory"));
    assert!(
        inventory.contains("check-package.mjs"),
        "command: {inventory}"
    );
    let runtime = joined(step(&steps, "packaged-runtime"));
    assert!(
        runtime.contains("check-packaged-runtime.mjs"),
        "command: {runtime}"
    );
    assert!(runtime.contains("Pagis.app"), "command: {runtime}");
}

#[test]
fn a_tag_verifies_the_existing_server_tuple_before_client_upload() {
    let steps = desktop_plan(Path::new("/repo"), &context(Some("v1.2.3"), &[]));
    let finalize = joined(step(&steps, "finalize-dmg"));
    assert!(
        finalize.contains("codesign --verify --strict"),
        "command: {finalize}"
    );
    assert!(
        finalize.contains("notarytool submit"),
        "command: {finalize}"
    );
    assert!(finalize.contains("stapler staple"), "command: {finalize}");
    assert!(
        finalize.contains("Pagis-1.2.3-arm64.dmg"),
        "command: {finalize}"
    );
    let tuple = joined(step(&steps, "released-tuple"));
    assert!(
        tuple.contains("gh release download v1.2.3"),
        "command: {tuple}"
    );
    assert!(
        tuple.contains("cmp dist/runtime-lock-darwin-arm64.json"),
        "command: {tuple}"
    );
    assert!(
        tuple.contains("pagis-server-1.2.3-aarch64-apple-darwin.dmg"),
        "command: {tuple}"
    );
    assert!(tuple.contains("asset.sha256"), "command: {tuple}");
    assert!(tuple.contains("shasum -a 256"), "command: {tuple}");
    assert!(
        tuple.contains("anonymous_pull \"$image\""),
        "command: {tuple}"
    );
    assert!(!tuple.contains("imagetools"), "command: {tuple}");
    let signed = joined(step(&steps, "signed-client"));
    assert!(signed.contains("hdiutil attach"), "command: {signed}");
    assert!(
        signed.contains("cmp dist/runtime-lock-darwin-arm64.json")
            && signed.contains("Contents/Resources/runtime-lock.json"),
        "the exact DMG must embed the validated Runtime Lock: {signed}"
    );
    assert!(signed.contains("check-package.mjs"), "command: {signed}");
    assert!(
        signed.contains("check-packaged-runtime.mjs"),
        "command: {signed}"
    );
    assert!(signed.contains("--smoke"), "command: {signed}");
    assert!(
        signed.contains("codesign --verify --deep --strict"),
        "command: {signed}"
    );
    assert!(
        signed.contains("Developer ID Application"),
        "command: {signed}"
    );
    assert!(signed.contains("TeamIdentifier"), "command: {signed}");
    assert!(signed.contains("stapler validate"), "command: {signed}");
    assert!(signed.contains("spctl --assess"), "command: {signed}");
}

#[test]
fn a_prepared_package_stops_before_publication_and_publish_reuses_its_bytes() {
    let mut prepare = context(Some("v1.2.3"), &[]);
    prepare.prepare_only = true;
    let steps = desktop_plan(Path::new("/repo"), &prepare);
    assert!(matches!(step(&steps, "pack").action, Action::Run(_)));
    assert!(matches!(
        step(&steps, "finalize-dmg").action,
        Action::Run(_)
    ));
    assert!(matches!(
        step(&steps, "signed-client").action,
        Action::Run(_)
    ));
    assert!(skip_reason(step(&steps, "publish")).contains("awaits"));

    let mut publish = context(Some("v1.2.3"), &[]);
    publish.publish_existing = true;
    let steps = desktop_plan(Path::new("/repo"), &publish);
    assert!(skip_reason(step(&steps, "deps")).contains("exact package"));
    assert!(skip_reason(step(&steps, "pack")).contains("exact package"));
    assert!(skip_reason(step(&steps, "finalize-dmg")).contains("finalized when it was prepared"));
    assert!(matches!(
        step(&steps, "signed-client").action,
        Action::Run(_)
    ));
    assert!(matches!(step(&steps, "publish").action, Action::Run(_)));
}

#[test]
fn a_release_checks_the_app_from_the_exact_dmg_instead_of_a_loose_build_directory() {
    let steps = desktop_plan(Path::new("/repo"), &context(Some("v1.2.3"), &[]));
    for name in ["inventory", "packaged-runtime", "smoke"] {
        assert!(
            skip_reason(step(&steps, name)).contains("exact DMG"),
            "step {name} must not inspect a loose app"
        );
    }
}

#[test]
fn electron_builder_signs_the_outer_dmg() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let config = std::fs::read_to_string(root.join("desktop/electron-builder.yml")).unwrap();
    let dmg = config.split("\ndmg:\n").nth(1).expect("dmg section");
    assert!(dmg.lines().any(|line| line.trim() == "sign: true"));
}

/// The release, not electron-builder, uploads each client package. On a
/// tag with `GH_TOKEN` set, electron-builder publishes on its own and
/// writes an update feed for the app, and it fails when it finds no
/// repository. `publish: null` turns off both.
#[test]
fn electron_builder_publishes_nothing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let config = std::fs::read_to_string(root.join("desktop/electron-builder.yml")).unwrap();
    assert!(
        config.lines().any(|line| line == "publish: null"),
        "{config}"
    );
}

#[test]
fn generated_shell_steps_are_valid_shell() {
    for cx in [context(None, &[]), context(Some("v1.2.3"), &[])] {
        for step in desktop_plan(Path::new("/repo"), &cx) {
            let Action::Run(commands) = step.action else {
                continue;
            };
            for command in commands
                .into_iter()
                .filter(|command| command.program == "sh")
            {
                let status = Command::new("sh")
                    .args(["-n", "-c", &command.args[1]])
                    .status()
                    .unwrap();
                assert!(status.success(), "invalid shell in step {}", step.name);
            }
        }
    }
}

#[test]
fn the_dmg_is_attached_to_the_release_of_the_tag() {
    let steps = desktop_plan(Path::new("/repo"), &context(Some("v1.2.3"), &[]));
    assert_eq!(
        joined(step(&steps, "publish")),
        "gh release upload v1.2.3 desktop/release/Pagis-1.2.3-arm64.dmg --repo pagis-co/pagis"
    );
    assert_eq!(
        commands(step(&steps, "publish"))[0].cwd.as_deref(),
        Some(Path::new("/repo"))
    );
}

#[test]
fn without_a_tag_the_dmg_is_not_published() {
    let steps = desktop_plan(Path::new("/repo"), &context(None, &[]));
    assert_eq!(
        skip_reason(step(&steps, "publish")),
        "no release tag: the app is packed and smoke tested only"
    );
}

#[test]
fn a_missing_credential_refuses_the_tag_before_packaging() {
    let steps = desktop_plan(
        Path::new("/repo"),
        &context(Some("v1.2.3"), &["CSC_LINK", "APPLE_API_ISSUER"]),
    );
    let command = joined(step(&steps, "credentials"));
    assert!(command.contains("CSC_LINK"), "command: {command}");
    assert!(command.contains("APPLE_API_ISSUER"), "command: {command}");
    assert!(command.contains("exit 1"), "command: {command}");
}

// --- credentials ---

#[test]
fn signing_accepts_local_keychain_credentials_without_exporting_a_key() {
    let local = |name: &str| matches!(name, "CSC_NAME" | "APPLE_KEYCHAIN_PROFILE");
    assert!(missing_signing_inputs(&local).is_empty());

    let imported = |name: &str| {
        matches!(
            name,
            "CSC_LINK"
                | "CSC_KEY_PASSWORD"
                | "APPLE_API_KEY"
                | "APPLE_API_KEY_ID"
                | "APPLE_API_ISSUER"
        )
    };
    assert!(missing_signing_inputs(&imported).is_empty());
}

#[test]
fn missing_signing_inputs_names_the_unmet_alternatives() {
    let missing = missing_signing_inputs(&|_| false);
    assert_eq!(missing.len(), 2);
    assert!(missing[0].contains("CSC_NAME"), "missing: {missing:?}");
    assert!(missing[0].contains("CSC_LINK"), "missing: {missing:?}");
    assert!(
        missing[1].contains("APPLE_KEYCHAIN_PROFILE"),
        "missing: {missing:?}"
    );
    assert!(missing[1].contains("APPLE_API_KEY"), "missing: {missing:?}");
    // An Apple ID and an app-specific password are not an alternative.
    let account = |name: &str| {
        matches!(
            name,
            "CSC_NAME" | "APPLE_ID" | "APPLE_APP_SPECIFIC_PASSWORD" | "APPLE_TEAM_ID"
        )
    };
    assert_eq!(missing_signing_inputs(&account).len(), 1);
}

// --- versions ---

#[test]
fn the_dmg_carries_the_release_version() {
    assert_eq!(dmg_name("1.2.3"), "Pagis-1.2.3-arm64.dmg");
}

#[test]
fn the_app_version_must_equal_the_workspace_version() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("desktop")).unwrap();
    std::fs::write(
        tmp.path().join("desktop/package.json"),
        "{\"version\": \"1.2.3\"}",
    )
    .unwrap();
    assert!(check_version(tmp.path(), "1.2.3").is_ok());
    let error = check_version(tmp.path(), "1.3.0").unwrap_err().to_string();
    assert!(error.contains("1.2.3"), "error: {error}");
    assert!(error.contains("1.3.0"), "error: {error}");
}

#[test]
fn the_tag_must_name_the_version_that_is_packed() {
    assert!(check_tag("v1.2.3", "1.2.3").is_ok());
    let error = check_tag("v1.3.0", "1.2.3").unwrap_err().to_string();
    assert!(error.contains("v1.2.3"), "error: {error}");
}
