//! Member invitations stay in the Calendar of their owning Shared Workspace.
use crate::{
    BusyInterval, CalendarEvent, CalendarRange, CalendarStore, ParticipationStatus, StoreError,
    UserStore, Workspace, WorkspaceId, WorkspaceKind, WorkspaceMemberStore, WorkspaceStore,
};

pub async fn member_events(
    calendar: &dyn CalendarStore,
    workspaces: &dyn WorkspaceStore,
    members: &dyn WorkspaceMemberStore,
    users: &dyn UserStore,
    private: &WorkspaceId,
    range: CalendarRange,
) -> Result<Vec<(Workspace, Vec<CalendarEvent>)>, StoreError> {
    let Some(workspace) = workspaces.get(private).await? else {
        return Ok(Vec::new());
    };
    let Some(person_id) = workspace.user_id else {
        return Ok(Vec::new());
    };
    let Some(person) = users.get(&person_id).await?.filter(|p| !p.is_disabled()) else {
        return Ok(Vec::new());
    };
    let Some(email) = person.email else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    for owner in workspaces.list().await? {
        if owner.kind == WorkspaceKind::Shared
            && members.get(&owner.id, &person_id).await?.is_some()
        {
            let rows = calendar
                .attendee_events(&owner.id, &email, range.from(), range.to())
                .await?;
            events.push((owner, rows));
        }
    }
    Ok(events)
}

pub async fn free_busy(
    calendar: &dyn CalendarStore,
    workspaces: &dyn WorkspaceStore,
    members: &dyn WorkspaceMemberStore,
    users: &dyn UserStore,
    workspace: &WorkspaceId,
    range: CalendarRange,
) -> Result<Vec<BusyInterval>, StoreError> {
    let mut busy = calendar
        .free_busy(workspace, range.from(), range.to())
        .await?;
    let email = match workspaces.get(workspace).await?.and_then(|w| w.user_id) {
        Some(id) => users.get(&id).await?.and_then(|p| p.email),
        None => None,
    };
    if let Some(email) = email {
        for (_, events) in
            member_events(calendar, workspaces, members, users, workspace, range).await?
        {
            let events = events
                .into_iter()
                .filter(|event| {
                    event.data.attendees.iter().any(|a| {
                        a.email.eq_ignore_ascii_case(&email)
                            && a.status != ParticipationStatus::Declined
                    })
                })
                .collect();
            busy.extend(crate::calendar::free_busy(events, range)?);
        }
    }
    busy.sort_by_key(|interval| interval.start_ms);
    let mut merged: Vec<BusyInterval> = Vec::new();
    for interval in busy {
        if let Some(last) = merged.last_mut()
            && interval.start_ms <= last.end_ms
        {
            last.end_ms = last.end_ms.max(interval.end_ms);
        } else {
            merged.push(interval);
        }
    }
    Ok(merged)
}
