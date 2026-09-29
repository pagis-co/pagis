//! Tests for the contract check of the Google adapter with the pinned
//! `gog`, and for the pin of `gog`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use pagis_versions::{GOG_SHA256, GOG_VERSION};
use sha2::{Digest, Sha256};
use xtask::gog::{self, contract_step};
use xtask::tools::{self, Pin};
use xtask::{Action, Cmd, target_dir};

use crate::support::{host_index, host_is_pinned, run, script, workspace_root};

/// The step downloads the pinned `gog` archive of the host into the
/// shared target directory, checks its SHA-256, and runs the pinned `gog`
/// tests of the Google adapter with that binary.
#[test]
fn the_contract_check_runs_the_pinned_gog_tests_of_the_google_adapter() {
    let tree = tempfile::tempdir().unwrap();
    let step = contract_step(tree.path(), Path::new("/shared/pagis-target"));
    assert_eq!(step.name, "gog-contract");
    if !host_is_pinned() {
        let Action::Skip(reason) = &step.action else {
            panic!("a host without a pin must skip, got {:?}", step.action);
        };
        assert!(reason.contains("gog"), "{reason}");
        return;
    }
    let script = script(&step);
    assert!(
        script.contains(&format!(
            "https://github.com/openclaw/gogcli/releases/download/v{GOG_VERSION}/gogcli_{GOG_VERSION}_"
        )),
        "{script}"
    );
    assert!(
        GOG_SHA256.iter().any(|(_, sha256)| script.contains(sha256)),
        "{script}"
    );
    assert!(script.contains("shasum -a 256 -c"), "{script}");
    assert!(
        script.contains("/shared/pagis-target/gog"),
        "the download is kept in the shared target directory: {script}"
    );
    assert!(
        script.contains(
            "PAGIS_GOG=\"$work/gog\" cargo nextest run -p pagis-google --run-ignored only -E 'test(/^pinned_gog::/)'"
        ),
        "{script}"
    );
    let Action::Run(cmds) = &step.action else {
        unreachable!()
    };
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].cwd.as_deref(), Some(tree.path()));
}

/// A local `gog` release archive, and a `curl` that serves it for each
/// address and logs the address. With it on `PATH`, a check downloads
/// nothing.
struct LocalRelease {
    dir: tempfile::TempDir,
    sha256: String,
}

impl LocalRelease {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let package = root.join("package");
        std::fs::create_dir(&package).unwrap();
        executable(
            &package.join("gog"),
            "#!/bin/sh\necho 'gog v99.1.0 (local)'\n",
        );
        let archive = root.join("gog.tar.gz");
        let status = std::process::Command::new("tar")
            .args(["-czf", archive.to_str().unwrap(), "-C"])
            .arg(&package)
            .arg("./gog")
            .env("COPYFILE_DISABLE", "1")
            .status()
            .expect("run tar");
        assert!(status.success());
        let sha256 = hex::encode(Sha256::digest(std::fs::read(&archive).unwrap()));
        std::fs::create_dir(root.join("bin")).unwrap();
        executable(
            &root.join("bin/curl"),
            &format!(
                "#!/bin/sh\n\
                 out=''\n\
                 while [ \"$#\" -gt 0 ]; do\n\
                   case \"$1\" in\n\
                     -o) out=$2; shift 2 ;;\n\
                     -*) shift ;;\n\
                     *) echo \"$1\" >> '{log}'; shift ;;\n\
                   esac\n\
                 done\n\
                 cp '{archive}' \"$out\"\n",
                log = root.join("curl.log").display(),
                archive = archive.display(),
            ),
        );
        executable(
            &root.join("bin/cargo"),
            &format!(
                "#!/bin/sh\n\
                 echo \"cargo $*\" >> '{log}'\n\
                 \"$PAGIS_GOG\" --version >> '{log}'\n",
                log = root.join("cargo.log").display()
            ),
        );
        LocalRelease { dir, sha256 }
    }

    /// Each command that the fake `cargo` got, which is where the check
    /// runs the tests of the Google adapter.
    fn tested(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("cargo.log")).unwrap_or_default()
    }

    /// `cmd` with this `curl` first on `PATH`.
    fn serving(&self, cmd: Cmd) -> Cmd {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env(
            "PATH",
            &format!("{}:{path}", self.dir.path().join("bin").display()),
        )
    }

    /// Each address that the `curl` served.
    fn served(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("curl.log")).unwrap_or_default()
    }
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The check fails when the downloaded archive does not match the pinned
/// SHA-256, and it runs no test with that archive and keeps no copy of it.
#[test]
fn the_contract_check_fails_on_an_archive_that_does_not_match_the_pinned_hash() {
    if !host_is_pinned() {
        return;
    }
    let release = LocalRelease::new();
    let (platform, pinned) = GOG_SHA256[host_index()];
    assert_ne!(
        release.sha256, pinned,
        "the local archive is not the release"
    );
    let url = format!(
        "https://github.com/openclaw/gogcli/releases/download/v{GOG_VERSION}/gogcli_{GOG_VERSION}_{platform}.tar.gz"
    );

    let tree = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let step = contract_step(tree.path(), target.path());
    let Action::Run(cmds) = &step.action else {
        panic!("the check must run, got {:?}", step.action);
    };
    let (passed, printed) = run(&release.serving(cmds[0].clone()));
    assert!(!passed, "a mismatched archive passed the check: {printed}");
    assert!(
        printed.contains("the gog download failed or does not match its pinned SHA-256"),
        "{printed}"
    );
    assert_eq!(release.tested(), "", "the tests ran with the archive");
    assert_eq!(release.served().trim(), url);
    assert!(
        !target
            .path()
            .join("gog")
            .join(url.rsplit('/').next().unwrap())
            .exists(),
        "the check kept the mismatched archive"
    );
}

/// An archive that matches its pin is unpacked, and the command runs its
/// executable.
#[test]
fn a_tool_command_runs_the_executable_of_an_archive_that_matches_its_pin() {
    let release = LocalRelease::new();
    let pin = Pin {
        tool: "gog",
        url: "https://example.com/releases/gog.tar.gz".into(),
        sha256: release.sha256.clone(),
        member: "./gog".into(),
    };
    let tree = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let cmd = tools::command(
        tree.path(),
        target.path(),
        Ok(pin),
        "\"$work/gog\" --version\n",
        &[],
    );
    let (passed, printed) = run(&release.serving(cmd));
    assert!(passed, "{printed}");
    assert!(printed.contains("gog v99.1.0 (local)"), "{printed}");
    assert_eq!(
        release.served().trim(),
        "https://example.com/releases/gog.tar.gz"
    );
    assert!(target.path().join("gog/gog.tar.gz").exists());
}

/// The pinned `gog` of the host runs from the archive that the check
/// keeps. The first run downloads it into the target directory, and
/// later runs use that copy.
#[test]
fn the_pinned_gog_of_the_host_runs_from_its_checked_archive() {
    let root = workspace_root();
    let command = match gog::command(&root, &target_dir(&root), "\"$work/gog\" --version\n") {
        Ok(command) => command,
        Err(reason) => {
            assert!(!host_is_pinned(), "{reason}");
            eprintln!("{reason}; the gog run did nothing");
            return;
        }
    };
    let output = std::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(
            command
                .cwd
                .as_deref()
                .expect("the command runs in the tree"),
        )
        .output()
        .expect("start the pinned gog");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{printed}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        printed
            .split_whitespace()
            .any(|word| word == format!("v{GOG_VERSION}")),
        "{printed}"
    );
}

/// The value of `ARG <name>=<value>` in `dockerfile`.
fn dockerfile_arg<'a>(dockerfile: &'a str, name: &str) -> Option<&'a str> {
    dockerfile.lines().find_map(|line| {
        line.trim()
            .strip_prefix("ARG ")?
            .strip_prefix(name)?
            .strip_prefix('=')
    })
}

/// The Headless Server image downloads `gog` in a stage of the root
/// `Dockerfile`, with the version and the SHA-256 of each Linux archive in
/// an `ARG`. These are the pins of `pagis-versions`, so the image and the
/// Server Packages bundle the same `gog`, and each architecture checks the
/// archive of its own platform.
#[test]
fn the_server_image_bundles_the_pinned_gog() {
    let dockerfile = std::fs::read_to_string(workspace_root().join("Dockerfile"))
        .expect("read the root Dockerfile");
    assert_eq!(
        dockerfile_arg(&dockerfile, "GOG_VERSION"),
        Some(GOG_VERSION)
    );
    for (arch, platform) in [("amd64", "linux_amd64"), ("arm64", "linux_arm64")] {
        let pinned = GOG_SHA256
            .iter()
            .find(|(name, _)| *name == platform)
            .map(|(_, sha256)| *sha256);
        assert_eq!(
            dockerfile_arg(&dockerfile, &format!("GOG_SHA256_{arch}")),
            pinned,
            "{arch}"
        );
        let selection = format!("{arch}) platform={platform}; sha=\"$GOG_SHA256_{arch}\"");
        assert!(dockerfile.contains(&selection), "no `{selection}`");
    }
}

/// The release bundles `gog` for macOS arm64, Linux amd64 and Linux
/// arm64, and the gate runs on these hosts, so each one has a pin.
#[test]
fn gog_is_pinned_for_every_platform_of_the_release() {
    let platforms: Vec<&str> = GOG_SHA256.iter().map(|(name, _)| *name).collect();
    assert_eq!(platforms, ["darwin_arm64", "linux_amd64", "linux_arm64"]);
    for (platform, sha256) in GOG_SHA256 {
        assert!(
            sha256.len() == 64
                && sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{platform}: {sha256}"
        );
    }
}
