//! Every publish rule, one test per rule.

use std::path::Path;

use pagis_software::{MAX_MANIFEST_BYTES, validate};

const SCHEMA: &str = r#"{"type": "object", "properties": {"city": {"type": "string"}}}"#;

/// A working copy that passes, with the manifest the caller supplies.
fn working_copy(manifest: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("working copy");
    write(root.path(), "pagis-software.toml", manifest);
    write(root.path(), "schemas/forecast.json", SCHEMA);
    write(root.path(), "bin/forecast.py", "#!/usr/bin/env python3\n");
    make_executable(&root.path().join("bin/forecast.py"));
    root
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
    std::fs::write(path, content).expect("write");
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn manifest_with(package: &str, tool: &str) -> String {
    format!(
        r#"
[package]
name = "weather"
description = "Weather for a city"
{package}

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
{tool}
"#
    )
}

fn problems(root: &Path) -> Vec<String> {
    validate(root).expect_err("validation failed").0
}

#[test]
fn a_sound_working_copy_passes_and_carries_its_schemas() {
    let root = working_copy(&manifest_with("", ""));

    let version = validate(root.path()).expect("the package is sound");

    assert_eq!(version.manifest.package.name, "weather");
    assert_eq!(
        version.schemas["forecast"],
        serde_json::from_str::<serde_json::Value>(SCHEMA).unwrap()
    );
}

#[test]
fn every_problem_comes_back_at_once() {
    let root = tempfile::tempdir().expect("working copy");
    write(
        root.path(),
        "pagis-software.toml",
        r#"
[package]
name = "core"
description = ""

[[tool]]
name = "for ecast"
description = "Fine"
entry = "bin/missing"
schema = "schemas/missing.json"
"#,
    );

    let problems = problems(root.path());

    assert!(problems.len() >= 5, "{problems:?}");
    assert!(
        problems.iter().any(|p| p.contains("reserved")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("package description")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("tool name")),
        "{problems:?}"
    );
    assert!(problems.iter().any(|p| p.contains("entry")), "{problems:?}");
    assert!(
        problems.iter().any(|p| p.contains("schema")),
        "{problems:?}"
    );
}

#[test]
fn a_package_name_outside_the_rules_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace("name = \"weather\"", "name = \"weather.app\""),
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("package name")),
    );
}

#[test]
fn an_over_long_description_is_refused() {
    let long = "a".repeat(1025);
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace("Tomorrow's forecast", &long),
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("longer than 1024")),
    );
}

#[test]
fn a_duplicated_tool_name_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    let manifest = format!(
        "{}\n{}",
        manifest_with("", ""),
        r#"
[[tool]]
name = "forecast"
description = "The same name again"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
"#
    );
    write(root.path(), "pagis-software.toml", &manifest);

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("more than once")),
    );
}

#[test]
fn an_entry_without_the_executable_bit_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        root.path().join("bin/forecast.py"),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("not executable")),
    );
}

#[test]
fn an_entry_that_is_not_a_regular_file_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace("bin/forecast.py", "bin"),
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("not a regular file")),
    );
}

#[test]
fn an_entry_that_climbs_out_of_the_root_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace("bin/forecast.py", "../outside"),
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("leaves the package root")),
    );
}

#[test]
fn an_entry_that_is_a_symbolic_link_out_of_the_root_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    std::os::unix::fs::symlink("/bin/sh", root.path().join("bin/escape")).expect("symlink");
    write(
        root.path(),
        "pagis-software.toml",
        &manifest_with("", "").replace("bin/forecast.py", "bin/escape"),
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("leaves the package root")),
    );
}

#[test]
fn a_schema_that_is_not_an_object_type_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(root.path(), "schemas/forecast.json", r#"{"type": "array"}"#);

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("\"type\": \"object\"")),
    );
}

#[test]
fn a_schema_that_is_not_json_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(root.path(), "schemas/forecast.json", "type: object");

    assert!(problems(root.path()).iter().any(|p| p.contains("not JSON")));
}

#[test]
fn a_schema_with_a_ref_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "schemas/forecast.json",
        r#"{"type": "object", "properties": {"city": {"$ref": "other.json"}}}"#,
    );

    assert!(problems(root.path()).iter().any(|p| p.contains("$ref")));
}

#[test]
fn a_schema_that_is_not_a_json_schema_is_refused() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "schemas/forecast.json",
        r#"{"type": "object", "properties": {"city": 3}}"#,
    );

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("valid JSON Schema")),
    );
}

#[test]
fn a_missing_manifest_says_so() {
    let root = tempfile::tempdir().expect("working copy");

    assert!(
        problems(root.path())
            .iter()
            .any(|p| p.contains("pagis-software.toml")),
    );
}

/// The manifest read is the last boundary before host bytes reach the
/// Agent, so it refuses a link even when the tar check did not run.
#[test]
fn a_manifest_that_is_a_symbolic_link_is_refused_unread() {
    let outside = tempfile::tempdir().expect("outside");
    let marker = outside.path().join("marker");
    std::fs::write(&marker, "MARKER-from-the-daemon-host\n").expect("marker");
    let root = working_copy(&manifest_with("", ""));
    let manifest = root.path().join("pagis-software.toml");
    std::fs::remove_file(&manifest).expect("remove the manifest");
    std::os::unix::fs::symlink(&marker, &manifest).expect("symlink");

    let problems = problems(root.path());

    assert!(
        problems.iter().all(|p| !p.contains("MARKER")),
        "{problems:?}"
    );
    assert_eq!(
        problems,
        vec!["pagis-software.toml is not a regular file".to_string()]
    );
}

#[test]
fn a_manifest_over_the_size_limit_is_refused_before_it_parses() {
    let root = working_copy(&manifest_with("", ""));
    write(
        root.path(),
        "pagis-software.toml",
        &"x".repeat(MAX_MANIFEST_BYTES as usize + 1),
    );

    let problems = problems(root.path());

    let previews: Vec<String> = problems
        .iter()
        .map(|p| p.chars().take(300).collect())
        .collect();
    assert_eq!(
        previews,
        vec![format!(
            "pagis-software.toml is larger than the limit of {MAX_MANIFEST_BYTES} bytes"
        )]
    );
}

#[test]
fn a_manifest_at_the_size_limit_is_read() {
    let manifest = manifest_with("", "");
    let padding = MAX_MANIFEST_BYTES as usize - manifest.len() - 1;
    let root = working_copy(&format!("{manifest}#{}", " ".repeat(padding)));

    validate(root.path()).expect("a manifest at the limit is read");
}
