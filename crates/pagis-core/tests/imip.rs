use pagis_core::imip::{SchedulingMessage, SchedulingMethod};
use pagis_core::{Author, CalendarEvent, ParticipationStatus, WorkspaceId};

#[test]
fn common_calendar_messages_parse_and_round_trip() {
    for (request, reply, cancel) in [
        (
            include_str!("fixtures/imip/google-request.ics"),
            include_str!("fixtures/imip/google-reply.ics"),
            include_str!("fixtures/imip/google-cancel.ics"),
        ),
        (
            include_str!("fixtures/imip/outlook-request.ics"),
            include_str!("fixtures/imip/outlook-reply.ics"),
            include_str!("fixtures/imip/outlook-cancel.ics"),
        ),
        (
            include_str!("fixtures/imip/apple-request.ics"),
            include_str!("fixtures/imip/apple-reply.ics"),
            include_str!("fixtures/imip/apple-cancel.ics"),
        ),
    ] {
        let parsed = SchedulingMessage::parse(request).unwrap();
        assert_eq!(parsed.method, SchedulingMethod::Request);
        assert_eq!(parsed.uid, "meeting@example.org");
        let data = parsed.data.unwrap();
        assert_eq!(data.attendees[0].status, ParticipationStatus::NeedsAction);
        assert_eq!(data.description, "First line\nSecond line, with text");
        let mut event = CalendarEvent::new(WorkspaceId::generate(), Author::System, data);
        event.uid = parsed.uid;
        let built = SchedulingMessage::build(&event, SchedulingMethod::Request).unwrap();
        let again = SchedulingMessage::parse(&built).unwrap();
        assert_eq!(again.data.unwrap(), event.data);
        assert_eq!(
            SchedulingMessage::parse(reply).unwrap().attendees[0].status,
            ParticipationStatus::Accepted
        );
        assert_eq!(
            SchedulingMessage::parse(cancel).unwrap().method,
            SchedulingMethod::Cancel
        );
    }
}

#[test]
fn malformed_or_unsupported_scheduling_is_rejected() {
    let request = include_str!("fixtures/imip/google-request.ics");
    for input in [
        request.replace("METHOD:REQUEST", "METHOD:PUBLISH"),
        request.replace("SEQUENCE:1", "SEQUENCE:-1"),
        request.replace("UID:meeting@example.org", "UID:"),
        include_str!("fixtures/imip/invalid-zone-request.ics").to_string(),
    ] {
        assert!(SchedulingMessage::parse(&input).is_err());
    }
}

#[tokio::test]
async fn replies_change_only_the_named_attendee_and_old_messages_do_not_change_the_event() {
    use pagis_core::calendar::EventStatus;
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let request = include_str!("fixtures/imip/google-request.ics");
    let message = SchedulingMessage::parse(request).unwrap();
    let event = message
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "reader@example.com",
            "organizer@example.org",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.data.attendees[0].status,
        ParticipationStatus::NeedsAction
    );
    let reply = include_str!("fixtures/imip/google-reply.ics");
    SchedulingMessage::parse(reply)
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "organizer@example.org",
            "intruder@example.com",
        )
        .await
        .unwrap();
    assert_eq!(
        stores
            .calendar
            .get(&workspace, &event.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .attendees[0]
            .status,
        ParticipationStatus::NeedsAction
    );
    SchedulingMessage::parse(reply)
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "organizer@example.org",
            "reader@example.com",
        )
        .await
        .unwrap();
    assert_eq!(
        stores
            .calendar
            .get(&workspace, &event.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .attendees[0]
            .status,
        ParticipationStatus::Accepted
    );
    let cancel = include_str!("fixtures/imip/google-cancel.ics");
    SchedulingMessage::parse(cancel)
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "reader@example.com",
            "organizer@example.org",
        )
        .await
        .unwrap();
    SchedulingMessage::parse(request)
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "reader@example.com",
            "organizer@example.org",
        )
        .await
        .unwrap();
    assert_eq!(
        stores
            .calendar
            .get(&workspace, &event.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .invitation
            .unwrap()
            .status,
        EventStatus::Cancelled
    );
    let other = WorkspaceId::generate();
    SchedulingMessage::parse(reply)
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &other,
            "organizer@example.org",
            "reader@example.com",
        )
        .await
        .unwrap();
    assert!(
        stores
            .calendar
            .get_by_uid(&other, &event.uid)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_store_refuses_a_stale_write_after_cancellation() {
    use pagis_core::calendar::EventStatus;
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let data = SchedulingMessage::parse(include_str!("fixtures/imip/google-request.ics"))
        .unwrap()
        .data
        .unwrap();
    let mut event = CalendarEvent::new(workspace.clone(), Author::System, data);
    stores.calendar.create(&event).await.unwrap();
    let old = event.data.clone();
    event.data.invitation.as_mut().unwrap().sequence = 2;
    event.data.invitation.as_mut().unwrap().status = EventStatus::Cancelled;
    assert!(
        stores
            .calendar
            .update(&workspace, &event.id, &event.data)
            .await
            .unwrap()
    );
    assert!(
        !stores
            .calendar
            .update(&workspace, &event.id, &old)
            .await
            .unwrap()
    );
}

#[test]
fn zones_use_the_database_and_output_iana_names() {
    for fixture in [
        include_str!("fixtures/imip/google-request.ics"),
        include_str!("fixtures/imip/outlook-request.ics"),
    ] {
        let data = SchedulingMessage::parse(fixture).unwrap().data.unwrap();
        assert_eq!(data.timezone, "America/New_York");
        assert_eq!(data.start, "2026-10-20T09:00:00");
        let event = CalendarEvent::new(WorkspaceId::generate(), Author::System, data);
        let built = SchedulingMessage::build(&event, SchedulingMethod::Request).unwrap();
        assert!(built.contains("TZID=America/New_York"));
        assert!(!built.contains("Eastern Standard Time"));
        let changed_body = fixture.replace("TZOFFSETTO:-0500", "TZOFFSETTO:+1200");
        assert_eq!(
            SchedulingMessage::parse(&changed_body)
                .unwrap()
                .data
                .unwrap(),
            event.data
        );
    }
    assert!(
        SchedulingMessage::parse(include_str!("fixtures/imip/invalid-zone-request.ics")).is_err()
    );
}

#[test]
fn duration_and_default_date_end_are_supported() {
    let apple = include_str!("fixtures/imip/apple-request.ics");
    assert_eq!(
        SchedulingMessage::parse(apple).unwrap().data.unwrap().end,
        "2026-10-20T10:00:00"
    );
    let date = include_str!("fixtures/imip/apple-date-request.ics");
    assert_eq!(
        SchedulingMessage::parse(date).unwrap().data.unwrap().end,
        "2026-10-21"
    );
    for (duration, end) in [("P2D", "2026-10-22"), ("P1W", "2026-10-27")] {
        let input = date.replace("SUMMARY:", &format!("DURATION:{duration}\r\nSUMMARY:"));
        assert_eq!(
            SchedulingMessage::parse(&input).unwrap().data.unwrap().end,
            end
        );
    }
    for duration in ["-PT1H", "P1M", "P1Y", "PT0.5S", "P0D", "garbage"] {
        assert!(
            SchedulingMessage::parse(&apple.replace("PT1H", duration)).is_err(),
            "{duration}"
        );
    }
    assert!(
        SchedulingMessage::parse(&date.replace("SUMMARY:", "DURATION:PT1H\r\nSUMMARY:")).is_err()
    );
    // Calendar days cross daylight saving changes before exact hours are added.
    let dst = apple
        .replace("20261020T090000", "20260307T120000")
        .replace("PT1H", "P1DT2H");
    assert_eq!(
        SchedulingMessage::parse(&dst).unwrap().data.unwrap().end,
        "2026-03-08T14:00:00"
    );
    let exact = dst.replace("P1DT2H", "PT24H");
    assert_eq!(
        SchedulingMessage::parse(&exact).unwrap().data.unwrap().end,
        "2026-03-08T13:00:00"
    );
}

async fn apply_fixture(
    store: &dyn pagis_core::CalendarStore,
    workspace: &WorkspaceId,
    input: &str,
) -> Option<CalendarEvent> {
    SchedulingMessage::parse(input)
        .unwrap()
        .apply(
            store,
            workspace,
            "reader@example.com",
            "organizer@example.org",
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn occurrence_cancellation_keeps_the_series_and_excludes_only_that_date() {
    use pagis_core::calendar::EventStatus;
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let event = apply_fixture(
        stores.calendar.as_ref(),
        &workspace,
        include_str!("fixtures/imip/google-request.ics"),
    )
    .await
    .unwrap();
    apply_fixture(
        stores.calendar.as_ref(),
        &workspace,
        include_str!("fixtures/imip/google-occurrence-cancel.ics"),
    )
    .await;
    let event = stores
        .calendar
        .get(&workspace, &event.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.data.invitation.unwrap().status,
        EventStatus::Tentative
    );
    assert!(event.data.exdates.contains(&"2026-11-03T09:00:00".into()));
    assert!(event.data.rrule.is_some());
}

#[tokio::test]
async fn occurrence_reply_does_not_change_series_participation() {
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let event = apply_fixture(
        stores.calendar.as_ref(),
        &workspace,
        include_str!("fixtures/imip/google-request.ics"),
    )
    .await
    .unwrap();
    SchedulingMessage::parse(include_str!("fixtures/imip/google-occurrence-reply.ics"))
        .unwrap()
        .apply(
            stores.calendar.as_ref(),
            &workspace,
            "organizer@example.org",
            "reader@example.com",
        )
        .await
        .unwrap();
    assert_eq!(
        stores
            .calendar
            .get(&workspace, &event.id)
            .await
            .unwrap()
            .unwrap(),
        event
    );
}

#[tokio::test]
async fn moved_occurrence_is_a_single_linked_event_and_series_cancel_cancels_both() {
    use pagis_core::calendar::EventStatus;
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let request = include_str!("fixtures/imip/google-occurrence-request.ics");
    assert!(
        apply_fixture(stores.calendar.as_ref(), &workspace, request)
            .await
            .is_none()
    );
    let series = apply_fixture(
        stores.calendar.as_ref(),
        &workspace,
        include_str!("fixtures/imip/google-request.ics"),
    )
    .await
    .unwrap();
    let moved = apply_fixture(stores.calendar.as_ref(), &workspace, request)
        .await
        .unwrap();
    assert_ne!(moved.id, series.id);
    assert_eq!(moved.data.start, "2026-11-03T11:00:00");
    assert!(moved.data.rrule.is_none());
    let built = SchedulingMessage::build(&moved, SchedulingMethod::Reply).unwrap();
    assert!(built.contains("UID:meeting@example.org"));
    assert!(built.contains("RECURRENCE-ID:20261103T140000Z"));
    apply_fixture(stores.calendar.as_ref(), &workspace, request).await;
    assert_eq!(
        stores
            .calendar
            .search(&workspace, "", 20)
            .await
            .unwrap()
            .len(),
        2
    );
    let master = stores
        .calendar
        .get(&workspace, &series.id)
        .await
        .unwrap()
        .unwrap();
    assert!(master.data.exdates.contains(&"2026-11-03T09:00:00".into()));
    apply_fixture(
        stores.calendar.as_ref(),
        &workspace,
        include_str!("fixtures/imip/google-cancel.ics"),
    )
    .await;
    for event in stores.calendar.search(&workspace, "", 20).await.unwrap() {
        assert_eq!(
            event.data.invitation.unwrap().status,
            EventStatus::Cancelled
        );
    }
    apply_fixture(stores.calendar.as_ref(), &workspace, request).await;
    assert_eq!(
        stores
            .calendar
            .get(&workspace, &moved.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .invitation
            .unwrap()
            .status,
        EventStatus::Cancelled
    );
}

#[tokio::test]
async fn occurrence_sequence_blocks_old_messages_and_equal_sequence_resurrection() {
    use pagis_core::calendar::EventStatus;
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let store = stores.calendar.as_ref();
    apply_fixture(
        store,
        &workspace,
        include_str!("fixtures/imip/google-request.ics"),
    )
    .await;
    let cancel = include_str!("fixtures/imip/google-occurrence-cancel.ics");
    let request = include_str!("fixtures/imip/google-occurrence-request.ics");
    let cancelled = apply_fixture(store, &workspace, cancel).await.unwrap();
    let unchanged = apply_fixture(store, &workspace, request).await.unwrap();
    assert_eq!(unchanged, cancelled);
    let newer = request.replace("SEQUENCE:2", "SEQUENCE:3");
    let moved = apply_fixture(store, &workspace, &newer).await.unwrap();
    assert_eq!(moved.id, cancelled.id);
    assert_eq!(
        moved.data.invitation.as_ref().unwrap().status,
        EventStatus::Tentative
    );
    apply_fixture(store, &workspace, cancel).await;
    apply_fixture(
        store,
        &workspace,
        include_str!("fixtures/imip/google-cancel.ics"),
    )
    .await;
    assert_eq!(
        store.get(&workspace, &moved.id).await.unwrap().unwrap(),
        moved
    );
    apply_fixture(
        store,
        &workspace,
        &include_str!("fixtures/imip/google-cancel.ics").replace("SEQUENCE:2", "SEQUENCE:3"),
    )
    .await;
    assert_eq!(
        store
            .get(&workspace, &moved.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .invitation
            .unwrap()
            .status,
        EventStatus::Cancelled
    );
}

#[tokio::test]
async fn all_day_occurrences_keep_date_identity() {
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    let workspace = WorkspaceId::generate();
    let request = include_str!("fixtures/imip/apple-date-request.ics")
        .replace("SUMMARY:", "RRULE:FREQ=WEEKLY;COUNT=3\r\nSUMMARY:");
    let series = apply_fixture(stores.calendar.as_ref(), &workspace, &request)
        .await
        .unwrap();
    let occurrence = request
        .replace("SEQUENCE:1", "SEQUENCE:2")
        .replace(
            "DTSTART;VALUE=DATE:20261020",
            "RECURRENCE-ID;VALUE=DATE:20261103\r\nDTSTART;VALUE=DATE:20261104",
        )
        .replace("RRULE:FREQ=WEEKLY;COUNT=3\r\n", "");
    let moved = apply_fixture(stores.calendar.as_ref(), &workspace, &occurrence)
        .await
        .unwrap();
    assert_eq!(moved.data.end, "2026-11-05");
    assert!(
        SchedulingMessage::build(&moved, SchedulingMethod::Reply)
            .unwrap()
            .contains("RECURRENCE-ID;VALUE=DATE:20261103")
    );
    assert!(
        stores
            .calendar
            .get(&workspace, &series.id)
            .await
            .unwrap()
            .unwrap()
            .data
            .exdates
            .contains(&"2026-11-03".into())
    );
}
