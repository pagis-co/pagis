//! Junk policies and message decisions in memory.
use super::*;

#[derive(Clone, Default)]
pub(super) struct FakeJunkStore(pub(super) Shared);
#[async_trait::async_trait]
impl crate::JunkStore for FakeJunkStore {
    async fn org_policy(&self) -> Result<crate::OrgJunkPolicy, StoreError> {
        Ok(self.0.lock().unwrap().junk_org.clone())
    }
    async fn set_org_policy(&self, policy: &crate::OrgJunkPolicy) -> Result<(), StoreError> {
        policy.lists.validate()?;
        self.0.lock().unwrap().junk_org = policy.clone();
        Ok(())
    }
    async fn workspace_lists(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<crate::MailLists, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .junk_lists
            .get(workspace)
            .cloned()
            .unwrap_or_default())
    }
    async fn set_workspace_lists(
        &self,
        workspace: &WorkspaceId,
        lists: &crate::MailLists,
    ) -> Result<(), StoreError> {
        lists.validate()?;
        self.0
            .lock()
            .unwrap()
            .junk_lists
            .insert(workspace.clone(), lists.clone());
        Ok(())
    }
    async fn message(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        key: &str,
    ) -> Result<Option<crate::JunkMessage>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .junk_messages
            .get(&(workspace.clone(), mailbox.into(), key.into()))
            .cloned())
    }
    async fn observe(&self, message: &crate::JunkMessage) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = (
            message.workspace_id.clone(),
            message.mailbox.clone(),
            message.message_key.clone(),
        );
        if !message.quarantined && !data.junk_messages.contains_key(&key) {
            return Ok(());
        }
        let mut value = message.clone();
        value.person_mark = None;
        value.released = false;
        if let Some(old) = data.junk_messages.get(&key) {
            if old.quarantined {
                value = old.clone();
                value.host_id = message.host_id.clone();
            }
            value.person_mark = old.person_mark;
            value.released = old.released;
            value.quarantined |= old.quarantined;
        }
        data.junk_messages.insert(key, value);
        Ok(())
    }
    async fn relocate(
        &self,
        old_key: &str,
        message: &crate::JunkMessage,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(mut moved) = data.junk_messages.remove(&(
            message.workspace_id.clone(),
            message.mailbox.clone(),
            old_key.into(),
        )) else {
            return Ok(());
        };
        let new_key = (
            message.workspace_id.clone(),
            message.mailbox.clone(),
            message.message_key.clone(),
        );
        if old_key != message.message_key && data.junk_messages.contains_key(&new_key) {
            data.junk_messages.insert(
                (
                    moved.workspace_id.clone(),
                    moved.mailbox.clone(),
                    old_key.into(),
                ),
                moved,
            );
            return Err(StoreError::Conflict("mail record exists".into()));
        }
        moved.host_id = message.host_id.clone();
        moved.message_key = message.message_key.clone();
        data.junk_messages.insert(
            (
                moved.workspace_id.clone(),
                moved.mailbox.clone(),
                moved.message_key.clone(),
            ),
            moved,
        );
        Ok(())
    }
    async fn mark(&self, message: &crate::JunkMessage, spam: bool) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let value = data
            .junk_messages
            .entry((
                message.workspace_id.clone(),
                message.mailbox.clone(),
                message.message_key.clone(),
            ))
            .or_insert_with(|| {
                let mut value = message.clone();
                value.released = false;
                value
            });
        value.person_mark = Some(spam);
        Ok(())
    }
    async fn quarantine(&self) -> Result<Vec<crate::JunkMessage>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .junk_messages
            .values()
            .filter(|m| m.quarantined && !m.released)
            .cloned()
            .collect())
    }
    async fn release(
        &self,
        mailbox: &str,
        key: &str,
        audit: &crate::AuditEntry,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.audit.contains_key(&audit.id) {
            return Err(StoreError::Conflict("audit entry exists".into()));
        }
        let message = data
            .junk_messages
            .get_mut(&(
                audit.subject_workspace_id.clone(),
                mailbox.into(),
                key.into(),
            ))
            .filter(|m| m.quarantined && !m.released)
            .ok_or(StoreError::Conflict("mail record does not exist".into()))?;
        message.released = true;
        data.audit.insert(audit.id.clone(), audit.clone());
        Ok(())
    }
}
