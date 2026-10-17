//! Bounded Mail Assignment timeout reassignment.
use super::*;

impl MailAssignments {
    /// The due-time index bounds each sweep. Each message uses each member once.
    pub async fn sweep(&self, at: UnixMillis) -> Result<(), StoreError> {
        for row in self.records().overdue(at, 50).await? {
            if !self
                .records()
                .update(
                    &row.workspace_id,
                    &row.id,
                    MailAssignmentState::Assigned,
                    MailAssignmentState::TimedOut,
                    "The assignment timed out.",
                    at,
                )
                .await?
            {
                continue;
            }
            let history = self
                .records()
                .list(&row.workspace_id, Some(&row.message_key), u32::MAX)
                .await?;
            let mut members = self
                .stores
                .workspace_members
                .list(&row.workspace_id)
                .await?;
            members.sort_by(|a, b| a.person_id.cmp(&b.person_id));
            let run = row.assigning_run_id.as_ref();
            let mut reassigned = false;
            for member in members {
                if member.role < WorkspaceRole::Responder
                    || history.iter().any(|a| a.assignee == member.person_id)
                {
                    continue;
                }
                match self
                    .assign_at(
                        &row.workspace_id,
                        &row.assigning_agent_id,
                        run,
                        &row.message_key,
                        &row.message_id,
                        &[member.person_id],
                        row.role,
                        &row.summary,
                        at,
                    )
                    .await
                {
                    Ok(_) => {
                        reassigned = true;
                        break;
                    }
                    Err(StoreError::Conflict(_)) => continue,
                    Err(error) => return Err(error),
                }
            }
            if !reassigned {
                self.stores
                    .workspace_mailboxes
                    .record_triage(
                        &row.workspace_id,
                        &row.message_key,
                        &MailTriage {
                            outcome: MailTriageOutcome::NeedsYou,
                            note: "The assignment timed out. No other member can take it.".into(),
                            author: Author::System,
                            assignee: None,
                            recorded_at: at,
                            resolved_at: None,
                            message_id: row.message_id.clone(),
                        },
                    )
                    .await?;
            }
            self.changed(&row).await?;
        }
        Ok(())
    }
}
