//! The `pagis-software.toml` manifest: what an author
//! writes, and the Capability Manifest the broker reads.

use std::collections::BTreeMap;
use std::time::Duration;

use pagis_broker::{
    CapabilityManifest, EffectClass, ManifestTool, ToolDef, ToolRoute, ToolVisibility, ToolWidget,
};
use serde::{Deserialize, Serialize};

use crate::widget::{Visibility, WidgetSpec, default_visibility};

/// The manifest file at the root of a Software Package.
pub const MANIFEST_FILE: &str = "pagis-software.toml";
/// The deadline of one tool call when the manifest names none.
pub const DEFAULT_TIMEOUT_S: u64 = 120;
/// The longest deadline a manifest can ask for. A larger request is
/// clamped, as `computer_shell` clamps its own.
pub const MAX_TIMEOUT_S: u64 = 600;
/// The grace between the `TERM` and the `KILL` of the in-container
/// deadline, in seconds.
const KILL_GRACE_S: u64 = 5;
/// How long a cold computer gets to wake before a call gives up.
/// The dispatcher must wait for the wake and the call.
const WAKE_DEADLINE_S: u64 = 600;

/// One package version's manifest, as the author wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    #[serde(default, rename = "tool")]
    pub tools: Vec<ToolSpec>,
    /// The Widgets the package ships (ADR-0016). A tool renders into
    /// one by name.
    #[serde(default, rename = "widget")]
    pub widgets: Vec<WidgetSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// The namespace of the package. Its tools are exposed as
    /// `<name>__<tool>`.
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// The command that prepares a materialized copy. It runs once per
    /// version in each Computer.
    pub setup: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The executable to run, relative to the package root.
    pub entry: String,
    /// The JSON Schema file of the arguments, relative to the package
    /// root.
    pub schema: String,
    pub timeout_s: Option<u64>,
    /// The Widget this tool renders into (ADR-0016). The result then
    /// splits: `structuredContent` reaches the Widget alone and
    /// `content` reaches the model alone.
    #[serde(default)]
    pub widget: Option<String>,
    /// Who may call this tool. The default is the model and the
    /// Widget; `["app"]` hides it from the model.
    #[serde(default = "default_visibility")]
    pub visibility: Vec<Visibility>,
    /// Whether the Run parks on a `widget` Request for the Widget's
    /// answer. Only a tool that renders a Widget can ask.
    #[serde(default)]
    pub awaits_input: bool,
}

impl ToolSpec {
    /// The deadline of one call to this tool, inside its bounds.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(
            self.timeout_s
                .unwrap_or(DEFAULT_TIMEOUT_S)
                .clamp(1, MAX_TIMEOUT_S),
        )
    }
}

impl ToolSpec {
    /// Whether the model sees this tool in its tool list.
    pub fn visible_to_model(&self) -> bool {
        self.visibility.contains(&Visibility::Model)
    }

    /// Whether a Widget may call this tool through the daemon.
    pub fn visible_to_app(&self) -> bool {
        self.visibility.contains(&Visibility::App)
    }
}

impl Manifest {
    /// Read a manifest from its TOML text.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("{MANIFEST_FILE} does not parse: {error}"))
    }

    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    pub fn widget(&self, name: &str) -> Option<&WidgetSpec> {
        self.widgets.iter().find(|widget| widget.name == name)
    }
}

/// One package version the daemon can offer and run: the manifest plus
/// the argument schema of every tool, read from the package root. It
/// is stored as JSON with the Version record, so a run start reads it
/// without opening the repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PackageVersion {
    pub manifest: Manifest,
    /// The argument schema per tool name.
    pub schemas: BTreeMap<String, serde_json::Value>,
    /// The `structuredContent` schema per Widget name (ADR-0016). A
    /// Version that ships no Widget holds none.
    #[serde(default)]
    pub widget_schemas: BTreeMap<String, serde_json::Value>,
}

impl PackageVersion {
    /// The Capability Manifest of this version. Every tool is
    /// `Free`: a Software tool runs in the agent's own Computer, which
    /// is the sandbox (ADR-0014), so it needs neither a grant nor an
    /// approval card.
    pub fn to_capability_manifest(&self, version: &str) -> CapabilityManifest {
        let package = &self.manifest.package.name;
        let tools = self
            .manifest
            .tools
            .iter()
            .map(|tool| ManifestTool {
                definition: ToolDef {
                    name: format!("{package}__{}", tool.name),
                    description: tool.description.clone(),
                    parameters: self
                        .schemas
                        .get(&tool.name)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                },
                capability: None,
                effect: EffectClass::Free,
                presentation: None,
                visibility: ToolVisibility {
                    model: tool.visible_to_model(),
                    app: tool.visible_to_app(),
                },
                widget: tool.widget.as_ref().map(|name| ToolWidget {
                    name: name.clone(),
                    awaits_input: tool.awaits_input,
                }),
                route: ToolRoute::Software {
                    package: package.clone(),
                    tool: tool.name.clone(),
                },
                call_timeout: Some(Duration::from_secs(
                    tool.timeout().as_secs() + KILL_GRACE_S + WAKE_DEADLINE_S,
                )),
            })
            .collect();
        CapabilityManifest {
            namespace: package.clone(),
            source_version: version.to_string(),
            tools,
            event_kinds: Vec::new(),
        }
    }
}
