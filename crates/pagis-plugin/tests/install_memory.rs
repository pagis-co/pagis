//! The shared install path over portable stores and sources.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    AgentId, GrantStore, MemorySecretStore, Plugin, PluginBinding, PluginId, PluginState,
    PluginStore, SecretStore, WorkspaceId, fake::FakeDatabase,
};
use pagis_plugin::{
    BindingInput, BindingValueInput, ChangeStatus, Namespaces, PackageFiles, PluginError,
    PluginPackage, PluginServers, PluginStates, Plugins, PluginsDeps, SourceInput,
    fake::{FixedPackageSource, MemoryPluginStates},
};

use crate::memory_support::minimal;

#[derive(Default)]
struct Servers(Mutex<Vec<&'static str>>);

#[async_trait]
impl PluginServers for Servers {
    async fn freeze(
        &self,
        _: &Plugin,
        _: &PluginPackage,
        _: &[PluginBinding],
    ) -> Result<(), String> {
        self.0.lock().unwrap().push("freeze");
        Ok(())
    }
    async fn stop(&self, _: &PluginId) {
        self.0.lock().unwrap().push("stop");
    }
    async fn forget(&self, _: &PluginId) {
        self.0.lock().unwrap().push("forget");
    }
}

struct EmptyNamespaces;
impl Namespaces for EmptyNamespaces {
    fn holds(&self, _: &WorkspaceId, _: &str) -> bool {
        false
    }
}

struct Harness {
    plugins: Plugins,
    records: Arc<dyn PluginStore>,
    states: Arc<MemoryPluginStates>,
    source: Arc<FixedPackageSource>,
    servers: Arc<Servers>,
    grants: Arc<dyn GrantStore>,
    secrets: Arc<MemorySecretStore>,
    workspace: WorkspaceId,
}
impl Harness {
    fn new(files: PackageFiles) -> Self {
        let db = FakeDatabase::default();
        let stores = db.stores();
        let states = Arc::new(MemoryPluginStates::default());
        let source = Arc::new(FixedPackageSource::new(files));
        let servers = Arc::new(Servers::default());
        let secrets = Arc::new(MemorySecretStore::default());
        let plugins = Plugins::new(PluginsDeps {
            plugins: stores.plugins.clone(),
            connections: stores.connections,
            grants: stores.grants.clone(),
            secrets: secrets.clone(),
            states: states.clone(),
            source: source.clone(),
            namespaces: Arc::new(EmptyNamespaces),
            servers: servers.clone(),
            bus: Arc::new(db.bus()),
        });
        Self {
            plugins,
            records: stores.plugins,
            states,
            source,
            servers,
            grants: stores.grants,
            secrets,
            workspace: WorkspaceId::generate(),
        }
    }
    async fn install(&self) -> Plugin {
        self.plugins
            .install(
                &self.workspace,
                &SourceInput::Git {
                    url: "https://example.org/weather.git".into(),
                    reference: Some("stable".into()),
                },
                &[],
            )
            .await
            .unwrap()
            .plugin
    }
}

fn tar(files: &PackageFiles) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, file) in files.iter() {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.bytes.len() as u64);
        header.set_mode(if file.executable { 0o755 } else { 0o644 });
        header.set_cksum();
        builder
            .append_data(&mut header, path, file.bytes.as_slice())
            .unwrap();
    }
    builder.into_inner().unwrap()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn install_from_a_source_keeps_files_and_the_source_reference() {
    let files = minimal().file("bin/run", "run").files;
    let h = Harness::new(files.clone());
    let plugin = h.install().await;
    assert_eq!(plugin.state, PluginState::Enabled);
    assert_eq!(
        h.source.requests(),
        vec![(
            "https://example.org/weather.git".into(),
            Some("stable".into())
        )]
    );
    assert_eq!(
        h.states
            .read(&h.workspace, plugin.id.as_str(), &plugin.installed_commit)
            .await
            .unwrap(),
        files
    );
    assert_eq!(
        h.plugins
            .installed(&h.workspace, &plugin.id)
            .await
            .unwrap()
            .package
            .manifest
            .name,
        "weather"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn install_from_an_upload_needs_no_source() {
    let mut files = minimal().files;
    files.insert("bin/run", b"run".to_vec(), true).unwrap();
    let h = Harness::new(PackageFiles::default());
    let installed = h
        .plugins
        .install(&h.workspace, &SourceInput::Upload { tar: tar(&files) }, &[])
        .await
        .unwrap();
    assert!(h.source.requests().is_empty());
    assert_eq!(
        h.states
            .read(
                &h.workspace,
                installed.plugin.id.as_str(),
                &installed.plugin.installed_commit
            )
            .await
            .unwrap(),
        files
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn update_compares_bytes_and_executable_flags_in_memory() {
    let old = minimal()
        .file("old", "gone")
        .file("change", "one")
        .file("mode", "run")
        .files;
    let h = Harness::new(old.clone());
    let plugin = h.install().await;
    let mut new = minimal().file("added", "new").file("change", "two").files;
    new.insert("mode", b"run".to_vec(), true).unwrap();
    h.source.set(new.clone());
    let updated = h
        .plugins
        .update(&h.workspace, &plugin.id, None)
        .await
        .unwrap();
    assert_eq!(
        updated
            .changed
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status))
            .collect::<Vec<_>>(),
        vec![
            ("added", ChangeStatus::Added),
            ("change", ChangeStatus::Modified),
            ("mode", ChangeStatus::Modified),
            ("old", ChangeStatus::Deleted)
        ]
    );
    assert_eq!(updated.plugin.manifest_version, "v2");
    assert_ne!(updated.plugin.installed_commit, plugin.installed_commit);
    assert_eq!(
        h.states
            .read(&h.workspace, plugin.id.as_str(), &plugin.installed_commit)
            .await
            .unwrap(),
        old
    );
    assert_eq!(
        h.states
            .read(
                &h.workspace,
                plugin.id.as_str(),
                &updated.plugin.installed_commit
            )
            .await
            .unwrap(),
        new
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_unchanged_update_writes_nothing_and_keeps_servers_running() {
    let h = Harness::new(minimal().files);
    let plugin = h.install().await;
    let calls = h.servers.0.lock().unwrap().clone();
    let error = h
        .plugins
        .update(&h.workspace, &plugin.id, None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, PluginError::Refused(ref message) if message == "the plugin is unchanged")
    );
    assert_eq!(h.states.len(), 1);
    assert_eq!(*h.servers.0.lock().unwrap(), calls);
    assert_eq!(
        h.plugins
            .installed(&h.workspace, &plugin.id)
            .await
            .unwrap()
            .plugin
            .installed_commit,
        plugin.installed_commit
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_grant_belongs_to_the_agents_workspace() {
    let h = Harness::new(minimal().files);
    let plugin = h.install().await;
    let tenant = WorkspaceId::generate();
    let agent = AgentId::generate();
    let grant = h
        .plugins
        .grant(&h.workspace, &tenant, &plugin.id, &agent)
        .await
        .unwrap();
    assert_eq!(grant.resource_id.as_deref(), Some(plugin.id.as_str()));
    assert_eq!(
        h.grants.list_live_for_agent(&tenant, &agent).await.unwrap(),
        vec![grant]
    );
    assert!(
        h.grants
            .list_live_for_agent(&h.workspace, &agent)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn uninstall_removes_all_states_bindings_secrets_and_grants() {
    let files = crate::memory_support::Package::new().manifest(serde_json::json!({"name": "weather", "extensions": {"pagis": {"config": {"key": {"type": "secret", "title": "Key"}}}}})).files;
    let h = Harness::new(files.clone());
    let installed = h
        .plugins
        .install(
            &h.workspace,
            &SourceInput::Upload { tar: tar(&files) },
            &[BindingInput {
                field: "key".into(),
                value: BindingValueInput::Secret {
                    secret: "secret".into(),
                },
            }],
        )
        .await
        .unwrap();
    let id = &installed.plugin.id;
    let tenant = WorkspaceId::generate();
    let agent = AgentId::generate();
    h.plugins
        .grant(&h.workspace, &tenant, id, &agent)
        .await
        .unwrap();
    let new = crate::memory_support::Package { files }
        .file("new", "new")
        .files;
    h.plugins
        .update(&h.workspace, id, Some(tar(&new)))
        .await
        .unwrap();
    assert_eq!(h.states.len(), 2);
    h.plugins.uninstall(&h.workspace, id).await.unwrap();
    assert!(h.states.is_empty());
    assert!(
        h.records
            .list_bindings(&h.workspace, id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        h.secrets
            .get(&format!("plugin/{id}/key"))
            .unwrap()
            .is_none()
    );
    assert!(h.plugins.list(&h.workspace).await.unwrap().is_empty());
    assert!(
        h.grants
            .list_live_for_agent(&tenant, &agent)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        h.plugins.installed(&h.workspace, id).await,
        Err(PluginError::NotFound)
    ));
    assert!(
        h.states
            .read(
                &h.workspace,
                id.as_str(),
                &installed.plugin.installed_commit
            )
            .await
            .is_err()
    );
    assert_eq!(h.servers.0.lock().unwrap().last(), Some(&"forget"));
}
