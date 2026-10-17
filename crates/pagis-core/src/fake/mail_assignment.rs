use super::FakeMailAssignmentStore;
use crate::*;
use async_trait::async_trait;

#[async_trait]
impl MailAssignmentStore for FakeMailAssignmentStore {
    async fn find(&self, id: &MailAssignmentId) -> Result<Option<MailAssignment>, StoreError> {
        Ok(self.0.lock().unwrap().mail_assignments.get(id).cloned())
    }
    async fn by_channel(&self, channel: &ChannelId) -> Result<Option<MailAssignment>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assignments
            .values()
            .find(|a| &a.channel_id == channel)
            .cloned())
    }
    async fn settings(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<MailAssignmentSettings, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assignment_settings
            .get(workspace)
            .cloned()
            .unwrap_or_default())
    }
    async fn set_settings(
        &self,
        workspace: &WorkspaceId,
        settings: &MailAssignmentSettings,
    ) -> Result<(), StoreError> {
        settings.validate()?;
        self.0
            .lock()
            .unwrap()
            .mail_assignment_settings
            .insert(workspace.clone(), settings.clone());
        Ok(())
    }
    async fn create_batch(
        &self,
        workspace: &WorkspaceId,
        assignments: &[MailAssignment],
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut rows = data.mail_assignments.clone();
        let settings = data
            .mail_assignment_settings
            .get(workspace)
            .cloned()
            .unwrap_or_default();
        for row in assignments {
            if &row.workspace_id != workspace
                || rows.contains_key(&row.id)
                || rows.values().any(|a| a.channel_id == row.channel_id)
                || data.channels.get(workspace, &row.channel_id).is_some()
            {
                return Err(StoreError::Conflict(
                    "Mail Assignment already exists or has a different Workspace.".into(),
                ));
            }
            if settings.assignment_limit.is_some_and(|limit| {
                rows.values()
                    .filter(|a| {
                        a.workspace_id == *workspace
                            && a.assignee == row.assignee
                            && a.state.is_open()
                    })
                    .count()
                    >= limit as usize
            }) {
                return Err(StoreError::Conflict(
                    "The member has reached the assignment limit. Select another member.".into(),
                ));
            }
            rows.insert(row.id.clone(), row.clone());
        }
        for row in assignments {
            data.channels.insert(
                workspace,
                row.channel_id.clone(),
                Channel {
                    id: row.channel_id.clone(),
                    workspace_id: workspace.clone(),
                    kind: ChannelKind::Conversation,
                    title: Some(row.summary.clone()),
                    created_at: row.created_at,
                    updated_at: row.created_at,
                },
            )?;
            for participant in [workspace, &row.private_workspace_id] {
                let record = ChannelParticipant {
                    id: ParticipantId::generate(),
                    workspace_id: workspace.clone(),
                    channel_id: row.channel_id.clone(),
                    kind: ParticipantKind::Workspace,
                    participant_workspace_id: Some(participant.clone()),
                    person_id: None,
                    agent_id: None,
                    joined_at: row.created_at,
                };
                data.participants
                    .insert(workspace, record.id.clone(), record)?;
            }
        }
        data.mail_assignments = rows;
        Ok(())
    }
    async fn list(
        &self,
        workspace: &WorkspaceId,
        message: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assignments
            .values()
            .rev()
            .filter(|a| {
                &a.workspace_id == workspace && message.is_none_or(|key| a.message_key == key)
            })
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn for_person(
        &self,
        person: &UserId,
        limit: u32,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assignments
            .values()
            .rev()
            .filter(|a| &a.assignee == person && a.state.is_open())
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn overdue(&self, at: UnixMillis, limit: u32) -> Result<Vec<MailAssignment>, StoreError> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .mail_assignments
            .values()
            .filter(|a| a.state == MailAssignmentState::Assigned && a.due_at <= at)
            .cloned()
            .collect();
        rows.sort_by(|a, b| (a.due_at, &a.id).cmp(&(b.due_at, &b.id)));
        rows.truncate(limit as usize);
        Ok(rows)
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
    ) -> Result<Option<MailAssignment>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .mail_assignments
            .get(id)
            .filter(|a| &a.workspace_id == workspace)
            .cloned())
    }
    async fn update(
        &self,
        workspace: &WorkspaceId,
        id: &MailAssignmentId,
        expected: MailAssignmentState,
        state: MailAssignmentState,
        note: &str,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .mail_assignments
            .get_mut(id)
            .filter(|a| &a.workspace_id == workspace && a.state == expected)
        else {
            return Ok(false);
        };
        row.state = state;
        row.note = note.into();
        row.changed_at = at;
        Ok(true)
    }
}
