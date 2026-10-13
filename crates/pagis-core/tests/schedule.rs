use pagis_core::{
    CreatorKind, Schedule, ScheduleKind, ScheduleState, cron_window, interval_window, missed_count,
    next_after, next_cron_occurrence, schedule_window,
};

fn millis(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_millis()
}

fn schedule(kind: ScheduleKind) -> Schedule {
    Schedule {
        id: "schedule".to_string().into(),
        workspace_id: "workspace".to_string().into(),
        agent_id: "agent".to_string().into(),
        channel_id: "channel".to_string().into(),
        name: "Schedule".into(),
        instruction: "Read the work record".into(),
        subject_page_path: None,
        root_message_id: None,
        kind,
        cron_expression: Some("0 9 * * *".into()),
        interval_ms: Some(60_000),
        anchor_at: Some(120_000),
        timezone: "America/Los_Angeles".into(),
        scheduled_at: 120_000,
        next_due_at: Some(120_000),
        last_result: None,
        state: ScheduleState::Active,
        revision: 1,
        approved_revision: Some(1),
        creator: CreatorKind::User,
        creating_run_id: None,
        created_at: 0,
        updated_at: 0,
        archived_at: None,
    }
}

#[test]
fn interval_window_before_at_and_after_the_anchor() {
    for (now, expected) in [
        (-1, (None, 120_000)),
        (119_999, (None, 120_000)),
        (120_000, (Some(120_000), 180_000)),
        (179_999, (Some(120_000), 180_000)),
        (180_000, (Some(180_000), 240_000)),
        (300_001, (Some(300_000), 360_000)),
    ] {
        assert_eq!(interval_window(120_000, 60_000, now).unwrap(), expected);
        let row = schedule(ScheduleKind::Interval);
        assert_eq!(
            schedule_window(&row, now).unwrap(),
            (expected.0, Some(expected.1))
        );
        assert_eq!(next_after(&row, now).unwrap(), Some(expected.1));
    }
}

#[test]
fn cron_window_keeps_local_time_across_timezone_offset_changes() {
    for (now, latest, next) in [
        (
            "2026-03-07T17:00:00Z",
            "2026-03-07T17:00:00Z",
            "2026-03-08T16:00:00Z",
        ),
        (
            "2026-03-08T15:59:59Z",
            "2026-03-07T17:00:00Z",
            "2026-03-08T16:00:00Z",
        ),
        (
            "2026-03-08T16:00:00Z",
            "2026-03-08T16:00:00Z",
            "2026-03-09T16:00:00Z",
        ),
        (
            "2026-10-31T16:00:00Z",
            "2026-10-31T16:00:00Z",
            "2026-11-01T17:00:00Z",
        ),
        (
            "2026-11-01T17:00:00Z",
            "2026-11-01T17:00:00Z",
            "2026-11-02T17:00:00Z",
        ),
    ] {
        let row = schedule(ScheduleKind::Cron);
        let now = millis(now);
        let (latest, next) = (millis(latest), millis(next));
        assert_eq!(
            cron_window("0 9 * * *", &row.timezone, now).unwrap(),
            (latest, next)
        );
        assert_eq!(
            schedule_window(&row, now).unwrap(),
            (Some(latest), Some(next))
        );
        assert_eq!(next_after(&row, now).unwrap(), Some(next));
        assert_eq!(
            next_cron_occurrence("0 9 * * *", &row.timezone, now).unwrap(),
            next
        );
    }
}

#[test]
fn cron_window_uses_the_schedules_timezone() {
    let now = millis("2026-03-08T16:30:00Z");
    assert_eq!(
        cron_window("0 9 * * *", "UTC", now).unwrap(),
        (
            millis("2026-03-08T09:00:00Z"),
            millis("2026-03-09T09:00:00Z")
        )
    );
    assert_eq!(
        cron_window("0 9 * * *", "America/Los_Angeles", now).unwrap(),
        (
            millis("2026-03-08T16:00:00Z"),
            millis("2026-03-09T16:00:00Z")
        )
    );
}

#[test]
fn interval_missed_count_includes_the_previous_due_time() {
    let row = schedule(ScheduleKind::Interval);
    assert_eq!(missed_count(&row, 120_000, 120_000).unwrap(), 0);
    assert_eq!(missed_count(&row, 180_000, 120_000).unwrap(), 0);
    assert_eq!(missed_count(&row, 120_000, 180_000).unwrap(), 1);
    assert_eq!(missed_count(&row, 120_000, 300_000).unwrap(), 3);
    assert_eq!(missed_count(&row, 0, i64::MAX).unwrap(), u32::MAX);
}

#[test]
fn cron_missed_count_excludes_both_due_times_across_an_offset_change() {
    let row = schedule(ScheduleKind::Cron);
    let previous = millis("2026-03-07T17:00:00Z");
    assert_eq!(missed_count(&row, previous, previous).unwrap(), 0);
    assert_eq!(missed_count(&row, previous, previous - 1).unwrap(), 0);
    assert_eq!(
        missed_count(&row, previous, millis("2026-03-08T16:00:00Z")).unwrap(),
        0
    );
    assert_eq!(
        missed_count(&row, previous, millis("2026-03-10T16:00:00Z")).unwrap(),
        2
    );
}

#[test]
fn one_shot_has_no_next_time_after_it_is_due() {
    let row = schedule(ScheduleKind::OneShot);
    assert_eq!(
        schedule_window(&row, 119_999).unwrap(),
        (None, Some(120_000))
    );
    assert_eq!(
        schedule_window(&row, 120_000).unwrap(),
        (Some(120_000), None)
    );
    assert_eq!(
        schedule_window(&row, 180_000).unwrap(),
        (Some(120_000), None)
    );
    assert_eq!(next_after(&row, 120_000).unwrap(), None);
    assert_eq!(missed_count(&row, 0, 180_000).unwrap(), 0);
}

#[test]
fn invalid_schedule_fields_return_errors() {
    for interval in [0, -1] {
        assert!(interval_window(0, interval, 1).is_err());
    }
    assert!(interval_window(i64::MAX, 60_000, i64::MAX).is_err());
    assert!(cron_window("invalid", "UTC", 0).is_err());
    assert!(next_cron_occurrence("0 0 9 * * *", "UTC", 0).is_err());
    assert!(cron_window("0 9 * * *", "invalid", 0).is_err());
    assert!(cron_window("0 9 * * *", "UTC", i64::MAX).is_err());
    let mut row = schedule(ScheduleKind::Interval);
    row.anchor_at = None;
    assert!(schedule_window(&row, 120_000).is_err());
    row = schedule(ScheduleKind::Cron);
    row.cron_expression = None;
    assert!(next_after(&row, 0).is_err());
}
