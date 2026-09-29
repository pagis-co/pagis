//! The install path of a Plugin (ADR-0017): fetch, validate,
//! bind, commit, and the update and uninstall that follow.
//!
//! Every act here is the user's. A Plugin is never installed, updated
//! or removed by an Agent, because installing one is a decision about
//! which code may run on this computer (ADR-0017).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{
    Connection, ConnectionId, ConnectionStore, EventBus, Grant, GrantId, GrantStore, NewEvent,
    Plugin, PluginBinding, PluginBindingValue, PluginId, PluginSource, PluginState, PluginStore,
    SecretStore, StoreError, WorkspaceId, now_ms,
};

use crate::git::{DiffSummary, GitStoreError, PluginGitStore};
use crate::manifest::{ConfigField, FieldKind, PluginPackage};
use crate::source::{SourceError, SourceInput, fetch};
use crate::validate::{ValidationErrors, validate};

/// The namespaces the broker already holds for one tenant. A Plugin
/// name may not collide with one, because two manifests with one
/// namespace would fight over the same tool names (ADR-0005,
/// ADR-0017). The registry is per Workspace, so the question is about
/// one tenant and never about the installation.
pub trait Namespaces: Send + Sync {
    fn holds(&self, workspace_id: &WorkspaceId, name: &str) -> bool;
}

impl Namespaces for pagis_broker::Broker {
    fn holds(&self, workspace_id: &WorkspaceId, name: &str) -> bool {
        self.namespaces(workspace_id).contains(name)
    }
}

/// The MCP host of the installed Plugins (ADR-0017). The
/// install path reaches it three ways: it freezes the tool list of a
/// state that is ready to serve, it stops a Plugin's servers before
/// the files under them move, and it forgets a Plugin the user
/// removed.
#[async_trait]
pub trait PluginServers: Send + Sync {
    /// Start every declared server once, take `tools/list`, and keep
    /// the answer as the Capability Manifest of this state. A state
    /// that already has a frozen list only installs its manifest.
    async fn freeze(
        &self,
        plugin: &Plugin,
        package: &PluginPackage,
        bindings: &[PluginBinding],
    ) -> Result<(), String>;
    async fn stop(&self, plugin_id: &PluginId);
    /// Stop the servers and drop everything the host holds about one
    /// Plugin: the frozen lists and the server log.
    async fn forget(&self, plugin_id: &PluginId);
}

/// Why an install, update, bind or uninstall did not happen.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("no such plugin")]
    NotFound,
    /// The name is taken, or the Plugin already stands where the
    /// caller wants to put it.
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Refused(String),
    #[error("the plugin package is not valid:\n{0}")]
    Invalid(#[from] ValidationErrors),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("{0}")]
    Storage(String),
}

impl From<StoreError> for PluginError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Conflict(message) => PluginError::Conflict(message),
            other => PluginError::Storage(other.to_string()),
        }
    }
}

impl From<GitStoreError> for PluginError {
    fn from(error: GitStoreError) -> Self {
        match error {
            GitStoreError::NoChange => PluginError::Refused("the plugin is unchanged".to_string()),
            GitStoreError::Storage(message) => PluginError::Storage(message),
        }
    }
}

/// What the user binds one declared field to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInput {
    pub field: String,
    pub value: BindingValueInput,
}

/// The value side of a Binding, as the user supplies it. A secret
/// arrives here once and goes straight to the secret store; the desk
/// never reads it back (ADR-0017).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingValueInput {
    Connection { connection_id: ConnectionId },
    Secret { secret: String },
    Value { value: serde_json::Value },
}

/// One installed Plugin with everything the install card shows: the
/// record, the Bindings, and the package the state carries.
#[derive(Debug, Clone)]
pub struct Installed {
    pub plugin: Plugin,
    pub bindings: Vec<PluginBinding>,
    pub package: PluginPackage,
    /// What an update changed. It is empty for a first install.
    pub changed: DiffSummary,
}

pub struct PluginsDeps {
    /// The Plugin records. The Org's Workspace holds them.
    pub plugins: Arc<dyn PluginStore>,
    pub connections: Arc<dyn ConnectionStore>,
    pub grants: Arc<dyn GrantStore>,
    /// Where a `secret` Binding's value goes (ADR-0013).
    pub secrets: Arc<dyn SecretStore>,
    pub git: Arc<PluginGitStore>,
    pub namespaces: Arc<dyn Namespaces>,
    pub servers: Arc<dyn PluginServers>,
    pub bus: Arc<dyn EventBus>,
}

/// The installed Plugins of one Workspace: the one path that writes
/// them.
pub struct Plugins {
    deps: PluginsDeps,
}

impl Plugins {
    pub fn new(deps: PluginsDeps) -> Self {
        Self { deps }
    }

    /// Install one Plugin from a git URL or an upload. A required
    /// field with no usable Binding leaves the Plugin `disabled`; the
    /// user binds it later and the Plugin enables itself.
    pub async fn install(
        &self,
        workspace_id: &WorkspaceId,
        source: &SourceInput,
        bindings: &[BindingInput],
    ) -> Result<Installed, PluginError> {
        let scratch =
            tempfile::tempdir().map_err(|error| PluginError::Storage(error.to_string()))?;
        let tree = scratch.path().join("plugin");
        std::fs::create_dir(&tree).map_err(|error| PluginError::Storage(error.to_string()))?;
        let recorded = fetch(source, &tree).await?;
        let package = validate(&tree)?;
        let name = package.manifest.name.clone();

        self.check_name_free(workspace_id, &name).await?;

        let id = PluginId::generate();
        let (bindings, state) = self.resolve(workspace_id, &id, &package, bindings).await?;

        let commit = self
            .deps
            .git
            .install_state(workspace_id, id.as_str(), &tree, "v1")
            .await?;
        let now = now_ms();
        let plugin = Plugin {
            id,
            workspace_id: workspace_id.clone(),
            name: name.clone(),
            source: recorded,
            installed_commit: commit,
            manifest_version: "v1".to_string(),
            state,
            created_at: now,
            updated_at: now,
        };
        if let Err(error) = self.deps.plugins.create(&plugin, &bindings).await {
            // The name was taken between the check and the write. The
            // directories are the daemon's alone, so they go with it.
            self.deps
                .git
                .remove(workspace_id, plugin.id.as_str())
                .await
                .ok();
            return Err(error.into());
        }
        // The tool list is frozen before the Plugin is offered: two
        // servers that share a tool name, or a name no model provider
        // can carry, fail the install rather than the first call
        // (ADR-0017). A Plugin that is not ready to serve freezes when
        // its last Binding arrives.
        if state == PluginState::Enabled
            && let Err(problem) = self.deps.servers.freeze(&plugin, &package, &bindings).await
        {
            self.rollback(workspace_id, &plugin).await;
            return Err(PluginError::Refused(problem));
        }
        self.publish(&plugin, "plugin.installed").await;
        Ok(Installed {
            plugin,
            bindings,
            package,
            changed: DiffSummary::default(),
        })
    }

    /// Install the next state of one Plugin. A git Plugin is read
    /// again from its recorded address; an uploaded Plugin needs the
    /// new upload. The answer names what changed (ADR-0017).
    pub async fn update(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        upload: Option<Vec<u8>>,
    ) -> Result<Installed, PluginError> {
        let plugin = self.plugin(workspace_id, id).await?;
        let source = match (&plugin.source, upload) {
            (PluginSource::Git { url, reference }, None) => SourceInput::Git {
                url: url.clone(),
                reference: reference.clone(),
            },
            (PluginSource::Git { .. }, Some(_)) => {
                return Err(PluginError::Refused(
                    "this plugin is installed from a repository; it updates from there".to_string(),
                ));
            }
            (PluginSource::Upload, Some(tar)) => SourceInput::Upload { tar },
            (PluginSource::Upload, None) => {
                return Err(PluginError::Refused(
                    "this plugin was uploaded; an update needs a new upload".to_string(),
                ));
            }
        };

        let scratch =
            tempfile::tempdir().map_err(|error| PluginError::Storage(error.to_string()))?;
        let tree = scratch.path().join("plugin");
        std::fs::create_dir(&tree).map_err(|error| PluginError::Storage(error.to_string()))?;
        fetch(&source, &tree).await?;
        let package = validate(&tree)?;
        if package.manifest.name != plugin.name {
            return Err(PluginError::Refused(format!(
                "the update names the plugin {:?}, not {:?}; a renamed plugin is another plugin",
                package.manifest.name, plugin.name
            )));
        }

        // The servers of the old state stop before its files go, so no
        // running server reads a tree that is no longer installed.
        self.deps.servers.stop(&plugin.id).await;
        let previous = plugin.installed_commit.clone();
        let version = next_manifest_version(&plugin.manifest_version);
        let commit = self
            .deps
            .git
            .install_state(workspace_id, plugin.id.as_str(), &tree, &version)
            .await?;
        let changed = self
            .deps
            .git
            .diff(workspace_id, plugin.id.as_str(), &previous, &commit)
            .await?;

        let held = self
            .deps
            .plugins
            .list_bindings(workspace_id, &plugin.id)
            .await?;
        let (bindings, dropped) = keep_bindings(&package, held);
        for binding in &dropped {
            self.deps
                .plugins
                .delete_binding(workspace_id, &plugin.id, &binding.field)
                .await?;
            if let PluginBindingValue::Secret { secret_name } = &binding.value {
                self.deps.secrets.delete(secret_name).ok();
            }
        }
        let state = self.state_of(workspace_id, &package, &bindings).await?;
        self.deps
            .plugins
            .set_installed(workspace_id, &plugin.id, &commit, &version, state, now_ms())
            .await?;
        let plugin = Plugin {
            installed_commit: commit,
            manifest_version: version,
            state,
            updated_at: now_ms(),
            ..plugin
        };
        // The update mints the next Capability Manifest version, so
        // the freeze reads the new state's servers (ADR-0017).
        if state == PluginState::Enabled
            && let Err(problem) = self.deps.servers.freeze(&plugin, &package, &bindings).await
        {
            return Err(PluginError::Refused(problem));
        }
        self.publish(&plugin, "plugin.updated").await;
        Ok(Installed {
            plugin,
            bindings,
            package,
            changed,
        })
    }

    /// Stop the servers, revoke every Grant, forget the secrets and
    /// delete the repository, the checkout and the data directory. The
    /// audit rows stand alone and are not touched (ADR-0017).
    pub async fn uninstall(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<(), PluginError> {
        let plugin = self.plugin(workspace_id, id).await?;
        self.deps.servers.forget(&plugin.id).await;
        let bindings = self
            .deps
            .plugins
            .list_bindings(workspace_id, &plugin.id)
            .await?;
        let removed = self
            .deps
            .plugins
            .delete_and_revoke(workspace_id, &plugin.id, now_ms())
            .await?;
        if !removed {
            return Err(PluginError::NotFound);
        }
        for binding in &bindings {
            if let PluginBindingValue::Secret { secret_name } = &binding.value {
                self.deps.secrets.delete(secret_name).ok();
            }
        }
        self.deps
            .git
            .remove(workspace_id, plugin.id.as_str())
            .await?;
        self.publish(&plugin, "plugin.uninstalled").await;
        Ok(())
    }

    /// Bind one declared field after the install, and move the Plugin
    /// to the state the new set of Bindings allows.
    pub async fn bind(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        input: &BindingInput,
    ) -> Result<Installed, PluginError> {
        let plugin = self.plugin(workspace_id, id).await?;
        let package = self.package(workspace_id, &plugin)?;
        let Some(field) = package.pagis.config.get(&input.field) else {
            return Err(PluginError::Refused(format!(
                "the plugin declares no field {:?}",
                input.field
            )));
        };
        let binding = self
            .binding(workspace_id, &plugin.id, &input.field, field, &input.value)
            .await?;
        self.deps
            .plugins
            .put_binding(workspace_id, &binding)
            .await?;
        // A bound value reaches a server at spawn, so the running
        // servers are older than the binding.
        self.deps.servers.stop(&plugin.id).await;

        let bindings = self
            .deps
            .plugins
            .list_bindings(workspace_id, &plugin.id)
            .await?;
        let state = self.state_of(workspace_id, &package, &bindings).await?;
        self.deps
            .plugins
            .set_state(workspace_id, &plugin.id, state, now_ms())
            .await?;
        let plugin = Plugin { state, ..plugin };
        // A Plugin that installed without every required Binding
        // freezes its tool list here, at the moment it can serve.
        if state == PluginState::Enabled
            && let Err(problem) = self.deps.servers.freeze(&plugin, &package, &bindings).await
        {
            return Err(PluginError::Refused(problem));
        }
        Ok(Installed {
            plugin,
            bindings,
            package,
            changed: DiffSummary::default(),
        })
    }

    /// Grant one Agent the Plugin (ADR-0017). A Plugin is one trust
    /// unit, so the Grant carries no scope.
    ///
    /// The Plugin is the Org's install, so it is read from the Org's
    /// Workspace; the Grant belongs to the Agent's own tenant, because
    /// which sprite may call a plugin tool is each person's own answer.
    pub async fn grant(
        &self,
        org_workspace_id: &WorkspaceId,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        agent_id: &pagis_core::AgentId,
    ) -> Result<Grant, PluginError> {
        let plugin = self.plugin(org_workspace_id, id).await?;
        let grant = Grant {
            id: GrantId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            resource_kind: Grant::PLUGIN_KIND.to_string(),
            resource_id: Some(plugin.id.to_string()),
            scope: serde_json::json!({}),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        };
        self.deps.grants.create(&grant).await?;
        Ok(grant)
    }

    /// One installed Plugin with its Bindings and its package.
    pub async fn installed(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Installed, PluginError> {
        let plugin = self.plugin(workspace_id, id).await?;
        let package = self.package(workspace_id, &plugin)?;
        let bindings = self
            .deps
            .plugins
            .list_bindings(workspace_id, &plugin.id)
            .await?;
        Ok(Installed {
            plugin,
            bindings,
            package,
            changed: DiffSummary::default(),
        })
    }

    /// Every installed Plugin of the Workspace, by name.
    pub async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<Plugin>, PluginError> {
        Ok(self.deps.plugins.list(workspace_id).await?)
    }

    /// The package of the installed state, read from the checkout.
    pub fn package(
        &self,
        workspace_id: &WorkspaceId,
        plugin: &Plugin,
    ) -> Result<PluginPackage, PluginError> {
        let paths = self.deps.git.paths(workspace_id, plugin.id.as_str());
        Ok(validate(&paths.root)?)
    }

    /// Take back a first install that could not be finished. The
    /// record, the secrets and the directories go, so a refused
    /// install leaves nothing behind (ADR-0017).
    async fn rollback(&self, workspace_id: &WorkspaceId, plugin: &Plugin) {
        self.deps.servers.forget(&plugin.id).await;
        if let Ok(bindings) = self
            .deps
            .plugins
            .list_bindings(workspace_id, &plugin.id)
            .await
        {
            for binding in &bindings {
                if let PluginBindingValue::Secret { secret_name } = &binding.value {
                    self.deps.secrets.delete(secret_name).ok();
                }
            }
        }
        self.deps
            .plugins
            .delete_and_revoke(workspace_id, &plugin.id, now_ms())
            .await
            .ok();
        self.deps
            .git
            .remove(workspace_id, plugin.id.as_str())
            .await
            .ok();
    }

    async fn plugin(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
    ) -> Result<Plugin, PluginError> {
        self.deps
            .plugins
            .get(workspace_id, id)
            .await?
            .ok_or(PluginError::NotFound)
    }

    async fn check_name_free(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<(), PluginError> {
        if self.deps.namespaces.holds(workspace_id, name) {
            return Err(PluginError::Conflict(format!(
                "the name {name:?} belongs to another set of tools"
            )));
        }
        if self
            .deps
            .plugins
            .get_by_name(workspace_id, name)
            .await?
            .is_some()
        {
            return Err(PluginError::Conflict(format!(
                "the plugin {name:?} is installed"
            )));
        }
        Ok(())
    }

    /// Turn what the user supplied into Bindings, and say which state
    /// the result leaves the Plugin in.
    async fn resolve(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        package: &PluginPackage,
        inputs: &[BindingInput],
    ) -> Result<(Vec<PluginBinding>, PluginState), PluginError> {
        let mut bindings = Vec::new();
        for input in inputs {
            let Some(field) = package.pagis.config.get(&input.field) else {
                return Err(PluginError::Refused(format!(
                    "the plugin declares no field {:?}",
                    input.field
                )));
            };
            if bindings
                .iter()
                .any(|held: &PluginBinding| held.field == input.field)
            {
                return Err(PluginError::Refused(format!(
                    "the field {:?} is bound twice",
                    input.field
                )));
            }
            bindings.push(
                self.binding(workspace_id, id, &input.field, field, &input.value)
                    .await?,
            );
        }
        let state = self.state_of(workspace_id, package, &bindings).await?;
        Ok((bindings, state))
    }

    /// One Binding, checked against the field it belongs to. A secret
    /// goes to the secret store here and never into the record.
    async fn binding(
        &self,
        workspace_id: &WorkspaceId,
        id: &PluginId,
        field_name: &str,
        field: &ConfigField,
        input: &BindingValueInput,
    ) -> Result<PluginBinding, PluginError> {
        let value = match (field.kind, input) {
            (FieldKind::Connection, BindingValueInput::Connection { connection_id }) => {
                let connection = self
                    .deps
                    .connections
                    .get(workspace_id, connection_id)
                    .await?
                    .ok_or_else(|| PluginError::Refused("no such connection".to_string()))?;
                let wanted = field.provider.as_deref().unwrap_or_default();
                if connection.provider != wanted {
                    return Err(PluginError::Refused(format!(
                        "the field {field_name:?} needs a {wanted} connection, not a {}",
                        connection.provider
                    )));
                }
                PluginBindingValue::Connection {
                    connection_id: connection.id,
                    capabilities: field.capabilities.clone(),
                }
            }
            (FieldKind::Secret, BindingValueInput::Secret { secret }) => {
                let secret_name = secret_name(id, field_name);
                self.deps
                    .secrets
                    .set(&secret_name, secret)
                    .map_err(|error| PluginError::Storage(error.to_string()))?;
                PluginBindingValue::Secret { secret_name }
            }
            (FieldKind::String, BindingValueInput::Value { value }) if value.is_string() => {
                PluginBindingValue::Value {
                    value: value.clone(),
                }
            }
            (FieldKind::Number, BindingValueInput::Value { value }) if value.is_number() => {
                PluginBindingValue::Value {
                    value: value.clone(),
                }
            }
            (FieldKind::Boolean, BindingValueInput::Value { value }) if value.is_boolean() => {
                PluginBindingValue::Value {
                    value: value.clone(),
                }
            }
            (kind, _) => {
                return Err(PluginError::Refused(format!(
                    "the field {field_name:?} takes a {} value",
                    kind.as_str()
                )));
            }
        };
        Ok(PluginBinding {
            plugin_id: id.clone(),
            field: field_name.to_string(),
            value,
        })
    }

    /// `enabled` when every required field is bound and every bound
    /// Connection is still there; `disabled` otherwise (ADR-0017).
    async fn state_of(
        &self,
        workspace_id: &WorkspaceId,
        package: &PluginPackage,
        bindings: &[PluginBinding],
    ) -> Result<PluginState, PluginError> {
        let by_field: BTreeMap<&str, &PluginBinding> = bindings
            .iter()
            .map(|binding| (binding.field.as_str(), binding))
            .collect();
        for (name, field) in &package.pagis.config {
            if field.required && !by_field.contains_key(name.as_str()) {
                return Ok(PluginState::Disabled);
            }
        }
        let connections = self.deps.connections.list(workspace_id).await?;
        for binding in bindings {
            if let PluginBindingValue::Connection { connection_id, .. } = &binding.value
                && !connections
                    .iter()
                    .any(|connection: &Connection| &connection.id == connection_id)
            {
                return Ok(PluginState::Disabled);
            }
        }
        Ok(PluginState::Enabled)
    }

    async fn publish(&self, plugin: &Plugin, event_type: &str) {
        let event = NewEvent {
            workspace_id: plugin.workspace_id.clone(),
            event_type: event_type.to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "plugin_id": plugin.id.as_str(),
                "name": plugin.name,
                "installed_commit": plugin.installed_commit,
                "manifest_version": plugin.manifest_version,
                "state": plugin.state.as_str(),
            }),
        };
        if let Err(error) = self.deps.bus.publish(event).await {
            tracing::warn!(%error, plugin = %plugin.name, "cannot publish the plugin event");
        }
    }
}

/// The secret store name of one `secret` Binding. The Plugin id is in
/// the name, so two Plugins never share a value.
fn secret_name(id: &PluginId, field: &str) -> String {
    format!("plugin/{id}/{field}")
}

/// Split the held Bindings into those an update keeps and those it
/// drops. A Binding is kept when the new state declares its field with
/// the same kind; every other Binding names a field that is gone.
fn keep_bindings(
    package: &PluginPackage,
    held: Vec<PluginBinding>,
) -> (Vec<PluginBinding>, Vec<PluginBinding>) {
    held.into_iter().partition(|binding| {
        package
            .pagis
            .config
            .get(&binding.field)
            .is_some_and(|field| {
                matches!(
                    (&binding.value, field.kind),
                    (PluginBindingValue::Connection { .. }, FieldKind::Connection)
                        | (PluginBindingValue::Secret { .. }, FieldKind::Secret)
                        | (
                            PluginBindingValue::Value { .. },
                            FieldKind::String | FieldKind::Number | FieldKind::Boolean,
                        )
                )
            })
    })
}

/// The Capability Manifest version after `current`: `v1`, `v2` and so
/// on. The daemon mints it, so a plugin author cannot pick one
/// (ADR-0016).
pub fn next_manifest_version(current: &str) -> String {
    let number = current
        .strip_prefix('v')
        .and_then(|digits| digits.parse::<u64>().ok())
        .unwrap_or(0);
    format!("v{}", number + 1)
}
