//! The two Grants one Agent needs to call a Plugin tool
//! (ADR-0017): the Plugin Grant says "this Agent may run this code",
//! and the Grant on each bound Connection says "this Agent may touch
//! this account". Connection Grants stay the one truth for account
//! access, so a Plugin never widens one.
//!
//! The check is a pure function over the records the broker already
//! holds at dispatch, so it needs no store and no manifest.

use pagis_core::{Connection, Grant, Plugin, PluginBinding, PluginBindingValue, PluginState};

/// Why one Agent may not call a Plugin now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccessDenied {
    #[error("the plugin {0} is not enabled")]
    NotEnabled(String),
    #[error("the agent holds no grant on the plugin {0}")]
    NoPluginGrant(String),
    /// A bound Connection is gone from the Workspace, or it is not
    /// authorized now.
    #[error("the connection bound to {0} is not available")]
    ConnectionUnavailable(String),
    #[error("the agent holds no grant on the connection bound to {0}")]
    NoConnectionGrant(String),
    #[error("the grant on the connection bound to {field} is missing {capability}")]
    MissingCapability { field: String, capability: String },
}

/// Whether one Agent may call `plugin` now. `grants` are the Agent's
/// live Grants and `connections` are the Workspace's Connections, as
/// the broker reads them at dispatch (ADR-0005).
pub fn check_access(
    plugin: &Plugin,
    bindings: &[PluginBinding],
    grants: &[Grant],
    connections: &[Connection],
) -> Result<(), AccessDenied> {
    if plugin.state != PluginState::Enabled {
        return Err(AccessDenied::NotEnabled(plugin.name.clone()));
    }
    let holds_plugin = grants.iter().any(|grant| {
        grant.revoked_at.is_none()
            && grant.resource_kind == Grant::PLUGIN_KIND
            && grant.resource_id.as_deref() == Some(plugin.id.as_str())
    });
    if !holds_plugin {
        return Err(AccessDenied::NoPluginGrant(plugin.name.clone()));
    }

    for binding in bindings {
        let PluginBindingValue::Connection {
            connection_id,
            capabilities,
        } = &binding.value
        else {
            continue;
        };
        let available = connections.iter().any(|connection| {
            &connection.id == connection_id && connection.status == Connection::CONNECTED
        });
        if !available {
            return Err(AccessDenied::ConnectionUnavailable(binding.field.clone()));
        }
        let grant = grants
            .iter()
            .find(|grant| {
                grant.revoked_at.is_none()
                    && grant.resource_kind == Grant::CONNECTION_KIND
                    && grant.resource_id.as_deref() == Some(connection_id.as_str())
            })
            .ok_or_else(|| AccessDenied::NoConnectionGrant(binding.field.clone()))?;
        let held = grant.capabilities();
        for capability in capabilities {
            if !held.contains(capability) {
                return Err(AccessDenied::MissingCapability {
                    field: binding.field.clone(),
                    capability: capability.clone(),
                });
            }
        }
    }
    Ok(())
}
