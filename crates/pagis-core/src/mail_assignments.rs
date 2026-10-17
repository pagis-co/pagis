//! Assignment work stays in the Shared Workspace. Each Chief of Staff receives a Delegation.
use crate::*;
use std::sync::Arc;

mod conversation;
mod sweep;

pub struct MailAssignments {
    stores: Stores,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
}
impl MailAssignments {
    pub fn new(stores: Stores, bus: Arc<dyn EventBus>, clock: Arc<dyn Clock>) -> Self {
        Self {
            stores,
            // Conversation delivery uses this Event Bus.
            bus,
            clock,
        }
    }
    pub fn records(&self) -> &dyn MailAssignmentStore {
        self.stores.mail_assignments.as_ref()
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn assign(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
        run: Option<&RunId>,
        key: &str,
        message_id: &str,
        members: &[UserId],
        role: MailAssignmentRole,
        summary: &str,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        self.assign_at(
            workspace,
            agent,
            run,
            key,
            message_id,
            members,
            role,
            summary,
            self.clock.now_ms(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn assign_at(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
        run: Option<&RunId>,
        key: &str,
        message_id: &str,
        members: &[UserId],
        role: MailAssignmentRole,
        summary: &str,
        now: UnixMillis,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        let shared = self
            .stores
            .workspaces
            .get(workspace)
            .await?
            .filter(|w| w.kind == WorkspaceKind::Shared)
            .ok_or_else(|| {
                StoreError::Conflict("Mail Assignments need a Shared Workspace.".into())
            })?;
        if members.is_empty() || summary.trim().is_empty() || summary.chars().count() > 2000 {
            return Err(StoreError::Conflict(
                "Select members and give a short summary.".into(),
            ));
        }
        let settings = self.records().settings(workspace).await?;
        let mut rows = Vec::new();
        for person in members.iter().collect::<std::collections::BTreeSet<_>>() {
            if self
                .stores
                .workspace_members
                .get(workspace, person)
                .await?
                .is_none()
            {
                return Err(StoreError::Conflict(
                    "The Person is not a current member of this Shared Workspace.".into(),
                ));
            }
            if self
                .stores
                .users
                .get(person)
                .await?
                .is_none_or(|p| p.is_disabled())
            {
                return Err(StoreError::Conflict("The member is not enabled.".into()));
            }
            let private = self
                .stores
                .workspaces
                .for_user(person)
                .await?
                .ok_or_else(|| {
                    StoreError::Conflict("The member has no Private Workspace.".into())
                })?;
            if role != MailAssignmentRole::Fyi {
                let chief = match &private.chief_of_staff_agent_id {
                    Some(id) => self.stores.agents.get(&private.id, id).await?,
                    None => None,
                };
                if chief.is_none_or(|a| a.status != AgentStatus::Active) {
                    return Err(StoreError::Conflict(
                        "The member has no active Chief of Staff.".into(),
                    ));
                }
            }
            rows.push(MailAssignment {
                id: MailAssignmentId::generate(),
                workspace_id: workspace.clone(),
                message_key: key.into(),
                message_id: message_id.into(),
                assignee: person.clone(),
                private_workspace_id: private.id,
                role,
                state: MailAssignmentState::Assigned,
                assigning_agent_id: agent.clone(),
                assigning_run_id: run.cloned(),
                channel_id: ChannelId::generate(),
                summary: summary.into(),
                note: String::new(),
                due_at: now + i64::from(settings.assignment_timeout_minutes) * 60_000,
                created_at: now,
                changed_at: now,
            });
        }
        self.records().create_batch(workspace, &rows).await?;
        self.stores
            .workspace_mailboxes
            .record_triage(
                workspace,
                key,
                &MailTriage {
                    outcome: MailTriageOutcome::Assigned,
                    note: summary.into(),
                    author: Author::Agent(agent.clone()),
                    assignee: None,
                    recorded_at: now,
                    resolved_at: None,
                    message_id: message_id.into(),
                },
            )
            .await?;
        for row in &rows {
            self.post(row, run, &shared.name).await?;
        }
        Ok(rows)
    }

    async fn changed(&self, row: &MailAssignment) -> Result<(), StoreError> {
        for workspace in [&row.workspace_id, &row.private_workspace_id] {
            self.bus
                .publish(NewEvent {
                    workspace_id: workspace.clone(),
                    event_type: "assignment.changed".into(),
                    agent_id: None,
                    run_id: None,
                    channel_id: Some(row.channel_id.clone()),
                    payload: serde_json::json!({
                        "assignment_id": row.id,
                    }),
                })
                .await?;
        }
        Ok(())
    }

    pub async fn update_by_chief(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
        id: &MailAssignmentId,
        state: MailAssignmentState,
        note: &str,
    ) -> Result<(), StoreError> {
        let row = self.records().find(id).await?.ok_or(StoreError::Conflict(
            "Mail Assignment or Workspace does not exist.".into(),
        ))?;
        let private = self
            .stores
            .workspaces
            .get(workspace)
            .await?
            .ok_or(StoreError::Conflict(
                "Mail Assignment or Workspace does not exist.".into(),
            ))?;
        if row.private_workspace_id != *workspace
            || private.chief_of_staff_agent_id.as_ref() != Some(agent)
            || !matches!(
                state,
                MailAssignmentState::Handled | MailAssignmentState::Escalated
            )
            || row.role == MailAssignmentRole::Fyi
            || note.trim().is_empty()
            || note.chars().count() > 2000
        {
            return Err(StoreError::Conflict("Only the member's Chief of Staff handles or escalates this assignment with a short note.".into()));
        }
        if !self
            .records()
            .update(
                &row.workspace_id,
                id,
                MailAssignmentState::Assigned,
                state,
                note,
                self.clock.now_ms(),
            )
            .await?
        {
            return Err(StoreError::Conflict(
                "The assignment is no longer assigned.".into(),
            ));
        }
        // The Run's final reply carries the answer and its Origin to the assigning Agent.
        self.changed(&row).await
    }

    pub async fn finish_by_person(
        &self,
        person: &UserId,
        id: &MailAssignmentId,
    ) -> Result<(), StoreError> {
        let row = self.records().find(id).await?.ok_or(StoreError::Conflict(
            "Mail Assignment or Workspace does not exist.".into(),
        ))?;
        if row.assignee != *person
            || !(row.state == MailAssignmentState::Escalated
                || row.role == MailAssignmentRole::Fyi
                    && row.state == MailAssignmentState::Assigned)
        {
            return Err(StoreError::Conflict(
                "Only the assignee closes an escalated or FYI assignment.".into(),
            ));
        }
        self.records()
            .update(
                &row.workspace_id,
                id,
                row.state,
                MailAssignmentState::Handled,
                &row.note,
                self.clock.now_ms(),
            )
            .await?;
        self.changed(&row).await
    }

    pub async fn feed(
        &self,
        person: &UserId,
        now: UnixMillis,
        timezone: &str,
    ) -> Result<Vec<MailAssignment>, StoreError> {
        let mut rows = Vec::new();
        for row in self.records().for_person(person, 500).await? {
            if row.role == MailAssignmentRole::Fyi
                && row.state == MailAssignmentState::Assigned
                && local_date(row.created_at, timezone) == local_date(now, timezone)
                && self
                    .stores
                    .workspace_members
                    .get(&row.workspace_id, person)
                    .await?
                    .is_some()
            {
                rows.push(row);
            }
        }
        Ok(rows)
    }
}
