//! Tests for the release build (`cargo xtask release`).

use std::path::{Path, PathBuf};

use pagis_versions::{CARGO_AUDITABLE_SHA256, CARGO_AUDITABLE_VERSION};
use xtask::release::ClientPlatform;
use xtask::{Action, Cmd, ReleaseContext, Step, gog_asset_name, release_plan};

use crate::support::workspace_root;

/// The file name of the Linux server package of each architecture.
fn linux_archives() -> [String; 2] {
    ClientPlatform::LINUX.map(|platform| platform.server_package("1.2.3"))
}

const MAC_ARCHIVE: &str = "pagis-server-1.2.3-aarch64-apple-darwin.tar.gz";

fn context(host_macos: bool) -> ReleaseContext {
    ReleaseContext {
        version: "1.2.3".into(),
        image: "ghcr.io/pagis-co/pagis-computer:0.3.0".into(),
        host_macos,
        target_dir: PathBuf::from("/shared/pagis-target"),
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

fn joined(step: &Step) -> String {
    commands(step)
        .iter()
        .map(|c| format!("{} {}", c.program, c.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

// --- plan ---

#[test]
fn plan_lists_every_release_step_in_order() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "builder",
            "image",
            "image-secret-scan",
            "image-vuln-scan",
            "image-push",
            "image-digest",
            "server-image",
            "server-image-secret-scan",
            "server-image-vuln-scan",
            "server-image-push",
            "anonymous-pull",
            "ui-build",
            "build-aarch64-apple-darwin",
            "build-x86_64-unknown-linux-gnu",
            "build-aarch64-unknown-linux-gnu",
            "assemble-server",
            "server-package-vuln-scan",
            "macos-server-dmg",
            "linux-server-archives",
            "linux-runtime-locks",
            "validate-server-release",
            "publish",
        ]
    );
}

#[test]
fn the_immutable_image_digest_is_resolved_before_server_builds() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let digest = joined(step(&steps, "image-digest"));
    assert!(digest.contains("imagetools inspect"), "{digest}");
    assert!(digest.contains("computer-image.txt"), "{digest}");
    let build = joined(step(&steps, "build-aarch64-apple-darwin"));
    assert!(build.contains("PAGIS_COMPUTER_IMAGE"), "{build}");
    assert!(build.contains("computer-image.txt"), "{build}");
}

/// A new person pulls both images with no registry login, so the
/// release pulls them the same way before it builds anything that
/// names them, and a refusal stops the release.
#[test]
fn both_pinned_images_pull_with_no_credentials_before_the_builds() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let pull = joined(step(&steps, "anonymous-pull"));
    for (repository, tag) in [
        ("pagis-co/pagis-computer", "0.3.0"),
        ("pagis-co/pagis-server", "1.2.3"),
    ] {
        assert!(
            pull.contains(&format!(
                "https://ghcr.io/token?scope=repository:{repository}:pull"
            )),
            "{pull}"
        );
        assert!(
            pull.contains(&format!("https://ghcr.io/v2/{repository}/manifests/{tag}")),
            "{pull}"
        );
    }
    // No stored login takes part.
    assert!(!pull.contains("docker login"), "{pull}");
    assert!(!pull.contains("GITHUB_TOKEN"), "{pull}");
    assert!(pull.contains("exit 1"), "{pull}");
}

#[test]
fn macos_builds_natively_and_linux_builds_through_cross() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    assert!(joined(step(&steps, "build-aarch64-apple-darwin")).contains("cargo auditable build"));
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let s = step(&steps, &format!("build-{target}"));
        let cmd = &commands(s)[0];
        assert_eq!(cmd.program, "sh");
        assert!(joined(s).contains("cross build"), "{:?}", cmd.args);
        assert!(joined(s).contains(target), "{:?}", cmd.args);
    }
}

#[test]
fn every_build_is_a_release_build_of_the_pagis_binary() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    for name in [
        "build-aarch64-apple-darwin",
        "build-x86_64-unknown-linux-gnu",
        "build-aarch64-unknown-linux-gnu",
    ] {
        let command = joined(step(&steps, name));
        assert!(
            command.contains("build --release -p pagis"),
            "{name}: {command}"
        );
    }
}

/// The text that `script` writes to `path` with a quoted here-document
/// (`cat > "<path>" <<'EOF'`).
fn heredoc<'a>(script: &'a str, path: &str) -> &'a str {
    let start = format!("cat > \"{path}\" <<'EOF'\n");
    let body = &script[script
        .find(&start)
        .unwrap_or_else(|| panic!("no here-document for {path} in {script}"))
        + start.len()..];
    &body[..body.find("\nEOF\n").expect("the here-document ends")]
}

/// The pinned archive of cargo-auditable for `platform`, and its SHA-256.
fn cargo_auditable_archive(platform: &str) -> (String, &'static str) {
    let (_, sha256) = CARGO_AUDITABLE_SHA256
        .iter()
        .find(|(name, _)| *name == platform)
        .unwrap_or_else(|| panic!("no cargo-auditable pin for {platform}"));
    (
        format!(
            "https://github.com/rust-secure-code/cargo-auditable/releases/download/v{CARGO_AUDITABLE_VERSION}/cargo-auditable-{platform}.tar.xz"
        ),
        sha256,
    )
}

/// cargo-auditable writes the list of the crates of `pagis` into the
/// executable, so Trivy identifies them in the Server Package. The macOS
/// build runs `cargo auditable build` on the host, with the pinned
/// cargo-auditable of the host, which the step checks against its SHA-256
/// and keeps in the shared target directory.
#[test]
fn the_macos_build_runs_cargo_auditable_with_the_pinned_cargo_auditable() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let script = joined(step(&steps, "build-aarch64-apple-darwin"));
    let (url, sha256) = cargo_auditable_archive("aarch64-apple-darwin");
    assert!(script.contains(&url), "{script}");
    assert!(script.contains(sha256), "{script}");
    assert!(script.contains("shasum -a 256 -c"), "{script}");
    assert!(
        script.contains("/shared/pagis-target/cargo-auditable"),
        "{script}"
    );
    assert!(
        script.contains(
            "PATH=\"$work:$PATH\" cargo auditable build --release -p pagis --target aarch64-apple-darwin"
        ),
        "{script}"
    );
}

/// `cross` runs `cargo build` in the Linux image of the target, and it
/// runs every other cargo subcommand on the host, so `cross auditable
/// build` would build on the macOS host. The Linux build therefore gives
/// `cross` an image of its own (`CROSS_BUILD_DOCKERFILE`): the default
/// image of the target, the pinned cargo-auditable of that linux/amd64
/// image, and a `cargo` in front of the toolchain's own.
#[test]
fn each_linux_build_runs_cargo_auditable_in_the_image_of_cross() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let (url, sha256) = cargo_auditable_archive("x86_64-unknown-linux-musl");
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let script = joined(step(&steps, &format!("build-{target}")));
        assert!(script.contains(&url), "{script}");
        assert!(script.contains(sha256), "{script}");
        assert!(script.contains("shasum -a 256 -c"), "{script}");
        assert!(
            script.contains(&format!(
                "CROSS_BUILD_DOCKERFILE=\"$work/Dockerfile\" CROSS_BUILD_DOCKERFILE_CONTEXT=\"$work\" \
                 cross build --release -p pagis --target {target}"
            )),
            "{script}"
        );
        let dockerfile: Vec<&str> = heredoc(&script, "$work/Dockerfile")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect();
        assert_eq!(
            dockerfile,
            [
                "ARG CROSS_BASE_IMAGE",
                "FROM --platform=linux/amd64 $CROSS_BASE_IMAGE",
                "COPY cargo cargo-auditable /usr/local/bin/",
            ]
        );
    }
}

/// The images of `cross` are linux/amd64 only, and the pinned
/// cargo-auditable of the image is an x86_64 executable. The release runs
/// on an arm64 Mac, where a build with no platform asks for linux/arm64
/// and finds no such image. So the Dockerfile names the platform of its
/// base, which works with each version of `cross` and adds no second
/// `--platform` to the build.
#[test]
fn the_image_of_cross_is_linux_amd64_on_every_host() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let script = joined(step(&steps, &format!("build-{target}")));
        let from: Vec<&str> = heredoc(&script, "$work/Dockerfile")
            .lines()
            .filter(|line| line.starts_with("FROM "))
            .collect();
        assert_eq!(from, ["FROM --platform=linux/amd64 $CROSS_BASE_IMAGE"]);
        assert!(!script.contains("CROSS_BUILD_OPTS"), "{script}");
    }
}

/// The `cargo` of that image. `cross` starts `cargo` by name, with the
/// directory of the toolchain last in PATH. This `cargo` is earlier in
/// PATH: it starts the next `cargo` in PATH as `cargo auditable`, with the
/// same arguments. It never starts itself again, and with no other `cargo`
/// in PATH it fails.
#[test]
fn the_cargo_of_the_cross_image_starts_the_toolchain_cargo_as_cargo_auditable() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let script = joined(step(&steps, "build-x86_64-unknown-linux-gnu"));
    let front = tempfile::tempdir().unwrap();
    let toolchain = tempfile::tempdir().unwrap();
    let executable = |path: &Path, text: &str| {
        std::fs::write(path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    };
    executable(
        &front.path().join("cargo"),
        &format!("{}\n", heredoc(&script, "$work/cargo")),
    );
    executable(
        &toolchain.path().join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\n",
    );
    let run = |path: String| {
        std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "cargo build --release -p pagis --target x86_64-unknown-linux-gnu",
            ])
            .env("PATH", path)
            .output()
            .expect("start sh")
    };

    let ran = run(format!(
        "{}:{}",
        front.path().display(),
        toolchain.path().display()
    ));
    assert!(ran.status.success(), "{ran:?}");
    let args = std::fs::read_to_string(toolchain.path().join("cargo.args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "auditable",
            "build",
            "--release",
            "-p",
            "pagis",
            "--target",
            "x86_64-unknown-linux-gnu"
        ]
    );

    let alone = run(front.path().display().to_string());
    assert_eq!(alone.status.code(), Some(127), "{alone:?}");
}

#[test]
fn the_macos_build_skips_on_a_linux_host() {
    let steps = release_plan(Path::new("/repo"), &context(false));
    assert!(matches!(
        step(&steps, "build-aarch64-apple-darwin").action,
        Action::Skip(_)
    ));
}

#[test]
fn the_ui_build_runs_before_the_binaries_so_the_spa_embeds() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let ui = step(&steps, "ui-build");
    let cmds = commands(ui);
    assert_eq!(cmds.last().unwrap().args, ["run", "build"]);
    for cmd in cmds {
        assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/repo/ui")));
    }
}

#[test]
fn packaging_refuses_a_missing_or_newer_product_app() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let script = joined(step(&steps, "assemble-server"));
    assert!(script.contains("ui/dist/index.html"), "{script}");
    assert!(script.contains("-newer \"$CARGO_TARGET_DIR/"), "{script}");
    assert!(script.contains("stale embedded Product App"), "{script}");
    assert!(
        script.contains("$CARGO_TARGET_DIR/aarch64-apple-darwin/release/pagis"),
        "{script}"
    );
    assert_eq!(
        commands(step(&steps, "assemble-server"))[0].env,
        [(
            "CARGO_TARGET_DIR".to_string(),
            "/shared/pagis-target".to_string()
        )]
    );
}

/// Each package tree holds the five server files. Only Linux gets an
/// archive: it is the server package of the Linux Client App, and the
/// macOS Client App installs the disk image.
#[test]
fn packaging_archives_the_linux_server_packages_only() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let assemble = joined(step(&steps, "assemble-server"));
    let script = joined(step(&steps, "linux-server-archives"));
    for archive in linux_archives() {
        assert!(script.contains(&archive), "missing {archive} in {script}");
    }
    assert!(!script.contains("aarch64-apple-darwin"), "{script}");
    assert!(!script.contains("SHA256SUMS"), "{script}");
    for upstream in [
        "gogcli_0.42.0_darwin_arm64.tar.gz",
        "gogcli_0.42.0_linux_amd64.tar.gz",
        "gogcli_0.42.0_linux_arm64.tar.gz",
    ] {
        assert!(
            assemble.contains(upstream),
            "missing {upstream} in {assemble}"
        );
    }
    assert!(
        assemble.contains("github.com/openclaw/gogcli/releases/download/v0.42.0"),
        "{assemble}"
    );
    assert!(assemble.contains("LICENSE.gog"), "{assemble}");
    assert!(assemble.contains("cp LICENSE "), "{assemble}");
    assert!(assemble.contains("THIRD_PARTY_NOTICES"), "{assemble}");
    assert!(
        script.contains("pagis gog LICENSE LICENSE.gog THIRD_PARTY_NOTICES"),
        "{script}"
    );
}

#[test]
fn packaging_leaves_out_the_target_whose_build_was_skipped() {
    let steps = release_plan(Path::new("/repo"), &context(false));
    let script = joined(step(&steps, "assemble-server"));
    assert!(
        !script.contains("aarch64-apple-darwin"),
        "skipped target packaged: {script}"
    );
}

#[test]
fn the_macos_server_is_signed_packaged_notarized_stapled_and_locked() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let script = joined(step(&steps, "macos-server-dmg"));
    assert!(script.contains("Developer ID Application"), "{script}");
    assert!(script.contains("CSC_NAME"), "{script}");
    assert!(script.contains("APPLE_KEYCHAIN_PROFILE"), "{script}");
    assert!(script.contains("APPLE_KEYCHAIN"), "{script}");
    assert!(script.contains("com.pagis.server"), "{script}");
    assert!(script.contains("com.pagis.gog"), "{script}");
    assert!(script.contains("codesign --verify --strict"), "{script}");
    assert!(script.contains("hdiutil create"), "{script}");
    assert!(script.contains("notarytool submit"), "{script}");
    assert!(script.contains("stapler staple"), "{script}");
    assert!(script.contains("stapler validate"), "{script}");
    assert!(
        script.contains("runtime-lock write darwin-arm64"),
        "{script}"
    );
    assert!(
        script.contains("dist/runtime-lock-darwin-arm64.json"),
        "{script}"
    );
    assert!(script.contains("hdiutil attach -readonly"), "{script}");
}

/// The Linux Client App's lock names the server archive of its
/// architecture, from the finished bytes and the tree they extract to.
#[test]
fn each_linux_archive_gets_the_runtime_lock_of_its_architecture() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    let locks = names
        .iter()
        .position(|n| *n == "linux-runtime-locks")
        .unwrap();
    assert!(
        names
            .iter()
            .position(|n| *n == "linux-server-archives")
            .unwrap()
            < locks
    );
    let script = joined(step(&steps, "linux-runtime-locks"));
    for platform in ClientPlatform::LINUX {
        let name = platform.name();
        assert!(
            script.contains(&format!(
                "runtime-lock write {name} 1.2.3 \"$(cat dist/computer-image.txt)\" dist/{}",
                platform.server_package("1.2.3")
            )),
            "{script}"
        );
        assert!(
            script.contains(&format!("dist/runtime-lock-{name}.json")),
            "{script}"
        );
    }
    assert!(script.contains("tar -xzpf"), "{script}");
}

#[test]
fn the_archives_carry_no_macos_metadata_member() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let script = joined(step(&steps, "linux-server-archives"));
    assert!(script.contains("COPYFILE_DISABLE=1"), "{script}");
}

/// A pushed layer of a public package cannot be recalled. So each image
/// is built for both architectures and exported, gitleaks scans the
/// exported filesystem, and the push comes after the scan. The push
/// builds with the same builder, platforms and build arguments, so it
/// takes every layer from the build cache of the scanned build. After a
/// push the export has no use, and it holds gigabytes, so it goes.
#[test]
fn each_image_is_scanned_for_secrets_before_its_push() {
    // The release runs from its own tree, and its scans read the pins of
    // that tree.
    let steps = release_plan(&workspace_root(), &context(true));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    let at = |name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("no step {name} in {names:?}"))
    };
    for (build, scan, push, export, tag) in [
        (
            "image",
            "image-secret-scan",
            "image-push",
            "dist/image-fs/computer",
            "ghcr.io/pagis-co/pagis-computer:0.3.0",
        ),
        (
            "server-image",
            "server-image-secret-scan",
            "server-image-push",
            "dist/image-fs/server",
            "ghcr.io/pagis-co/pagis-server:1.2.3",
        ),
    ] {
        assert!(at(build) < at(scan) && at(scan) < at(push), "{names:?}");

        let built = joined(step(&steps, build));
        assert!(!built.contains("--push"), "{built}");
        assert!(!built.contains(tag), "{built}");
        assert!(
            built.contains(&format!("type=local,dest={export}")),
            "{built}"
        );

        let scanned = joined(step(&steps, scan));
        assert!(scanned.contains("gitleaks"), "{scanned}");
        assert!(scanned.contains(export), "{scanned}");
        assert!(
            scanned.contains("/shared/pagis-target/gitleaks"),
            "{scanned}"
        );

        let pushed = joined(step(&steps, push));
        assert!(pushed.contains("--push"), "{pushed}");
        assert!(pushed.contains(tag), "{pushed}");
        assert!(
            pushed.ends_with(&format!("rm -rf {export}")),
            "the export is removed after the push: {pushed}"
        );
        for same in ["--builder pagis", "--platform linux/amd64,linux/arm64"] {
            assert!(built.contains(same), "{built}");
            assert!(pushed.contains(same), "{pushed}");
        }
    }
    let server_build = joined(step(&steps, "server-image"));
    let server_push = joined(step(&steps, "server-image-push"));
    for script in [server_build, server_push] {
        assert!(
            script.contains("--build-arg PAGIS_COMPUTER_IMAGE=\"$computer\""),
            "{script}"
        );
    }
}

/// A release cannot ship with an unreviewed advisory. So Trivy scans each
/// image after its secret scan and before its push, and the package tree
/// of each built Server Package before it is signed, packed and
/// published.
#[test]
fn each_image_and_server_package_is_scanned_for_vulnerabilities_before_it_ships() {
    // The release runs from its own tree, and its scans read the pins of
    // that tree.
    let steps = release_plan(&workspace_root(), &context(true));
    let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
    let at = |name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("no step {name} in {names:?}"))
    };
    for (secret_scan, scan, push, export) in [
        (
            "image-secret-scan",
            "image-vuln-scan",
            "image-push",
            "dist/image-fs/computer",
        ),
        (
            "server-image-secret-scan",
            "server-image-vuln-scan",
            "server-image-push",
            "dist/image-fs/server",
        ),
    ] {
        assert!(
            at(secret_scan) < at(scan) && at(scan) < at(push),
            "{names:?}"
        );
        let scanned = joined(step(&steps, scan));
        assert!(scanned.contains("--scanners vuln"), "{scanned}");
        assert!(scanned.contains("/shared/pagis-target/trivy"), "{scanned}");
        for platform in ["linux_amd64", "linux_arm64"] {
            assert!(
                scanned.contains(&format!("{export}/{platform}")),
                "{scanned}"
            );
        }
    }

    let packages = at("server-package-vuln-scan");
    assert!(at("assemble-server") < packages, "{names:?}");
    for later in ["macos-server-dmg", "linux-server-archives", "publish"] {
        assert!(packages < at(later), "{names:?}");
    }
    let scanned = joined(step(&steps, "server-package-vuln-scan"));
    assert!(scanned.contains("--scanners vuln"), "{scanned}");
    for triple in [
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        assert!(
            scanned.contains(&format!("dist/package-{triple}")),
            "{scanned}"
        );
    }
}

/// A host that builds no macOS package scans the packages it built.
#[test]
fn the_server_package_scan_leaves_out_the_target_whose_build_was_skipped() {
    // The release runs from its own tree, and its scans read the pins of
    // that tree.
    let steps = release_plan(&workspace_root(), &context(false));
    let scanned = joined(step(&steps, "server-package-vuln-scan"));
    assert!(!scanned.contains("aarch64-apple-darwin"), "{scanned}");
    assert!(
        scanned.contains("dist/package-x86_64-unknown-linux-gnu"),
        "{scanned}"
    );
}

#[test]
fn the_image_push_pushes_both_architectures_under_the_pinned_tag() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let cmd = &commands(step(&steps, "image-push"))[0];
    assert_eq!(cmd.program, "docker");
    let args = cmd.args.join(" ");
    assert!(args.contains("buildx"), "{args}");
    assert!(args.contains("linux/amd64,linux/arm64"), "{args}");
    assert!(
        args.contains("ghcr.io/pagis-co/pagis-computer:0.3.0"),
        "{args}"
    );
    assert!(args.contains("--push"), "{args}");
}

/// The server release publishes the server package of each Client App
/// platform and the Runtime Lock that names it, and nothing else.
#[test]
fn publishing_uploads_the_server_packages_and_their_locks_only() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let publish = commands(step(&steps, "publish"));
    assert_eq!(
        publish[0].program, "sh",
        "validation must run before upload"
    );
    let cmd = &publish[1];
    assert_eq!(cmd.program, "gh");
    assert!(cmd.args.contains(&"v1.2.3".to_string()), "{:?}", cmd.args);
    let assets: Vec<&str> = cmd
        .args
        .iter()
        .filter(|arg| arg.starts_with("dist/"))
        .map(String::as_str)
        .collect();
    let [x64, arm64] = linux_archives();
    assert_eq!(
        assets,
        [
            format!("dist/{x64}"),
            format!("dist/{arm64}"),
            "dist/pagis-server-1.2.3-aarch64-apple-darwin.dmg".to_string(),
            "dist/runtime-lock-darwin-arm64.json".to_string(),
            "dist/runtime-lock-linux-x64.json".to_string(),
            "dist/runtime-lock-linux-arm64.json".to_string(),
        ]
    );
}

#[test]
fn release_validation_fails_closed_over_every_server_artifact() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let validation = joined(step(&steps, "validate-server-release"));
    for archive in linux_archives() {
        assert!(validation.contains(&archive), "{validation}");
    }
    assert!(!validation.contains(MAC_ARCHIVE), "{validation}");
    assert!(!validation.contains("SHA256SUMS"), "{validation}");
    for lock in [
        "runtime-lock-darwin-arm64.json",
        "runtime-lock-linux-x64.json",
        "runtime-lock-linux-arm64.json",
    ] {
        assert!(
            validation.contains(&format!("runtime-lock validate dist/{lock}")),
            "{validation}"
        );
        assert!(
            validation.contains(&format!("missing {lock}")),
            "{validation}"
        );
    }
    assert!(
        validation.contains("codesign --verify --strict"),
        "{validation}"
    );
    assert!(validation.contains("stapler validate"), "{validation}");
    assert!(
        validation.contains("env -i PATH=/usr/bin:/bin"),
        "{validation}"
    );
    assert!(validation.contains("package-check"), "{validation}");
}

/// The release checks each `gog` archive that it bundles against the
/// SHA-256 that `pagis-versions` pins for its platform.
#[test]
fn the_release_checks_each_gog_archive_against_its_pin() {
    let steps = release_plan(Path::new("/repo"), &context(true));
    let assemble = joined(step(&steps, "assemble-server"));
    for (platform, sha256) in pagis_versions::GOG_SHA256 {
        let archive = format!("gogcli_{}_{platform}.tar.gz", pagis_versions::GOG_VERSION);
        let check = assemble
            .lines()
            .find(|line| line.contains("shasum -a 256 -c") && line.contains(&archive))
            .unwrap_or_else(|| panic!("no check of {archive} in {assemble}"));
        assert!(check.contains(sha256), "{check}");
    }
}

#[test]
fn gog_asset_names_pin_v0420_for_every_pagis_target() {
    assert_eq!(
        gog_asset_name("aarch64-apple-darwin"),
        "gogcli_0.42.0_darwin_arm64.tar.gz"
    );
    assert_eq!(
        gog_asset_name("x86_64-unknown-linux-gnu"),
        "gogcli_0.42.0_linux_amd64.tar.gz"
    );
    assert_eq!(
        gog_asset_name("aarch64-unknown-linux-gnu"),
        "gogcli_0.42.0_linux_arm64.tar.gz"
    );
}
