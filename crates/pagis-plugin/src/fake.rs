//! Portable package sources and installed states held in memory.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use pagis_core::{StoreError, WorkspaceId};

use crate::{PackageFiles, PackageSource, PluginStates, SourceError};

type StateKey = (WorkspaceId, String, String);

#[derive(Default)]
pub struct MemoryPluginStates(Mutex<BTreeMap<StateKey, PackageFiles>>);

impl MemoryPluginStates {
    pub fn len(&self) -> usize {
        self.0.lock().expect("states lock").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl PluginStates for MemoryPluginStates {
    async fn write(
        &self,
        workspace: &WorkspaceId,
        plugin: &str,
        files: &PackageFiles,
        _: &str,
    ) -> Result<String, StoreError> {
        let id = pagis_core::PluginId::generate().to_string();
        self.0.lock().expect("states lock").insert(
            (workspace.clone(), plugin.into(), id.clone()),
            files.clone(),
        );
        Ok(id)
    }
    async fn read(
        &self,
        workspace: &WorkspaceId,
        plugin: &str,
        state: &str,
    ) -> Result<PackageFiles, StoreError> {
        self.0
            .lock()
            .expect("states lock")
            .get(&(workspace.clone(), plugin.into(), state.into()))
            .cloned()
            .ok_or_else(|| StoreError::Corrupt("no such plugin state".into()))
    }
    async fn remove(&self, workspace: &WorkspaceId, plugin: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .expect("states lock")
            .retain(|(held, name, _), _| held != workspace || name != plugin);
        Ok(())
    }
}

/// A source that answers with fixed files and records the URL and reference.
pub struct FixedPackageSource {
    files: Mutex<PackageFiles>,
    requests: Mutex<Vec<(String, Option<String>)>>,
}
impl FixedPackageSource {
    pub fn new(files: PackageFiles) -> Self {
        Self {
            files: Mutex::new(files),
            requests: Mutex::new(Vec::new()),
        }
    }
    pub fn set(&self, files: PackageFiles) {
        *self.files.lock().expect("source lock") = files;
    }
    pub fn requests(&self) -> Vec<(String, Option<String>)> {
        self.requests.lock().expect("requests lock").clone()
    }
}
#[async_trait]
impl PackageSource for FixedPackageSource {
    async fn fetch(&self, url: &str, reference: Option<&str>) -> Result<PackageFiles, SourceError> {
        self.requests
            .lock()
            .expect("requests lock")
            .push((url.into(), reference.map(str::to_string)));
        Ok(self.files.lock().expect("source lock").clone())
    }
}
