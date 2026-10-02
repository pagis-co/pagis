//! Tests for the gate step that runs the tests of screend
//! (`computer/screend`) in the builder image of the Computer Image.

use std::path::Path;

use xtask::screend::{builder_image, test_step};
use xtask::{Action, Step, TEST_LABEL};

use crate::support::workspace_root;

const BASE: &str =
    "debian:trixie-slim@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BUILDER: &str =
    "rust:1-trixie@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A Dockerfile with the stages of the Computer Image: screend builds in
/// its own stage, between others.
fn dockerfile() -> String {
    format!(
        "FROM {BASE} AS compositor\n\
         RUN true\n\
         \n\
         # The builder of screend.\n\
         FROM {BUILDER} AS screend\n\
         RUN cargo install --locked cargo-auditable\n\
         \n\
         FROM --platform=$BUILDPLATFORM {BASE} AS blocker\n\
         FROM {BASE}\n\
         COPY --from=screend /src/screend/target/release/pagis-screend /usr/local/bin/\n"
    )
}

/// A repository with `dockerfile` as `computer/Dockerfile`.
fn repository(dockerfile: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("computer/screend")).unwrap();
    std::fs::write(dir.path().join("computer/Dockerfile"), dockerfile).unwrap();
    dir
}

// --- the builder image ---

#[test]
fn the_builder_image_is_the_image_of_the_screend_stage() {
    assert_eq!(builder_image(&dockerfile()), Some(BUILDER));
    // Stage names are case-insensitive, and a flag can come before the
    // image.
    assert_eq!(
        builder_image("from --platform=$BUILDPLATFORM rust:1@sha256:c as ScreenD\n"),
        Some("rust:1@sha256:c")
    );
    assert_eq!(
        builder_image(&format!("FROM {BUILDER} AS compositor\n")),
        None
    );
    assert_eq!(builder_image(&format!("FROM {BUILDER}\n")), None);
    assert_eq!(builder_image(""), None);
}

/// The step takes the image that builds screend in the Computer Image, so
/// one pin moves both.
#[test]
fn the_computer_image_builds_screend_in_a_pinned_rust_image() {
    let dockerfile = std::fs::read_to_string(workspace_root().join("computer/Dockerfile"))
        .expect("read computer/Dockerfile");
    let image = builder_image(&dockerfile).expect("computer/Dockerfile has a screend stage");
    assert!(image.starts_with("rust:"), "{image}");
    assert!(image.contains("@sha256:"), "{image}");
}

// --- the step ---

#[test]
fn without_docker_the_screend_tests_skip() {
    let dir = repository(&dockerfile());
    let step = test_step(dir.path(), false);
    assert_eq!(step.name, "screend-test");
    assert_eq!(step.action, Action::Skip("Docker unavailable".into()));
}

/// A Dockerfile that the step cannot read, or that has no screend stage,
/// fails the step: the tests must not pass because they did not run.
#[cfg(unix)]
#[test]
fn the_step_fails_without_the_screend_stage() {
    let dir = repository("FROM debian AS compositor\n");
    let (passed, output) = run_with_fake_docker(&test_step(dir.path(), true), dir.path(), "");
    assert!(!passed);
    assert!(
        output.contains("computer/Dockerfile has no `screend` stage"),
        "{output}"
    );

    let dir = tempfile::tempdir().unwrap();
    let (passed, output) = run_with_fake_docker(&test_step(dir.path(), true), dir.path(), "");
    assert!(!passed);
    assert!(
        output.contains("cannot read computer/Dockerfile"),
        "{output}"
    );
    assert!(calls(dir.path()).is_empty(), "the step ran docker");
}

/// The fake `docker` of a run in which the targets serve.
const SERVING: &str = "case \"$1\" in\n\
     logs) echo 'the proxy test targets serve' ;;\n\
     inspect) echo true ;;\n\
     esac\n";

/// The unit tests run first. The ignored tests of the Exit Proxy then
/// run on a network of the step's own, against the targets in a second
/// container, and the step removes both when it ends. Each container
/// keeps the Cargo registry and the target directory in the same two
/// named volumes.
#[cfg(unix)]
#[test]
fn the_unit_tests_run_then_the_proxy_tests_against_a_second_container() {
    let dir = repository(&dockerfile());
    let (passed, output) = run_with_fake_docker(&test_step(dir.path(), true), dir.path(), SERVING);
    assert!(passed, "{output}");

    let calls = calls(dir.path());
    let network = calls[1].rsplit(' ').next().unwrap().to_string();
    let owner = network
        .strip_prefix("pagis-screend-test-")
        .unwrap_or_else(|| panic!("{calls:?}"));
    // The sweep of the gate reads the process id of the owner from the
    // label, and removes the objects of a step that was killed.
    owner.parse::<u32>().expect("the owner is a process id");
    let targets = format!("pagis-screend-targets-{owner}");
    let label = format!("{TEST_LABEL}={owner}-screend");
    let builder = |rest: String| {
        format!(
            "run --volume {}:/src/screend:ro \
             --volume pagis-screend-registry:/usr/local/cargo/registry \
             --volume pagis-screend-target:/target \
             --env CARGO_TARGET_DIR=/target --workdir /src/screend {rest}",
            dir.path().join("computer/screend").display()
        )
    };
    assert_eq!(
        calls,
        [
            builder(format!("--rm {BUILDER} cargo test --locked")),
            format!("network create --label {label} {network}"),
            builder(format!(
                "--detach --name {targets} --network {network} --label {label} \
                 {BUILDER} cargo test --locked serve_the_proxy_test_targets -- --ignored --nocapture"
            )),
            format!("logs {targets}"),
            builder(format!(
                "--rm --network {network} --env PAGIS_EXIT_TEST_TARGETS={targets} \
                 {BUILDER} cargo test --locked -- --ignored --skip serve_the_proxy_test_targets"
            )),
            format!("rm -f {targets}"),
            format!("network rm {network}"),
        ]
    );
}

/// A failed test fails the step, and the step still removes the targets
/// and the network.
#[cfg(unix)]
#[test]
fn a_failed_test_fails_the_step_and_removes_the_targets_and_the_network() {
    for failing in ["*' cargo test --locked'", "*PAGIS_EXIT_TEST_TARGETS*"] {
        let dir = repository(&dockerfile());
        let fake = format!("{SERVING}case \"$*\" in {failing}) exit 101 ;; esac\n");
        let (passed, output) =
            run_with_fake_docker(&test_step(dir.path(), true), dir.path(), &fake);
        assert!(!passed, "{failing}: {output}");
        assert_cleaned_up(&calls(dir.path()));
    }
}

/// A step that is stopped, as CI stops a cancelled job, fails, and still
/// removes the targets and the network.
#[cfg(unix)]
#[test]
fn a_stopped_step_removes_the_targets_and_the_network() {
    for signal in ["INT", "TERM"] {
        let dir = repository(&dockerfile());
        // The parent of the fake is the shell of the step.
        let fake = format!(
            "{SERVING}case \"$*\" in *PAGIS_EXIT_TEST_TARGETS*) kill -{signal} $PPID ;; esac\n"
        );
        let (passed, output) =
            run_with_fake_docker(&test_step(dir.path(), true), dir.path(), &fake);
        assert!(!passed, "{signal}: {output}");
        assert_cleaned_up(&calls(dir.path()));
    }
}

/// Targets that stop before they serve, such as targets that do not
/// compile, fail the step with their output, and no proxy test runs.
#[cfg(unix)]
#[test]
fn targets_that_stop_before_they_serve_fail_the_step() {
    let dir = repository(&dockerfile());
    let fake = "case \"$1\" in\n\
         logs) echo 'error[E0425]: cannot find value' ;;\n\
         inspect) echo false ;;\n\
         esac\n";
    let (passed, output) = run_with_fake_docker(&test_step(dir.path(), true), dir.path(), fake);
    assert!(!passed);
    assert!(output.contains("error[E0425]"), "{output}");
    assert!(output.contains("do not serve"), "{output}");
    let calls = calls(dir.path());
    assert!(
        !calls
            .iter()
            .any(|call| call.contains("PAGIS_EXIT_TEST_TARGETS")),
        "{calls:?}"
    );
    assert_cleaned_up(&calls);
}

/// The last two calls remove the targets container, then its network:
/// Docker removes no network while a container is attached to it.
fn assert_cleaned_up(calls: &[String]) {
    let [.., remove_targets, remove_network] = calls else {
        panic!("{calls:?}");
    };
    assert!(
        remove_targets.starts_with("rm -f pagis-screend-targets-"),
        "{calls:?}"
    );
    assert!(
        remove_network.starts_with("network rm pagis-screend-test-"),
        "{calls:?}"
    );
}

/// Run `step` with a fake `docker` first on `PATH`. The fake logs each
/// call to `docker.log` in `dir`, then runs `body` with the arguments of
/// the call. Returns whether the step passed, and what it printed.
#[cfg(unix)]
fn run_with_fake_docker(step: &Step, dir: &Path, body: &str) -> (bool, String) {
    use std::os::unix::fs::PermissionsExt;

    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let docker = bin.join("docker");
    std::fs::write(
        &docker,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\n{body}",
            dir.join("docker.log").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

    let Action::Run(cmds) = &step.action else {
        panic!("step {} must run, got {:?}", step.name, step.action);
    };
    let mut printed = String::new();
    for cmd in cmds {
        let output = std::process::Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(cmd.cwd.as_ref().unwrap())
            .env("PATH", &path)
            .output()
            .unwrap();
        printed.push_str(&String::from_utf8_lossy(&output.stdout));
        printed.push_str(&String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            return (false, printed);
        }
    }
    (true, printed)
}

/// Each call that the fake `docker` got, in order.
fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("docker.log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}
