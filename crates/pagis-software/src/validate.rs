//! Publish validation. One pass over a working copy
//! collects every problem, because an author must see all of them at
//! once and not one per attempt.
//!
//! The size limits of a package are not checked here: the pack applies
//! them before any file of the working copy reaches the disk.
//!
//! Ownership of a package name by another agent is not checked here:
//! only the store knows who published what.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pagis_broker::{MAX_NAME, is_native_namespace, is_provider_safe_name};

use crate::manifest::{MANIFEST_FILE, Manifest, PackageVersion};
use crate::widget::{MAX_WIDGET_HTML_BYTES, is_csp_origin};

/// The longest description of a package or of a tool.
const MAX_DESCRIPTION: usize = 1024;
/// The largest manifest a publish reads.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Every problem one working copy has. The publish tool prints the
/// list as one `is_error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.join("\n"))]
pub struct ValidationErrors(pub Vec<String>);

/// Read and check the working copy at `root`. The package that
/// comes back is the one the store publishes and the runner runs.
pub fn validate(root: &Path) -> Result<PackageVersion, ValidationErrors> {
    let text = match read_manifest(root) {
        Ok(text) => text,
        Err(problem) => return Err(ValidationErrors(vec![problem])),
    };
    let manifest = match Manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(error) => return Err(ValidationErrors(vec![error])),
    };

    let mut problems = Vec::new();
    check_package(&manifest, &mut problems);
    let widget_schemas = check_widgets(root, &manifest, &mut problems);
    let schemas = check_tools(root, &manifest, &mut problems);

    if problems.is_empty() {
        Ok(PackageVersion {
            manifest,
            schemas,
            widget_schemas,
        })
    } else {
        Err(ValidationErrors(problems))
    }
}

/// The text of the manifest. The parser error goes back to the Agent
/// with a line of this text, so the read accepts only a regular file
/// at the package root: it does not follow a link, and it stops after
/// `MAX_MANIFEST_BYTES`.
fn read_manifest(root: &Path) -> Result<String, String> {
    use std::io::Read;

    let path = root.join(MANIFEST_FILE);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot read {MANIFEST_FILE}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{MANIFEST_FILE} is not a regular file"));
    }
    let file = std::fs::File::open(&path)
        .map_err(|error| format!("cannot read {MANIFEST_FILE}: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {MANIFEST_FILE}: {error}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST_FILE} is larger than the limit of {MAX_MANIFEST_BYTES} bytes"
        ));
    }
    String::from_utf8(bytes).map_err(|error| format!("{MANIFEST_FILE} is not UTF-8: {error}"))
}

/// Every Widget names an HTML page and a data schema that are both in
/// the tree, and declares only origins the daemon can put in a CSP
/// (ADR-0016).
fn check_widgets(
    root: &Path,
    manifest: &Manifest,
    problems: &mut Vec<String>,
) -> BTreeMap<String, serde_json::Value> {
    let mut schemas = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for widget in &manifest.widgets {
        let name = &widget.name;
        if !is_provider_safe_name(name) {
            problems.push(format!(
                "widget name {name:?} is not 1 to {MAX_NAME} ASCII letters, digits, underscores or hyphens"
            ));
        }
        if !seen.insert(name.clone()) {
            problems.push(format!("widget {name:?} is declared more than once"));
        }
        if let Err(problem) = check_html(root, &widget.html) {
            problems.push(format!("widget {name:?} html {problem}"));
        }
        match read_schema(root, &widget.schema) {
            Ok(schema) => {
                schemas.insert(name.clone(), schema);
            }
            Err(problem) => problems.push(format!("widget {name:?} schema {problem}")),
        }
        for origin in widget.csp.connect.iter().chain(widget.csp.resource.iter()) {
            if !is_csp_origin(origin) {
                problems.push(format!(
                    "widget {name:?} declares {origin:?}, which is not an https origin"
                ));
            }
        }
    }
    schemas
}

/// The page must be a regular file inside the package root, and small
/// enough to serve in one response.
fn check_html(root: &Path, html: &str) -> Result<(), String> {
    let path = resolve_inside(root, html)?;
    let metadata = std::fs::metadata(&path).map_err(|error| format!("{html:?}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{html:?} is not a regular file"));
    }
    if metadata.len() > MAX_WIDGET_HTML_BYTES {
        return Err(format!(
            "{html:?} is {} bytes; the limit is {MAX_WIDGET_HTML_BYTES}",
            metadata.len()
        ));
    }
    Ok(())
}

fn check_package(manifest: &Manifest, problems: &mut Vec<String>) {
    let name = &manifest.package.name;
    if !is_provider_safe_name(name) {
        problems.push(format!(
            "package name {name:?} is not 1 to {MAX_NAME} ASCII letters, digits, underscores or hyphens"
        ));
    }
    if is_native_namespace(name) {
        problems.push(format!("package name {name:?} is reserved"));
    }
    if let Some(problem) = description_problem(&manifest.package.description) {
        problems.push(format!("package description {problem}"));
    }
    if manifest.tools.is_empty() {
        problems.push("the package declares no tool".to_string());
    }
}

fn check_tools(
    root: &Path,
    manifest: &Manifest,
    problems: &mut Vec<String>,
) -> BTreeMap<String, serde_json::Value> {
    let mut schemas = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for tool in &manifest.tools {
        let name = &tool.name;
        if !is_provider_safe_name(name) {
            problems.push(format!(
                "tool name {name:?} is not 1 to {MAX_NAME} ASCII letters, digits, underscores or hyphens"
            ));
        }
        if !seen.insert(name.clone()) {
            problems.push(format!("tool {name:?} is declared more than once"));
        }
        if let Some(problem) = description_problem(&tool.description) {
            problems.push(format!("tool {name:?} description {problem}"));
        }
        if let Err(problem) = check_entry(root, &tool.entry) {
            problems.push(format!("tool {name:?} entry {problem}"));
        }
        match read_schema(root, &tool.schema) {
            Ok(schema) => {
                schemas.insert(name.clone(), schema);
            }
            Err(problem) => problems.push(format!("tool {name:?} schema {problem}")),
        }
        if tool.visibility.is_empty() {
            problems.push(format!(
                "tool {name:?} visibility is empty; name \"model\", \"app\", or both"
            ));
        }
        match &tool.widget {
            Some(widget) if manifest.widget(widget).is_none() => problems.push(format!(
                "tool {name:?} renders the widget {widget:?}, which the package does not declare"
            )),
            Some(_) => {}
            None if tool.awaits_input => problems.push(format!(
                "tool {name:?} sets awaits_input but renders no widget; only a widget can ask"
            )),
            None => {}
        }
    }
    schemas
}

/// The entry must be a regular executable file inside the package
/// root.
fn check_entry(root: &Path, entry: &str) -> Result<(), String> {
    let path = resolve_inside(root, entry)?;
    let metadata = std::fs::metadata(&path).map_err(|error| format!("{entry:?}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{entry:?} is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{entry:?} is not executable"));
        }
    }
    Ok(())
}

fn read_schema(root: &Path, schema: &str) -> Result<serde_json::Value, String> {
    let path = resolve_inside(root, schema)?;
    let text = std::fs::read_to_string(&path).map_err(|error| format!("{schema:?}: {error}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("{schema:?} is not JSON: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err(format!("{schema:?} is not a JSON object"));
    };
    if object.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(format!("{schema:?} does not declare \"type\": \"object\""));
    }
    if has_ref(&value) {
        return Err(format!("{schema:?} uses $ref, which is not supported"));
    }
    jsonschema::validator_for(&value)
        .map_err(|error| format!("{schema:?} is not a valid JSON Schema: {error}"))?;
    Ok(value)
}

/// `$ref` anywhere would send the schema compiler outside the package.
fn has_ref(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.contains_key("$ref") || map.values().any(has_ref),
        serde_json::Value::Array(items) => items.iter().any(has_ref),
        _ => false,
    }
}

/// The path a manifest names, made absolute inside the root. A path
/// that leaves the root, by `..` or by a symbolic link, is refused.
fn resolve_inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(format!("{relative:?} is not a relative path"));
    }
    if path
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!("{relative:?} leaves the package root"));
    }
    let full = root.join(path);
    let real = full
        .canonicalize()
        .map_err(|error| format!("{relative:?}: {error}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|error| format!("cannot read the package root: {error}"))?;
    if !real.starts_with(&real_root) {
        return Err(format!("{relative:?} leaves the package root"));
    }
    Ok(real)
}

fn description_problem(description: &str) -> Option<String> {
    let length = description.trim().chars().count();
    if length == 0 {
        return Some("must not be empty".to_string());
    }
    if length > MAX_DESCRIPTION {
        return Some(format!("is longer than {MAX_DESCRIPTION} characters"));
    }
    None
}
