//! The install validation table (ADR-0017): which
//! packages Pagis accepts and which it refuses, and why.

use crate::support;

use pagis_broker::EffectClass;
use pagis_plugin::{FieldKind, McpServer, validate};
use support::{MCP_SCHEMA, Package, minimal};

/// The problems one package has, as one string to search.
fn refusal(package: &Package) -> String {
    match validate(package.root()) {
        Ok(_) => panic!("the package was accepted"),
        Err(problems) => problems.to_string(),
    }
}

#[test]
fn a_plugin_of_one_manifest_is_accepted() {
    let package = minimal();
    let read = validate(package.root()).expect("accepted");
    assert_eq!(read.manifest.name, "weather");
    assert!(read.mcp.is_none());
    assert!(read.skills.is_empty());
    assert!(read.pagis.config.is_empty());
}

#[test]
fn a_missing_manifest_is_refused() {
    let package = Package::new();
    assert!(refusal(&package).contains("cannot read plugin.json"));
}

#[test]
fn a_manifest_that_is_not_json_is_refused() {
    let package = Package::new().file("plugin.json", "{");
    assert!(refusal(&package).contains("is not JSON"));
}

#[test]
fn a_manifest_without_the_1_0_0_schema_is_refused() {
    let package = Package::new().file(
        "plugin.json",
        &serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json",
            "name": "weather",
        })
        .to_string(),
    );
    assert!(refusal(&package).contains("plugin.json"));
}

#[test]
fn a_manifest_field_the_schema_does_not_allow_is_refused() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "hooks": {"onStart": "./run.sh"},
    }));
    assert!(refusal(&package).contains("plugin.json"));
}

#[test]
fn a_name_a_qualified_tool_name_cannot_carry_is_refused() {
    // The specification allows a period; a qualified tool name does
    // not (ADR-0005).
    let package = Package::new().manifest(serde_json::json!({ "name": "acme.weather" }));
    assert!(refusal(&package).contains("plugin name"));
}

#[test]
fn a_name_pagis_mints_itself_is_refused() {
    let package = Package::new().manifest(serde_json::json!({ "name": "core" }));
    assert!(refusal(&package).contains("reserved"));
}

#[test]
fn a_bare_command_and_a_dot_slash_command_are_accepted() {
    let package = minimal()
        .file("server.js", "// the server")
        .mcp(serde_json::json!({
            "onpath": {"type": "stdio", "command": "weather-mcp"},
            "bundled": {"type": "stdio", "command": "./server.js"},
        }));
    let read = validate(package.root()).expect("accepted");
    assert_eq!(read.mcp.expect("servers").servers.len(), 2);
}

#[test]
fn a_command_that_is_a_path_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "shell": {"type": "stdio", "command": "/usr/bin/env"},
    }));
    assert!(refusal(&package).contains("bare name"));
}

#[test]
fn a_dot_slash_command_that_is_not_in_the_package_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "bundled": {"type": "stdio", "command": "./server.js"},
    }));
    assert!(refusal(&package).contains("not a file of the plugin"));
}

#[test]
fn a_command_with_a_placeholder_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "bundled": {"type": "stdio", "command": "${PLUGIN_ROOT}/server.js"},
    }));
    assert!(refusal(&package).contains("one literal token"));
}

#[test]
fn a_cwd_inside_the_plugin_is_accepted() {
    let package = minimal()
        .directory("bin")
        .file("server.js", "// the server")
        .mcp(serde_json::json!({
            "here": {"type": "stdio", "command": "./server.js", "cwd": "./bin"},
            "data": {"type": "stdio", "command": "./server.js", "cwd": "${PLUGIN_DATA}/state"},
        }));
    validate(package.root()).expect("accepted");
}

#[test]
fn a_cwd_that_leaves_the_root_is_refused() {
    let package = minimal()
        .file("server.js", "// the server")
        .mcp(serde_json::json!({
            "out": {"type": "stdio", "command": "./server.js", "cwd": "./../elsewhere"},
        }));
    assert!(refusal(&package).contains("leaves the directory"));
}

#[test]
fn an_absolute_cwd_is_refused() {
    let package = minimal()
        .file("server.js", "// the server")
        .mcp(serde_json::json!({
            "out": {"type": "stdio", "command": "./server.js", "cwd": "/tmp"},
        }));
    assert!(refusal(&package).contains("mcp.json"));
}

#[test]
fn an_env_entry_that_declares_a_plugin_path_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "server": {
            "type": "stdio",
            "command": "weather-mcp",
            "env": {"PLUGIN_ROOT": "/elsewhere"},
        },
    }));
    assert!(refusal(&package).contains("mcp.json"));
}

#[test]
fn plain_http_to_a_remote_host_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "remote": {"type": "streamable-http", "url": "http://weather.example.com/mcp"},
    }));
    assert!(refusal(&package).contains("plain http"));
}

#[test]
fn plain_http_to_the_loopback_interface_is_accepted() {
    let package = minimal().mcp(serde_json::json!({
        "local": {"type": "streamable-http", "url": "http://127.0.0.1:8931/mcp"},
    }));
    validate(package.root()).expect("accepted");
}

/// The documents state that install accepts `https` to every host
/// (ADR-0017), which includes the private and link-local addresses that
/// the egress policy of a Computer refuses.
#[test]
fn https_to_a_private_or_link_local_address_is_accepted() {
    let package = minimal().mcp(serde_json::json!({
        "lan": {"type": "streamable-http", "url": "https://192.168.1.40/mcp"},
        "metadata": {"type": "sse", "url": "https://169.254.169.254/sse"},
    }));
    validate(package.root()).expect("accepted");
}

#[test]
fn a_url_with_user_information_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "remote": {"type": "streamable-http", "url": "https://user:pass@weather.example.com/mcp"},
    }));
    assert!(refusal(&package).contains("user information"));
}

#[test]
fn a_secret_reaches_an_env_value_and_a_header() {
    let package = declaring_a_secret().mcp(serde_json::json!({
        "stdio": {
            "type": "stdio",
            "command": "weather-mcp",
            "env": {"API_KEY": "${config.api_key}"},
        },
        "http": {
            "type": "streamable-http",
            "url": "https://weather.example.com/mcp",
            "headers": {"Authorization": "Bearer ${config.api_key}"},
        },
    }));
    validate(package.root()).expect("accepted");
}

#[test]
fn a_secret_in_args_is_refused() {
    let package = declaring_a_secret().mcp(serde_json::json!({
        "stdio": {
            "type": "stdio",
            "command": "weather-mcp",
            "args": ["--key", "${config.api_key}"],
        },
    }));
    assert!(refusal(&package).contains("as an env value or a header"));
}

#[test]
fn a_secret_in_the_url_is_refused() {
    let package = declaring_a_secret().mcp(serde_json::json!({
        "http": {
            "type": "streamable-http",
            "url": "https://weather.example.com/mcp?key=${config.api_key}",
        },
    }));
    assert!(refusal(&package).contains("as an env value or a header"));
}

#[test]
fn a_plain_field_reaches_args_and_the_url() {
    let package = Package::new()
        .manifest(serde_json::json!({
            "name": "weather",
            "extensions": {"pagis": {"config": {
                "region": {"type": "string", "title": "Region", "required": true},
            }}},
        }))
        .mcp(serde_json::json!({
            "stdio": {
                "type": "stdio",
                "command": "weather-mcp",
                "args": ["--region", "${config.region}"],
            },
        }));
    validate(package.root()).expect("accepted");
}

#[test]
fn a_reference_to_an_undeclared_field_is_refused() {
    let package = minimal().mcp(serde_json::json!({
        "stdio": {
            "type": "stdio",
            "command": "weather-mcp",
            "env": {"API_KEY": "${config.api_key}"},
        },
    }));
    assert!(refusal(&package).contains("undeclared field"));
}

#[test]
fn a_connection_field_names_a_provider_and_capabilities() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "account": {
                "type": "connection",
                "title": "Google account",
                "provider": "google",
                "capabilities": ["calendar.read"],
                "required": true,
            },
        }}},
    }));
    let read = validate(package.root()).expect("accepted");
    let field = read.pagis.config.get("account").expect("the field");
    assert_eq!(field.kind, FieldKind::Connection);
    assert_eq!(field.provider.as_deref(), Some("google"));
    assert_eq!(field.capabilities, vec!["calendar.read".to_string()]);
}

#[test]
fn a_connection_field_without_a_provider_is_refused() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "account": {"type": "connection", "title": "Account"},
        }}},
    }));
    assert!(refusal(&package).contains("names no provider"));
}

#[test]
fn a_config_field_of_an_unknown_type_is_refused() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "path": {"type": "file", "title": "A file"},
        }}},
    }));
    assert!(refusal(&package).contains("pagis extension"));
}

#[test]
fn a_declared_effect_is_read_and_every_other_tool_is_host() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"tools": {
            "forecast": {"effect": "free"},
        }}},
    }));
    let read = validate(package.root()).expect("accepted");
    assert_eq!(read.effect("forecast"), EffectClass::Free);
    assert_eq!(read.effect("purchase_umbrella"), EffectClass::Host);
}

#[test]
fn an_effect_that_is_not_a_class_is_refused() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"tools": {"forecast": {"effect": "harmless"}}}},
    }));
    assert!(refusal(&package).contains("pagis extension"));
}

#[test]
fn an_extension_of_another_client_is_ignored() {
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"com.example.client": {"anything": true}},
    }));
    validate(package.root()).expect("accepted");
}

#[test]
fn each_skill_directory_with_a_skill_file_is_one_skill() {
    let package = minimal()
        .file("skills/forecast/SKILL.md", "# Forecast")
        .file("skills/forecast/deeper/SKILL.md", "# Not a skill")
        .file("skills/notes/README.md", "no skill here");
    let read = validate(package.root()).expect("accepted");
    assert_eq!(
        read.skills,
        vec![pagis_core::Skill {
            plugin: "weather".to_string(),
            name: "forecast".to_string(),
            description: "Forecast".to_string(),
        }]
    );
}

#[test]
fn a_skill_carries_the_name_and_the_description_of_its_frontmatter() {
    let package = minimal().file(
        "skills/forecast/SKILL.md",
        "---\nname: forecast\ndescription: Read the weather forecast.\n---\n\n# Forecast\n\nSteps.\n",
    );
    let read = validate(package.root()).expect("accepted");
    assert_eq!(read.skills[0].name, "forecast");
    assert_eq!(read.skills[0].description, "Read the weather forecast.");
}

#[test]
fn a_description_over_two_hundred_characters_is_capped() {
    let long = "word ".repeat(80);
    let package = minimal().file(
        "skills/forecast/SKILL.md",
        &format!("---\ndescription: {long}\n---\n\n# Forecast\n"),
    );
    let read = validate(package.root()).expect("accepted");
    assert_eq!(
        read.skills[0].description.chars().count(),
        pagis_core::MAX_SKILL_DESCRIPTION
    );
}

#[test]
fn a_skill_that_declares_another_name_than_its_directory_is_refused() {
    let package = minimal().file(
        "skills/forecast/SKILL.md",
        "---\nname: weather-forecast\n---\n\n# Forecast\n",
    );
    assert!(refusal(&package).contains("a skill is named by its directory"));
}

#[test]
fn a_plugin_named_after_the_first_party_skills_is_refused() {
    let package = Package::new().manifest(serde_json::json!({"name": "pagis"}));
    assert!(refusal(&package).contains("is reserved"));
}

#[test]
fn an_mcp_file_that_is_not_json_is_refused() {
    let package = minimal().file("mcp.json", "{");
    assert!(refusal(&package).contains("mcp.json is not JSON"));
}

#[test]
fn an_mcp_file_without_the_matching_schema_is_refused() {
    let package = minimal().file(
        "mcp.json",
        &serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/0.9.0/mcp.schema.json",
            "mcpServers": {},
        })
        .to_string(),
    );
    assert!(refusal(&package).contains("mcp.json"));
}

#[test]
fn a_legacy_sse_server_is_read() {
    let package = minimal().file(
        "mcp.json",
        &serde_json::json!({
            "$schema": MCP_SCHEMA,
            "mcpServers": {
                "legacy": {"type": "sse", "url": "https://weather.example.com/sse"},
            },
        })
        .to_string(),
    );
    let read = validate(package.root()).expect("accepted");
    assert!(matches!(
        read.mcp.expect("servers").servers.get("legacy"),
        Some(McpServer::Sse { .. })
    ));
}

/// A package that declares one `secret` field and nothing else.
fn declaring_a_secret() -> Package {
    Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "api_key": {"type": "secret", "title": "API key", "required": true},
        }}},
    }))
}
