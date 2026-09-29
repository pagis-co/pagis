//! An in-memory Software List. It holds the same rules the
//! SQLite store holds — one name per Workspace, one row per Version —
//! so a test that drives a publish needs no database.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use pagis_core::{
    SoftwarePackage, SoftwarePackageId, SoftwareStore, SoftwareVersion, StoreError, WorkspaceId,
};

#[derive(Default)]
pub struct MemorySoftwareStore {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    packages: Vec<SoftwarePackage>,
    versions: HashMap<String, Vec<SoftwareVersion>>,
}

#[async_trait]
impl SoftwareStore for MemorySoftwareStore {
    async fn create_package(&self, package: &SoftwarePackage) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("software store lock");
        if state
            .packages
            .iter()
            .any(|held| held.workspace_id == package.workspace_id && held.name == package.name)
        {
            return Err(StoreError::Conflict(format!(
                "the package {} already exists",
                package.name
            )));
        }
        state.packages.push(package.clone());
        Ok(())
    }

    async fn get_by_name(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<Option<SoftwarePackage>, StoreError> {
        let state = self.state.lock().expect("software store lock");
        Ok(state
            .packages
            .iter()
            .find(|held| &held.workspace_id == workspace_id && held.name == name)
            .cloned())
    }

    async fn list_packages(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<SoftwarePackage>, StoreError> {
        let state = self.state.lock().expect("software store lock");
        let mut packages: Vec<SoftwarePackage> = state
            .packages
            .iter()
            .filter(|held| &held.workspace_id == workspace_id)
            .cloned()
            .collect();
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(packages)
    }

    async fn add_version(
        &self,
        package: &SoftwarePackage,
        version: &SoftwareVersion,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("software store lock");
        let versions = state
            .versions
            .entry(version.package_id.as_str().to_string())
            .or_default();
        if versions.iter().any(|held| held.version == version.version) {
            return Err(StoreError::Conflict(format!(
                "{} is already published",
                version.version
            )));
        }
        versions.push(version.clone());
        if let Some(held) = state.packages.iter_mut().find(|held| held.id == package.id) {
            *held = package.clone();
        }
        Ok(())
    }

    async fn list_versions(
        &self,
        workspace_id: &WorkspaceId,
        package_id: &SoftwarePackageId,
    ) -> Result<Vec<SoftwareVersion>, StoreError> {
        let state = self.state.lock().expect("software store lock");
        // A Version belongs to its Package, so the Package decides
        // whether this Workspace reads it.
        if !state
            .packages
            .iter()
            .any(|held| &held.id == package_id && &held.workspace_id == workspace_id)
        {
            return Ok(Vec::new());
        }
        Ok(state
            .versions
            .get(package_id.as_str())
            .cloned()
            .unwrap_or_default())
    }
}
