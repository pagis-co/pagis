//! Tests for the Linux Client App packaging (`cargo xtask desktop` on a
//! Linux host).

use std::path::Path;
use std::process::Command;

use xtask::desktop::{DesktopContext, DesktopPlatform, desktop_plan};
use xtask::desktop_linux::{
    RELEASE_KEY, UPDATE_KEY, appimage_name, checksums_name, deb_name, feed_name,
    missing_signing_inputs,
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

/// Each feed names the AppImage and the deb of its architecture with their
/// true SHA-512 and size, and the deb carries the Update Key and the
/// `package-type` file from which electron-updater knows a deb (ADR-0027).
#[test]
fn the_inventory_checks_the_update_feeds_the_update_key_and_the_deb_type() {
    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let inventory = joined(step(&steps, "inventory"));
    for (feed, appimage, deb) in [
        (
            "latest-linux.yml",
            "Pagis-1.2.3-x86_64.AppImage",
            "Pagis-1.2.3-amd64.deb",
        ),
        (
            "latest-linux-arm64.yml",
            "Pagis-1.2.3-arm64.AppImage",
            "Pagis-1.2.3-arm64.deb",
        ),
    ] {
        assert!(
            inventory.contains(&format!(
                "node scripts/check-update-feed.mjs release/{feed} 1.2.3 release/{appimage} release/{deb}"
            )),
            "{inventory}"
        );
    }
    assert!(
        inventory.contains(
            "cmp ../docs/update-key.pem \"$extracted/opt/Pagis/resources/update-key.pem\""
        ),
        "{inventory}"
    );
    assert!(
        inventory.contains("[ \"$(cat \"$extracted/opt/Pagis/resources/package-type\")\" = deb ]"),
        "{inventory}"
    );
}

#[test]
fn each_architecture_has_its_update_feed() {
    assert_eq!(feed_name(ClientPlatform::LinuxX64), "latest-linux.yml");
    assert_eq!(
        feed_name(ClientPlatform::LinuxArm64),
        "latest-linux-arm64.yml"
    );
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

/// The result of the smoke script against a stand-in client that exits
/// with `client_status`. The client records its `TMPDIR` and writes a file
/// there. A stand-in `timeout` records its arguments and runs the rest, so
/// a client that exits with 137 stands for a run that `timeout` killed.
struct SmokeRun {
    status: std::process::ExitStatus,
    stderr: String,
    timeout_args: String,
    client_tmpdir: String,
}

fn run_smoke(client_status: i32) -> SmokeRun {
    use std::os::unix::fs::PermissionsExt;

    let steps = desktop_plan(Path::new("/repo"), &context(None));
    let script = commands(step(&steps, "smoke"))[0].args[1].clone();
    let dir = tempfile::tempdir().unwrap();
    let executable = |path: &Path, body: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    for unpacked in ["linux-unpacked", "linux-arm64-unpacked"] {
        executable(
            &dir.path()
                .join("release")
                .join(unpacked)
                .join("pagis-client"),
            &format!(
                "#!/bin/sh\necho \"$TMPDIR\" > '{}'\ntouch \"$TMPDIR/profile\"\nexit {client_status}\n",
                dir.path().join("client-tmpdir").display()
            ),
        );
    }
    let bin = dir.path().join("bin");
    let args = dir.path().join("timeout-args");
    executable(
        &bin.join("timeout"),
        &format!(
            "#!/bin/sh\necho \"$@\" > '{}'\nshift 2\nexec \"$@\"\n",
            args.display()
        ),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new("sh")
        .args(["-c", &script])
        .current_dir(dir.path())
        .env("PATH", path)
        .env("DISPLAY", ":0")
        .output()
        .unwrap();
    SmokeRun {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        timeout_args: std::fs::read_to_string(&args).unwrap_or_default(),
        client_tmpdir: std::fs::read_to_string(dir.path().join("client-tmpdir"))
            .unwrap_or_default()
            .trim_end()
            .to_string(),
    }
}

#[test]
fn the_smoke_bounds_the_client_run_with_a_timeout() {
    let run = run_smoke(0);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(
        run.timeout_args.starts_with("--signal=KILL 120 release/"),
        "{}",
        run.timeout_args
    );
    assert!(
        run.timeout_args
            .trim_end()
            .ends_with("/pagis-client --smoke"),
        "{}",
        run.timeout_args
    );
}

#[test]
fn a_client_that_does_not_exit_in_time_fails_the_smoke() {
    let run = run_smoke(137);
    assert_eq!(run.status.code(), Some(1), "{}", run.stderr);
    assert!(
        run.stderr
            .contains("the client did not exit within 120 seconds"),
        "{}",
        run.stderr
    );
}

/// The client writes its profile under the temporary directory until the
/// process ends, so the smoke, not the client, removes it.
#[test]
fn the_smoke_removes_the_temporary_directory_of_the_client_after_it_exits() {
    for client_status in [0, 137] {
        let run = run_smoke(client_status);
        assert!(!run.client_tmpdir.is_empty(), "{}", run.stderr);
        assert_ne!(
            run.client_tmpdir,
            std::env::var("TMPDIR").unwrap_or_default()
        );
        assert!(
            !Path::new(&run.client_tmpdir).exists(),
            "{} remains",
            run.client_tmpdir
        );
    }
}

#[test]
fn a_client_failure_keeps_its_exit_status() {
    let run = run_smoke(3);
    assert_eq!(run.status.code(), Some(3), "{}", run.stderr);
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
    for feed in ["latest-linux.yml", "latest-linux-arm64.yml"] {
        assert!(
            upload.contains(&format!("desktop/release/{feed}")),
            "{upload}"
        );
    }
    assert!(!upload.contains("--clobber"), "{upload}");
}

/// The files that the publication uploads.
fn upload_files(steps: &[Step]) -> Vec<String> {
    commands(step(steps, "publish"))[0].args.clone()
}

/// The Update Key signs the exact checksum list with Ed25519 (ADR-0027).
/// The private key goes from the environment to a file that only the job
/// user reads, and the file is gone before the check of the signature with
/// the public key of the repository.
#[test]
fn publication_signs_the_checksum_list_with_the_update_key() {
    let mut publish = context(Some("v1.2.3"));
    publish.publish_existing = true;
    let steps = desktop_plan(Path::new("/repo"), &publish);
    let sign = joined(step(&steps, "sign-checksums"));
    let sums = "desktop/release/Pagis-1.2.3-linux.SHA256SUMS";
    let ordered = [
        format!("test -f {UPDATE_KEY} ||"),
        ": \"${PAGIS_UPDATE_SIGNING_KEY:?".to_string(),
        format!("rm -f {sums}.asc {sums}.sig"),
        "key=$(mktemp)".to_string(),
        "chmod 600 \"$key\"".to_string(),
        "printf '%s\\n' \"$PAGIS_UPDATE_SIGNING_KEY\" > \"$key\"".to_string(),
        format!("openssl pkeyutl -sign -rawin -inkey \"$key\" -in {sums} -out {sums}.sig"),
        "rm -f \"$key\"".to_string(),
        format!(
            "openssl pkeyutl -verify -pubin -inkey {UPDATE_KEY} -rawin -in {sums} -sigfile {sums}.sig"
        ),
    ];
    let mut from = 0;
    for part in &ordered {
        let at = sign[from..]
            .find(part.as_str())
            .unwrap_or_else(|| panic!("no {part} after byte {from}: {sign}"));
        from += at + part.len();
    }
    assert_eq!(UPDATE_KEY, "docs/update-key.pem");
    assert!(
        sign.contains("trap 'rm -f \"$keyring\" \"$key\"' EXIT"),
        "{sign}"
    );
    let upload = upload_files(&steps);
    assert!(upload.contains(&format!("{sums}.sig")), "{upload:?}");
    assert!(upload.contains(&format!("{sums}.asc")), "{upload:?}");
}

#[test]
fn publication_without_the_signing_keys_fails() {
    let mut publish = context(Some("v1.2.3"));
    publish.publish_existing = true;
    publish.missing_credentials = vec!["PAGIS_UPDATE_SIGNING_KEY".into()];
    let steps = desktop_plan(Path::new("/repo"), &publish);
    assert!(joined(step(&steps, "credentials")).contains("exit 1"));
    assert!(joined(step(&steps, "publish")).contains("exit 1"));
    assert_eq!(
        missing_signing_inputs(&|_| false),
        ["PAGIS_RELEASE_GPG_KEY", "PAGIS_UPDATE_SIGNING_KEY"]
    );
    assert_eq!(
        missing_signing_inputs(&|name| name == "PAGIS_RELEASE_GPG_KEY"),
        ["PAGIS_UPDATE_SIGNING_KEY"]
    );
    assert!(
        missing_signing_inputs(&|name| {
            name == "PAGIS_RELEASE_GPG_KEY" || name == "PAGIS_UPDATE_SIGNING_KEY"
        })
        .is_empty()
    );
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
    assert!(
        linux.contains("    - from: ../docs/update-key.pem\n      to: update-key.pem\n"),
        "{linux}"
    );
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
