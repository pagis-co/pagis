use super::FakeDisclosureStore;
use crate::{DisclosureRule, DisclosureStore, StoreError, WorkspaceId};
use async_trait::async_trait;
#[async_trait]
impl DisclosureStore for FakeDisclosureStore {
    async fn profile(&self, workspace: &WorkspaceId) -> Result<String, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .disclosure_profiles
            .get(workspace)
            .cloned()
            .unwrap_or_default())
    }
    async fn set_profile(&self, workspace: &WorkspaceId, profile: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .disclosure_profiles
            .insert(workspace.clone(), profile.into());
        Ok(())
    }
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<DisclosureRule>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .disclosure_rules
            .get(workspace)
            .cloned()
            .unwrap_or_default())
    }
    async fn add(&self, workspace: &WorkspaceId, rule: &DisclosureRule) -> Result<(), StoreError> {
        let mut db = self.0.lock().unwrap();
        let rules = db.disclosure_rules.entry(workspace.clone()).or_default();
        if !rules.contains(rule) {
            rules.push(rule.clone());
        }
        Ok(())
    }
    async fn remove(
        &self,
        workspace: &WorkspaceId,
        rule: &DisclosureRule,
    ) -> Result<(), StoreError> {
        if let Some(rules) = self.0.lock().unwrap().disclosure_rules.get_mut(workspace) {
            rules.retain(|r| r != rule);
        }
        Ok(())
    }
}
