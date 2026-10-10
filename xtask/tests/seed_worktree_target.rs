//! Tests for `scripts/seed-worktree-target.sh`: a git worktree gets its
//! own target directory, cloned from the main checkout, in which Cargo
//! builds every workspace member again from the sources of the worktree.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::support::workspace_root;

/// A main checkout of a one-member workspace, with a linked worktree.
struct Checkouts {
    _dir: tempfile::TempDir,
    main: PathBuf,
    worktree: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn checkouts() -> Checkouts {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    write(
        &main.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/alpha\"]\n",
    );
    write(
        &main.join("crates/alpha/Cargo.toml"),
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(&main.join("crates/alpha/src/lib.rs"), "");
    write(&main.join(".gitignore"), "target/\n");
    git(&main, &["init", "--quiet"]);
    git(&main, &["add", "."]);
    git(&main, &["commit", "--quiet", "-m", "workspace"]);
    git(
        &main,
        &["worktree", "add", "--quiet", "--detach", "../worktree"],
    );

    // The main checkout's build output: a member and a third-party crate,
    // in two profiles.
    let target = main.join("target");
    write(&target.join("debug/.fingerprint/alpha-1111/lib-alpha"), "a");
    write(&target.join("debug/.fingerprint/serde-2222/lib-serde"), "s");
    write(&target.join("debug/deps/libserde-2222.rlib"), "serde");
    write(
        &target.join("release/.fingerprint/alpha-3333/lib-alpha"),
        "a",
    );

    Checkouts {
        worktree: dir.path().join("worktree"),
        main,
        _dir: dir,
    }
}

fn seed(dir: &Path) -> Output {
    let output = Command::new("bash")
        .arg(workspace_root().join("scripts/seed-worktree-target.sh"))
        .current_dir(dir)
        .output()
        .expect("bash runs");
    assert!(
        output.status.success(),
        "the script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn a_new_worktree_gets_the_third_party_output_and_no_member_fingerprint() {
    let checkouts = checkouts();

    seed(&checkouts.worktree);

    let target = checkouts.worktree.join("target");
    assert_eq!(
        std::fs::read_to_string(target.join("debug/deps/libserde-2222.rlib")).unwrap(),
        "serde"
    );
    assert!(target.join("debug/.fingerprint/serde-2222").exists());
    assert!(!target.join("debug/.fingerprint/alpha-1111").exists());
    assert!(!target.join("release/.fingerprint/alpha-3333").exists());
    // The main checkout keeps its own output.
    assert!(
        checkouts
            .main
            .join("target/debug/.fingerprint/alpha-1111")
            .exists()
    );
}

#[test]
fn a_worktree_with_a_target_directory_and_no_build_gets_the_seed() {
    let checkouts = checkouts();
    // Nextest writes its store under target/ before the first build.
    write(
        &checkouts.worktree.join("target/nextest/default/junit.xml"),
        "<testsuites/>",
    );

    seed(&checkouts.worktree);

    let target = checkouts.worktree.join("target");
    assert!(target.join("nextest/default/junit.xml").exists());
    assert!(target.join("debug/deps/libserde-2222.rlib").exists());
    assert!(!target.join("debug/.fingerprint/alpha-1111").exists());
}

#[test]
fn a_worktree_that_cargo_built_keeps_its_target_directory() {
    let checkouts = checkouts();
    write(
        &checkouts
            .worktree
            .join("target/debug/.fingerprint/alpha-4444/lib-alpha"),
        "own",
    );

    seed(&checkouts.worktree);

    let target = checkouts.worktree.join("target");
    assert!(target.join("debug/.fingerprint/alpha-4444").exists());
    assert!(!target.join("debug/deps").exists());
}

#[test]
fn the_main_checkout_keeps_its_target_directory() {
    let checkouts = checkouts();

    seed(&checkouts.main);

    assert!(
        checkouts
            .main
            .join("target/debug/.fingerprint/alpha-1111")
            .exists()
    );
}
