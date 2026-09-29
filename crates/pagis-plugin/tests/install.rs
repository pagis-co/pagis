//! The install path (ADR-0017): the sources, the Bindings the
//! user makes, the state a missing Connection leaves, the update that
//! shows what changed, and the uninstall that leaves nothing behind.

use crate::support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    Connection, ConnectionId, ConnectionStore, EventBus, EventScope, EventStream, GrantStore,
    MemorySecretStore, NewEvent, Plugin, PluginBindingValue, PluginId, PluginSource, PluginState,
    PluginStore, SecretStore, StoreError, WorkspaceId, now_ms,
};
use pagis_plugin::{
    BindingInput, BindingValueInput, ChangeStatus, Namespaces, PluginError, PluginGitStore,
    PluginServers, Plugins, PluginsDeps, SourceInput, next_manifest_version,
};
use pagis_testkit::{MemoryConnectionStore, MemoryGrantStore, MemoryPluginStore};
use support::{Package, minimal};

/// A bus that stores nothing: these tests read records, not the feed.
struct SilentBus;

#[async_trait]
impl EventBus for SilentBus {
    async fn publish(&self, event: NewEvent) -> Result<pagis_core::Event, StoreError> {
        Ok(pagis_core::Event {
            id: pagis_core::EventId::generate(),
            seq: 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// The namespaces the broker holds, as a test fixes them.
struct HeldNamespaces(Vec<String>);

impl Namespaces for HeldNamespaces {
    fn holds(&self, _workspace_id: &WorkspaceId, name: &str) -> bool {
        self.0.iter().any(|held| held == name)
    }
}

/// An MCP host that records what the install path asked of it, and
/// starts nothing.
#[derive(Default)]
struct StoppedServers {
    stopped: Mutex<Vec<String>>,
    frozen: Mutex<Vec<String>>,
    forgotten: Mutex<Vec<String>>,
    /// What the next freeze answers. `None` is a freeze that works.
    refusal: Mutex<Option<String>>,
}

#[async_trait]
impl PluginServers for StoppedServers {
    async fn freeze(
        &self,
        plugin: &pagis_core::Plugin,
        _package: &pagis_plugin::PluginPackage,
        _bindings: &[pagis_core::PluginBinding],
    ) -> Result<(), String> {
        self.frozen
            .lock()
            .expect("frozen lock")
            .push(plugin.id.to_string());
        match self.refusal.lock().expect("refusal lock").clone() {
            Some(problem) => Err(problem),
            None => Ok(()),
        }
    }

    async fn stop(&self, plugin_id: &PluginId) {
        self.stopped
            .lock()
            .expect("stopped lock")
            .push(plugin_id.to_string());
    }

    async fn forget(&self, plugin_id: &PluginId) {
        self.stop(plugin_id).await;
        self.forgotten
            .lock()
            .expect("forgotten lock")
            .push(plugin_id.to_string());
    }
}

struct Harness {
    plugins: Plugins,
    workspace_id: WorkspaceId,
    store: Arc<MemoryPluginStore>,
    grants: Arc<MemoryGrantStore>,
    connections: Arc<MemoryConnectionStore>,
    secrets: Arc<MemorySecretStore>,
    servers: Arc<StoppedServers>,
    git: Arc<PluginGitStore>,
    _home: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::holding(Vec::new())
    }

    /// A daemon whose broker already holds these namespaces.
    fn holding(namespaces: Vec<String>) -> Self {
        let home = tempfile::tempdir().expect("a data root");
        let workspace_id = WorkspaceId::generate();
        let grants = Arc::new(MemoryGrantStore::default());
        let connections = Arc::new(MemoryConnectionStore::new(Arc::clone(&grants) as _));
        let store = Arc::new(MemoryPluginStore::new(Arc::clone(&grants) as _));
        let secrets = Arc::new(MemorySecretStore::default());
        let servers = Arc::new(StoppedServers::default());
        let git = Arc::new(PluginGitStore::new(home.path().join("plugins")));
        let plugins = Plugins::new(PluginsDeps {
            plugins: Arc::clone(&store) as _,
            connections: Arc::clone(&connections) as _,
            grants: Arc::clone(&grants) as _,
            secrets: Arc::clone(&secrets) as _,
            git: Arc::clone(&git),
            namespaces: Arc::new(HeldNamespaces(namespaces)),
            servers: Arc::clone(&servers) as _,
            bus: Arc::new(SilentBus),
        });
        Self {
            plugins,
            workspace_id,
            store,
            grants,
            connections,
            secrets,
            servers,
            git,
            _home: home,
        }
    }

    /// One authorized Connection of a provider.
    async fn connection(&self, provider: &str, alias: &str) -> ConnectionId {
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: self.workspace_id.clone(),
            provider: provider.to_string(),
            alias: alias.to_string(),
            display_name: alias.to_string(),
            status: Connection::CONNECTED.to_string(),
            auth_mode: Connection::AUTH_MODE_BYO.to_string(),
            authorized_capabilities: vec!["calendar.read".to_string()],
            config: serde_json::json!({}),
            created_at: now_ms(),
        };
        let id = connection.id.clone();
        self.connections.create(&connection).await.expect("created");
        id
    }
}

/// A package with one secret field and one connection field.
fn declaring_both() -> Package {
    Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "api_key": {"type": "secret", "title": "API key", "required": true},
            "account": {
                "type": "connection",
                "title": "Google account",
                "provider": "google",
                "capabilities": ["calendar.read"],
                "required": true,
            },
        }}},
    }))
}

fn upload(package: &Package) -> SourceInput {
    SourceInput::Upload { tar: package.tar() }
}

#[tokio::test]
async fn an_upload_installs_as_the_first_state() {
    let harness = Harness::new();
    let package = minimal().file("README.md", "the weather plugin");

    let installed = harness
        .plugins
        .install(&harness.workspace_id, &upload(&package), &[])
        .await
        .expect("installed");

    assert_eq!(installed.plugin.name, "weather");
    assert_eq!(installed.plugin.manifest_version, "v1");
    assert_eq!(installed.plugin.state, PluginState::Enabled);
    assert_eq!(installed.plugin.source, PluginSource::Upload);
    assert_eq!(installed.plugin.installed_commit.len(), 40);
    let paths = harness
        .git
        .paths(&harness.workspace_id, installed.plugin.id.as_str());
    assert!(paths.root.join("README.md").is_file(), "the checkout");
    assert!(paths.data.is_dir(), "PLUGIN_DATA is made");
}

#[tokio::test]
async fn an_upload_of_one_top_directory_is_unpacked_without_it() {
    let harness = Harness::new();
    let outer = tempfile::tempdir().expect("a scratch directory");
    let inner = outer.path().join("weather-plugin");
    std::fs::create_dir(&inner).expect("the folder");
    std::fs::write(
        inner.join("plugin.json"),
        serde_json::json!({
            "$schema": support::PLUGIN_SCHEMA,
            "name": "weather",
        })
        .to_string(),
    )
    .expect("the manifest");
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", outer.path())
        .expect("the upload tar");
    let tar = builder.into_inner().expect("the upload tar");

    let installed = harness
        .plugins
        .install(&harness.workspace_id, &SourceInput::Upload { tar }, &[])
        .await
        .expect("installed");

    assert_eq!(installed.plugin.name, "weather");
}

#[tokio::test]
async fn a_repository_installs_and_records_its_address() {
    let harness = Harness::new();
    let remote = repository(minimal().file("README.md", "one"));

    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &SourceInput::Git {
                url: format!("file://{}", remote.path().display()),
                reference: None,
            },
            &[],
        )
        .await
        .expect("installed");

    assert!(matches!(
        installed.plugin.source,
        PluginSource::Git {
            reference: None,
            ..
        }
    ));
    let paths = harness
        .git
        .paths(&harness.workspace_id, installed.plugin.id.as_str());
    assert!(
        !paths.root.join(".git").exists(),
        "the clone history is not part of the state"
    );
}

#[tokio::test]
async fn a_source_that_is_not_a_clone_address_is_refused() {
    let harness = Harness::new();

    let refused = harness
        .plugins
        .install(
            &harness.workspace_id,
            &SourceInput::Git {
                url: "ext::sh -c whoami".to_string(),
                reference: None,
            },
            &[],
        )
        .await;

    assert!(
        matches!(refused, Err(PluginError::Source(_))),
        "a scheme that runs a command is refused"
    );
}

#[tokio::test]
async fn a_name_another_set_of_tools_holds_is_refused() {
    let harness = Harness::holding(vec!["weather".to_string()]);
    let package = minimal();

    let refused = harness
        .plugins
        .install(&harness.workspace_id, &upload(&package), &[])
        .await;

    assert!(matches!(refused, Err(PluginError::Conflict(_))));
}

#[tokio::test]
async fn the_same_name_installs_once() {
    let harness = Harness::new();
    let package = minimal();
    harness
        .plugins
        .install(&harness.workspace_id, &upload(&package), &[])
        .await
        .expect("installed");

    let again = harness
        .plugins
        .install(&harness.workspace_id, &upload(&package), &[])
        .await;

    assert!(matches!(again, Err(PluginError::Conflict(_))));
}

#[tokio::test]
async fn a_secret_binding_holds_the_name_and_the_store_holds_the_value() {
    let harness = Harness::new();
    let package = declaring_both();
    let account = harness.connection("google", "work").await;

    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[
                BindingInput {
                    field: "api_key".to_string(),
                    value: BindingValueInput::Secret {
                        secret: "s3cret".to_string(),
                    },
                },
                BindingInput {
                    field: "account".to_string(),
                    value: BindingValueInput::Connection {
                        connection_id: account.clone(),
                    },
                },
            ],
        )
        .await
        .expect("installed");

    assert_eq!(installed.plugin.state, PluginState::Enabled);
    let secret = installed
        .bindings
        .iter()
        .find(|binding| binding.field == "api_key")
        .expect("the secret binding");
    let PluginBindingValue::Secret { secret_name } = &secret.value else {
        panic!("the binding holds a secret name");
    };
    assert!(
        !secret_name.contains("s3cret"),
        "the record never holds the value"
    );
    assert_eq!(
        harness.secrets.get(secret_name).expect("the secret store"),
        Some("s3cret".to_string())
    );

    let bound = installed
        .bindings
        .iter()
        .find(|binding| binding.field == "account")
        .expect("the connection binding");
    assert_eq!(
        bound.value,
        PluginBindingValue::Connection {
            connection_id: account,
            capabilities: vec!["calendar.read".to_string()],
        },
        "the binding carries the capabilities the plugin declared"
    );
}

#[tokio::test]
async fn a_plugin_whose_provider_has_no_connection_installs_disabled() {
    let harness = Harness::new();
    let package = declaring_both();

    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[BindingInput {
                field: "api_key".to_string(),
                value: BindingValueInput::Secret {
                    secret: "s3cret".to_string(),
                },
            }],
        )
        .await
        .expect("installed");

    assert_eq!(installed.plugin.state, PluginState::Disabled);
}

#[tokio::test]
async fn a_binding_made_later_enables_the_plugin() {
    let harness = Harness::new();
    let package = declaring_both();
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[BindingInput {
                field: "api_key".to_string(),
                value: BindingValueInput::Secret {
                    secret: "s3cret".to_string(),
                },
            }],
        )
        .await
        .expect("installed");
    let account = harness.connection("google", "work").await;

    let bound = harness
        .plugins
        .bind(
            &harness.workspace_id,
            &installed.plugin.id,
            &BindingInput {
                field: "account".to_string(),
                value: BindingValueInput::Connection {
                    connection_id: account,
                },
            },
        )
        .await
        .expect("bound");

    assert_eq!(bound.plugin.state, PluginState::Enabled);
    assert_eq!(bound.bindings.len(), 2);
    assert_eq!(
        harness.servers.stopped.lock().expect("stopped lock").len(),
        1,
        "a new binding stops the servers that hold the old one"
    );
}

#[tokio::test]
async fn a_connection_of_another_provider_is_refused() {
    let harness = Harness::new();
    let package = declaring_both();
    let other = harness.connection("slack", "team").await;

    let refused = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[BindingInput {
                field: "account".to_string(),
                value: BindingValueInput::Connection {
                    connection_id: other,
                },
            }],
        )
        .await;

    assert!(matches!(refused, Err(PluginError::Refused(_))));
}

#[tokio::test]
async fn a_binding_of_an_undeclared_field_is_refused() {
    let harness = Harness::new();
    let package = minimal();

    let refused = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[BindingInput {
                field: "api_key".to_string(),
                value: BindingValueInput::Secret {
                    secret: "s3cret".to_string(),
                },
            }],
        )
        .await;

    assert!(matches!(refused, Err(PluginError::Refused(_))));
}

#[tokio::test]
async fn a_value_of_the_wrong_type_is_refused() {
    let harness = Harness::new();
    let package = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "retries": {"type": "number", "title": "Retries"},
        }}},
    }));

    let refused = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[BindingInput {
                field: "retries".to_string(),
                value: BindingValueInput::Value {
                    value: serde_json::json!("three"),
                },
            }],
        )
        .await;

    assert!(matches!(refused, Err(PluginError::Refused(_))));
}

#[tokio::test]
async fn an_update_names_what_changed_and_mints_the_next_version() {
    let harness = Harness::new();
    let remote = repository(minimal().file("README.md", "one"));
    let url = format!("file://{}", remote.path().display());
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &SourceInput::Git {
                url,
                reference: None,
            },
            &[],
        )
        .await
        .expect("installed");
    commit(&remote, "README.md", "two");

    let updated = harness
        .plugins
        .update(&harness.workspace_id, &installed.plugin.id, None)
        .await
        .expect("updated");

    assert_eq!(updated.plugin.manifest_version, "v2");
    assert_ne!(
        updated.plugin.installed_commit,
        installed.plugin.installed_commit
    );
    assert_eq!(updated.changed.files.len(), 1);
    assert_eq!(updated.changed.files[0].path, "README.md");
    assert_eq!(updated.changed.files[0].status, ChangeStatus::Modified);
    assert_eq!(
        harness.servers.stopped.lock().expect("stopped lock").len(),
        1,
        "the servers of the old state stop"
    );
}

#[tokio::test]
async fn an_update_that_changes_nothing_is_refused() {
    let harness = Harness::new();
    let remote = repository(minimal().file("README.md", "one"));
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &SourceInput::Git {
                url: format!("file://{}", remote.path().display()),
                reference: None,
            },
            &[],
        )
        .await
        .expect("installed");

    let refused = harness
        .plugins
        .update(&harness.workspace_id, &installed.plugin.id, None)
        .await;

    assert!(matches!(refused, Err(PluginError::Refused(_))));
}

#[tokio::test]
async fn an_update_that_renames_the_plugin_is_refused() {
    let harness = Harness::new();
    let remote = repository(minimal().file("README.md", "one"));
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &SourceInput::Git {
                url: format!("file://{}", remote.path().display()),
                reference: None,
            },
            &[],
        )
        .await
        .expect("installed");
    commit(
        &remote,
        "plugin.json",
        &serde_json::json!({"$schema": support::PLUGIN_SCHEMA, "name": "forecast"}).to_string(),
    );

    let refused = harness
        .plugins
        .update(&harness.workspace_id, &installed.plugin.id, None)
        .await;

    assert!(matches!(refused, Err(PluginError::Refused(_))));
}

#[tokio::test]
async fn an_uploaded_plugin_updates_from_a_new_upload() {
    let harness = Harness::new();
    let first = minimal().file("README.md", "one");
    let installed = harness
        .plugins
        .install(&harness.workspace_id, &upload(&first), &[])
        .await
        .expect("installed");
    let second = minimal().file("README.md", "two");

    let updated = harness
        .plugins
        .update(
            &harness.workspace_id,
            &installed.plugin.id,
            Some(second.tar()),
        )
        .await
        .expect("updated");

    assert_eq!(updated.plugin.manifest_version, "v2");
}

#[tokio::test]
async fn an_update_drops_the_binding_of_a_field_that_is_gone() {
    let harness = Harness::new();
    let first = Package::new().manifest(serde_json::json!({
        "name": "weather",
        "extensions": {"pagis": {"config": {
            "api_key": {"type": "secret", "title": "API key", "required": true},
        }}},
    }));
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&first),
            &[BindingInput {
                field: "api_key".to_string(),
                value: BindingValueInput::Secret {
                    secret: "s3cret".to_string(),
                },
            }],
        )
        .await
        .expect("installed");

    let updated = harness
        .plugins
        .update(
            &harness.workspace_id,
            &installed.plugin.id,
            Some(minimal().tar()),
        )
        .await
        .expect("updated");

    assert!(updated.bindings.is_empty());
    assert!(
        harness
            .store
            .list_bindings(&installed.plugin.workspace_id, &installed.plugin.id)
            .await
            .expect("the bindings")
            .is_empty()
    );
}

#[tokio::test]
async fn an_uninstall_revokes_the_grants_and_leaves_no_files() {
    let harness = Harness::new();
    let package = declaring_both();
    let account = harness.connection("google", "work").await;
    let installed = harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&package),
            &[
                BindingInput {
                    field: "api_key".to_string(),
                    value: BindingValueInput::Secret {
                        secret: "s3cret".to_string(),
                    },
                },
                BindingInput {
                    field: "account".to_string(),
                    value: BindingValueInput::Connection {
                        connection_id: account,
                    },
                },
            ],
        )
        .await
        .expect("installed");
    let agent_id = pagis_core::AgentId::generate();
    harness
        .plugins
        .grant(
            &harness.workspace_id,
            &harness.workspace_id,
            &installed.plugin.id,
            &agent_id,
        )
        .await
        .expect("granted");
    let secret_name = secret_name_of(&installed.bindings);
    let paths = harness
        .git
        .paths(&harness.workspace_id, installed.plugin.id.as_str());

    harness
        .plugins
        .uninstall(&harness.workspace_id, &installed.plugin.id)
        .await
        .expect("uninstalled");

    assert!(
        harness
            .store
            .get(&harness.workspace_id, &installed.plugin.id)
            .await
            .expect("the record")
            .is_none()
    );
    assert!(
        harness
            .grants
            .list_live(&harness.workspace_id)
            .await
            .expect("the grants")
            .is_empty(),
        "every grant on the plugin is revoked"
    );
    assert_eq!(
        harness.secrets.get(&secret_name).expect("the secret store"),
        None,
        "the secret is forgotten"
    );
    assert!(!paths.repository.exists());
    assert!(!paths.root.exists());
    assert!(!paths.data.exists());
    assert_eq!(
        harness.servers.stopped.lock().expect("stopped lock").len(),
        1,
        "the servers stop before the files go"
    );
}

#[tokio::test]
async fn the_list_holds_every_installed_plugin() {
    let harness = Harness::new();
    harness
        .plugins
        .install(&harness.workspace_id, &upload(&minimal()), &[])
        .await
        .expect("installed");
    harness
        .plugins
        .install(
            &harness.workspace_id,
            &upload(&Package::new().manifest(serde_json::json!({"name": "atlas"}))),
            &[],
        )
        .await
        .expect("installed");

    let list: Vec<String> = harness
        .plugins
        .list(&harness.workspace_id)
        .await
        .expect("the list")
        .into_iter()
        .map(|plugin: Plugin| plugin.name)
        .collect();

    assert_eq!(list, vec!["atlas".to_string(), "weather".to_string()]);
}

#[test]
fn a_manifest_version_follows_the_one_before_it() {
    assert_eq!(next_manifest_version("v1"), "v2");
    assert_eq!(next_manifest_version("v9"), "v10");
}

fn secret_name_of(bindings: &[pagis_core::PluginBinding]) -> String {
    bindings
        .iter()
        .find_map(|binding| match &binding.value {
            PluginBindingValue::Secret { secret_name } => Some(secret_name.clone()),
            _ => None,
        })
        .expect("the secret binding")
}

/// A repository a plugin is cloned from. It is a real one on disk, so
/// the clone runs the same code an https address runs.
fn repository(package: Package) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("a repository");
    for entry in std::fs::read_dir(package.root()).expect("the package") {
        let entry = entry.expect("an entry");
        std::fs::copy(entry.path(), directory.path().join(entry.file_name()))
            .expect("copy into the repository");
    }
    run(
        directory.path(),
        &["init", "--quiet", "--initial-branch=main"],
    );
    run(
        directory.path(),
        &["config", "user.email", "test@pagis.local"],
    );
    run(directory.path(), &["config", "user.name", "Test"]);
    run(directory.path(), &["add", "."]);
    run(directory.path(), &["commit", "--quiet", "-m", "one"]);
    directory
}

/// Write one file into the repository and commit it.
fn commit(repository: &tempfile::TempDir, path: &str, text: &str) {
    std::fs::write(repository.path().join(path), text).expect("the file");
    run(repository.path(), &["add", "."]);
    run(repository.path(), &["commit", "--quiet", "-m", "two"]);
}

fn run(directory: &std::path::Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
