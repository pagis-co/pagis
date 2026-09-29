//! Manifest parsing and the Capability Manifest one version offers.

use std::collections::BTreeMap;
use std::time::Duration;

use pagis_broker::{EffectClass, ToolRoute};
use pagis_software::manifest::PackageVersion;
use pagis_software::{DEFAULT_TIMEOUT_S, Manifest};

const FULL: &str = r#"
[package]
name = "weather"
description = "Weather for a city"
keywords = ["weather", "forecast"]
setup = "pnpm install --frozen-lockfile"

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
timeout_s = 60
"#;

#[test]
fn a_manifest_carries_the_package_and_its_tools() {
    let manifest = Manifest::parse(FULL).expect("the manifest parses");

    assert_eq!(manifest.package.name, "weather");
    assert_eq!(manifest.package.description, "Weather for a city");
    assert_eq!(manifest.package.keywords, vec!["weather", "forecast"]);
    assert_eq!(
        manifest.package.setup.as_deref(),
        Some("pnpm install --frozen-lockfile")
    );
    let tool = manifest.tool("forecast").expect("the tool is declared");
    assert_eq!(tool.entry, "bin/forecast.py");
    assert_eq!(tool.schema, "schemas/forecast.json");
    assert_eq!(tool.timeout(), Duration::from_secs(60));
}

#[test]
fn a_manifest_has_no_version_field() {
    // The daemon mints the tag at every publish, so a manifest
    // that carries one is refused instead of quietly ignored.
    let error = Manifest::parse(
        r#"
[package]
name = "weather"
description = "Weather"
version = "1.2.3"
"#,
    )
    .expect_err("a version field is refused");

    assert!(error.contains("version"), "{error}");
}

#[test]
fn a_tool_without_a_deadline_takes_the_default_and_a_long_one_is_clamped() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "weather"
description = "Weather"

[[tool]]
name = "now"
description = "Now"
entry = "bin/now"
schema = "schemas/now.json"

[[tool]]
name = "slow"
description = "Slow"
entry = "bin/slow"
schema = "schemas/slow.json"
timeout_s = 9000
"#,
    )
    .expect("the manifest parses");

    assert_eq!(
        manifest.tool("now").unwrap().timeout(),
        Duration::from_secs(DEFAULT_TIMEOUT_S)
    );
    assert_eq!(
        manifest.tool("slow").unwrap().timeout(),
        Duration::from_secs(600)
    );
}

#[test]
fn broken_toml_says_so() {
    let error = Manifest::parse("[package").expect_err("broken TOML is refused");

    assert!(error.contains("pagis-software.toml"), "{error}");
}

#[test]
fn a_version_becomes_one_capability_manifest() {
    let manifest = Manifest::parse(FULL).expect("the manifest parses");
    let schema = serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}});
    let version = PackageVersion {
        manifest,
        schemas: BTreeMap::from([("forecast".to_string(), schema.clone())]),
        widget_schemas: BTreeMap::new(),
    };

    let capability = version.to_capability_manifest("v3");

    assert_eq!(capability.namespace, "weather");
    assert_eq!(capability.source_version, "v3");
    assert!(capability.event_kinds.is_empty());
    let tool = capability.tools.first().expect("one tool");
    assert_eq!(tool.definition.name, "weather__forecast");
    assert_eq!(tool.definition.parameters, schema);
    assert_eq!(tool.effect, EffectClass::Free);
    assert!(tool.capability.is_none());
    assert!(tool.presentation.is_none());
    assert_eq!(
        tool.route,
        ToolRoute::Software {
            package: "weather".to_string(),
            tool: "forecast".to_string(),
        }
    );
    // 60 s of tool, 5 s of kill grace, 600 s of wake.
    assert_eq!(tool.call_timeout, Some(Duration::from_secs(665)));
}

#[test]
fn a_software_result_carries_foreign_text() {
    // Whatever the author's program prints comes from outside Pagis
    // (ADR-0005).
    assert!(
        ToolRoute::Software {
            package: "weather".to_string(),
            tool: "forecast".to_string(),
        }
        .returns_foreign_text()
    );
}
