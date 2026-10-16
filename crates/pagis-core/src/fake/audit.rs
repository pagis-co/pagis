use super::*;

pub(super) fn append(data: &mut Data, entry: &AuditEntry) -> Result<(), StoreError> {
    if !data
        .orgs
        .values()
        .any(|o| o.workspace_id == entry.org_workspace_id)
    {
        return Err(conflict("The Org Workspace is missing."));
    }
    if !data.workspaces.contains_key(&entry.subject_workspace_id) {
        return Err(conflict("The subject Workspace is missing."));
    }
    if data.audit.contains_key(&entry.id) {
        return Err(conflict("The Audit Entry exists."));
    }
    data.audit.insert(entry.id.clone(), entry.clone());
    Ok(())
}
pub(super) fn change_hold(
    data: &mut Data,
    entry: &AuditEntry,
    start: bool,
) -> Result<(), StoreError> {
    let action = if start {
        "legal_hold.started"
    } else {
        "legal_hold.ended"
    };
    if entry.action != action
        || (start && entry.reason.as_ref().is_none_or(|r| r.trim().is_empty()))
    {
        return Err(conflict(
            "A Legal Hold needs a reason and the correct action.",
        ));
    }
    let workspace = data
        .workspaces
        .get(&entry.subject_workspace_id)
        .ok_or_else(|| conflict("The Workspace is missing."))?;
    if workspace.legal_hold.is_some() == start {
        return Err(conflict("The Legal Hold state changed."));
    }
    append(data, entry)?;
    data.workspaces
        .get_mut(&entry.subject_workspace_id)
        .unwrap()
        .legal_hold = start.then(|| LegalHold {
        started_at: entry.at,
        started_by: entry.actor.clone(),
    });
    Ok(())
}
pub(super) fn refuse_delete(data: &Data, workspace: &WorkspaceId) -> Result<(), StoreError> {
    LegalHold::refuse_delete(
        data.workspaces
            .get(workspace)
            .is_some_and(|w| w.legal_hold.is_some()),
    )
}
#[async_trait::async_trait]
impl AuditLog for FakeAuditLog {
    async fn append(&self, entry: &AuditEntry) -> Result<(), StoreError> {
        append(&mut self.0.lock().unwrap(), entry)
    }
    async fn list(
        &self,
        org: &WorkspaceId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        let data = self.0.lock().unwrap();
        let mut rows: Vec<_> = data
            .audit
            .values()
            .filter(|e| &e.org_workspace_id == org)
            .cloned()
            .collect();
        rows.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
        Ok(rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }
}
