//! Tests for the release build (`cargo xtask release`).

use std::path::{Path, PathBuf};

use pagis_versions::{CARGO_AUDITABLE_SHA256, CARGO_AUDITABLE_VERSION};
use xtask::release::ClientPlatform;
use xtask::{
    Action, Cmd, ImagePlatform, ReleaseContext, ReleaseStage, Step, gog_asset_name, release_plan,
};

use crate::support::workspace_root;

/// The file name of the Linux server package of each architecture.
fn linux_archives() -> [String; 2] {
    ClientPlatform::LINUX.map(|platform| platform.server_package("1.2.3"))
}

const MAC_ARCHIVE: &str = "pagis-server-1.2.3-aarch64-apple-darwin.tar.gz";

fn context() -> ReleaseContext {
    ReleaseContext {
        version: "1.2.3".into(),
        image: "ghcr.io/pagis-co/pagis-computer:0.3.0".into(),
        computer_published: false,
        platforms: ImagePlatform::ALL.to_vec(),
        target_dir: PathBuf::from("/shared/pagis-target"),
    }
}

fn plan(stage: ReleaseStage) -> Vec<Step> {
    release_plan(Path::new("/repo"), &context(), stage)
}

/// The plan of `stage` for this tree: its scans read the pins of the tree.
fn tree_plan(stage: ReleaseStage) -> Vec<Step> {
    release_plan(&workspace_root(), &context(), stage)
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
fn each_stage_lists_its_steps_in_order() {
    let names = |stage| plan(stage).iter().map(|s| s.name).collect::<Vec<_>>();
    assert_eq!(
        names(ReleaseStage::ComputerImage),
        [
            "builder",
            "image-amd64",
            "image-secret-scan-amd64",
            "image-vuln-scan-amd64",
            "image-push-amd64",
            "image-arm64",
            "image-secret-scan-arm64",
            "image-vuln-scan-arm64",
            "image-push-arm64",
        ]
    );
    assert_eq!(
        names(ReleaseStage::ComputerManifest),
        ["image-manifest", "image-digest"]
    );
    assert_eq!(
        names(ReleaseStage::ServerImage),
        [
            "builder",
            "server-image-amd64",
            "server-image-secret-scan-amd64",
            "server-image-vuln-scan-amd64",
            "server-image-push-amd64",
            "server-image-arm64",
            "server-image-secret-scan-arm64",
            "server-image-vuln-scan-arm64",
            "server-image-push-arm64",
        ]
    );
    assert_eq!(
        names(ReleaseStage::ServerManifest),
        ["server-image-manifest", "anonymous-pull"]
    );
    assert_eq!(
        names(ReleaseStage::Linux),
        [
            "ui-build",
            "build-x86_64-unknown-linux-gnu",
            "build-aarch64-unknown-linux-gnu",
            "assemble-server",
            "server-package-vuln-scan",
            "linux-server-archives",
            "linux-runtime-locks",
        ]
    );
    assert_eq!(
        names(ReleaseStage::Macos),
        [
            "ui-build",
            "build-aarch64-apple-darwin",
            "assemble-server",
            "server-package-vuln-scan",
            "macos-server-dmg",
        ]
    );
    assert_eq!(names(ReleaseStage::Draft), ["draft"]);
}

#[test]
fn each_stage_parses_from_its_name() {
    for stage in ReleaseStage::ALL {
        assert_eq!(ReleaseStage::parse(stage.name()).unwrap(), stage);
    }
    assert!(ReleaseStage::parse("publish").is_err());
    assert!(ReleaseStage::parse("images").is_err());
    assert_eq!(
        ReleaseStage::ALL.map(ReleaseStage::name),
        [
            "computer-image",
            "computer-manifest",
            "server-image",
            "server-manifest",
            "linux",
            "macos",
            "draft"
        ]
    );
    let on_macos: Vec<&str> = ReleaseStage::ALL
        .into_iter()
        .filter(|stage| stage.needs_macos())
        .map(ReleaseStage::name)
        .collect();
    assert_eq!(on_macos, ["macos", "draft"]);
}

/// A release job builds the image stages of one platform, on a runner of
/// that architecture.
#[test]
fn an_image_stage_builds_only_the_platforms_of_its_context() {
    let cx = ReleaseContext {
        platforms: vec![ImagePlatform::Arm64],
        ..context()
    };
    let names = |stage| {
        release_plan(Path::new("/repo"), &cx, stage)
            .iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(ReleaseStage::ComputerImage),
        [
            "builder",
            "image-arm64",
            "image-secret-scan-arm64",
            "image-vuln-scan-arm64",
            "image-push-arm64",
        ]
    );
    assert_eq!(
        names(ReleaseStage::ServerImage),
        [
            "builder",
            "server-image-arm64",
            "server-image-secret-scan-arm64",
            "server-image-vuln-scan-arm64",
            "server-image-push-arm64",
        ]
    );
    // The manifest joins every platform, whichever job runs it.
    let manifest = joined(step(
        &release_plan(Path::new("/repo"), &cx, ReleaseStage::ComputerManifest),
        "image-manifest",
    ));
    for arch in ["amd64", "arm64"] {
        assert!(
            manifest.contains(&format!("dist/image-digests/computer-{arch}.txt")),
            "{manifest}"
        );
    }
}

/// A published Computer image version is never pushed again: every
/// release that pins it pulls the same bytes. The release still resolves
/// its digest and builds the server image against it.
#[test]
fn a_published_computer_image_is_not_built_or_pushed_again() {
    let cx = ReleaseContext {
        computer_published: true,
        ..context()
    };
    let computer = release_plan(Path::new("/repo"), &cx, ReleaseStage::ComputerImage);
    assert_eq!(computer.len(), 9);
    for step in &computer {
        assert!(
            matches!(&step.action, Action::Skip(reason) if reason.contains("never pushed again")),
            "{} must skip",
            step.name
        );
    }
    let manifest = release_plan(Path::new("/repo"), &cx, ReleaseStage::ComputerManifest);
    assert!(matches!(
        step(&manifest, "image-manifest").action,
        Action::Skip(_)
    ));
    commands(step(&manifest, "image-digest"));
    for step in release_plan(Path::new("/repo"), &cx, ReleaseStage::ServerImage)
        .iter()
        .chain(&release_plan(
            Path::new("/repo"),
            &cx,
            ReleaseStage::ServerManifest,
        ))
    {
        commands(step);
    }
}

/// The registry answer decides whether the image is published. Only "not
/// found" means no; any other failure stops the release, because a wrong
/// no would push over a published version.
#[test]
fn only_a_not_found_answer_means_the_image_is_not_published() {
    let image = "ghcr.io/pagis-co/pagis-computer:0.3.0";
    assert!(xtask::inspect_answer(image, true, "").unwrap());
    assert!(
        !xtask::inspect_answer(
            image,
            false,
            "ERROR: ghcr.io/pagis-co/pagis-computer:0.3.0: not found"
        )
        .unwrap()
    );
    let error = xtask::inspect_answer(image, false, "ERROR: unexpected status: 401 Unauthorized")
        .unwrap_err()
        .to_string();
    assert!(error.contains("401 Unauthorized"), "{error}");
}

#[test]
fn the_immutable_image_digest_is_resolved_before_server_builds() {
    let digest = joined(step(&plan(ReleaseStage::ComputerManifest), "image-digest"));
    assert!(digest.contains("imagetools inspect"), "{digest}");
    assert!(digest.contains("computer-image.txt"), "{digest}");
    for (stage, target) in [
        (ReleaseStage::Macos, "aarch64-apple-darwin"),
        (ReleaseStage::Linux, "x86_64-unknown-linux-gnu"),
    ] {
        let build = joined(step(&plan(stage), &format!("build-{target}")));
        assert!(build.contains("PAGIS_COMPUTER_IMAGE"), "{build}");
        assert!(build.contains("computer-image.txt"), "{build}");
    }
}

/// A new person pulls both images with no registry login, so the
/// release pulls them the same way, and a refusal stops the release.
#[test]
fn both_pinned_images_pull_with_no_credentials() {
    let pull = joined(step(&plan(ReleaseStage::ServerManifest), "anonymous-pull"));
    for image in [
        "ghcr.io/pagis-co/pagis-computer:0.3.0",
        "ghcr.io/pagis-co/pagis-server:1.2.3",
    ] {
        assert!(
            pull.contains(&format!("anonymous_pull '{image}'")),
            "{pull}"
        );
    }
    // No stored login takes part.
    assert!(!pull.contains("docker login"), "{pull}");
    assert!(!pull.contains("GITHUB_TOKEN"), "{pull}");
}

/// The pull asks the registry for an anonymous token and then for the
/// manifest, by tag or by digest, and stops on a refusal.
#[cfg(unix)]
#[test]
fn the_anonymous_pull_asks_for_a_token_and_the_manifest() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("curl.log");
    let curl = dir.path().join("curl");
    // The fake registry grants a token and refuses the manifest of the
    // `missing` repository.
    std::fs::write(
        &curl,
        format!(
            "#!/bin/sh\nfor arg; do url=$arg; done\necho \"$url\" >> '{}'\n\
             case \"$url\" in\n*/token*) echo '{{\"token\":\"t\"}}' ;;\n*/missing/*) exit 22 ;;\nesac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let run = |image: &str| {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "set -eu\n{}anonymous_pull '{image}'\n",
                xtask::image::ANONYMOUS_PULL_FN
            ))
            .env("PATH", &path)
            .output()
            .unwrap()
    };

    let digest = format!("sha256:{}", "a".repeat(64));
    assert!(
        run("ghcr.io/pagis-co/pagis-computer:0.3.0")
            .status
            .success()
    );
    assert!(
        run(&format!("ghcr.io/pagis-co/pagis-computer@{digest}"))
            .status
            .success()
    );
    let refused = run("ghcr.io/pagis-co/missing:1.0.0");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("make the package public"),
        "{refused:?}"
    );

    let urls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        urls.lines().collect::<Vec<_>>(),
        [
            "https://ghcr.io/token?scope=repository:pagis-co/pagis-computer:pull",
            "https://ghcr.io/v2/pagis-co/pagis-computer/manifests/0.3.0",
            "https://ghcr.io/token?scope=repository:pagis-co/pagis-computer:pull",
            &format!("https://ghcr.io/v2/pagis-co/pagis-computer/manifests/{digest}"),
            "https://ghcr.io/token?scope=repository:pagis-co/missing:pull",
            "https://ghcr.io/v2/pagis-co/missing/manifests/1.0.0",
        ]
    );
}

#[test]
fn macos_builds_natively_and_linux_builds_through_cross() {
    let mac = plan(ReleaseStage::Macos);
    assert!(joined(step(&mac, "build-aarch64-apple-darwin")).contains("cargo auditable build"));
    let linux = plan(ReleaseStage::Linux);
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let s = step(&linux, &format!("build-{target}"));
        let cmd = &commands(s)[0];
        assert_eq!(cmd.program, "sh");
        assert!(joined(s).contains("cross build"), "{:?}", cmd.args);
        assert!(joined(s).contains(target), "{:?}", cmd.args);
    }
}

#[test]
fn every_build_is_a_release_build_of_the_pagis_binary() {
    for (stage, name) in [
        (ReleaseStage::Macos, "build-aarch64-apple-darwin"),
        (ReleaseStage::Linux, "build-x86_64-unknown-linux-gnu"),
        (ReleaseStage::Linux, "build-aarch64-unknown-linux-gnu"),
    ] {
        let command = joined(step(&plan(stage), name));
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
    let steps = plan(ReleaseStage::Macos);
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
    let steps = plan(ReleaseStage::Linux);
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
    let steps = plan(ReleaseStage::Linux);
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
    let steps = plan(ReleaseStage::Linux);
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

/// Each stage runs on one host, so it builds the targets of that host
/// and no other.
#[test]
fn each_stage_builds_only_the_targets_of_its_host() {
    let linux = plan(ReleaseStage::Linux);
    assert!(linux.iter().all(|s| s.name != "build-aarch64-apple-darwin"));
    let mac = plan(ReleaseStage::Macos);
    assert!(mac.iter().all(|s| !s.name.contains("linux")));
}

#[test]
fn the_ui_build_runs_before_the_binaries_so_the_spa_embeds() {
    let steps = plan(ReleaseStage::Macos);
    let ui = step(&steps, "ui-build");
    let cmds = commands(ui);
    assert_eq!(cmds.last().unwrap().args, ["run", "build"]);
    for cmd in cmds {
        assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/repo/ui")));
    }
}

#[test]
fn packaging_refuses_a_missing_or_newer_product_app() {
    let steps = plan(ReleaseStage::Macos);
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
    let steps = plan(ReleaseStage::Linux);
    let assemble = joined(step(&steps, "assemble-server"));
    let script = joined(step(&steps, "linux-server-archives"));
    for archive in linux_archives() {
        assert!(script.contains(&archive), "missing {archive} in {script}");
    }
    assert!(!script.contains("aarch64-apple-darwin"), "{script}");
    assert!(!script.contains("SHA256SUMS"), "{script}");
    for upstream in [
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
fn each_stage_assembles_only_the_packages_that_it_built() {
    let linux = joined(step(&plan(ReleaseStage::Linux), "assemble-server"));
    assert!(!linux.contains("aarch64-apple-darwin"), "{linux}");
    let mac = joined(step(&plan(ReleaseStage::Macos), "assemble-server"));
    assert!(mac.contains("gogcli_0.42.0_darwin_arm64.tar.gz"), "{mac}");
    assert!(!mac.contains("linux"), "{mac}");
}

#[test]
fn the_macos_server_is_signed_packaged_notarized_stapled_and_locked() {
    let steps = plan(ReleaseStage::Macos);
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
    assert!(
        script.contains("notarize dist/pagis-server-1.2.3-aarch64-apple-darwin.dmg"),
        "{script}"
    );
    // CI notarizes with an App Store Connect API key.
    for part in [
        "--key \"$APPLE_API_KEY\"",
        "--key-id \"$APPLE_API_KEY_ID\"",
        "--issuer \"$APPLE_API_ISSUER\"",
    ] {
        assert!(script.contains(part), "{part}: {script}");
    }
    assert!(!script.contains("APPLE_APP_SPECIFIC_PASSWORD"), "{script}");
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
    let steps = plan(ReleaseStage::Linux);
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
    let steps = plan(ReleaseStage::Linux);
    let script = joined(step(&steps, "linux-server-archives"));
    assert!(script.contains("COPYFILE_DISABLE=1"), "{script}");
}

/// A pushed layer of a public package cannot be recalled. So each image
/// is built for one architecture and exported, gitleaks scans the
/// exported filesystem, and the push of that architecture comes after
/// the scan. The push builds with the same builder, platform and build
/// arguments, so it takes every layer from the build cache of the scanned
/// build. It pushes by digest only, and the manifest stage puts the tag
/// on the index of both digests. After a push the export has no use, and
/// it holds gigabytes, so it goes.
#[test]
fn each_image_is_scanned_for_secrets_before_its_push() {
    // The release runs from its own tree, and its scans read the pins of
    // that tree.
    for (stage, prefix, name, repository) in [
        (
            ReleaseStage::ComputerImage,
            "image",
            "computer",
            "ghcr.io/pagis-co/pagis-computer",
        ),
        (
            ReleaseStage::ServerImage,
            "server-image",
            "server",
            "ghcr.io/pagis-co/pagis-server",
        ),
    ] {
        let steps = tree_plan(stage);
        let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
        let at = |name: &str| {
            names
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("no step {name} in {names:?}"))
        };
        for arch in ["amd64", "arm64"] {
            let build = format!("{prefix}-{arch}");
            let scan = format!("{prefix}-secret-scan-{arch}");
            let push = format!("{prefix}-push-{arch}");
            let export = format!("dist/image-fs/{name}/linux_{arch}");
            assert!(at(&build) < at(&scan) && at(&scan) < at(&push), "{names:?}");

            let built = joined(step(&steps, &build));
            assert!(!built.contains("push=true"), "{built}");
            assert!(!built.contains(repository), "{built}");
            assert!(
                built.contains(&format!("type=local,dest={export} ")),
                "{built}"
            );

            let scanned = joined(step(&steps, &scan));
            assert!(scanned.contains("gitleaks"), "{scanned}");
            assert!(scanned.contains(&export), "{scanned}");
            assert!(
                scanned.contains("/shared/pagis-target/gitleaks"),
                "{scanned}"
            );

            let pushed = joined(step(&steps, &push));
            assert!(
                pushed.contains(&format!(
                    "type=image,name={repository},push-by-digest=true,name-canonical=true,push=true"
                )),
                "{pushed}"
            );
            assert!(
                pushed.ends_with(&format!("rm -rf {export}")),
                "the export is removed after the push: {pushed}"
            );
            for same in [
                "--builder pagis".to_string(),
                format!("--platform linux/{arch} "),
            ] {
                assert!(built.contains(&same), "{built}");
                assert!(pushed.contains(&same), "{pushed}");
            }
            if stage == ReleaseStage::ServerImage {
                for script in [&built, &pushed] {
                    assert!(
                        script.contains("--build-arg PAGIS_COMPUTER_IMAGE=\"$computer\""),
                        "{script}"
                    );
                }
            }
        }
    }
}

/// A release cannot ship with an unreviewed advisory. So Trivy scans each
/// image after its secret scan and before its push, and the package tree
/// of each built Server Package before it is signed, packed and
/// published.
#[test]
fn each_image_and_server_package_is_scanned_for_vulnerabilities_before_it_ships() {
    let order = |steps: &[Step]| steps.iter().map(|s| s.name).collect::<Vec<_>>();
    let at = |names: &[&str], name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("no step {name} in {names:?}"))
    };
    for (stage, prefix, export) in [
        (
            ReleaseStage::ComputerImage,
            "image",
            "dist/image-fs/computer",
        ),
        (
            ReleaseStage::ServerImage,
            "server-image",
            "dist/image-fs/server",
        ),
    ] {
        let steps = tree_plan(stage);
        let names = order(&steps);
        for arch in ["amd64", "arm64"] {
            let scan = format!("{prefix}-vuln-scan-{arch}");
            assert!(
                at(&names, &format!("{prefix}-secret-scan-{arch}")) < at(&names, &scan)
                    && at(&names, &scan) < at(&names, &format!("{prefix}-push-{arch}")),
                "{names:?}"
            );
            let scanned = joined(step(&steps, &scan));
            assert!(scanned.contains("--scanners vuln"), "{scanned}");
            assert!(scanned.contains("/shared/pagis-target/trivy"), "{scanned}");
            assert!(
                scanned.contains(&format!("{export}/linux_{arch}")),
                "{scanned}"
            );
        }
    }

    for (stage, packed, triples) in [
        (
            ReleaseStage::Linux,
            "linux-server-archives",
            &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"][..],
        ),
        (
            ReleaseStage::Macos,
            "macos-server-dmg",
            &["aarch64-apple-darwin"][..],
        ),
    ] {
        let steps = tree_plan(stage);
        let names = order(&steps);
        let packages = at(&names, "server-package-vuln-scan");
        assert!(at(&names, "assemble-server") < packages, "{names:?}");
        assert!(packages < at(&names, packed), "{names:?}");
        let scanned = joined(step(&steps, "server-package-vuln-scan"));
        assert!(scanned.contains("--scanners vuln"), "{scanned}");
        for triple in triples {
            assert!(
                scanned.contains(&format!("dist/package-{triple}")),
                "{scanned}"
            );
        }
        let other = if stage == ReleaseStage::Linux {
            "apple-darwin"
        } else {
            "linux"
        };
        assert!(!scanned.contains(other), "{scanned}");
    }
}

/// The draft release of the tag holds the server package of each Client
/// App platform and the Runtime Lock that names it, and nothing else. It
/// stays a draft until a maintainer approves it, and it needs the tag the
/// release workflow runs on.
#[test]
fn the_draft_holds_the_server_packages_and_their_locks_only() {
    let steps = plan(ReleaseStage::Draft);
    let publish = commands(step(&steps, "draft"));
    assert_eq!(
        publish[0].program, "sh",
        "validation must run before upload"
    );
    let cmd = &publish[1];
    assert_eq!(cmd.program, "gh");
    assert_eq!(cmd.args[..3], ["release", "create", "v1.2.3"]);
    for flag in ["--draft", "--verify-tag"] {
        assert!(cmd.args.contains(&flag.to_string()), "{:?}", cmd.args);
    }
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
    let steps = plan(ReleaseStage::Draft);
    let validation = format!(
        "{} {}",
        commands(step(&steps, "draft"))[0].program,
        commands(step(&steps, "draft"))[0].args.join(" ")
    );
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
    let assemble = [ReleaseStage::Linux, ReleaseStage::Macos]
        .map(|stage| joined(step(&plan(stage), "assemble-server")))
        .join("\n");
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

// --- the release workflow ---

/// A tag runs the gate, then each stage in its own job after the stage it
/// reads, and holds each publication job in the `release` environment.
#[test]
fn the_tag_workflow_runs_every_stage_after_the_gate() {
    let workflow = std::fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("the release workflow");
    // Each job is the text from its key to the next key at the same depth.
    let job = |name: &str| -> String {
        let jobs = &workflow[workflow.find("\njobs:\n").expect("jobs")..];
        let mut body = String::new();
        let mut inside = false;
        for line in jobs.lines() {
            let is_key = line.starts_with("  ")
                && !line.starts_with("   ")
                && !line.trim_start().starts_with('#')
                && line.ends_with(':');
            if is_key {
                inside = line.trim() == format!("{name}:");
            } else if inside {
                body.push_str(line);
                body.push('\n');
            }
        }
        assert!(!body.is_empty(), "no job {name}");
        body
    };

    assert!(workflow.contains("tags: [\"v*\"]"), "{workflow}");
    assert!(job("gate").contains("uses: ./.github/workflows/ci.yml"));
    for (name, needs, command) in [
        (
            "computer-image",
            "needs: gate",
            "cargo xtask release computer-image --platform \"$PLATFORM\" --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "computer-manifest",
            "needs: computer-image",
            "cargo xtask release computer-manifest --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "server-image",
            "needs: computer-manifest",
            "cargo xtask release server-image --platform \"$PLATFORM\" --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "server-manifest",
            "needs: server-image",
            "cargo xtask release server-manifest --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "server-linux",
            "needs: computer-manifest",
            "cargo xtask release linux --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "server-macos",
            "needs: computer-manifest",
            "cargo xtask release macos --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "draft",
            "needs: [server-linux, server-macos, server-manifest]",
            "cargo xtask release draft --tag \"$GITHUB_REF_NAME\"",
        ),
        (
            "client-macos",
            "needs: draft",
            "cargo xtask desktop --tag \"$GITHUB_REF_NAME\" --prepare",
        ),
        (
            "client-linux",
            "needs: draft",
            "cargo xtask desktop --tag \"$GITHUB_REF_NAME\" --prepare",
        ),
        (
            "publish-macos",
            "needs: [client-macos, client-linux]",
            "--publish-existing",
        ),
        (
            "publish-linux",
            "needs: [client-macos, client-linux]",
            "--linux --tag \"$GITHUB_REF_NAME\" --publish-existing",
        ),
        (
            "publish",
            "needs: [publish-macos, publish-linux]",
            "--draft=false",
        ),
    ] {
        let body = job(name);
        assert!(body.contains(needs), "{name}: {body}");
        assert!(body.contains(command), "{name}: {body}");
    }
    for name in ["publish-macos", "publish-linux"] {
        assert!(job(name).contains("environment: release"), "{name}");
    }
    // The signing jobs read the API key, never an Apple ID.
    assert!(!workflow.contains("APPLE_APP_SPECIFIC_PASSWORD"));
    for name in ["server-macos", "client-macos"] {
        assert!(job(name).contains("secrets.APPLE_API_KEY_P8"), "{name}");
    }
    assert!(job("server-macos").contains("import-signing-identity.sh"));
    // Each job that builds the daemon writes the analytics project into it.
    for name in ["server-image", "server-linux", "server-macos"] {
        for variable in ["PAGIS_POSTHOG_PROJECT_ID", "PAGIS_POSTHOG_TOKEN"] {
            assert!(
                job(name).contains(&format!("{variable}: ${{{{ secrets.{variable} }}}}")),
                "{name}: {variable}"
            );
        }
    }
    for name in [
        "computer-manifest",
        "server-manifest",
        "draft",
        "client-macos",
        "client-linux",
    ] {
        assert!(job(name).contains("attest-build-provenance"), "{name}");
    }
    // Each image job builds one platform on a runner of that
    // architecture, with no emulation, and hands its digest to the
    // manifest job.
    for (name, digests) in [
        ("computer-image", "image-digest-computer-"),
        ("server-image", "image-digest-server-"),
    ] {
        let body = job(name);
        for runner in [
            "{ platform: amd64, runner: ubuntu-24.04 }",
            "{ platform: arm64, runner: ubuntu-24.04-arm }",
        ] {
            assert!(body.contains(runner), "{name}: {body}");
        }
        assert!(body.contains("runs-on: ${{ matrix.runner }}"), "{name}");
        assert!(body.contains("PLATFORM: ${{ matrix.platform }}"), "{name}");
        assert!(
            body.contains(&format!("name: {digests}${{{{ matrix.platform }}}}")),
            "{name}: {body}"
        );
    }
    for (name, digests) in [
        ("computer-manifest", "pattern: image-digest-computer-*"),
        ("server-manifest", "pattern: image-digest-server-*"),
    ] {
        assert!(job(name).contains(digests), "{name}");
    }
    assert!(!workflow.contains("setup-qemu"), "{workflow}");
}
