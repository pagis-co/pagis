//! Tests for the secret scan: the gate step that scans the tracked files
//! with gitleaks, the pin of gitleaks, and the allowlist the scan reads.
//!
//! The scan tests run the pinned gitleaks. The first run downloads it
//! into the target directory, and later runs use that copy.

use std::path::Path;
use std::process::Command;

use pagis_versions::{GITLEAKS_SHA256, GITLEAKS_VERSION};
use xtask::secrets::{CONFIG, image_secret_scan_step, tree_secret_scan_step};
use xtask::{Action, Step, target_dir};

use crate::support::workspace_root;

/// `len` letters and digits in an order with a high entropy.
fn varied(len: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    (0..len)
        .map(|i| CHARS[(i * 17 + 5) % CHARS.len()] as char)
        .collect()
}

/// An Anthropic key shape. The test makes it at run time, so no key
/// shape is in the tree.
fn fake_provider_key() -> String {
    format!("sk-ant-api03-{}AA", varied(93))
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn write(dir: &Path, path: &str, body: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A Git working tree that tracks the repository's `.gitleaks.toml` and
/// `files`.
fn tracked_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tree = tempfile::tempdir().unwrap();
    git(tree.path(), &["init", "--quiet"]);
    std::fs::copy(workspace_root().join(CONFIG), tree.path().join(CONFIG)).unwrap();
    for (path, body) in files {
        write(tree.path(), path, body);
    }
    git(tree.path(), &["add", "--all"]);
    tree
}

/// Run the scan step on `tree`, as the gate does. The result is whether
/// the step passed, and what it printed.
fn scan(tree: &Path) -> (bool, String) {
    run(tree_secret_scan_step(tree, &target_dir(&workspace_root())))
}

/// Run the scan step of an image publish on the export `export` under
/// `root`, as the release does.
fn image_scan(root: &Path, export: &str) -> (bool, String) {
    run(image_secret_scan_step(
        root,
        "image-secret-scan",
        export,
        &target_dir(&workspace_root()),
    ))
}

/// A repository root with the stored `.gitleaks.toml`, and the exported
/// filesystem of one platform of an image under `export`, holding
/// `files`.
fn exported_image(export: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::copy(workspace_root().join(CONFIG), root.path().join(CONFIG)).unwrap();
    for (path, body) in files {
        write(root.path(), &format!("{export}/linux_arm64/{path}"), body);
    }
    root
}

/// The third-party content of the Computer Image that the default rules
/// flag, with fake values of the same shapes that the test makes at run
/// time: the Google keys of the Debian chromium package, a uBlock Origin
/// Lite filter list and a Node header.
fn computer_image_third_party_files() -> Vec<(&'static str, String)> {
    vec![
        (
            "etc/chromium.d/apikeys",
            format!(
                "export GOOGLE_API_KEY=\"AIza{}\"\nexport GOOGLE_DEFAULT_CLIENT_SECRET=\"{}\"\n",
                varied(35),
                varied(24)
            ),
        ),
        (
            "opt/pagis/ublock-origin-lite/rulesets/main/filters.json",
            format!("{{\"api_key\": \"{}\"}}\n", varied(32)),
        ),
        (
            "usr/local/include/node/v8-internal.h",
            format!("const char* api_key = \"{}\";\n", varied(32)),
        ),
    ]
}

/// Run the commands of `step`. The result is whether the step passed,
/// and what it printed.
fn run(step: Step) -> (bool, String) {
    let Action::Run(cmds) = step.action else {
        panic!("the scan must run, got {:?}", step.action);
    };
    let mut printed = String::new();
    for cmd in cmds {
        let output = Command::new(&cmd.program)
            .args(&cmd.args)
            .envs(cmd.env.iter().map(|(key, value)| (key, value)))
            .current_dir(cmd.cwd.as_deref().expect("the scan runs in the tree"))
            .output()
            .expect("start the scan");
        printed.push_str(&String::from_utf8_lossy(&output.stdout));
        printed.push_str(&String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            return (false, printed);
        }
    }
    (true, printed)
}

fn script(step: &Step) -> String {
    let Action::Run(cmds) = &step.action else {
        panic!("step {} must run, got {:?}", step.name, step.action);
    };
    cmds.iter()
        .map(|cmd| format!("{} {}", cmd.program, cmd.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

// --- the scan of the tracked tree ---

#[test]
fn a_provider_key_in_a_tracked_file_fails_the_scan() {
    let key = fake_provider_key();
    let tree = tracked_tree(&[(
        "crates/app/src/lib.rs",
        &format!("const KEY: &str = \"{key}\";\n"),
    )]);

    let (passed, printed) = scan(tree.path());

    assert!(!passed, "{printed}");
    assert!(printed.contains("anthropic-api-key"), "{printed}");
    assert!(printed.contains("crates/app/src/lib.rs"), "{printed}");
    assert!(
        !printed.contains(&key),
        "the scan prints no secret: {printed}"
    );
}

#[test]
fn a_tree_with_only_the_allowlisted_example_values_passes_the_scan() {
    let example = std::fs::read_to_string(workspace_root().join("deploy/.env.example"))
        .expect("read deploy/.env.example");
    let tree = tracked_tree(&[
        ("deploy/.env.example", &example),
        (
            "numbers.txt",
            "+14155550100\n+1 415 555 0123\n415-555-0199\n4155550150\n",
        ),
    ]);

    let (passed, printed) = scan(tree.path());

    assert!(passed, "{printed}");
}

/// The gate scans the tracked files as the working tree holds them, so
/// a change shows before it is staged or committed.
#[test]
fn a_change_to_a_tracked_file_is_scanned_before_it_is_staged() {
    let tree = tracked_tree(&[("notes.txt", "nothing here\n")]);
    write(
        tree.path(),
        "notes.txt",
        &format!("key {}\n", fake_provider_key()),
    );

    let (passed, printed) = scan(tree.path());

    assert!(!passed, "{printed}");
    assert!(printed.contains("notes.txt"), "{printed}");
}

/// An untracked or ignored file, such as a local `.env`, never enters
/// the published tree, so the scan does not read it. A tracked file
/// that the working tree no longer has does not stop the scan.
#[test]
fn the_scan_reads_the_tracked_files_only() {
    let tree = tracked_tree(&[(".gitignore", ".env\n"), ("gone.txt", "old\n")]);
    write(
        tree.path(),
        ".env",
        &format!("ANTHROPIC_API_KEY={}\n", fake_provider_key()),
    );
    write(
        tree.path(),
        "scratch.txt",
        &format!("{}\n", fake_provider_key()),
    );
    std::fs::remove_file(tree.path().join("gone.txt")).unwrap();

    let (passed, printed) = scan(tree.path());

    assert!(passed, "{printed}");
}

/// Each exception is in `.gitleaks.toml` with its reason, so an inline
/// `gitleaks:allow` comment does not hide a finding.
#[test]
fn a_gitleaks_allow_comment_does_not_hide_a_finding() {
    let tree = tracked_tree(&[(
        "lib.rs",
        &format!(
            "const KEY: &str = \"{}\"; // gitleaks:allow\n",
            fake_provider_key()
        ),
    )]);

    let (passed, printed) = scan(tree.path());

    assert!(!passed, "{printed}");
}

/// A `.gitleaksignore` file hides findings by fingerprint and holds no
/// reason, so the scan refuses a tree that has one.
#[test]
fn a_gitleaksignore_file_fails_the_scan() {
    let tree = tracked_tree(&[(".gitleaksignore", "lib.rs:anthropic-api-key:1\n")]);

    let (passed, printed) = scan(tree.path());

    assert!(!passed, "{printed}");
    assert!(printed.contains(".gitleaksignore"), "{printed}");
}

// --- the scan of an image ---

/// The Computer Image carries third-party files that the default rules
/// flag and that hold no Pagis secret. The allowlist names each one by
/// its path in the export of the Computer Image and by the rules that
/// flag it.
#[test]
fn the_third_party_content_of_the_computer_image_passes_the_scan() {
    let files = computer_image_third_party_files();
    let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let root = exported_image("dist/image-fs/computer", &files);

    let (passed, printed) = image_scan(root.path(), "dist/image-fs/computer");

    assert!(passed, "{printed}");
}

/// The entries for the third-party files of the Computer Image allow only
/// the rules that flag them, so a provider key in one of those files is
/// still a finding.
#[test]
fn a_provider_key_in_an_allowlisted_image_file_fails_the_scan() {
    let key = format!("ANTHROPIC_API_KEY={}\n", fake_provider_key());
    let files: Vec<(&str, &str)> = computer_image_third_party_files()
        .into_iter()
        .map(|(path, _)| (path, key.as_str()))
        .collect();
    let root = exported_image("dist/image-fs/computer", &files);

    let (passed, printed) = image_scan(root.path(), "dist/image-fs/computer");

    assert!(!passed, "{printed}");
    for (path, _) in files {
        assert!(printed.contains(path), "{path} is not a finding: {printed}");
    }
}

/// The entries name paths in the export of the Computer Image, so the
/// same content elsewhere, in the tracked tree or in the export of the
/// Headless Server image, is a finding.
#[test]
fn the_computer_image_entries_apply_to_its_export_only() {
    let files = computer_image_third_party_files();
    let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();

    let server = exported_image("dist/image-fs/server", &files);
    let (passed, printed) = image_scan(server.path(), "dist/image-fs/server");
    assert!(!passed, "{printed}");

    let tree = tracked_tree(&files);
    let (passed, printed) = scan(tree.path());
    assert!(!passed, "{printed}");
}

/// The Headless Server image carries the Perl that the PostgreSQL client
/// scripts run, and the C headers of its library are code that the
/// generic rule flags. The entry allows that rule there, so a provider
/// key in a header is still a finding.
#[test]
fn the_perl_headers_of_the_server_image_pass_the_scan_for_the_generic_rule_only() {
    for arch in ["aarch64", "x86_64"] {
        let header = format!("usr/lib/{arch}-linux-gnu/perl/5.40.1/CORE/cop.h");
        let code = format!("=for apidoc Amnh||{}\n", varied(32));
        let root = exported_image("dist/image-fs/server", &[(&header, &code)]);
        let (passed, printed) = image_scan(root.path(), "dist/image-fs/server");
        assert!(passed, "{printed}");

        let key = format!("ANTHROPIC_API_KEY={}\n", fake_provider_key());
        let root = exported_image("dist/image-fs/server", &[(&header, &key)]);
        let (passed, printed) = image_scan(root.path(), "dist/image-fs/server");
        assert!(!passed, "{printed}");
    }
}

// --- the pin ---

#[test]
fn the_scan_runs_the_pinned_gitleaks_and_checks_its_hash() {
    let tree = tempfile::tempdir().unwrap();
    let step = tree_secret_scan_step(tree.path(), Path::new("/shared/pagis-target"));
    let script = script(&step);

    assert!(
        script.contains(&format!(
            "https://github.com/gitleaks/gitleaks/releases/download/v{GITLEAKS_VERSION}/gitleaks_{GITLEAKS_VERSION}_"
        )),
        "{script}"
    );
    assert!(
        GITLEAKS_SHA256
            .iter()
            .any(|(_, sha256)| script.contains(sha256)),
        "{script}"
    );
    assert!(script.contains("shasum -a 256 -c"), "{script}");
    assert!(
        script.contains("/shared/pagis-target/gitleaks"),
        "the download is kept in the shared target directory: {script}"
    );
}

/// The gate runs on macOS arm64 and on Linux amd64, and the release on
/// macOS arm64. The pin covers the platforms the release builds for.
#[test]
fn gitleaks_is_pinned_for_every_host_of_the_gate_and_the_release() {
    let platforms: Vec<&str> = GITLEAKS_SHA256.iter().map(|(name, _)| *name).collect();
    assert_eq!(platforms, ["darwin_arm64", "linux_x64", "linux_arm64"]);
    for (platform, sha256) in GITLEAKS_SHA256 {
        assert!(
            sha256.len() == 64
                && sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{platform}: {sha256}"
        );
    }
}

// --- the allowlist ---

/// Every allowlist table in `value`, at any depth: the global
/// `[[allowlists]]` and the `[[rules.allowlists]]` of a rule.
fn allowlists(value: &toml::Value) -> Vec<&toml::Table> {
    match value {
        toml::Value::Table(table) => table
            .iter()
            .flat_map(|(key, value)| {
                let mut found = Vec::new();
                if key == "allowlist" || key == "allowlists" {
                    match value {
                        toml::Value::Table(entry) => found.push(entry),
                        toml::Value::Array(entries) => {
                            found.extend(entries.iter().filter_map(toml::Value::as_table))
                        }
                        _ => {}
                    }
                }
                found.extend(allowlists(value));
                found
            })
            .collect(),
        toml::Value::Array(items) => items.iter().flat_map(allowlists).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn every_allowlist_entry_holds_a_reason() {
    let config: toml::Value = std::fs::read_to_string(workspace_root().join(CONFIG))
        .expect("read .gitleaks.toml")
        .parse::<toml::Table>()
        .expect(".gitleaks.toml is TOML")
        .into();

    let entries = allowlists(&config);

    assert!(!entries.is_empty(), ".gitleaks.toml holds no allowlist");
    for entry in entries {
        let reason = entry
            .get("description")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();
        assert!(
            !reason.trim().is_empty(),
            "an allowlist entry has no reason: {entry:?}"
        );
    }
}
