//! Tests for the Linux Client App packaging (`cargo xtask desktop` on a
//! Linux host).

use std::path::Path;
use std::process::Command;

use xtask::desktop::{DesktopContext, DesktopPlatform, desktop_plan};
use xtask::desktop_linux::{
    RELEASE_KEY, appimage_name, checksums_name, deb_name, missing_signing_inputs,
};
use xtask::release::ClientPlatform;
use xtask::{Action, Cmd, Step};

fn context(tag: Option<&str>) -> DesktopContext {
    DesktopContext {
        platform: DesktopPlatform::Linux,
        version: "1.2.3".into(),
        tag: tag.map(str::to_string),
        missing_credentials: Vec::new(),
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

#[test]
fn the_linux_plan_lists_every_step_in_order() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "credentials",
            "runtime-locks",
            "deps",
            "pack",
            "inventory",
            "packaged-runtime",
            "smoke",
            "checksums",
            "released-tuple",
            "sign-checksums",
            "publish",
        ]
    );
}

#[test]
fn both_linux_locks_are_required_before_the_packages() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let locks = joined(step(&steps, "runtime-locks"));
    assert!(
        locks.contains("validate-metadata dist/runtime-lock-linux-x64.json 1.2.3 linux-x64"),
        "{locks}"
    );
    assert!(
        locks.contains("validate-metadata dist/runtime-lock-linux-arm64.json 1.2.3 linux-arm64"),
        "{locks}"
    );
}

#[test]
fn the_packages_build_for_both_architectures_in_the_desktop_package() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    assert_eq!(joined(step(&steps, "pack")), "npm run pack:linux");
    assert_eq!(
        commands(step(&steps, "pack"))[0].cwd.as_deref(),
        Some(Path::new("/repo/desktop"))
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("desktop/package.json")).unwrap())
            .unwrap();
    let pack = manifest["scripts"]["pack:linux"].as_str().unwrap();
    assert!(pack.contains("--linux"), "{pack}");
    assert!(pack.contains("--x64") && pack.contains("--arm64"), "{pack}");
}

/// Each package carries the lock of its own architecture, and the deb is
/// checked from its exact bytes.
#[test]
fn the_inventory_checks_each_architecture_and_the_exact_deb() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let inventory = joined(step(&steps, "inventory"));
    for (lock, unpacked, appimage, deb) in [
        (
            "runtime-lock-linux-x64.json",
            "release/linux-unpacked",
            "Pagis-1.2.3-x86_64.AppImage",
            "Pagis-1.2.3-amd64.deb",
        ),
        (
            "runtime-lock-linux-arm64.json",
            "release/linux-arm64-unpacked",
            "Pagis-1.2.3-arm64.AppImage",
            "Pagis-1.2.3-arm64.deb",
        ),
    ] {
        assert!(
            inventory.contains(&format!(
                "cmp ../dist/{lock} {unpacked}/resources/runtime-lock.json"
            )),
            "{inventory}"
        );
        assert!(
            inventory.contains(&format!(
                "check-package.mjs {unpacked} release/{appimage} release/{deb}"
            )),
            "{inventory}"
        );
        assert!(
            inventory.contains(&format!("dpkg-deb -x release/{deb}")),
            "{inventory}"
        );
    }
    assert!(inventory.contains("apparmor-profile"), "{inventory}");
}

#[test]
fn the_smoke_runs_the_client_of_the_host_architecture_on_a_display() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let smoke = joined(step(&steps, "smoke"));
    assert!(smoke.contains("uname -m"), "{smoke}");
    assert!(smoke.contains("pagis-client\" --smoke"), "{smoke}");
    assert!(smoke.contains("xvfb-run -a"), "{smoke}");
    let runtime = joined(step(&steps, "packaged-runtime"));
    assert!(
        runtime.contains("check-packaged-runtime.mjs release/linux-unpacked"),
        "{runtime}"
    );
}

#[test]
fn a_routine_build_publishes_and_signs_nothing() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    for name in ["credentials", "released-tuple", "sign-checksums", "publish"] {
        skip_reason(step(&steps, name));
    }
    let sums = joined(step(&steps, "checksums"));
    assert!(sums.contains("sha256sum"), "{sums}");
    assert!(sums.contains(&checksums_name("1.2.3")), "{sums}");
}

#[test]
fn a_prepared_tag_builds_the_packages_and_stops_before_publication() {
    let mut prepare = context(Some("v1.2.3"));
    prepare.prepare_only = true;
    let steps = desktop_plan(Path::new("/repo"), &prepare);
    for name in ["pack", "inventory", "smoke", "checksums"] {
        assert!(
            matches!(step(&steps, name).action, Action::Run(_)),
            "{name}"
        );
    }
    for name in ["released-tuple", "sign-checksums", "publish"] {
        assert!(skip_reason(step(&steps, name)).contains("await"), "{name}");
    }
}

#[test]
fn publication_reuses_the_prepared_bytes_and_signs_their_checksums() {
    let mut publish = context(Some("v1.2.3"));
    publish.publish_existing = true;
    let steps = desktop_plan(Path::new("/repo"), &publish);
    for name in ["deps", "pack", "inventory", "smoke", "checksums"] {
        assert!(
            skip_reason(step(&steps, name)).contains("exact packages"),
            "{name}"
        );
    }
    let tuple = joined(step(&steps, "released-tuple"));
    assert!(tuple.contains("gh release download v1.2.3"), "{tuple}");
    assert!(
        tuple.contains("pagis-server-1.2.3-aarch64-unknown-linux-gnu.tar.gz"),
        "{tuple}"
    );
    assert!(tuple.contains("anonymous_pull \"$image\""), "{tuple}");
    assert!(!tuple.contains("imagetools"), "{tuple}");
    let sign = joined(step(&steps, "sign-checksums"));
    assert!(sign.contains(RELEASE_KEY), "{sign}");
    assert!(sign.contains("sha256sum -c"), "{sign}");
    assert!(sign.contains("--detach-sign"), "{sign}");
    assert!(sign.contains("gpgv --keyring"), "{sign}");
    let upload = joined(step(&steps, "publish"));
    for platform in ClientPlatform::LINUX {
        assert!(
            upload.contains(&appimage_name("1.2.3", platform)),
            "{upload}"
        );
        assert!(upload.contains(&deb_name("1.2.3", platform)), "{upload}");
    }
    assert!(
        upload.contains("Pagis-1.2.3-linux.SHA256SUMS.asc"),
        "{upload}"
    );
}

#[test]
fn publication_without_the_release_key_fails() {
    let mut publish = context(Some("v1.2.3"));
    publish.publish_existing = true;
    publish.missing_credentials = vec!["PAGIS_RELEASE_GPG_KEY".into()];
    let steps = desktop_plan(Path::new("/repo"), &publish);
    assert!(joined(step(&steps, "credentials")).contains("exit 1"));
    assert!(joined(step(&steps, "publish")).contains("exit 1"));
    assert_eq!(
        missing_signing_inputs(&|_| false),
        ["PAGIS_RELEASE_GPG_KEY"]
    );
    assert!(missing_signing_inputs(&|name| name == "PAGIS_RELEASE_GPG_KEY").is_empty());
}

#[test]
fn the_linux_shell_steps_are_valid_shell() {
    let mut prepare = context(Some("v1.2.3"));
    prepare.prepare_only = true;
    let mut publish = context(Some("v1.2.3"));
    publish.publish_existing = true;
    for cx in [context(None), prepare, publish] {
        for step in desktop_plan(Path::new("/repo"), &cx) {
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

#[test]
fn package_names_follow_electron_builder_on_linux() {
    assert_eq!(
        appimage_name("1.2.3", ClientPlatform::LinuxX64),
        "Pagis-1.2.3-x86_64.AppImage"
    );
    assert_eq!(
        deb_name("1.2.3", ClientPlatform::LinuxX64),
        "Pagis-1.2.3-amd64.deb"
    );
    assert_eq!(
        appimage_name("1.2.3", ClientPlatform::LinuxArm64),
        "Pagis-1.2.3-arm64.AppImage"
    );
}

/// The electron-builder config builds both formats for both
/// architectures, embeds the lock of each architecture, and names the
/// packages as the plan expects.
#[test]
fn electron_builder_packs_linux_with_the_lock_of_each_architecture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let config = std::fs::read_to_string(root.join("desktop/electron-builder.yml")).unwrap();
    let linux = config.split("\nlinux:\n").nth(1).expect("linux section");
    let linux = linux.split("\n\n").next().unwrap();
    assert!(linux.contains("target: AppImage"), "{linux}");
    assert!(linux.contains("target: deb"), "{linux}");
    assert!(
        linux.contains("- x64") && linux.contains("- arm64"),
        "{linux}"
    );
    assert!(
        linux.contains("from: ../dist/runtime-lock-linux-${arch}.json"),
        "{linux}"
    );
    assert!(linux.contains("executableName: pagis-client"), "{linux}");
    for section in ["appImage", "deb"] {
        let body = config
            .split(&format!("\n{section}:\n"))
            .nth(1)
            .unwrap_or_else(|| panic!("{section} section"));
        assert!(
            body.lines()
                .take(6)
                .any(|l| l.trim() == "artifactName: ${productName}-${version}-${arch}.${ext}"),
            "{section}: {body}"
        );
    }
}
