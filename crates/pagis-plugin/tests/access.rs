//! The two Grants a Plugin call needs (ADR-0017): the Plugin
//! Grant and the Grant on every bound Connection, with the
//! capabilities the Plugin declared.

use pagis_core::{
    AgentId, Connection, ConnectionId, Grant, GrantId, Plugin, PluginBinding, PluginBindingValue,
    PluginId, PluginSource, PluginState, WorkspaceId, now_ms,
};
use pagis_plugin::{AccessDenied, check_access};

struct Fixture {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    plugin: Plugin,
    connection: Connection,
}

impl Fixture {
    fn new() -> Self {
        let workspace_id = WorkspaceId::generate();
        Self {
            agent_id: AgentId::generate(),
            plugin: Plugin {
                id: PluginId::generate(),
                workspace_id: workspace_id.clone(),
                name: "weather".to_string(),
                source: PluginSource::Upload,
                installed_commit: "0".repeat(40),
                manifest_version: "v1".to_string(),
                state: PluginState::Enabled,
                created_at: now_ms(),
                updated_at: now_ms(),
            },
            connection: Connection {
                id: ConnectionId::generate(),
                workspace_id: workspace_id.clone(),
                provider: "google".to_string(),
                alias: "work".to_string(),
                display_name: "Work".to_string(),
                status: Connection::CONNECTED.to_string(),
                auth_mode: Connection::AUTH_MODE_BYO.to_string(),
                authorized_capabilities: vec!["calendar.read".to_string()],
                config: serde_json::json!({}),
                created_at: now_ms(),
            },
            workspace_id,
        }
    }

    /// The Binding that makes the Plugin need the Connection Grant.
    fn binding(&self, capabilities: &[&str]) -> PluginBinding {
        PluginBinding {
            plugin_id: self.plugin.id.clone(),
            field: "account".to_string(),
            value: PluginBindingValue::Connection {
                connection_id: self.connection.id.clone(),
                capabilities: capabilities.iter().map(|held| held.to_string()).collect(),
            },
        }
    }

    fn plugin_grant(&self) -> Grant {
        self.grant(Grant::PLUGIN_KIND, self.plugin.id.to_string(), &[])
    }

    fn connection_grant(&self, capabilities: &[&str]) -> Grant {
        self.grant(
            Grant::CONNECTION_KIND,
            self.connection.id.to_string(),
            capabilities,
        )
    }

    fn grant(&self, kind: &str, resource_id: String, capabilities: &[&str]) -> Grant {
        Grant {
            id: GrantId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            resource_kind: kind.to_string(),
            resource_id: Some(resource_id),
            scope: Grant::connection_scope(
                &capabilities
                    .iter()
                    .map(|held| held.to_string())
                    .collect::<Vec<_>>(),
            ),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        }
    }
}

#[test]
fn both_grants_together_allow_the_call() {
    let fixture = Fixture::new();

    check_access(
        &fixture.plugin,
        &[fixture.binding(&["calendar.read"])],
        &[
            fixture.plugin_grant(),
            fixture.connection_grant(&["calendar.read"]),
        ],
        std::slice::from_ref(&fixture.connection),
    )
    .expect("allowed");
}

#[test]
fn a_plugin_without_bindings_needs_only_the_plugin_grant() {
    let fixture = Fixture::new();

    check_access(&fixture.plugin, &[], &[fixture.plugin_grant()], &[]).expect("allowed");
}

#[test]
fn no_plugin_grant_refuses_the_call() {
    let fixture = Fixture::new();

    let refused = check_access(
        &fixture.plugin,
        &[fixture.binding(&["calendar.read"])],
        &[fixture.connection_grant(&["calendar.read"])],
        std::slice::from_ref(&fixture.connection),
    );

    assert_eq!(
        refused,
        Err(AccessDenied::NoPluginGrant("weather".to_string()))
    );
}

#[test]
fn no_connection_grant_refuses_the_call() {
    let fixture = Fixture::new();

    let refused = check_access(
        &fixture.plugin,
        &[fixture.binding(&["calendar.read"])],
        &[fixture.plugin_grant()],
        std::slice::from_ref(&fixture.connection),
    );

    assert_eq!(
        refused,
        Err(AccessDenied::NoConnectionGrant("account".to_string()))
    );
}

#[test]
fn a_connection_grant_without_the_declared_capability_refuses_the_call() {
    let fixture = Fixture::new();

    let refused = check_access(
        &fixture.plugin,
        &[fixture.binding(&["calendar.read", "calendar.write"])],
        &[
            fixture.plugin_grant(),
            fixture.connection_grant(&["calendar.read"]),
        ],
        std::slice::from_ref(&fixture.connection),
    );

    assert_eq!(
        refused,
        Err(AccessDenied::MissingCapability {
            field: "account".to_string(),
            capability: "calendar.write".to_string(),
        })
    );
}

#[test]
fn a_revoked_plugin_grant_refuses_the_call() {
    let fixture = Fixture::new();
    let revoked = Grant {
        revoked_at: Some(now_ms()),
        ..fixture.plugin_grant()
    };

    let refused = check_access(&fixture.plugin, &[], &[revoked], &[]);

    assert_eq!(
        refused,
        Err(AccessDenied::NoPluginGrant("weather".to_string()))
    );
}

#[test]
fn a_connection_that_needs_authorization_again_refuses_the_call() {
    let fixture = Fixture::new();
    let connection = Connection {
        status: Connection::REAUTH_REQUIRED.to_string(),
        ..fixture.connection.clone()
    };

    let refused = check_access(
        &fixture.plugin,
        &[fixture.binding(&["calendar.read"])],
        &[
            fixture.plugin_grant(),
            fixture.connection_grant(&["calendar.read"]),
        ],
        &[connection],
    );

    assert_eq!(
        refused,
        Err(AccessDenied::ConnectionUnavailable("account".to_string()))
    );
}

#[test]
fn a_plugin_that_is_not_enabled_refuses_the_call() {
    let fixture = Fixture::new();
    let plugin = Plugin {
        state: PluginState::Disabled,
        ..fixture.plugin.clone()
    };

    let refused = check_access(&plugin, &[], &[fixture.plugin_grant()], &[]);

    assert_eq!(
        refused,
        Err(AccessDenied::NotEnabled("weather".to_string()))
    );
}
