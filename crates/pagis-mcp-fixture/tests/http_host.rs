//! The MCP host over streamable HTTP on the loopback interface
//! (ADR-0017). The rules that differ from stdio are here: the address
//! and the headers are built per request, and a `secret` Binding
//! reaches the server as a header and never as a process environment.

use crate::support;

use pagis_plugin::{BindingInput, BindingValueInput};
use support::{Harness, http_plugin};

/// The fixture server on a loopback port of its own.
async fn serve() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = listener.local_addr().expect("the address").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, pagis_mcp_fixture::http_router()).await;
    });
    port
}

fn bindings(port: u16, token: &str) -> Vec<BindingInput> {
    vec![
        BindingInput {
            field: "endpoint".to_string(),
            value: BindingValueInput::Value {
                value: serde_json::Value::String(port.to_string()),
            },
        },
        BindingInput {
            field: "token".to_string(),
            value: BindingValueInput::Secret {
                secret: token.to_string(),
            },
        },
    ]
}

#[tokio::test]
async fn an_install_freezes_the_list_of_an_http_server() {
    let harness = Harness::new();
    let directory = http_plugin();
    let port = serve().await;

    let plugin = harness
        .install(directory.path(), &bindings(port, "the-token"))
        .await;

    let frozen = harness.catalog(&plugin).await;
    assert!(frozen.tools.iter().any(|tool| tool.name == "echo"));
    assert!(frozen.tools.iter().all(|tool| tool.server == "fixture"));
}

#[tokio::test]
async fn a_call_reaches_the_server_over_streamable_http() {
    let harness = Harness::new();
    let directory = http_plugin();
    let port = serve().await;
    let plugin = harness
        .install(directory.path(), &bindings(port, "the-token"))
        .await;

    let result = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "over http"}))
        .await;

    assert!(!result.is_error, "{result:?}");
    assert_eq!(result.content, "over http");
}

#[tokio::test]
async fn the_secret_reaches_an_http_server_as_a_header() {
    let harness = Harness::new();
    let directory = http_plugin();
    let port = serve().await;
    let plugin = harness
        .install(directory.path(), &bindings(port, "the-token"))
        .await;

    let result = harness
        .dispatch(&plugin, "headers", serde_json::json!({}))
        .await;

    let held: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&result.content).expect("the headers are JSON");
    assert_eq!(
        held.get("authorization").map(String::as_str),
        Some("Bearer the-token")
    );
}
