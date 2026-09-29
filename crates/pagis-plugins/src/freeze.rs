//! The frozen manifest of a Plugin (ADR-0005, ADR-0017).
//!
//! An install starts every declared server once and takes its
//! `tools/list`. What comes back is judged here and then never moves:
//! a name a model provider cannot carry fails the install, two servers
//! of one Plugin that offer one name fail the install, and the list
//! that passes becomes one Capability Manifest version beside the
//! commit it was read from.
//!
//! A later list is only ever a proposal. The host marks the Plugin
//! "the tools changed" and keeps offering the frozen list, because a
//! Run must not find its tools different from the ones its snapshot
//! named.

use pagis_broker::{
    AllowRuleBuilder, ApprovalPresentation, CapabilityManifest, EffectClass, MAX_NAME,
    ManifestTool, ToolDef, ToolRoute, is_provider_safe_name,
};
use pagis_core::{PluginTool, PluginTools};
use pagis_plugin::PluginPackage;

/// Why one tool list could not be frozen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FreezeError {
    #[error(
        "the server {server:?} offers the tool {tool:?}, whose qualified name is not 1 to \
         {MAX_NAME} ASCII letters, digits, underscores or hyphens"
    )]
    Name { server: String, tool: String },
    #[error("the servers {first:?} and {second:?} both offer the tool {tool:?}")]
    Duplicate {
        tool: String,
        first: String,
        second: String,
    },
    #[error("the server {server:?} declares an argument schema for {tool:?} that is not an object")]
    Schema { server: String, tool: String },
}

/// The name the model sees: `<plugin>__<tool>` (ADR-0005).
pub fn qualified_name(plugin: &str, tool: &str) -> String {
    format!("{plugin}__{tool}")
}

/// Judge what the servers offered and answer with the list to freeze.
/// `offered` is one entry per server, in `mcp.json` order.
pub fn freeze_tools(
    plugin_name: &str,
    offered: &[(String, Vec<PluginTool>)],
) -> Result<Vec<PluginTool>, FreezeError> {
    let mut frozen: Vec<PluginTool> = Vec::new();
    for (server, tools) in offered {
        for tool in tools {
            if !is_provider_safe_name(&tool.name)
                || !is_provider_safe_name(&qualified_name(plugin_name, &tool.name))
            {
                return Err(FreezeError::Name {
                    server: server.clone(),
                    tool: tool.name.clone(),
                });
            }
            if !tool.schema.is_object() {
                return Err(FreezeError::Schema {
                    server: server.clone(),
                    tool: tool.name.clone(),
                });
            }
            if let Some(held) = frozen.iter().find(|held| held.name == tool.name) {
                return Err(FreezeError::Duplicate {
                    tool: tool.name.clone(),
                    first: held.server.clone(),
                    second: server.clone(),
                });
            }
            frozen.push(PluginTool {
                server: server.clone(),
                ..tool.clone()
            });
        }
    }
    Ok(frozen)
}

/// The Capability Manifest of one frozen version. Every tool is
/// `Host` unless the plugin declared a class for it (ADR-0017), and
/// every tool offers the one allow rule that names it.
pub fn capability_manifest(
    plugin_id: &str,
    plugin_name: &str,
    package: &PluginPackage,
    frozen: &PluginTools,
) -> CapabilityManifest {
    let tools = frozen
        .tools
        .iter()
        .map(|tool| {
            let name = qualified_name(plugin_name, &tool.name);
            ManifestTool {
                definition: ToolDef {
                    name: name.clone(),
                    description: tool.description.clone(),
                    parameters: object_schema(&tool.schema),
                },
                capability: None,
                effect: package.effect(&tool.name),
                presentation: Some(ApprovalPresentation {
                    action_title: format!("Run {name}"),
                    body_argument: None,
                    // Every class but `Free` shows a card, and the
                    // card offers "always allow this tool".
                    allow_rule_builder: (package.effect(&tool.name) != EffectClass::Free)
                        .then_some(AllowRuleBuilder::PluginTool),
                }),
                route: ToolRoute::Plugin {
                    plugin: plugin_id.to_string(),
                    server: tool.server.clone(),
                    tool: tool.name.clone(),
                },
                call_timeout: Some(package.call_timeout(&tool.name)),
                visibility: Default::default(),
                widget: None,
            }
        })
        .collect();
    CapabilityManifest {
        namespace: plugin_name.to_string(),
        source_version: frozen.version.clone(),
        tools,
        event_kinds: Vec::new(),
    }
}

/// The argument schema, in the shape the broker validates a call
/// against. A `$ref` is dropped rather than installed: the broker
/// refuses one, and a plugin must not fail to install over a part of a
/// schema that only describes itself.
fn object_schema(schema: &serde_json::Value) -> serde_json::Value {
    if contains_ref(schema) {
        return serde_json::json!({"type": "object"});
    }
    schema.clone()
}

fn contains_ref(schema: &serde_json::Value) -> bool {
    match schema {
        serde_json::Value::Object(fields) => {
            fields.contains_key("$ref") || fields.values().any(contains_ref)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_ref),
        _ => false,
    }
}
