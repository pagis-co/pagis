//! The rules the freeze applies to what a server offered, and the
//! Capability Manifest that follows (ADR-0005, ADR-0017).

use std::time::Duration;

use pagis_broker::{AllowRuleBuilder, EffectClass, ToolRoute};
use pagis_core::{PluginTool, PluginTools};
use pagis_plugin::PluginPackage;
use pagis_plugins::{FreezeError, capability_manifest, freeze_tools};

fn tool(name: &str) -> PluginTool {
    PluginTool {
        server: String::new(),
        name: name.to_string(),
        description: format!("the {name} tool"),
        schema: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn package(extension: serde_json::Value) -> PluginPackage {
    let manifest: pagis_plugin::PluginManifest = serde_json::from_value(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": extension.clone()},
    }))
    .expect("the manifest reads");
    PluginPackage {
        manifest,
        mcp: None,
        pagis: serde_json::from_value(extension).expect("the extension reads"),
        skills: Vec::new(),
    }
}

fn frozen(tools: Vec<PluginTool>) -> PluginTools {
    PluginTools {
        plugin_id: pagis_core::PluginId::generate(),
        version: "v1".to_string(),
        installed_commit: "0".repeat(40),
        tools,
        tools_changed: false,
        created_at: 0,
    }
}

#[test]
fn the_frozen_list_keeps_the_server_of_every_tool() {
    let list = freeze_tools(
        "weather",
        &[
            ("today".to_string(), vec![tool("forecast")]),
            ("history".to_string(), vec![tool("past")]),
        ],
    )
    .expect("the list freezes");

    assert_eq!(list.len(), 2);
    assert_eq!(list[0].server, "today");
    assert_eq!(list[1].server, "history");
}

#[test]
fn two_servers_that_offer_one_tool_name_fail_the_freeze() {
    let refused = freeze_tools(
        "weather",
        &[
            ("today".to_string(), vec![tool("forecast")]),
            ("history".to_string(), vec![tool("forecast")]),
        ],
    );

    assert_eq!(
        refused.err(),
        Some(FreezeError::Duplicate {
            tool: "forecast".to_string(),
            first: "today".to_string(),
            second: "history".to_string(),
        })
    );
}

#[test]
fn a_tool_name_no_model_provider_carries_fails_the_freeze() {
    let refused = freeze_tools(
        "weather",
        &[("today".to_string(), vec![tool("get.forecast")])],
    );

    assert!(matches!(refused, Err(FreezeError::Name { .. })));
}

#[test]
fn every_tool_is_host_unless_the_plugin_declared_a_class() {
    let package = package(serde_json::json!({
        "tools": {"forecast": {"effect": "free"}, "order": {"effect": "purchase"}},
    }));
    let frozen = frozen(vec![tool("forecast"), tool("order"), tool("wind")]);

    let manifest = capability_manifest("plugin-1", "weather", &package, &frozen);

    let effects: Vec<(&str, EffectClass)> = manifest
        .tools
        .iter()
        .map(|tool| (tool.definition.name.as_str(), tool.effect))
        .collect();
    assert_eq!(
        effects,
        [
            ("weather__forecast", EffectClass::Free),
            ("weather__order", EffectClass::Purchase),
            ("weather__wind", EffectClass::Host),
        ]
    );
    // A `free` tool shows no card, so it offers no rule; every other
    // tool offers the one rule that names it.
    assert_eq!(
        manifest.tools[0]
            .presentation
            .as_ref()
            .and_then(|presentation| presentation.allow_rule_builder.clone()),
        None
    );
    assert_eq!(
        manifest.tools[2]
            .presentation
            .as_ref()
            .and_then(|presentation| presentation.allow_rule_builder.clone()),
        Some(AllowRuleBuilder::PluginTool)
    );
}

#[test]
fn the_route_names_the_plugin_the_server_and_the_bare_tool() {
    let package = package(serde_json::json!({}));
    let mut frozen = frozen(vec![tool("forecast")]);
    frozen.tools[0].server = "today".to_string();

    let manifest = capability_manifest("plugin-1", "weather", &package, &frozen);

    assert_eq!(
        manifest.tools[0].route,
        ToolRoute::Plugin {
            plugin: "plugin-1".to_string(),
            server: "today".to_string(),
            tool: "forecast".to_string(),
        }
    );
    assert_eq!(manifest.namespace, "weather");
    assert_eq!(manifest.source_version, "v1");
}

#[test]
fn a_declared_deadline_stands_inside_its_bounds() {
    let package = package(serde_json::json!({
        "tools": {
            "slow": {"effect": "host", "timeout_s": 300},
            "endless": {"effect": "host", "timeout_s": 4000},
        },
    }));
    let frozen = frozen(vec![tool("slow"), tool("endless"), tool("quick")]);

    let manifest = capability_manifest("plugin-1", "weather", &package, &frozen);

    let timeouts: Vec<Option<Duration>> = manifest
        .tools
        .iter()
        .map(|tool| tool.call_timeout)
        .collect();
    assert_eq!(
        timeouts,
        [
            Some(Duration::from_secs(300)),
            Some(pagis_plugin::MAX_CALL_TIMEOUT),
            Some(pagis_plugin::DEFAULT_CALL_TIMEOUT),
        ]
    );
}
