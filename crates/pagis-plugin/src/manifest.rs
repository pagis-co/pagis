//! The files of one plugin package (ADR-0017): `plugin.json`,
//! `mcp.json`, the `skills/` directory, and the `pagis` extension that
//! declares the config fields and the tool effect classes.
//!
//! The types here read the agent-plugins.org 1.0.0 package as it is
//! written. The rules Pagis adds on top live in [`crate::validate`].

use std::collections::BTreeMap;

use pagis_broker::EffectClass;
use serde::{Deserialize, Serialize};

/// The manifest every plugin package carries, at its root.
pub const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
/// The MCP server configuration, at the root. It is optional: a plugin
/// of Skills alone carries none (ADR-0017).
pub const MCP_FILE: &str = "mcp.json";
/// The one directory Skills are discovered in (ADR-0017).
pub const SKILLS_DIR: &str = "skills";
/// The `$schema` of `plugin.json` for the version Pagis reads.
pub const PLUGIN_SCHEMA_ID: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
/// The `$schema` of `mcp.json` for the same version.
pub const MCP_SCHEMA_ID: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
/// The `extensions` key Pagis reads. Every other namespace is ignored,
/// as the specification requires.
pub const PAGIS_EXTENSION: &str = "pagis";

/// The published 1.0.0 schema of `plugin.json`, compiled in. An
/// offline daemon cannot fetch one, and an install must judge a
/// package the same way every time.
pub const PLUGIN_SCHEMA: &str = include_str!("../schemas/plugin.schema.json");
/// The published 1.0.0 schema of `mcp.json`, compiled in.
pub const MCP_SCHEMA: &str = include_str!("../schemas/mcp.schema.json");

/// `plugin.json`, as the specification defines it. Fields Pagis does
/// not read are not held: the schema check already accepted them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Client-specific data by reverse-domain namespace. Pagis reads
    /// [`PAGIS_EXTENSION`] and ignores the rest.
    #[serde(default)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// `mcp.json`, as the specification defines it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub servers: BTreeMap<String, McpServer>,
}

/// One MCP server of a plugin. The `sse` transport is deprecated by
/// the specification but still legal, so it is read and kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum McpServer {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl McpServer {
    /// The `env` entries of a stdio server; an HTTP server has none.
    pub fn env(&self) -> &BTreeMap<String, String> {
        static EMPTY: std::sync::LazyLock<BTreeMap<String, String>> =
            std::sync::LazyLock::new(BTreeMap::new);
        match self {
            McpServer::Stdio { env, .. } => env,
            McpServer::StreamableHttp { .. } | McpServer::Sse { .. } => &EMPTY,
        }
    }

    /// The `headers` of an HTTP server; a stdio server has none.
    pub fn headers(&self) -> &BTreeMap<String, String> {
        static EMPTY: std::sync::LazyLock<BTreeMap<String, String>> =
            std::sync::LazyLock::new(BTreeMap::new);
        match self {
            McpServer::StreamableHttp { headers, .. } | McpServer::Sse { headers, .. } => headers,
            McpServer::Stdio { .. } => &EMPTY,
        }
    }
}

/// What a plugin declares under the `pagis` extension
/// (ADR-0017). A package with no such entry declares no field and no
/// effect, which is legal: it then needs no Binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PagisExtension {
    /// The config fields the user binds at install, by field name.
    #[serde(default)]
    pub config: BTreeMap<String, ConfigField>,
    /// The effect class the plugin claims for one of its tools, by
    /// bare tool name.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolPolicy>,
}

/// One config field a plugin declares (ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigField {
    #[serde(rename = "type")]
    pub kind: FieldKind,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    /// A `connection` field names the Connection provider it needs.
    #[serde(default)]
    pub provider: Option<String>,
    /// The capabilities the plugin needs on that Connection. The
    /// Agent's Connection Grant must carry every one of them.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The five kinds of config field (ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Secret,
    Connection,
    String,
    Number,
    Boolean,
}

impl FieldKind {
    /// Whether the bound value is a credential. A credential reaches a
    /// stdio server only as an `env` value and an HTTP server only as
    /// a header (ADR-0017).
    pub fn is_sensitive(self) -> bool {
        matches!(self, FieldKind::Secret | FieldKind::Connection)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::Secret => "secret",
            FieldKind::Connection => "connection",
            FieldKind::String => "string",
            FieldKind::Number => "number",
            FieldKind::Boolean => "boolean",
        }
    }
}

/// What a plugin claims about one of its tools (ADR-0017): the effect
/// class, and how long one call may run. Every other part of the
/// approval card belongs to Pagis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    pub effect: EffectClass,
    /// The deadline of one call, in seconds. It is clamped to
    /// [`MAX_CALL_TIMEOUT`]: a claim about the tool cannot hold a Run
    /// open for longer than the daemon allows.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// How long one plugin tool call may run when the plugin declares no
/// deadline (ADR-0017).
pub const DEFAULT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// The longest deadline a plugin may declare.
pub const MAX_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// One plugin package, read and accepted. It is what an install
/// commits and what the install card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackage {
    pub manifest: PluginManifest,
    /// The MCP servers, or `None` when the package carries no
    /// `mcp.json`.
    pub mcp: Option<McpConfig>,
    pub pagis: PagisExtension,
    /// The Skills the package ships, in directory order (ADR-0017).
    pub skills: Vec<pagis_core::Skill>,
}

impl PluginPackage {
    /// The effect class of one tool: what the plugin declared, or
    /// `Host` (ADR-0017). A plugin tool runs on this computer with the
    /// user's privileges, so `Host` is the floor a plugin can only
    /// raise or ask the user to lower at install.
    pub fn effect(&self, tool: &str) -> EffectClass {
        self.pagis
            .tools
            .get(tool)
            .map(|policy| policy.effect)
            .unwrap_or(EffectClass::Host)
    }

    /// How long one call to this tool may run: what the plugin
    /// declared inside its bounds, or [`DEFAULT_CALL_TIMEOUT`].
    pub fn call_timeout(&self, tool: &str) -> std::time::Duration {
        self.pagis
            .tools
            .get(tool)
            .and_then(|policy| policy.timeout_s)
            .map(|seconds| {
                std::time::Duration::from_secs(seconds)
                    .clamp(std::time::Duration::from_secs(1), MAX_CALL_TIMEOUT)
            })
            .unwrap_or(DEFAULT_CALL_TIMEOUT)
    }
}

/// The prefix of a reference to a bound config field. Pagis expands it
/// where the plugin's kind of value is allowed; the specification's
/// own `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are untouched.
const REFERENCE_PREFIX: &str = "${config.";

/// Every config field one string refers to, in the order they appear.
/// A reference is `${config.<field>}`.
pub fn config_references(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(REFERENCE_PREFIX) {
        let after = &rest[start + REFERENCE_PREFIX.len()..];
        match after.find('}') {
            Some(end) => {
                fields.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    fields
}
