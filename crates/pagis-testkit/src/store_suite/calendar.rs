//! The Calendar contract, shared by both storage backends.
use super::Backend;
use pagis_core::{Author, CalendarEvent, CalendarEventData, WorkspaceId};

fn event(workspace_id: WorkspaceId) -> CalendarEvent {
    CalendarEvent::new(
        workspace_id,
        Author::System,
        CalendarEventData {
            title: "Review".into(),
            description: "Read the plan".into(),
            location: "Office".into(),
            start: "2026-03-07T09:00:00".into(),
            end: "2026-03-07T10:00:00".into(),
            all_day: false,
            timezone: "America/New_York".into(),
            rrule: Some("FREQ=DAILY;COUNT=4".into()),
            exdates: vec!["2026-03-09T09:00:00".into()],
            attendees: vec![pagis_core::CalendarAttendee {
                email: "reader@example.com".into(),
                name: "Reader".into(),
                status: pagis_core::ParticipationStatus::Tentative,
            }],
            reminders: vec![10, 30],
            meeting_url: Some("https://meet.example.com/review".into()),
        },
    )
}

pub async fn calendar_crud_and_workspace_boundary(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().calendar;
    let mut row = event(workspace.id.clone());
    store.create(&row).await.unwrap();
    assert!(store.create(&row).await.is_err());
    assert_eq!(
        store.get(&workspace.id, &row.id).await.unwrap(),
        Some(row.clone())
    );
    let other = WorkspaceId::generate();
    assert!(store.get(&other, &row.id).await.unwrap().is_none());
    assert!(!store.update(&other, &row.id, &row.data).await.unwrap());
    assert!(!store.delete(&other, &row.id).await.unwrap());
    assert!(
        store
            .list(&other, 0, 2_000_000_000_000)
            .await
            .unwrap()
            .is_empty()
    );
    row.data.title = "Changed".into();
    assert!(
        store
            .update(&workspace.id, &row.id, &row.data)
            .await
            .unwrap()
    );
    assert_eq!(
        store.get(&workspace.id, &row.id).await.unwrap(),
        Some(row.clone())
    );
    assert!(store.delete(&workspace.id, &row.id).await.unwrap());
    assert!(!store.delete(&workspace.id, &row.id).await.unwrap());
    assert!(store.get(&workspace.id, &row.id).await.unwrap().is_none());
}

pub async fn calendar_recurrence_dst_exdate_and_range(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let row = event(workspace.id.clone());
    let store = &backend.stores().calendar;
    store.create(&row).await.unwrap();
    let ms = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .timestamp_millis()
    };
    let from = ms("2026-03-07T00:00:00Z");
    let to = ms("2026-03-11T00:00:00Z");
    let found = store.list(&workspace.id, from, to).await.unwrap();
    assert_eq!(
        found.iter().map(|o| o.start.as_str()).collect::<Vec<_>>(),
        vec![
            "2026-03-07T09:00:00-05:00",
            "2026-03-08T09:00:00-04:00",
            "2026-03-10T09:00:00-04:00",
        ]
    );
    assert_eq!(found[0].event, row);
    assert_eq!(found[1].event.data.reminders, vec![10, 30]);
    assert_eq!(
        store
            .list(
                &workspace.id,
                ms("2026-03-08T13:30:00Z"),
                ms("2026-03-08T14:00:00Z")
            )
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .list(&workspace.id, to, to + 86_400_000)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.list(&workspace.id, to, from).await.is_err());
}

pub async fn calendar_all_day_and_occurrence_cap(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().calendar;
    let mut row = event(workspace.id.clone());
    row.data.all_day = true;
    row.data.start = "2026-03-07".into();
    row.data.end = "2026-03-09".into();
    row.data.rrule = None;
    row.data.exdates.clear();
    store.create(&row).await.unwrap();
    let found = store
        .list(&workspace.id, 1_772_841_600_000, 1_773_187_200_000)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (&found[0].start, &found[0].end),
        (&row.data.start, &row.data.end)
    );
    row.data.rrule = Some("FREQ=DAILY".into());
    store
        .update(&workspace.id, &row.id, &row.data)
        .await
        .unwrap();
    let found = store
        .list(&workspace.id, 1_772_841_600_000, 2_000_000_000_000)
        .await
        .unwrap();
    assert_eq!(found.len(), pagis_core::calendar::MAX_OCCURRENCES);
}

pub async fn calendar_range_does_not_read_unrelated_event_data(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().calendar;
    let mut before = event(workspace.id.clone());
    before.data.rrule = None;
    before.data.exdates.clear();
    let mut after = before.clone();
    after.id = pagis_core::CalendarEventId::generate();
    after.uid = after.id.to_string();
    after.data.start = "2026-04-07T09:00:00".into();
    after.data.end = "2026-04-07T10:00:00".into();
    for row in [&before, &after] {
        store.create(row).await.unwrap();
        backend
            .rows()
            .execute(
                "UPDATE calendar_events SET data = ? WHERE id = ?",
                &["invalid JSON".into(), row.id.as_str().into()],
            )
            .await
            .unwrap();
    }
    let from = chrono::DateTime::parse_from_rfc3339("2026-03-15T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert!(
        store
            .list(&workspace.id, from, from + 86_400_000)
            .await
            .unwrap()
            .is_empty()
    );
    before.data.start = "2026-03-15T09:00:00".into();
    before.data.end = "2026-03-15T10:00:00".into();
    store
        .update(&workspace.id, &before.id, &before.data)
        .await
        .unwrap();
    let found = store
        .list(&workspace.id, from, from + 86_400_000)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].event.id, before.id);
}

pub async fn calendar_authors_use_checked_columns(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = crate::fixture::agent(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    for author in [
        Author::System,
        Author::Person(
            workspace
                .user_id
                .clone()
                .expect("a seeded Private Workspace has its Person"),
        ),
        Author::Agent(agent.id),
    ] {
        let mut row = event(workspace.id.clone());
        row.created_by = author;
        backend.stores().calendar.create(&row).await.unwrap();
        assert_eq!(
            backend
                .stores()
                .calendar
                .get(&workspace.id, &row.id)
                .await
                .unwrap(),
            Some(row.clone())
        );
        assert!(
            backend
                .rows()
                .execute(
                    "UPDATE calendar_events SET author_kind = ? WHERE id = ?",
                    &["unknown".into(), row.id.as_str().into()]
                )
                .await
                .is_err()
        );
    }
}

pub async fn calendar_free_busy(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().calendar;
    let ms = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .timestamp_millis()
    };
    let row = event(workspace.id.clone());
    store.create(&row).await.unwrap();
    for (start, end) in [("09:30:00", "10:30:00"), ("10:30:00", "11:00:00")] {
        let mut overlap = event(workspace.id.clone());
        overlap.data.rrule = None;
        overlap.data.exdates.clear();
        overlap.data.start = format!("2026-03-08T{start}");
        overlap.data.end = format!("2026-03-08T{end}");
        store.create(&overlap).await.unwrap();
    }
    let mut all_day = event(workspace.id.clone());
    all_day.data.all_day = true;
    all_day.data.start = "2026-03-07".into();
    all_day.data.end = "2026-03-11".into();
    all_day.data.rrule = None;
    all_day.data.exdates.clear();
    store.create(&all_day).await.unwrap();
    let from = ms("2026-03-07T14:30:00Z");
    let to = ms("2026-03-11T00:00:00Z");
    let busy = store.free_busy(&workspace.id, from, to).await.unwrap();
    assert_eq!(
        busy,
        vec![
            pagis_core::BusyInterval {
                start_ms: from,
                end_ms: ms("2026-03-07T15:00:00Z")
            },
            pagis_core::BusyInterval {
                start_ms: ms("2026-03-08T13:00:00Z"),
                end_ms: ms("2026-03-08T15:00:00Z")
            },
            pagis_core::BusyInterval {
                start_ms: ms("2026-03-10T13:00:00Z"),
                end_ms: ms("2026-03-10T14:00:00Z")
            },
        ]
    );
    assert!(
        store
            .free_busy(&WorkspaceId::generate(), from, to)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.free_busy(&workspace.id, to, from).await.is_err());
    assert!(
        store
            .free_busy(
                &workspace.id,
                ms("2026-03-08T15:00:00Z"),
                ms("2026-03-09T00:00:00Z")
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .free_busy(
                &workspace.id,
                ms("2026-03-08T13:00:00Z"),
                ms("2026-03-08T13:30:00Z")
            )
            .await
            .unwrap()[0]
            .end_ms,
        ms("2026-03-08T13:30:00Z")
    );
}

pub async fn legal_hold_preserves_events_and_occurrences(backend: &Backend) {
    use pagis_core::{AuditEntry, AuditEntryId, StoreError};
    let workspace = backend.seeded_workspace().await;
    let stores = backend.stores();
    let org = stores.orgs.list().await.unwrap().remove(0);
    let row = event(workspace.id.clone());
    stores.calendar.create(&row).await.unwrap();
    let start = AuditEntry {
        id: AuditEntryId::generate(),
        org_workspace_id: org.workspace_id,
        subject_workspace_id: workspace.id.clone(),
        action: "legal_hold.started".into(),
        actor: Author::System,
        reason: Some("Preserve".into()),
        at: 100,
    };
    stores.workspaces.start_legal_hold(&start).await.unwrap();
    assert!(matches!(
        stores.calendar.delete(&workspace.id, &row.id).await,
        Err(StoreError::LegalHold)
    ));
    let mut data = row.data.clone();
    data.exdates.push("2026-03-07T09:00:00".into());
    assert!(matches!(
        stores.calendar.update(&workspace.id, &row.id, &data).await,
        Err(StoreError::LegalHold)
    ));
    assert_eq!(
        stores.calendar.get(&workspace.id, &row.id).await.unwrap(),
        Some(row.clone())
    );
    stores
        .workspaces
        .end_legal_hold(&AuditEntry {
            id: AuditEntryId::generate(),
            action: "legal_hold.ended".into(),
            at: 200,
            ..start
        })
        .await
        .unwrap();
    assert!(
        stores
            .calendar
            .delete(&workspace.id, &row.id)
            .await
            .unwrap()
    );
}

#[macro_export]
macro_rules! store_suite_calendar {
    ($emit:path) => {
        $emit!(
            calendar,
            calendar_free_busy,
            legal_hold_preserves_events_and_occurrences,
            calendar_crud_and_workspace_boundary,
            calendar_recurrence_dst_exdate_and_range,
            calendar_all_day_and_occurrence_cap,
            calendar_range_does_not_read_unrelated_event_data,
            calendar_authors_use_checked_columns,
        );
    };
}
