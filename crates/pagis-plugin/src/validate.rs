//! Install validation (ADR-0017). One pass over a
//! plugin directory collects every problem, because the user must see
//! all of them at once and not one per attempt.
//!
//! Two layers apply. The agent-plugins.org 1.0.0 schemas judge the
//! shape of `plugin.json` and `mcp.json`. The rules Pagis adds judge
//! what the daemon must be able to run safely: the name is a tool
//! namespace, a `command` is one token, a `cwd` stays inside the
//! plugin, and a credential reference reaches only an `env` value or a
//! header.

use std::path::{Component, Path};

use pagis_broker::{MAX_NAME, is_native_namespace, is_provider_safe_name};
use url::Url;

use crate::manifest::{
    ConfigField, FieldKind, MCP_FILE, MCP_SCHEMA, McpConfig, McpServer, PAGIS_EXTENSION,
    PLUGIN_MANIFEST_FILE, PLUGIN_SCHEMA, PagisExtension, PluginManifest, PluginPackage,
    config_references,
};

/// The largest plugin package an install accepts. It matches the
/// Software Package limit: both are trees the daemon stores as git
/// objects (ADR-0016).
pub const MAX_PACKAGE_BYTES: u64 = 50 * 1024 * 1024;

/// The placeholders the specification defines. They are the plugin's
/// own paths and never a bound value.
const PATH_PLACEHOLDERS: [&str; 2] = ["${PLUGIN_ROOT}", "${PLUGIN_DATA}"];

/// Every problem one plugin package has. The install reports the list
/// as one refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.join("\n"))]
pub struct ValidationErrors(pub Vec<String>);

/// Read and check the plugin directory at `root`. The package that
/// comes back is the one the install commits.
pub fn validate(root: &Path) -> Result<PluginPackage, ValidationErrors> {
    let mut problems = Vec::new();

    let manifest = match read_manifest(root, &mut problems) {
        Some(manifest) => manifest,
        None => return Err(ValidationErrors(problems)),
    };
    check_name(&manifest.name, &mut problems);
    let pagis = read_extension(&manifest, &mut problems);
    let mcp = read_mcp(root, &mut problems);
    if let Some(mcp) = &mcp {
        check_servers(root, mcp, &pagis, &mut problems);
    }
    check_tool_names(&pagis, &mut problems);
    check_size(root, &mut problems);
    let read = crate::skills::read_skills(&manifest.name, root);
    problems.extend(read.problems);

    if problems.is_empty() {
        Ok(PluginPackage {
            manifest,
            mcp,
            pagis,
            skills: read.skills,
        })
    } else {
        Err(ValidationErrors(problems))
    }
}

/// Read `plugin.json` and judge it against the 1.0.0 schema. A
/// manifest that does not parse stops the pass: nothing below it can
/// be judged.
fn read_manifest(root: &Path, problems: &mut Vec<String>) -> Option<PluginManifest> {
    let text = match std::fs::read_to_string(root.join(PLUGIN_MANIFEST_FILE)) {
        Ok(text) => text,
        Err(error) => {
            problems.push(format!("cannot read {PLUGIN_MANIFEST_FILE}: {error}"));
            return None;
        }
    };
    let document: serde_json::Value = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(error) => {
            problems.push(format!("{PLUGIN_MANIFEST_FILE} is not JSON: {error}"));
            return None;
        }
    };
    if let Err(refusals) = check_schema(&PLUGIN_VALIDATOR, &document, PLUGIN_MANIFEST_FILE) {
        problems.extend(refusals);
        return None;
    }
    match serde_json::from_value::<PluginManifest>(document) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            problems.push(format!("{PLUGIN_MANIFEST_FILE} cannot be read: {error}"));
            None
        }
    }
}

/// The name is the tool namespace, so it takes the Pagis rules on top
/// of the specification's (ADR-0005, ADR-0017). The specification
/// allows a period; a qualified tool name may not carry one.
fn check_name(name: &str, problems: &mut Vec<String>) {
    if !is_provider_safe_name(name) {
        problems.push(format!(
            "plugin name {name:?} is not 1 to {MAX_NAME} ASCII letters, digits, underscores or \
             hyphens"
        ));
    }
    if is_native_namespace(name) || name == pagis_core::FIRST_PARTY_SKILLS {
        problems.push(format!("plugin name {name:?} is reserved"));
    }
}

fn read_extension(manifest: &PluginManifest, problems: &mut Vec<String>) -> PagisExtension {
    let Some(entry) = manifest.extensions.get(PAGIS_EXTENSION) else {
        return PagisExtension::default();
    };
    let extension: PagisExtension = match serde_json::from_value(entry.clone()) {
        Ok(extension) => extension,
        Err(error) => {
            problems.push(format!(
                "the {PAGIS_EXTENSION} extension cannot be read: {error}"
            ));
            return PagisExtension::default();
        }
    };
    for (name, field) in &extension.config {
        check_field(name, field, problems);
    }
    extension
}

fn check_field(name: &str, field: &ConfigField, problems: &mut Vec<String>) {
    if field.title.trim().is_empty() {
        problems.push(format!("config field {name:?} has no title"));
    }
    match field.kind {
        FieldKind::Connection => {
            if field
                .provider
                .as_ref()
                .is_none_or(|provider| provider.trim().is_empty())
            {
                problems.push(format!("connection field {name:?} names no provider"));
            }
        }
        _ => {
            if field.provider.is_some() {
                problems.push(format!(
                    "field {name:?} is a {} field and cannot name a provider",
                    field.kind.as_str()
                ));
            }
            if !field.capabilities.is_empty() {
                problems.push(format!(
                    "field {name:?} is a {} field and cannot name capabilities",
                    field.kind.as_str()
                ));
            }
        }
    }
}

/// A declared effect belongs to a tool the servers offer, so its name
/// must at least be one a qualified tool name can carry (ADR-0005).
fn check_tool_names(pagis: &PagisExtension, problems: &mut Vec<String>) {
    for name in pagis.tools.keys() {
        if !is_provider_safe_name(name) {
            problems.push(format!(
                "tool name {name:?} is not 1 to {MAX_NAME} ASCII letters, digits, underscores or \
                 hyphens"
            ));
        }
    }
}

/// Read `mcp.json` when the package carries one. An unreadable file is
/// a refusal, not a plugin without servers: the user asked to install
/// what the package declares.
fn read_mcp(root: &Path, problems: &mut Vec<String>) -> Option<McpConfig> {
    let path = root.join(MCP_FILE);
    if !path.is_file() {
        return None;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            problems.push(format!("cannot read {MCP_FILE}: {error}"));
            return None;
        }
    };
    let document: serde_json::Value = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(error) => {
            problems.push(format!("{MCP_FILE} is not JSON: {error}"));
            return None;
        }
    };
    if let Err(refusals) = check_schema(&MCP_VALIDATOR, &document, MCP_FILE) {
        problems.extend(refusals);
        return None;
    }
    match serde_json::from_value::<McpConfig>(document) {
        Ok(config) => Some(config),
        Err(error) => {
            problems.push(format!("{MCP_FILE} cannot be read: {error}"));
            None
        }
    }
}

fn check_servers(root: &Path, mcp: &McpConfig, pagis: &PagisExtension, problems: &mut Vec<String>) {
    if mcp.servers.is_empty() {
        problems.push(format!("{MCP_FILE} declares no server"));
    }
    for (name, server) in &mcp.servers {
        match server {
            McpServer::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                check_command(name, command, root, problems);
                for (index, argument) in args.iter().enumerate() {
                    check_references(
                        &format!("server {name:?} args[{index}]"),
                        argument,
                        pagis,
                        false,
                        problems,
                    );
                }
                for (key, value) in env {
                    check_references(
                        &format!("server {name:?} env {key:?}"),
                        value,
                        pagis,
                        true,
                        problems,
                    );
                }
                if let Some(cwd) = cwd {
                    check_cwd(name, cwd, root, problems);
                }
            }
            McpServer::StreamableHttp { url, headers } | McpServer::Sse { url, headers } => {
                check_url(name, url, problems);
                check_references(&format!("server {name:?} url"), url, pagis, false, problems);
                for (key, value) in headers {
                    check_references(
                        &format!("server {name:?} header {key:?}"),
                        value,
                        pagis,
                        true,
                        problems,
                    );
                }
            }
        }
    }
}

/// The command is one executable token: a bare name resolved on PATH,
/// or a `./` path that stays inside the plugin root. It takes no
/// placeholder and no reference.
fn check_command(server: &str, command: &str, root: &Path, problems: &mut Vec<String>) {
    if command.contains("${") {
        problems.push(format!(
            "server {server:?} command {command:?} carries a placeholder; a command is one literal \
             token"
        ));
        return;
    }
    if let Some(relative) = command.strip_prefix("./") {
        if relative.is_empty() {
            problems.push(format!(
                "server {server:?} command {command:?} names no file"
            ));
        } else if !stays_inside(Path::new(relative)) {
            problems.push(format!(
                "server {server:?} command {command:?} leaves the plugin root"
            ));
        } else if !root.join(relative).is_file() {
            problems.push(format!(
                "server {server:?} command {command:?} is not a file of the plugin"
            ));
        }
        return;
    }
    if command.contains('/') || command.contains('\\') {
        problems.push(format!(
            "server {server:?} command {command:?} is not a bare name or a \"./\" path"
        ));
    }
}

/// `cwd` is the plugin root, the plugin data directory, or a path
/// under one of them. Nothing else, and nothing that climbs out.
fn check_cwd(server: &str, cwd: &str, root: &Path, problems: &mut Vec<String>) {
    let relative = if let Some(rest) = cwd.strip_prefix("./") {
        rest
    } else if let Some(rest) = cwd
        .strip_prefix("${PLUGIN_ROOT}/")
        .or_else(|| cwd.strip_prefix("${PLUGIN_DATA}/"))
    {
        rest
    } else if PATH_PLACEHOLDERS.contains(&cwd) {
        return;
    } else {
        problems.push(format!(
            "server {server:?} cwd {cwd:?} is not \"./\", ${{PLUGIN_ROOT}} or ${{PLUGIN_DATA}}"
        ));
        return;
    };
    if relative.contains("${") || !stays_inside(Path::new(relative)) {
        problems.push(format!(
            "server {server:?} cwd {cwd:?} leaves the directory it is rooted in"
        ));
        return;
    }
    // A `./` cwd is the plugin's own tree, so it must be there. A
    // `${PLUGIN_DATA}` cwd is made by the daemon at spawn time.
    if cwd.starts_with("./") && !root.join(relative).is_dir() {
        problems.push(format!(
            "server {server:?} cwd {cwd:?} is not a directory of the plugin"
        ));
    }
}

/// The remote address rules of the specification: an absolute HTTP or
/// HTTPS URL, no user information, no fragment, and plain HTTP only to
/// the loopback interface (ADR-0017).
fn check_url(server: &str, url: &str, problems: &mut Vec<String>) {
    // A bound value stands where a placeholder is (ADR-0017), and a
    // placeholder is not a URL. The shape is judged with every
    // placeholder read as one digit; the address the daemon builds is
    // judged again at every start, where the values are known.
    let parsed = match Url::parse(&without_placeholders(url)) {
        Ok(parsed) => parsed,
        Err(error) => {
            problems.push(format!(
                "server {server:?} url {url:?} is not a URL: {error}"
            ));
            return;
        }
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        problems.push(format!(
            "server {server:?} url {url:?} is not an http or https address"
        ));
        return;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        problems.push(format!(
            "server {server:?} url {url:?} carries user information"
        ));
    }
    if parsed.fragment().is_some() {
        problems.push(format!("server {server:?} url {url:?} carries a fragment"));
    }
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if parsed.scheme() == "http" && !loopback {
        problems.push(format!(
            "server {server:?} url {url:?} is plain http to a remote host"
        ));
    }
}

/// One address with every `${...}` read as the digit `1`, which every
/// part of a URL accepts.
fn without_placeholders(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                out.push('1');
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Every `${config.<field>}` reference must name a declared field, and
/// a credential may reach only an `env` value or a header (ADR-0017).
fn check_references(
    place: &str,
    text: &str,
    pagis: &PagisExtension,
    credentials_allowed: bool,
    problems: &mut Vec<String>,
) {
    for field in config_references(text) {
        match pagis.config.get(&field) {
            None => problems.push(format!("{place} refers to the undeclared field {field:?}")),
            Some(declared) if declared.kind.is_sensitive() && !credentials_allowed => {
                problems.push(format!(
                    "{place} refers to the {} field {field:?}; a credential reaches a server only \
                     as an env value or a header",
                    declared.kind.as_str()
                ));
            }
            Some(_) => {}
        }
    }
}

/// The published schemas, compiled once each.
static PLUGIN_VALIDATOR: std::sync::LazyLock<jsonschema::Validator> =
    std::sync::LazyLock::new(|| compile(PLUGIN_SCHEMA));
static MCP_VALIDATOR: std::sync::LazyLock<jsonschema::Validator> =
    std::sync::LazyLock::new(|| compile(MCP_SCHEMA));

fn compile(schema: &str) -> jsonschema::Validator {
    let schema: serde_json::Value =
        serde_json::from_str(schema).expect("the compiled-in schema is JSON");
    jsonschema::validator_for(&schema).expect("the compiled-in schema compiles")
}

/// Judge one document against a published schema.
fn check_schema(
    validator: &jsonschema::Validator,
    document: &serde_json::Value,
    file: &str,
) -> Result<(), Vec<String>> {
    let refusals: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{file}{}: {error}", error.instance_path()))
        .collect();
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals)
    }
}

/// Whether a relative path stays where it is rooted, by `..` alone.
/// A symbolic link is caught when the daemon resolves the path.
pub(crate) fn stays_inside(path: &Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}

/// The size the repository takes. A plugin that is too large is
/// refused before anything is written.
fn check_size(root: &Path, problems: &mut Vec<String>) {
    let mut total: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                total += metadata.len();
            }
        }
    }
    if total > MAX_PACKAGE_BYTES {
        problems.push(format!(
            "the plugin is {total} bytes; the limit is {MAX_PACKAGE_BYTES}"
        ));
    }
}
