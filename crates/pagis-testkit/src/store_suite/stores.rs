//! The trait tests of the record stores of a Workspace.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_stores!` below:
//! the guard test of the parent module fails while one is missing.

// A field of the store set is an `Arc<dyn Trait>`, so a method of the
// trait reads without the trait in scope. Only the values a body builds
// are named here.
use pagis_core::{
    AgentId, Artifact, ArtifactId, ArtifactOutcome, Block, CapabilitySnapshotRecord, ChannelId,
    ConnectionId, DecideOutcome, Grant, GrantId, HostId, MemoryExposure, MessageId, MessageStatus,
    NewEvent, Plugin, PluginBinding, PluginBindingValue, PluginId, PluginSource, PluginState,
    PluginTool, PluginTools, RequestState, RunState, Schedule, ScheduleId, ScheduleState,
    SendOutcome, TrustEntry, TrustEntryId, TrustSubject, TrustTier, WorkspaceId,
};

use crate::fixture::{
    agent, agent_message, agent_participant, artifact, channel, credential_grant, host_grant,
    missed_call, pending_form_request, pending_request, queued_run, software_package,
    software_version, user_message, user_participant,
};

use super::Backend;

pub async fn set_chief_of_staff_names_an_agent_and_clears_it(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let store = &backend.stores().workspaces;
    let agent = agent(&ws.id);
    backend.stores().agents.create(&agent).await.unwrap();
    assert_eq!(
        store
            .get(&ws.id)
            .await
            .unwrap()
            .unwrap()
            .chief_of_staff_agent_id,
        None
    );

    store
        .set_chief_of_staff(&ws.id, Some(&agent.id))
        .await
        .unwrap();
    assert_eq!(
        store
            .get(&ws.id)
            .await
            .unwrap()
            .unwrap()
            .chief_of_staff_agent_id,
        Some(agent.id.clone())
    );

    store.set_chief_of_staff(&ws.id, None).await.unwrap();
    assert_eq!(
        store
            .get(&ws.id)
            .await
            .unwrap()
            .unwrap()
            .chief_of_staff_agent_id,
        None
    );
}

pub async fn one_shot_schedule_history_roundtrips_through_the_store(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    let channel = channel(&workspace.id);
    backend.stores().channels.create(&channel).await.unwrap();
    let now = pagis_core::now_ms();
    let schedule = Schedule {
        id: ScheduleId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        name: "Morning plan".to_string(),
        instruction: "Prepare today's plan".to_string(),
        subject_page_path: Some("private/subjects/gmail/cedar.md".into()),
        channel_id: channel.id,
        root_message_id: None,
        kind: pagis_core::ScheduleKind::OneShot,
        cron_expression: None,
        interval_ms: None,
        anchor_at: None,
        timezone: "UTC".to_string(),
        scheduled_at: now + 60_000,
        next_due_at: Some(now + 60_000),
        last_result: None,
        state: ScheduleState::Active,
        revision: 1,
        approved_revision: None,
        creator: pagis_core::CreatorKind::User,
        creating_run_id: None,
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    let schedules = &backend.stores().schedules;
    schedules.create(&schedule).await.unwrap();
    assert_eq!(
        schedules.get(&workspace.id, &schedule.id).await.unwrap(),
        Some(schedule.clone())
    );
    assert_eq!(
        schedules
            .list_revisions(&workspace.id, &schedule.id)
            .await
            .unwrap()[0]
            .subject_page_path,
        schedule.subject_page_path
    );
    assert_eq!(
        schedules
            .list_open_for_subject_page(
                &workspace.id,
                &agent.id,
                "private/subjects/gmail/cedar.md",
            )
            .await
            .unwrap(),
        vec![pagis_core::subject_page::OpenSchedule {
            id: schedule.id.to_string(),
            next_due_at: schedule.next_due_at.unwrap(),
            purpose: "Morning plan: Prepare today's plan".into(),
        }]
    );

    let triggers = &backend.stores().triggers;
    let due = triggers.process_due(now + 60_000).await.unwrap();
    assert_eq!(due.occurrences.len(), 1);
    assert_eq!(due.wakeups.len(), 1);
    let occurrences = schedules
        .list_occurrences(&workspace.id, &schedule.id, None, 10)
        .await
        .unwrap();
    let wakeups = schedules
        .list_wakeups(&workspace.id, &schedule.id, None, 10)
        .await
        .unwrap();
    assert_eq!(occurrences[0].wakeup_id.as_ref(), Some(&wakeups[0].id));
    let claims = triggers
        .claim_wakeups(
            &workspace.id,
            &agent.id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            now + 60_001,
        )
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].run.title, wakeups[0].rule_name);
    assert_eq!(
        claims[0].run.trigger_ref.as_deref(),
        Some(wakeups[0].id.as_str())
    );
}

pub async fn agent_update_writes_the_mutable_fields(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let store = &backend.stores().agents;
    let mut a = agent(&ws.id);
    store.create(&a).await.unwrap();
    assert_eq!(
        store.list_by_workspace(&ws.id).await.unwrap(),
        vec![a.clone()]
    );

    a.name = "Rex".to_string();
    a.job = "researcher".to_string();
    a.personality = "curious".to_string();
    a.voice = Some("nova".to_string());
    a.status = pagis_core::AgentStatus::Archived;
    a.updated_at += 1;
    store.update(&a).await.unwrap();

    assert_eq!(store.get(&ws.id, &a.id).await.unwrap(), Some(a.clone()));

    // A voice is cleared the same way it is set (ADR-0020: absent
    // declares absent).
    a.voice = None;
    store.update(&a).await.unwrap();
    assert_eq!(store.get(&ws.id, &a.id).await.unwrap(), Some(a));
}

pub async fn event_append_assigns_increasing_seq_and_lists_after(backend: &Backend) {
    let ws = backend.seeded_workspace().await;

    let log = &backend.stores().events;
    let first = log
        .append(NewEvent {
            workspace_id: ws.id.clone(),
            event_type: "workspace.created".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({"name": "Workspace"}),
        })
        .await
        .unwrap();
    let second = log
        .append(NewEvent {
            workspace_id: ws.id.clone(),
            event_type: "agent.created".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({"name": "Sage"}),
        })
        .await
        .unwrap();

    assert!(second.seq > first.seq);
    assert_eq!(first.event_type, "workspace.created");

    let after_first = log.list_after(None, first.seq, 100).await.unwrap();
    assert_eq!(after_first, vec![second.clone()]);

    let none_yet = log.list_after(None, i64::MAX, 100).await.unwrap();
    assert!(none_yet.is_empty());

    assert_eq!(log.latest_seq().await.unwrap(), second.seq);
}

/// A read of one Workspace's events skips every other Workspace's, on
/// both backends. The event bus is what asks for it: a socket
/// receives its own tenant's events and no other, live and on replay.
pub async fn events_of_one_workspace_list_without_the_others(backend: &Backend) {
    let first = backend.seeded_workspace().await;
    let second = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        ..first.clone()
    };
    backend
        .stores()
        .workspaces
        .create(&second)
        .await
        .expect("write the second Workspace");

    let log = &backend.stores().events;
    let mut appended = Vec::new();
    for workspace_id in [&first.id, &second.id, &first.id] {
        appended.push(
            log.append(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "agent.created".to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({}),
            })
            .await
            .unwrap(),
        );
    }
    let before = appended[0].seq - 1;

    let of_first = log.list_after(Some(&first.id), before, 100).await.unwrap();
    let of_second = log.list_after(Some(&second.id), before, 100).await.unwrap();

    assert_eq!(
        of_first.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![appended[0].seq, appended[2].seq],
        "the first Workspace reads its own two events in seq order"
    );
    assert_eq!(
        of_second.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![appended[1].seq]
    );
    // The unfiltered read still carries every Workspace, for the
    // daemon-lifetime consumers that serve the installation.
    assert_eq!(log.list_after(None, before, 100).await.unwrap().len(), 3);
}

pub async fn channel_roundtrips_through_store(backend: &Backend) {
    let ws = backend.seeded_workspace().await;

    let store = &backend.stores().channels;
    let ch = channel(&ws.id);
    let group = crate::fixture::group_channel(&ws.id, "ops");

    store.create(&ch).await.unwrap();
    store.create(&group).await.unwrap();

    assert_eq!(store.get(&ws.id, &ch.id).await.unwrap(), Some(ch));
    assert_eq!(store.get(&ws.id, &group.id).await.unwrap(), Some(group));
    assert_eq!(
        store.get(&ws.id, &ChannelId::generate()).await.unwrap(),
        None
    );
}

pub async fn channel_list_scopes_to_workspace_newest_update_first(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other_ws = backend.seeded_workspace().await;

    let store = &backend.stores().channels;
    let mut older = channel(&ws.id);
    older.updated_at -= 1000;
    let newer = channel(&ws.id);
    let elsewhere = channel(&other_ws.id);
    store.create(&older).await.unwrap();
    store.create(&newer).await.unwrap();
    store.create(&elsewhere).await.unwrap();

    assert_eq!(
        store.list_by_workspace(&ws.id).await.unwrap(),
        vec![newer, older]
    );
}

pub async fn message_insert_roundtrips_and_gets(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let msg = user_message(&ws.id, &ch.id, "hello");

    let outcome = store.insert(&msg).await.unwrap();

    assert_eq!(outcome, SendOutcome::Created(msg.clone()));
    assert_eq!(store.get(&ws.id, &msg.id).await.unwrap(), Some(msg));
    assert_eq!(
        store.get(&ws.id, &MessageId::generate()).await.unwrap(),
        None
    );
}

pub async fn retained_turn_exposure_is_daemon_stamped_and_round_trips(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();
    let store = &backend.stores().messages;
    let msg = user_message(&ws.id, &ch.id, "source-derived answer");
    store.insert(&msg).await.unwrap();
    assert_eq!(
        store.exposures(&ws.id, &msg.id).await.unwrap(),
        Some(vec![])
    );

    for author_kind in [
        pagis_core::AuthorKind::Agent,
        pagis_core::AuthorKind::System,
    ] {
        let mut unstamped = user_message(&ws.id, &ch.id, "unstamped generated text");
        unstamped.author_kind = author_kind;
        store.insert(&unstamped).await.unwrap();
        assert_eq!(store.exposures(&ws.id, &unstamped.id).await.unwrap(), None);
        store
            .set_exposures(&ws.id, &unstamped.id, &[])
            .await
            .unwrap();
        assert_eq!(
            store.exposures(&ws.id, &unstamped.id).await.unwrap(),
            Some(vec![])
        );
    }

    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 7,
    };
    store
        .set_exposures(&ws.id, &msg.id, std::slice::from_ref(&exposure))
        .await
        .unwrap();
    assert_eq!(
        store.exposures(&ws.id, &msg.id).await.unwrap(),
        Some(vec![exposure])
    );
}

pub async fn a_stamped_insert_writes_the_row_and_its_stamp_together(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();
    let store = &backend.stores().messages;
    let exposure = MemoryExposure {
        grant_id: GrantId::generate(),
        revision: 3,
    };
    let mut generated = user_message(&ws.id, &ch.id, "a sourced answer");
    generated.author_kind = pagis_core::AuthorKind::Agent;

    let outcome = store
        .insert_stamped(&generated, std::slice::from_ref(&exposure))
        .await
        .unwrap();

    assert_eq!(outcome, SendOutcome::Created(generated.clone()));
    assert_eq!(
        store.exposures(&ws.id, &generated.id).await.unwrap(),
        Some(vec![exposure.clone()])
    );

    // A stamp that cannot be written takes the row with it, so no
    // reader ever finds a generated message without its stamp.
    let mut rejected = user_message(&ws.id, &ch.id, "never stored");
    rejected.author_kind = pagis_core::AuthorKind::Agent;
    let repeated = [exposure.clone(), exposure];
    assert!(store.insert_stamped(&rejected, &repeated).await.is_err());
    assert_eq!(store.get(&ws.id, &rejected.id).await.unwrap(), None);
}

pub async fn duplicate_pending_id_returns_the_original(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let original = user_message(&ws.id, &ch.id, "hello");
    store.insert(&original).await.unwrap();

    let mut retry = user_message(&ws.id, &ch.id, "hello again");
    retry.pending_id = original.pending_id.clone();

    let outcome = store.insert(&retry).await.unwrap();

    assert_eq!(outcome, SendOutcome::Deduplicated(original.clone()));
    assert_eq!(store.get(&ws.id, &retry.id).await.unwrap(), None);
}

pub async fn same_pending_id_in_another_channel_is_a_new_message(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let channels = &backend.stores().channels;
    let first = channel(&ws.id);
    let second = channel(&ws.id);
    channels.create(&first).await.unwrap();
    channels.create(&second).await.unwrap();

    let store = &backend.stores().messages;
    let msg = user_message(&ws.id, &first.id, "hello");
    store.insert(&msg).await.unwrap();

    let mut other = user_message(&ws.id, &second.id, "hello");
    other.pending_id = msg.pending_id.clone();

    assert_eq!(
        store.insert(&other).await.unwrap(),
        SendOutcome::Created(other)
    );
}

pub async fn timeline_pages_newest_first_with_exclusive_cursor(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let mut sent = Vec::new();
    for i in 0..5 {
        let msg = user_message(&ws.id, &ch.id, &format!("m{i}"));
        store.insert(&msg).await.unwrap();
        sent.push(msg);
    }
    // A thread reply must not appear in the timeline.
    let mut reply = user_message(&ws.id, &ch.id, "reply");
    reply.parent_message_id = Some(sent[0].id.clone());
    store.insert(&reply).await.unwrap();

    let page = store.list_top_level(&ws.id, &ch.id, None, 2).await.unwrap();
    let messages: Vec<_> = page.iter().map(|e| e.message.clone()).collect();
    assert_eq!(messages, vec![sent[4].clone(), sent[3].clone()]);

    let next = store
        .list_top_level(&ws.id, &ch.id, Some(&page[1].message.id), 10)
        .await
        .unwrap();
    let messages: Vec<_> = next.iter().map(|e| e.message.clone()).collect();
    assert_eq!(
        messages,
        vec![sent[2].clone(), sent[1].clone(), sent[0].clone()]
    );
}

pub async fn timeline_entries_carry_thread_rollups(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let root = user_message(&ws.id, &ch.id, "root");
    store.insert(&root).await.unwrap();
    let bare = user_message(&ws.id, &ch.id, "no replies");
    store.insert(&bare).await.unwrap();
    let mut last_reply_at = 0;
    for i in 0..2 {
        let mut reply = user_message(&ws.id, &ch.id, &format!("r{i}"));
        reply.parent_message_id = Some(root.id.clone());
        store.insert(&reply).await.unwrap();
        last_reply_at = reply.created_at;
    }

    // A derived progress row sits in the thread but is not a
    // reply, so the rollup passes it over.
    let mut progress = user_message(&ws.id, &ch.id, "Done");
    progress.parent_message_id = Some(root.id.clone());
    progress.blocks = vec![pagis_core::Block::progress("run_1", "Done")];
    store.insert(&progress).await.unwrap();

    let page = store
        .list_top_level(&ws.id, &ch.id, None, 10)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    let bare_entry = &page[0];
    assert_eq!(bare_entry.message, bare);
    assert_eq!(bare_entry.reply_count, 0);
    assert_eq!(bare_entry.last_reply_at, None);
    assert!(bare_entry.reply_authors.is_empty());
    let root_entry = &page[1];
    assert_eq!(root_entry.message, root);
    assert_eq!(root_entry.reply_count, 2);
    assert_eq!(root_entry.last_reply_at, Some(last_reply_at));
}

pub async fn timeline_entries_name_the_newest_reply_authors(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let agents = &backend.stores().agents;
    let mut roster = Vec::new();
    for _ in 0..3 {
        let a = agent(&ws.id);
        agents.create(&a).await.unwrap();
        roster.push(a.id);
    }
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let root = user_message(&ws.id, &ch.id, "root");
    store.insert(&root).await.unwrap();
    // Six replies from four authors. The user writes twice, so the
    // user counts once, at the newest of the two replies.
    let authors = [
        None,
        Some(&roster[0]),
        Some(&roster[1]),
        None,
        Some(&roster[2]),
        Some(&roster[1]),
    ];
    for (i, author) in authors.iter().enumerate() {
        let mut reply = match author {
            None => user_message(&ws.id, &ch.id, &format!("r{i}")),
            Some(id) => agent_message(&ws.id, &ch.id, id, &format!("r{i}")),
        };
        reply.parent_message_id = Some(root.id.clone());
        reply.created_at = 1_000 + i as i64;
        store.insert(&reply).await.unwrap();
    }

    let page = store
        .list_top_level(&ws.id, &ch.id, None, 10)
        .await
        .unwrap();
    let names: Vec<_> = page[0]
        .reply_authors
        .iter()
        .map(|author| (author.kind, author.agent_id.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            (pagis_core::AuthorKind::Agent, Some(roster[1].clone())),
            (pagis_core::AuthorKind::Agent, Some(roster[2].clone())),
            (pagis_core::AuthorKind::User, None),
        ]
    );
}

/// A row of another Workspace under a Thread root is not a reply of
/// that Thread. The rollup counts, dates and names the replies of the
/// root's own Workspace only, whatever Channel or root the other row
/// holds.
pub async fn timeline_rollups_count_only_replies_of_their_own_workspace(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();
    let stranger = agent(&other.id);
    backend.stores().agents.create(&stranger).await.unwrap();

    let store = &backend.stores().messages;
    let root = user_message(&ws.id, &ch.id, "root");
    store.insert(&root).await.unwrap();
    let mut own = user_message(&ws.id, &ch.id, "own reply");
    own.parent_message_id = Some(root.id.clone());
    own.created_at = 1_000;
    store.insert(&own).await.unwrap();
    // The reply of an Agent of the other Workspace: the other
    // Workspace's id, with the Channel and the Thread root of this one.
    let mut foreign = agent_message(&other.id, &ch.id, &stranger.id, "foreign reply");
    foreign.parent_message_id = Some(root.id.clone());
    foreign.created_at = 2_000;
    store.insert(&foreign).await.unwrap();

    let page = store
        .list_top_level(&ws.id, &ch.id, None, 10)
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    let entry = &page[0];
    assert_eq!(entry.message, root);
    assert_eq!(entry.reply_count, 1);
    assert_eq!(entry.last_reply_at, Some(1_000));
    assert_eq!(
        entry.reply_authors,
        vec![pagis_core::ReplyAuthor {
            kind: pagis_core::AuthorKind::User,
            agent_id: None,
        }]
    );
}

pub async fn thread_lists_root_and_replies_oldest_first(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let root = user_message(&ws.id, &ch.id, "root");
    store.insert(&root).await.unwrap();
    let other = user_message(&ws.id, &ch.id, "other top-level");
    store.insert(&other).await.unwrap();
    let mut replies = Vec::new();
    for i in 0..2 {
        let mut reply = user_message(&ws.id, &ch.id, &format!("r{i}"));
        reply.parent_message_id = Some(root.id.clone());
        store.insert(&reply).await.unwrap();
        replies.push(reply);
    }

    let thread = store.list_thread(&ws.id, &root.id).await.unwrap();
    assert_eq!(thread, vec![root, replies[0].clone(), replies[1].clone()]);
}

pub async fn finalize_settles_a_streaming_message(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let mut msg = user_message(&ws.id, &ch.id, "");
    msg.status = MessageStatus::Streaming;
    msg.completed_at = None;
    store.insert(&msg).await.unwrap();

    let blocks = vec![Block::markdown("Hello world")];
    store
        .finalize(
            &ws.id,
            &msg.id,
            MessageStatus::Complete,
            &blocks,
            "Hello world",
            42,
        )
        .await
        .unwrap();

    let stored = store.get(&ws.id, &msg.id).await.unwrap().unwrap();
    assert_eq!(stored.status, MessageStatus::Complete);
    assert_eq!(stored.blocks, blocks);
    assert_eq!(stored.text_content, "Hello world");
    assert_eq!(stored.completed_at, Some(42));
}

pub async fn fail_streaming_marks_only_streaming_messages(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().messages;
    let complete = user_message(&ws.id, &ch.id, "done");
    store.insert(&complete).await.unwrap();
    let mut streaming = user_message(&ws.id, &ch.id, "partial");
    streaming.status = MessageStatus::Streaming;
    streaming.completed_at = None;
    store.insert(&streaming).await.unwrap();

    assert_eq!(store.fail_streaming(9).await.unwrap(), 1);

    let swept = store.get(&ws.id, &streaming.id).await.unwrap().unwrap();
    assert_eq!(swept.status, MessageStatus::Failed);
    assert_eq!(swept.completed_at, Some(9));
    let untouched = store.get(&ws.id, &complete.id).await.unwrap().unwrap();
    assert_eq!(untouched.status, MessageStatus::Complete);
}

pub async fn run_roundtrips_and_updates_through_store(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();

    let store = &backend.stores().runs;
    let mut run = queued_run(&ws.id, &a.id, &ch.id);
    run.title = "Book the Austin trip".into();
    store.create(&run).await.unwrap();
    assert_eq!(store.get(&ws.id, &run.id).await.unwrap(), Some(run.clone()));
    assert_eq!(store.list_unfinished().await.unwrap(), vec![run.clone()]);

    run.state = RunState::Failed;
    run.error = Some("boom".to_string());
    run.started_at = Some(1);
    run.ended_at = Some(2);
    run.title = "A later state update cannot rename the work".into();
    store.update(&run).await.unwrap();
    run.title = "Book the Austin trip".into();

    assert_eq!(store.get(&ws.id, &run.id).await.unwrap(), Some(run));
    assert_eq!(store.list_unfinished().await.unwrap(), vec![]);
}

pub async fn a_dismissed_run_keeps_its_first_dismissal(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other_ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();
    let store = &backend.stores().runs;
    let mut run = queued_run(&ws.id, &a.id, &ch.id);
    store.create(&run).await.unwrap();

    assert!(!store.dismiss(&other_ws.id, &run.id, 5).await.unwrap());
    assert!(store.dismiss(&ws.id, &run.id, 10).await.unwrap());
    assert!(store.dismiss(&ws.id, &run.id, 20).await.unwrap());
    // A later write of the run state leaves the dismissal alone.
    run.state = RunState::Failed;
    store.update(&run).await.unwrap();

    let read = store.get(&ws.id, &run.id).await.unwrap().unwrap();
    assert_eq!(read.dismissed_at, Some(10));
    assert_eq!(read.state, RunState::Failed);
}

pub async fn a_dismissed_call_keeps_its_first_dismissal(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other_ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let ch = channel(&ws.id);
    backend.stores().channels.create(&ch).await.unwrap();
    let run = queued_run(&ws.id, &a.id, &ch.id);
    backend.stores().runs.create(&run).await.unwrap();
    let store = &backend.stores().calls;
    let call = missed_call(&ws.id, &a.id, &run.id);
    store.insert(&call).await.unwrap();

    assert!(!store.dismiss(&other_ws.id, &call.id, 5).await.unwrap());
    assert!(store.dismiss(&ws.id, &call.id, 10).await.unwrap());
    assert!(store.dismiss(&ws.id, &call.id, 20).await.unwrap());
    // A later write of the record leaves the dismissal alone.
    store.update(&call).await.unwrap();

    let read = store.get(&ws.id, &call.id).await.unwrap().unwrap();
    assert_eq!(read.dismissed_at, Some(10));
}

pub async fn artifact_roundtrips_through_store(backend: &Backend) {
    let ws = backend.seeded_workspace().await;

    let store = &backend.stores().artifacts;
    let a = artifact(&ws.id, "aa11");

    let outcome = store.insert(&a).await.unwrap();

    assert_eq!(outcome, ArtifactOutcome::Created(a.clone()));
    assert_eq!(store.get(&ws.id, &a.id).await.unwrap(), Some(a));
    assert_eq!(
        store.get(&ws.id, &ArtifactId::generate()).await.unwrap(),
        None
    );
}

pub async fn duplicate_content_dedups_to_the_original_artifact(backend: &Backend) {
    let ws = backend.seeded_workspace().await;

    let store = &backend.stores().artifacts;
    let original = artifact(&ws.id, "aa11");
    store.insert(&original).await.unwrap();

    let mut retry = artifact(&ws.id, "aa11");
    retry.filename = Some("copy.png".to_string());

    let outcome = store.insert(&retry).await.unwrap();

    assert_eq!(outcome, ArtifactOutcome::Deduplicated(original));
    assert_eq!(store.get(&ws.id, &retry.id).await.unwrap(), None);
}

pub async fn same_content_in_another_workspace_is_a_new_artifact(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other_ws = backend.seeded_workspace().await;

    let store = &backend.stores().artifacts;
    store.insert(&artifact(&ws.id, "aa11")).await.unwrap();

    let elsewhere = artifact(&other_ws.id, "aa11");

    assert_eq!(
        store.insert(&elsewhere).await.unwrap(),
        ArtifactOutcome::Created(elsewhere)
    );
}

pub async fn participants_resolve_channel_agents(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let dm = channel(&ws.id);
    let plain = channel(&ws.id);
    let channels = &backend.stores().channels;
    channels.create(&dm).await.unwrap();
    channels.create(&plain).await.unwrap();

    let store = &backend.stores().participants;
    store
        .create(&agent_participant(&ws.id, &dm.id, &a.id))
        .await
        .unwrap();

    assert_eq!(
        store.agents_in_channel(&ws.id, &dm.id).await.unwrap(),
        vec![a.id.clone()]
    );
    assert_eq!(
        store.agents_in_channel(&ws.id, &plain.id).await.unwrap(),
        vec![]
    );
}

pub async fn dm_lookups_resolve_user_and_agent_dms(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let agents = &backend.stores().agents;
    let a = agent(&ws.id);
    let b = pagis_core::Agent {
        name: "Scout".to_string(),
        ..agent(&ws.id)
    };
    agents.create(&a).await.unwrap();
    agents.create(&b).await.unwrap();

    let channels = &backend.stores().channels;
    let participants = &backend.stores().participants;
    let user_dm = channel(&ws.id);
    channels.create(&user_dm).await.unwrap();
    participants
        .create(&user_participant(&ws.id, &user_dm.id))
        .await
        .unwrap();
    participants
        .create(&agent_participant(&ws.id, &user_dm.id, &a.id))
        .await
        .unwrap();
    let agent_dm = channel(&ws.id);
    channels.create(&agent_dm).await.unwrap();
    for agent_id in [&a.id, &b.id] {
        participants
            .create(&agent_participant(&ws.id, &agent_dm.id, agent_id))
            .await
            .unwrap();
    }

    assert_eq!(
        channels.find_user_dm(&ws.id, &a.id).await.unwrap(),
        Some(user_dm.clone())
    );
    assert_eq!(channels.find_user_dm(&ws.id, &b.id).await.unwrap(), None);
    // The agent DM resolves in both directions and never matches the
    // user's DM.
    assert_eq!(
        channels.find_agent_dm(&ws.id, &a.id, &b.id).await.unwrap(),
        Some(agent_dm.clone())
    );
    assert_eq!(
        channels.find_agent_dm(&ws.id, &b.id, &a.id).await.unwrap(),
        Some(agent_dm)
    );
}

pub async fn participants_list_for_channel_returns_all_kinds(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let dm = channel(&ws.id);
    backend.stores().channels.create(&dm).await.unwrap();
    let store = &backend.stores().participants;
    let user = user_participant(&ws.id, &dm.id);
    let agent_row = agent_participant(&ws.id, &dm.id, &a.id);
    store.create(&user).await.unwrap();
    store.create(&agent_row).await.unwrap();

    let listed = store.list_for_channel(&ws.id, &dm.id).await.unwrap();

    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&user));
    assert!(listed.contains(&agent_row));
}

pub async fn agent_messages_elsewhere_page_newest_first_and_exclude_the_dm_and_progress(
    backend: &Backend,
) {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let channels = &backend.stores().channels;
    let dm = channel(&ws.id);
    let elsewhere = channel(&ws.id);
    channels.create(&dm).await.unwrap();
    channels.create(&elsewhere).await.unwrap();

    let store = &backend.stores().messages;
    let in_dm = agent_message(&ws.id, &dm.id, &a.id, "in the DM");
    let older = agent_message(&ws.id, &elsewhere.id, &a.id, "older");
    let newer = agent_message(&ws.id, &elsewhere.id, &a.id, "newer");
    let streaming = pagis_core::Message {
        status: MessageStatus::Streaming,
        completed_at: None,
        ..agent_message(&ws.id, &elsewhere.id, &a.id, "half")
    };
    // A derived progress row is daemon state the channel view
    // hides, so it is no word of this agent in the DM either.
    let progress = pagis_core::Message {
        blocks: vec![pagis_core::Block::progress("run-1", "Done")],
        ..agent_message(&ws.id, &elsewhere.id, &a.id, "Done")
    };
    for message in [&in_dm, &older, &newer, &streaming, &progress] {
        store.insert(message).await.unwrap();
    }

    let page = store
        .list_agent_elsewhere(&ws.id, &a.id, &dm.id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        page.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        vec![newer.id.clone(), older.id.clone()]
    );

    let rest = store
        .list_agent_elsewhere(&ws.id, &a.id, &dm.id, Some(&newer.id), 10)
        .await
        .unwrap();
    assert_eq!(
        rest.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        vec![older.id]
    );
}

/// Seed a workspace, an agent, and one run for request tests, and
/// answer the pending request they hold.
async fn seed_request(backend: &Backend) -> pagis_core::Request {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    let dm = channel(&ws.id);
    backend.stores().channels.create(&dm).await.unwrap();
    let run = queued_run(&ws.id, &a.id, &dm.id);
    backend.stores().runs.create(&run).await.unwrap();
    pending_request(&ws.id, &a.id, &run.id, "echo hi")
}

pub async fn request_roundtrips_through_store(backend: &Backend) {
    let request = seed_request(backend).await;
    let store = &backend.stores().requests;

    store.create(&request).await.unwrap();

    assert_eq!(
        store.get(&request.workspace_id, &request.id).await.unwrap(),
        Some(request.clone())
    );
    assert_eq!(
        store
            .get(&request.workspace_id, &pagis_core::RequestId::generate())
            .await
            .unwrap(),
        None
    );
}

/// The form row of the same run, so a listing test can hold both
/// kinds without seeding a second workspace.
fn form_sibling(base: &pagis_core::Request) -> pagis_core::Request {
    pending_form_request(
        &base.workspace_id,
        &base.agent_id,
        base.run_id.as_ref().unwrap(),
    )
}

pub async fn a_form_decision_stores_its_values(backend: &Backend) {
    let base = seed_request(backend).await;
    let store = &backend.stores().requests;
    let form = form_sibling(&base);
    store.create(&form).await.unwrap();
    let values = serde_json::json!({"who": "Ada", "room": "b"});

    let DecideOutcome::Decided(decided) = store
        .decide(
            &form.workspace_id,
            &form.id,
            RequestState::Approved,
            Some(values.clone()),
            1_000,
        )
        .await
        .unwrap()
    else {
        panic!("the decision must land");
    };

    assert_eq!(decided.values, Some(values.clone()));
    assert_eq!(
        store
            .get(&form.workspace_id, &form.id)
            .await
            .unwrap()
            .unwrap()
            .values,
        Some(values)
    );
}

pub async fn list_by_state_narrows_to_one_kind(backend: &Backend) {
    let action = seed_request(backend).await;
    let store = &backend.stores().requests;
    let form = form_sibling(&action);
    store.create(&action).await.unwrap();
    store.create(&form).await.unwrap();
    store
        .decide(
            &action.workspace_id,
            &action.id,
            RequestState::Denied,
            None,
            1_000,
        )
        .await
        .unwrap();

    let pending = store
        .list_by_state(&action.workspace_id, RequestState::Pending, None)
        .await
        .unwrap();
    assert_eq!(
        pending.iter().map(|r| &r.id).collect::<Vec<_>>(),
        vec![&form.id]
    );

    let forms = store
        .list_by_state(
            &action.workspace_id,
            RequestState::Pending,
            Some(pagis_core::Request::FORM_KIND),
        )
        .await
        .unwrap();
    assert_eq!(forms.len(), 1);
    let actions = store
        .list_by_state(
            &action.workspace_id,
            RequestState::Pending,
            Some(pagis_core::Request::TOOL_ACTION_KIND),
        )
        .await
        .unwrap();
    assert!(actions.is_empty());
    let denied = store
        .list_by_state(&action.workspace_id, RequestState::Denied, None)
        .await
        .unwrap();
    assert_eq!(denied.len(), 1);
}

pub async fn a_decision_lands_once_and_conflicts_after(backend: &Backend) {
    let request = seed_request(backend).await;
    let store = &backend.stores().requests;
    store.create(&request).await.unwrap();

    let first = store
        .decide(
            &request.workspace_id,
            &request.id,
            RequestState::Approved,
            None,
            1_000,
        )
        .await
        .unwrap();
    let DecideOutcome::Decided(decided) = first else {
        panic!("first decision must land");
    };
    assert_eq!(decided.state, RequestState::Approved);
    assert_eq!(decided.decided_at, Some(1_000));

    // The second decision does not overwrite the first.
    let second = store
        .decide(
            &request.workspace_id,
            &request.id,
            RequestState::Denied,
            None,
            2_000,
        )
        .await
        .unwrap();
    let DecideOutcome::NotPending(current) = second else {
        panic!("second decision must not land");
    };
    assert_eq!(current.state, RequestState::Approved);
    assert_eq!(current.decided_at, Some(1_000));
}

pub async fn expire_pending_for_run_leaves_decided_rows_alone(backend: &Backend) {
    let pending = seed_request(backend).await;
    let store = &backend.stores().requests;
    store.create(&pending).await.unwrap();
    let mut decided = pending.clone();
    decided.id = pagis_core::RequestId::generate();
    store.create(&decided).await.unwrap();
    store
        .decide(
            &decided.workspace_id,
            &decided.id,
            RequestState::Denied,
            None,
            500,
        )
        .await
        .unwrap();

    let expired = store
        .expire_pending_for_run(
            &pending.workspace_id,
            pending.run_id.as_ref().unwrap(),
            1_000,
        )
        .await
        .unwrap();

    assert_eq!(expired, 1);
    let row = store
        .get(&pending.workspace_id, &pending.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, RequestState::Expired);
    assert_eq!(row.decided_at, Some(1_000));
    assert_eq!(
        store
            .get(&decided.workspace_id, &decided.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RequestState::Denied
    );
}

/// Seed a workspace and an agent for grant tests.
async fn seed_grant_deps(backend: &Backend) -> (pagis_core::Workspace, pagis_core::Agent) {
    let ws = backend.seeded_workspace().await;
    let a = agent(&ws.id);
    backend.stores().agents.create(&a).await.unwrap();
    (ws, a)
}

pub async fn grant_roundtrips_through_store(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().grants;
    let grant = host_grant(&ws.id, &a.id, &HostId::generate(), &["git status"]);

    store.create(&grant).await.unwrap();

    assert_eq!(
        store.get(&ws.id, &grant.id).await.unwrap(),
        Some(grant.clone())
    );
    assert_eq!(store.get(&ws.id, &GrantId::generate()).await.unwrap(), None);
    assert_eq!(store.list_live(&ws.id).await.unwrap(), vec![grant.clone()]);
    assert_eq!(
        store
            .live_grant(&ws.id, &a.id, Grant::HOST_KIND)
            .await
            .unwrap(),
        Some(grant.clone())
    );
    assert_eq!(grant.allow_rules(), vec!["git status".to_string()]);
}

pub async fn set_scope_replaces_the_live_scope_only(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().grants;
    let grant = host_grant(&ws.id, &a.id, &HostId::generate(), &[]);
    store.create(&grant).await.unwrap();

    let scope = Grant::allow_scope(&["npm run".to_string()]);
    assert!(store.set_scope(&ws.id, &grant.id, &scope).await.unwrap());
    let changed = store.get(&ws.id, &grant.id).await.unwrap().unwrap();
    assert_eq!(changed.allow_rules(), vec!["npm run".to_string()]);
    assert_eq!(changed.revision, 2);

    // A revoked grant's scope is frozen.
    store.revoke(&ws.id, &grant.id, 1_000).await.unwrap();
    assert!(!store.set_scope(&ws.id, &grant.id, &scope).await.unwrap());
    assert!(
        !store
            .set_scope(&ws.id, &GrantId::generate(), &scope)
            .await
            .unwrap()
    );
}

pub async fn live_grants_can_be_read_for_one_agent_and_resource(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().grants;
    let host = host_grant(&ws.id, &a.id, &HostId::generate(), &[]);
    let connection = Grant {
        id: GrantId::generate(),
        workspace_id: ws.id.clone(),
        agent_id: a.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: Some("google-work".to_string()),
        scope: Grant::connection_scope(&["gmail.read".to_string()]),
        revision: 1,
        created_at: pagis_core::now_ms(),
        revoked_at: None,
    };
    store.create(&host).await.unwrap();
    store.create(&connection).await.unwrap();

    assert_eq!(
        store.list_live_for_agent(&ws.id, &a.id).await.unwrap(),
        vec![host, connection.clone()]
    );
    assert_eq!(
        store
            .live_for_resource(&ws.id, &a.id, Grant::CONNECTION_KIND, "google-work")
            .await
            .unwrap(),
        Some(connection)
    );
}

pub async fn capability_snapshots_are_immutable_and_content_addressed(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let channel = channel(&ws.id);
    backend.stores().channels.create(&channel).await.unwrap();
    let run = queued_run(&ws.id, &a.id, &channel.id);
    backend.stores().runs.create(&run).await.unwrap();
    let store = &backend.stores().capability_snapshots;
    let snapshot = CapabilitySnapshotRecord {
        id: "sha256:abc123".to_string(),
        hash: "abc123".to_string(),
        content: serde_json::json!({"tools": ["memory_read"]}),
        created_at: 1_000,
    };

    store.put(&snapshot).await.unwrap();
    store.put(&snapshot).await.unwrap();
    let mut conflicting = snapshot.clone();
    conflicting.content = serde_json::json!({"tools": ["host_shell"]});
    let error = store.put(&conflicting).await.unwrap_err();
    assert!(error.to_string().contains("conflicts with stored content"));
    store.bind_run(&ws.id, &run.id, &snapshot.id).await.unwrap();

    let replacement = CapabilitySnapshotRecord {
        id: "sha256:def456".to_string(),
        hash: "def456".to_string(),
        content: serde_json::json!({"tools": ["host_shell"]}),
        created_at: 2_000,
    };
    store.put(&replacement).await.unwrap();
    let error = store
        .bind_run(&ws.id, &run.id, &replacement.id)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("already has capability snapshot")
    );

    assert_eq!(
        store.for_run(&ws.id, &run.id).await.unwrap().unwrap().id,
        "sha256:abc123"
    );
}

pub async fn revoke_lands_once_and_hides_the_grant_from_live_reads(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().grants;
    let grant = host_grant(&ws.id, &a.id, &HostId::generate(), &["echo"]);
    store.create(&grant).await.unwrap();

    assert!(store.revoke(&ws.id, &grant.id, 1_000).await.unwrap());
    assert!(!store.revoke(&ws.id, &grant.id, 2_000).await.unwrap());

    let row = store.get(&ws.id, &grant.id).await.unwrap().unwrap();
    assert_eq!(row.revoked_at, Some(1_000));
    assert_eq!(store.list_live(&ws.id).await.unwrap(), vec![]);
    assert_eq!(
        store
            .live_grant(&ws.id, &a.id, Grant::HOST_KIND)
            .await
            .unwrap(),
        None
    );
}

pub async fn one_live_grant_per_agent_and_resource(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().grants;
    let air = HostId::generate();
    let first = host_grant(&ws.id, &a.id, &air, &[]);
    store.create(&first).await.unwrap();

    // A second live grant on the same machine violates the partial
    // unique index.
    let second = host_grant(&ws.id, &a.id, &air, &[]);
    assert!(store.create(&second).await.is_err());

    // Another machine of the same person is another resource, so it has
    // its own live grant: one agent may reach two machines.
    let phone = host_grant(&ws.id, &a.id, &HostId::generate(), &[]);
    store.create(&phone).await.unwrap();
    assert_eq!(
        store
            .live_for_resource(
                &ws.id,
                &a.id,
                Grant::HOST_KIND,
                phone.resource_id.as_deref().unwrap()
            )
            .await
            .unwrap(),
        Some(phone.clone())
    );

    // A credential grant is a different resource kind, so it has its
    // own slot and its own rules.
    let credential = credential_grant(&ws.id, &a.id, &["example.com"]);
    store.create(&credential).await.unwrap();
    assert_eq!(
        store
            .live_grant(&ws.id, &a.id, Grant::CREDENTIAL_KIND)
            .await
            .unwrap(),
        Some(credential.clone())
    );
    assert_eq!(
        store
            .live_for_resource(
                &ws.id,
                &a.id,
                Grant::HOST_KIND,
                first.resource_id.as_deref().unwrap()
            )
            .await
            .unwrap(),
        Some(first.clone())
    );

    // Revoking the first frees the slot of that machine.
    store.revoke(&ws.id, &first.id, 1_000).await.unwrap();
    store.create(&second).await.unwrap();
}

pub async fn events_list_by_types_pages_newest_first(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let log = &backend.stores().events;
    let append = async |event_type: &str| {
        log.append(NewEvent {
            workspace_id: ws.id.clone(),
            event_type: event_type.to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({}),
        })
        .await
        .unwrap()
    };
    let committed_one = append("memory.committed").await;
    append("run.created").await; // another type: filtered out
    let reverted = append("memory.reverted").await;
    let committed_two = append("memory.committed").await;

    let types = ["memory.committed", "memory.reverted"];
    let page = log.list_by_types(&ws.id, &types, None, 2).await.unwrap();
    assert_eq!(
        page.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
        vec![committed_two.id, reverted.id.clone()]
    );

    let rest = log
        .list_by_types(&ws.id, &types, Some(&reverted.id), 10)
        .await
        .unwrap();
    assert_eq!(rest, vec![committed_one]);

    assert!(
        log.list_by_types(&ws.id, &[], None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn artifacts_page_by_class_and_age_and_delete(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other = backend.seeded_workspace().await;
    let store = &backend.stores().artifacts;
    let stored = |kind: pagis_core::ArtifactKind, age: i64, sha: &str| Artifact {
        id: ArtifactId::generate(),
        workspace_id: ws.id.clone(),
        creator_agent_id: None,
        run_id: None,
        kind,
        filename: Some("shot.png".to_string()),
        mime: "image/png".to_string(),
        size_bytes: 3,
        sha256: sha.to_string(),
        storage_key: format!("{}/{sha}", ws.id),
        created_at: age,
    };
    use pagis_core::ArtifactKind;
    let old_shot = stored(ArtifactKind::Screenshot, 100, "a");
    let fresh_shot = stored(ArtifactKind::Screenshot, 900, "b");
    let old_file = stored(ArtifactKind::File, 100, "c");
    for artifact in [&old_shot, &fresh_shot, &old_file] {
        store.insert(artifact).await.unwrap();
    }

    // Only artifacts of the named class older than the cutoff are work.
    let due = store
        .expired_before(&ws.id, ArtifactKind::Screenshot, 500, 100)
        .await
        .unwrap();
    assert_eq!(
        due.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        vec![old_shot.id.clone()]
    );
    assert_eq!(
        due[0].kind,
        ArtifactKind::Screenshot,
        "the class reads back"
    );

    assert!(store.delete(&ws.id, &old_shot.id).await.unwrap());
    assert!(
        !store.delete(&ws.id, &old_shot.id).await.unwrap(),
        "already gone"
    );
    assert!(
        store
            .expired_before(&ws.id, ArtifactKind::Screenshot, 500, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.get(&ws.id, &fresh_shot.id).await.unwrap().is_some());
    assert!(store.get(&ws.id, &old_file.id).await.unwrap().is_some());

    // Another workspace never reads this workspace's work list.
    assert!(
        store
            .expired_before(&other.id, ArtifactKind::File, 500, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Every class starts at "keep for ever", a set window reads back, and
/// clearing it returns the class to the default.
pub async fn retention_policies_default_to_keep_for_ever(backend: &Backend) {
    use pagis_core::ArtifactKind;

    let ws = backend.seeded_workspace().await;
    let store = &backend.stores().retention_policies;

    let policies = store.list(&ws.id).await.unwrap();
    assert_eq!(
        policies.iter().map(|p| p.kind).collect::<Vec<_>>(),
        ArtifactKind::ALL.to_vec()
    );
    assert!(policies.iter().all(|p| p.retain_days.is_none()));

    store
        .set(&ws.id, ArtifactKind::CallRecording, Some(30), 1)
        .await
        .unwrap();
    store
        .set(&ws.id, ArtifactKind::CallRecording, Some(7), 2)
        .await
        .unwrap();
    let policies = store.list(&ws.id).await.unwrap();
    let recording = policies
        .iter()
        .find(|p| p.kind == ArtifactKind::CallRecording)
        .unwrap();
    assert_eq!(recording.retain_days, Some(7), "the second set wins");
    assert_eq!(
        recording.cutoff(1_000),
        Some(1_000 - 7 * 24 * 60 * 60 * 1000)
    );

    store
        .set(&ws.id, ArtifactKind::CallRecording, None, 3)
        .await
        .unwrap();
    assert!(
        store
            .list(&ws.id)
            .await
            .unwrap()
            .iter()
            .all(|p| p.retain_days.is_none()),
        "no window is keep for ever"
    );
}

// ---- The vault (ADR-0013) ----

fn vault_credential(
    workspace_id: &WorkspaceId,
    agent_id: Option<&AgentId>,
    domain: &str,
) -> pagis_core::Credential {
    pagis_core::Credential {
        id: pagis_core::CredentialId::generate(),
        workspace_id: workspace_id.clone(),
        domain: domain.to_string(),
        username: "alice@example.com".to_string(),
        login_url: format!("https://{domain}/login"),
        secret: pagis_core::SealedSecret(vec![1, 2, 3]),
        totp_seed: Some(pagis_core::SealedSecret(vec![4, 5])),
        recipe: "maxlength: 8; required: digit".to_string(),
        owner_agent_id: agent_id.cloned(),
        provenance: pagis_core::CredentialProvenance::AgentMinted,
        created_run_id: None,
        created_at: 1_000,
    }
}

pub async fn a_credential_round_trips_and_lists_by_domain(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().credentials;
    let one = vault_credential(&ws.id, Some(&a.id), "example.com");
    let two = vault_credential(&ws.id, None, "other.test");
    store.create(&one).await.unwrap();
    store.create(&two).await.unwrap();

    assert_eq!(store.get(&ws.id, &one.id).await.unwrap(), Some(one.clone()));
    assert_eq!(store.list(&ws.id, None).await.unwrap().len(), 2);
    assert_eq!(
        store.list(&ws.id, Some("example.com")).await.unwrap(),
        vec![one.clone()]
    );
    assert_eq!(
        store.list(&ws.id, Some("nothing.test")).await.unwrap(),
        vec![]
    );
}

pub async fn archiving_an_agent_hands_its_credentials_to_the_user(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().credentials;
    let owned = vault_credential(&ws.id, Some(&a.id), "example.com");
    store.create(&owned).await.unwrap();

    assert_eq!(store.release_owner(&ws.id, &a.id).await.unwrap(), 1);

    let released = store.get(&ws.id, &owned.id).await.unwrap().unwrap();
    assert_eq!(released.owner_agent_id, None);
    // Everything that makes the record usable survives.
    assert_eq!(released.secret, owned.secret);
    assert_eq!(released.login_url, owned.login_url);
    assert_eq!(store.release_owner(&ws.id, &a.id).await.unwrap(), 0);
}

/// One number of the Workspace, ready to insert.
fn phone_number(
    workspace_id: &WorkspaceId,
    e164: &str,
    agent_id: Option<&AgentId>,
) -> pagis_core::PhoneNumber {
    pagis_core::PhoneNumber::new(
        pagis_core::PhoneNumberId::generate(),
        workspace_id.clone(),
        ConnectionId::generate(),
        e164.to_string(),
        format!("carrier-{e164}"),
        agent_id.cloned(),
        pagis_core::now_ms(),
    )
}

fn purchase_intent(number: &pagis_core::PhoneNumber) -> pagis_core::PurchaseIntent {
    pagis_core::PurchaseIntent {
        id: pagis_core::PurchaseIntentId::generate(),
        workspace_id: number.workspace_id.clone(),
        connection_id: number.connection_id.clone(),
        e164: number.e164.clone(),
        agent_id: number.agent_id.clone(),
        state: pagis_core::PurchaseIntentState::Pending,
        created_at: pagis_core::now_ms(),
        settled_at: None,
    }
}

/// The schema holds the one-Agent-one-number rule, not the caller
/// (ADR-0018).
pub async fn the_schema_refuses_a_second_number_for_one_agent(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().phone_numbers;
    let held = phone_number(&ws.id, "+14155550123", Some(&a.id));
    let intent = purchase_intent(&held);
    store.create_intent(&intent).await.unwrap();
    store
        .confirm_purchase(&ws.id, &intent.id, &held, pagis_core::now_ms())
        .await
        .unwrap();

    let second = phone_number(&ws.id, "+14155550124", Some(&a.id));
    let second_intent = purchase_intent(&second);
    store.create_intent(&second_intent).await.unwrap();
    let refused = store
        .confirm_purchase(&ws.id, &second_intent.id, &second, pagis_core::now_ms())
        .await;

    assert!(matches!(refused, Err(pagis_core::StoreError::Conflict(_))));
    assert_eq!(store.for_agent(&ws.id, &a.id).await.unwrap(), Some(held));
}

/// A released number keeps its row and loses its holder, so the Calls
/// that point at it still read.
pub async fn a_released_number_keeps_its_record(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().phone_numbers;
    let held = phone_number(&ws.id, "+14155550123", Some(&a.id));
    let intent = purchase_intent(&held);
    store.create_intent(&intent).await.unwrap();
    store
        .confirm_purchase(&ws.id, &intent.id, &held, pagis_core::now_ms())
        .await
        .unwrap();

    assert!(store.release(&ws.id, &held.id).await.unwrap());

    let stored = store.get(&ws.id, &held.id).await.unwrap().unwrap();
    assert_eq!(stored.status, pagis_core::PhoneNumberStatus::Released);
    assert_eq!(stored.e164, "+14155550123");
    assert_eq!(stored.agent_id, None);
    assert_eq!(store.for_agent(&ws.id, &a.id).await.unwrap(), None);
    // A released number no longer holds the Connection.
    assert!(
        !store
            .any_live_for_connection(&ws.id, &held.connection_id)
            .await
            .unwrap()
    );
    // It is gone for good.
    assert!(!store.release(&ws.id, &held.id).await.unwrap());
}

/// A number the account already held lands with no purchase intent,
/// and the schema still holds one live record per number.
pub async fn an_adopted_number_is_recorded_without_an_intent(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let store = &backend.stores().phone_numbers;
    let adopted = phone_number(&ws.id, "+14155550123", Some(&a.id));

    store.create(&adopted).await.unwrap();

    assert_eq!(
        store.for_agent(&ws.id, &a.id).await.unwrap(),
        Some(adopted.clone())
    );
    assert!(store.list_pending_intents().await.unwrap().is_empty());
    let again = phone_number(&ws.id, "+14155550123", None);
    assert!(matches!(
        store.create(&again).await,
        Err(pagis_core::StoreError::Conflict(_))
    ));
}

/// One carrier serves the whole Org, so the schema holds a number once
/// in the installation, whichever Workspace holds it. A released record
/// gives the number back.
pub async fn one_number_is_held_once_in_the_installation(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let other = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        ..ws.clone()
    };
    backend.stores().workspaces.create(&other).await.unwrap();
    let store = &backend.stores().phone_numbers;
    let held = phone_number(&ws.id, "+14155550123", Some(&a.id));
    store.create(&held).await.unwrap();

    let again = phone_number(&other.id, "+14155550123", None);
    assert!(matches!(
        store.create(&again).await,
        Err(pagis_core::StoreError::Conflict(_))
    ));

    assert!(store.release(&ws.id, &held.id).await.unwrap());
    store.create(&again).await.unwrap();
}

/// The carrier's line finds the Workspace and the Agent of an inbound
/// call by the dialed number, so the lookup reads every Workspace and
/// skips a released record (ADR-0020).
pub async fn the_live_records_of_a_number_span_every_workspace(backend: &Backend) {
    let (ws, a) = seed_grant_deps(backend).await;
    let other = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        ..ws.clone()
    };
    backend.stores().workspaces.create(&other).await.unwrap();
    let store = &backend.stores().phone_numbers;
    let released = phone_number(&ws.id, "+14155550123", Some(&a.id));
    store.create(&released).await.unwrap();
    assert!(store.release(&ws.id, &released.id).await.unwrap());
    let held = phone_number(&other.id, "+14155550123", None);
    store.create(&held).await.unwrap();
    let neighbour = phone_number(&ws.id, "+14155550124", None);
    store.create(&neighbour).await.unwrap();

    assert_eq!(
        store.live_for_e164("+14155550123").await.unwrap(),
        vec![held]
    );
    assert_eq!(
        store.live_for_e164("+14155550124").await.unwrap(),
        vec![neighbour]
    );
    assert!(
        store
            .live_for_e164("+14155550125")
            .await
            .unwrap()
            .is_empty()
    );
}

/// A pending intent is what reconciliation reads after a restart, and a
/// settled one is never read again.
pub async fn a_settled_purchase_intent_leaves_the_pending_list(backend: &Backend) {
    let (ws, _) = seed_grant_deps(backend).await;
    let store = &backend.stores().phone_numbers;
    let number = phone_number(&ws.id, "+14155550123", None);
    let intent = purchase_intent(&number);
    store.create_intent(&intent).await.unwrap();

    assert_eq!(
        store.list_pending_intents().await.unwrap(),
        vec![intent.clone()]
    );
    // One purchase of one number at a time.
    assert!(matches!(
        store.create_intent(&purchase_intent(&number)).await,
        Err(pagis_core::StoreError::Conflict(_))
    ));

    store
        .confirm_purchase(&ws.id, &intent.id, &number, pagis_core::now_ms())
        .await
        .unwrap();

    assert!(store.list_pending_intents().await.unwrap().is_empty());
    assert!(
        !store
            .abandon_intent(&ws.id, &intent.id, pagis_core::now_ms())
            .await
            .unwrap()
    );
}

pub async fn set_config_replaces_the_binding_and_nothing_else(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().connections;
    let connection = pagis_core::Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace.id.clone(),
        provider: "telnyx".to_string(),
        alias: "carrier".to_string(),
        display_name: "Telnyx".to_string(),
        status: pagis_core::Connection::CONNECTED.to_string(),
        auth_mode: pagis_core::Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: Vec::new(),
        config: serde_json::json!({}),
        created_at: 1,
    };
    store.create(&connection).await.unwrap();
    let config = serde_json::json!({ "sip_username": "robin", "sip_domain": "sip.telnyx.com" });

    assert!(
        store
            .set_config(&workspace.id, &connection.id, &config)
            .await
            .unwrap()
    );

    assert_eq!(
        store.get(&workspace.id, &connection.id).await.unwrap(),
        Some(pagis_core::Connection {
            config,
            ..connection.clone()
        })
    );
    assert!(
        !store
            .set_config(&workspace.id, &ConnectionId::generate(), &connection.config)
            .await
            .unwrap()
    );
}

/// One row of the Trust List (ADR-0021, ADR-0019).
fn trust_entry(
    workspace_id: &WorkspaceId,
    agent_id: Option<&AgentId>,
    value: &str,
    tier: TrustTier,
) -> TrustEntry {
    let (subject, value) = pagis_core::parse_trust_subject(value).unwrap();
    TrustEntry {
        id: TrustEntryId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.cloned(),
        subject,
        value,
        tier,
        label: "Home".to_string(),
        created_at: 1,
    }
}

/// The tier the list proposes for one number.
async fn number_candidate(
    backend: &Backend,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    e164: &str,
) -> Option<TrustTier> {
    backend
        .stores()
        .trust_list
        .candidate(workspace_id, agent_id, TrustSubject::Number, e164)
        .await
        .unwrap()
}

/// The Workspace and one Agent every Trust List test writes rows for.
async fn trust_list_fixture(backend: &Backend) -> (WorkspaceId, AgentId) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    (workspace.id, agent.id)
}

pub async fn a_workspace_wide_row_proposes_owner_for_every_agent(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let store = &backend.stores().trust_list;

    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "+15551230000",
            TrustTier::Owner,
        ))
        .await
        .unwrap();

    assert_eq!(
        number_candidate(backend, &workspace_id, &agent_id, "+15551230000").await,
        Some(TrustTier::Owner)
    );
}

pub async fn a_number_on_no_list_proposes_nothing(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;

    assert_eq!(
        number_candidate(backend, &workspace_id, &agent_id, "+15559999999").await,
        None
    );
}

pub async fn the_highest_matching_row_proposes_the_tier(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let store = &backend.stores().trust_list;

    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "+15551230000",
            TrustTier::Owner,
        ))
        .await
        .unwrap();
    store
        .upsert(&trust_entry(
            &workspace_id,
            Some(&agent_id),
            "+15551230000",
            TrustTier::Trusted,
        ))
        .await
        .unwrap();

    assert_eq!(
        number_candidate(backend, &workspace_id, &agent_id, "+15551230000").await,
        Some(TrustTier::Owner)
    );
}

pub async fn one_agent_list_does_not_reach_another_agent(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let other = agent(&workspace_id);
    backend.stores().agents.create(&other).await.unwrap();
    let store = &backend.stores().trust_list;

    store
        .upsert(&trust_entry(
            &workspace_id,
            Some(&agent_id),
            "+15551230000",
            TrustTier::Trusted,
        ))
        .await
        .unwrap();

    assert_eq!(
        number_candidate(backend, &workspace_id, &other.id, "+15551230000").await,
        None
    );
}

pub async fn a_second_row_for_one_number_moves_the_tier(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let store = &backend.stores().trust_list;

    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "+15551230000",
            TrustTier::Owner,
        ))
        .await
        .unwrap();
    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "+15551230000",
            TrustTier::Trusted,
        ))
        .await
        .unwrap();

    let rows = store.list(&workspace_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tier, TrustTier::Trusted);
    assert_eq!(
        number_candidate(backend, &workspace_id, &agent_id, "+15551230000").await,
        Some(TrustTier::Trusted)
    );
}

pub async fn a_deleted_row_proposes_nothing(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let store = &backend.stores().trust_list;
    let row = trust_entry(&workspace_id, None, "+15551230000", TrustTier::Owner);
    store.upsert(&row).await.unwrap();

    assert!(store.delete(&workspace_id, &row.id).await.unwrap());
    assert!(!store.delete(&workspace_id, &row.id).await.unwrap());
    assert_eq!(
        number_candidate(backend, &workspace_id, &agent_id, "+15551230000").await,
        None
    );
}

pub async fn one_number_and_one_address_of_the_same_text_are_two_rows(backend: &Backend) {
    let (workspace_id, agent_id) = trust_list_fixture(backend).await;
    let store = &backend.stores().trust_list;

    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "clinic@example.com",
            TrustTier::Trusted,
        ))
        .await
        .unwrap();
    store
        .upsert(&trust_entry(
            &workspace_id,
            None,
            "example.com",
            TrustTier::Owner,
        ))
        .await
        .unwrap();

    assert_eq!(store.list(&workspace_id).await.unwrap().len(), 2);
    assert_eq!(
        store
            .candidate(
                &workspace_id,
                &agent_id,
                TrustSubject::Address,
                "clinic@example.com"
            )
            .await
            .unwrap(),
        Some(TrustTier::Trusted)
    );
    assert_eq!(
        store
            .candidate(
                &workspace_id,
                &agent_id,
                TrustSubject::Domain,
                "example.com"
            )
            .await
            .unwrap(),
        Some(TrustTier::Owner)
    );
    // A domain row is not an address row: the resolver reads the two in
    // order and the store never folds one into the other.
    assert_eq!(
        store
            .candidate(
                &workspace_id,
                &agent_id,
                TrustSubject::Address,
                "other@example.com"
            )
            .await
            .unwrap(),
        None
    );
}

/// A Workspace with one Agent and one queued Run, which a Version row
/// points at.
async fn seed_software(backend: &Backend) -> (WorkspaceId, AgentId, pagis_core::RunId) {
    let ws = backend.seeded_workspace().await;
    let author = agent(&ws.id);
    backend.stores().agents.create(&author).await.unwrap();
    let dm = channel(&ws.id);
    backend.stores().channels.create(&dm).await.unwrap();
    let run = queued_run(&ws.id, &author.id, &dm.id);
    backend.stores().runs.create(&run).await.unwrap();
    (ws.id, author.id, run.id)
}

pub async fn software_package_and_versions_roundtrip(backend: &Backend) {
    let (workspace_id, author_id, run_id) = seed_software(backend).await;
    let store = &backend.stores().software;
    let package = software_package(&workspace_id, &author_id, "weather");
    assert_eq!(
        store.get_by_name(&workspace_id, "weather").await.unwrap(),
        None,
        "an unknown package name is none"
    );

    store.create_package(&package).await.unwrap();
    let first = software_version(&package.id, "v1", &run_id);
    store.add_version(&package, &first).await.unwrap();

    assert_eq!(
        store.get_by_name(&workspace_id, "weather").await.unwrap(),
        Some(package.clone())
    );
    assert_eq!(
        store.list_packages(&workspace_id).await.unwrap(),
        vec![package.clone()]
    );

    // A second Version moves the package and keeps the first row.
    let second = software_version(&package.id, "v2", &run_id);
    let moved = pagis_core::SoftwarePackage {
        latest_version: "v2".to_string(),
        description: "a better forecast".to_string(),
        keywords: vec!["weather".to_string(), "forecast".to_string()],
        updated_at: package.updated_at + 1,
        ..package.clone()
    };
    store.add_version(&moved, &second).await.unwrap();

    assert_eq!(
        store.get_by_name(&workspace_id, "weather").await.unwrap(),
        Some(moved)
    );
    assert_eq!(
        store
            .list_versions(&workspace_id, &package.id)
            .await
            .unwrap()
            .iter()
            .map(|version| version.version.clone())
            .collect::<Vec<_>>(),
        vec!["v1".to_string(), "v2".to_string()]
    );
}

pub async fn a_second_package_of_the_same_name_conflicts(backend: &Backend) {
    let (workspace_id, author_id, _) = seed_software(backend).await;
    let store = &backend.stores().software;
    let package = software_package(&workspace_id, &author_id, "weather");
    store.create_package(&package).await.unwrap();

    let again = software_package(&workspace_id, &author_id, "weather");

    assert!(matches!(
        store.create_package(&again).await,
        Err(pagis_core::StoreError::Conflict(_))
    ));
}

/// One Workspace with one Agent, ready to hold a Plugin.
async fn seed_plugins(backend: &Backend) -> (WorkspaceId, AgentId) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    (workspace.id, agent.id)
}

fn plugin_record(workspace_id: &WorkspaceId, name: &str) -> Plugin {
    Plugin {
        id: PluginId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
        source: PluginSource::Git {
            url: "https://example.com/weather.git".to_string(),
            reference: Some("v1.2.0".to_string()),
        },
        installed_commit: "0".repeat(40),
        manifest_version: "v1".to_string(),
        state: PluginState::Enabled,
        created_at: 1,
        updated_at: 1,
    }
}

pub async fn plugin_and_bindings_roundtrip(backend: &Backend) {
    let (workspace_id, _agent_id) = seed_plugins(backend).await;
    let store = &backend.stores().plugins;
    let plugin = plugin_record(&workspace_id, "weather");
    let bindings = vec![
        PluginBinding {
            plugin_id: plugin.id.clone(),
            field: "account".to_string(),
            value: PluginBindingValue::Connection {
                connection_id: ConnectionId::from("conn-work".to_string()),
                capabilities: vec!["calendar.read".to_string()],
            },
        },
        PluginBinding {
            plugin_id: plugin.id.clone(),
            field: "api_key".to_string(),
            value: PluginBindingValue::Secret {
                secret_name: format!("plugin/{}/api_key", plugin.id),
            },
        },
    ];
    assert_eq!(
        store.get_by_name(&workspace_id, "weather").await.unwrap(),
        None,
        "an unknown plugin name is none"
    );

    store.create(&plugin, &bindings).await.unwrap();

    assert_eq!(
        store.get(&workspace_id, &plugin.id).await.unwrap(),
        Some(plugin.clone())
    );
    assert_eq!(
        store
            .get(&workspace_id, &PluginId::generate())
            .await
            .unwrap(),
        None,
        "an unknown plugin id is none"
    );
    assert_eq!(
        store.get_by_name(&workspace_id, "weather").await.unwrap(),
        Some(plugin.clone())
    );
    assert_eq!(
        store.list(&workspace_id).await.unwrap(),
        vec![plugin.clone()]
    );
    assert_eq!(
        store
            .list_bindings(&workspace_id, &plugin.id)
            .await
            .unwrap(),
        bindings
    );
}

pub async fn a_second_plugin_of_the_same_name_conflicts(backend: &Backend) {
    let (workspace_id, _agent_id) = seed_plugins(backend).await;
    let store = &backend.stores().plugins;
    store
        .create(&plugin_record(&workspace_id, "weather"), &[])
        .await
        .unwrap();

    let again = store
        .create(&plugin_record(&workspace_id, "weather"), &[])
        .await;

    assert!(matches!(again, Err(pagis_core::StoreError::Conflict(_))));
}

pub async fn an_update_writes_the_commit_and_the_manifest_version(backend: &Backend) {
    let (workspace_id, _agent_id) = seed_plugins(backend).await;
    let store = &backend.stores().plugins;
    let plugin = plugin_record(&workspace_id, "weather");
    store.create(&plugin, &[]).await.unwrap();

    let written = store
        .set_installed(
            &workspace_id,
            &plugin.id,
            &"1".repeat(40),
            "v2",
            PluginState::Disabled,
            7,
        )
        .await
        .unwrap();

    assert!(written);
    let read = store
        .get(&workspace_id, &plugin.id)
        .await
        .unwrap()
        .expect("the record");
    assert_eq!(read.installed_commit, "1".repeat(40));
    assert_eq!(read.manifest_version, "v2");
    assert_eq!(read.state, PluginState::Disabled);
    assert_eq!(read.updated_at, 7);
}

pub async fn one_field_holds_one_binding(backend: &Backend) {
    let (workspace_id, _agent_id) = seed_plugins(backend).await;
    let store = &backend.stores().plugins;
    let plugin = plugin_record(&workspace_id, "weather");
    store.create(&plugin, &[]).await.unwrap();
    let binding = PluginBinding {
        plugin_id: plugin.id.clone(),
        field: "region".to_string(),
        value: PluginBindingValue::Value {
            value: serde_json::json!("eu"),
        },
    };

    store.put_binding(&workspace_id, &binding).await.unwrap();
    let replacement = PluginBinding {
        value: PluginBindingValue::Value {
            value: serde_json::json!("us"),
        },
        ..binding.clone()
    };
    store
        .put_binding(&workspace_id, &replacement)
        .await
        .unwrap();

    assert_eq!(
        store
            .list_bindings(&workspace_id, &plugin.id)
            .await
            .unwrap(),
        vec![replacement]
    );

    store
        .delete_binding(&workspace_id, &plugin.id, "region")
        .await
        .unwrap();
    assert!(
        store
            .list_bindings(&workspace_id, &plugin.id)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn an_uninstall_revokes_the_grants_and_keeps_nothing_else(backend: &Backend) {
    let (workspace_id, agent_id) = seed_plugins(backend).await;
    let store = &backend.stores().plugins;
    let grants = &backend.stores().grants;
    let plugin = plugin_record(&workspace_id, "weather");
    let binding = PluginBinding {
        plugin_id: plugin.id.clone(),
        field: "region".to_string(),
        value: PluginBindingValue::Value {
            value: serde_json::json!("eu"),
        },
    };
    store.create(&plugin, &[binding]).await.unwrap();
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id,
        resource_kind: Grant::PLUGIN_KIND.to_string(),
        resource_id: Some(plugin.id.to_string()),
        scope: serde_json::json!({}),
        revision: 1,
        created_at: 1,
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    // Another person grants the Org's Plugin in their own Workspace.
    let other = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        ..backend
            .stores()
            .workspaces
            .get(&workspace_id)
            .await
            .unwrap()
            .unwrap()
    };
    backend.stores().workspaces.create(&other).await.unwrap();
    let other_agent = agent(&other.id);
    backend.stores().agents.create(&other_agent).await.unwrap();
    let other_grant = Grant {
        id: GrantId::generate(),
        workspace_id: other.id.clone(),
        agent_id: other_agent.id.clone(),
        ..grant.clone()
    };
    grants.create(&other_grant).await.unwrap();

    let removed = store
        .delete_and_revoke(&workspace_id, &plugin.id, 9)
        .await
        .unwrap();

    assert!(removed);
    assert_eq!(store.get(&workspace_id, &plugin.id).await.unwrap(), None);
    assert!(
        store
            .list_bindings(&workspace_id, &plugin.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        grants
            .get(&workspace_id, &grant.id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at,
        Some(9)
    );
    assert_eq!(
        grants
            .get(&other.id, &other_grant.id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at,
        Some(9)
    );
}

pub async fn the_frozen_tool_catalog_belongs_to_one_manifest_version(backend: &Backend) {
    let (workspace_id, _agent_id) = seed_plugins(backend).await;
    let plugin = plugin_record(&workspace_id, "weather");
    backend.stores().plugins.create(&plugin, &[]).await.unwrap();
    let store = &backend.stores().plugin_tools;
    let frozen = PluginTools {
        plugin_id: plugin.id.clone(),
        version: "v1".to_string(),
        installed_commit: plugin.installed_commit.clone(),
        tools: vec![PluginTool {
            server: "today".to_string(),
            name: "forecast".to_string(),
            description: "The forecast of one place.".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }],
        tools_changed: false,
        created_at: 7,
    };

    store.put(&frozen).await.unwrap();

    assert_eq!(
        store.get(&workspace_id, &plugin.id, "v1").await.unwrap(),
        Some(frozen.clone())
    );
    // A later version of the same Plugin is another catalog, so a Run
    // that started on `v1` still reads what its snapshot names.
    assert_eq!(
        store.get(&workspace_id, &plugin.id, "v2").await.unwrap(),
        None
    );

    store
        .set_tools_changed(&workspace_id, &plugin.id, "v1", true)
        .await
        .unwrap();
    assert!(
        store
            .get(&workspace_id, &plugin.id, "v1")
            .await
            .unwrap()
            .unwrap()
            .tools_changed
    );

    store.delete(&workspace_id, &plugin.id).await.unwrap();
    assert_eq!(
        store.get(&workspace_id, &plugin.id, "v1").await.unwrap(),
        None
    );
}

/// A `brokered` Google Connection's refresh token is sealed ciphertext
/// on its own row. It is written and read through the store, it
/// is cleared with `None`, and it goes with the row when the person
/// disconnects.
pub async fn a_brokered_connection_holds_its_sealed_refresh_token(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let store = &backend.stores().connections;
    let connection = pagis_core::Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace.id.clone(),
        provider: "google".to_string(),
        alias: "google".to_string(),
        display_name: "Google".to_string(),
        status: pagis_core::Connection::CONNECTED.to_string(),
        auth_mode: pagis_core::Connection::AUTH_MODE_BROKERED.to_string(),
        authorized_capabilities: vec!["gmail_read".to_string()],
        config: serde_json::json!({ "account": "alice@example.com", "client": "google" }),
        created_at: 1,
    };
    store.create(&connection).await.unwrap();
    let sealed = pagis_core::SealedSecret(vec![7, 8, 9, 0]);

    assert_eq!(
        store
            .refresh_token(&workspace.id, &connection.id)
            .await
            .unwrap(),
        None
    );
    assert!(
        store
            .set_refresh_token(&workspace.id, &connection.id, Some(&sealed))
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .refresh_token(&workspace.id, &connection.id)
            .await
            .unwrap(),
        Some(sealed.clone())
    );
    // A Connection this Workspace does not own answers nothing and
    // writes nothing.
    assert!(
        !store
            .set_refresh_token(
                &WorkspaceId::generate(),
                &connection.id,
                Some(&pagis_core::SealedSecret(vec![1]))
            )
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .refresh_token(&WorkspaceId::generate(), &connection.id)
            .await
            .unwrap(),
        None
    );
    assert!(
        store
            .set_refresh_token(&workspace.id, &connection.id, None)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .refresh_token(&workspace.id, &connection.id)
            .await
            .unwrap(),
        None
    );
}

/// The installation's Google Web OAuth client id belongs to the Org,
/// so the connect flow finds it there and no Workspace holds one.
pub async fn the_org_holds_the_google_web_client_id(backend: &Backend) {
    backend.seeded_workspace().await;
    let store = &backend.stores().orgs;
    let org = store.list().await.unwrap().pop().expect("one Org");

    assert_eq!(org.google_client_id, None);
    assert!(
        store
            .set_google_client_id(&org.id, Some("installation.apps.googleusercontent.com"))
            .await
            .unwrap()
    );
    assert_eq!(
        store.list().await.unwrap()[0].google_client_id.as_deref(),
        Some("installation.apps.googleusercontent.com")
    );

    assert!(store.set_google_client_id(&org.id, None).await.unwrap());
    assert_eq!(store.list().await.unwrap()[0].google_client_id, None);
    assert!(
        !store
            .set_google_client_id(&pagis_core::OrgId::generate(), Some("other"))
            .await
            .unwrap()
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_stores {
    ($emit:path) => {
        $emit!(
            stores,
            a_brokered_connection_holds_its_sealed_refresh_token,
            the_org_holds_the_google_web_client_id,
            set_chief_of_staff_names_an_agent_and_clears_it,
            one_shot_schedule_history_roundtrips_through_the_store,
            agent_update_writes_the_mutable_fields,
            event_append_assigns_increasing_seq_and_lists_after,
            events_of_one_workspace_list_without_the_others,
            channel_roundtrips_through_store,
            channel_list_scopes_to_workspace_newest_update_first,
            message_insert_roundtrips_and_gets,
            retained_turn_exposure_is_daemon_stamped_and_round_trips,
            a_stamped_insert_writes_the_row_and_its_stamp_together,
            duplicate_pending_id_returns_the_original,
            same_pending_id_in_another_channel_is_a_new_message,
            timeline_pages_newest_first_with_exclusive_cursor,
            timeline_entries_carry_thread_rollups,
            timeline_entries_name_the_newest_reply_authors,
            timeline_rollups_count_only_replies_of_their_own_workspace,
            thread_lists_root_and_replies_oldest_first,
            finalize_settles_a_streaming_message,
            fail_streaming_marks_only_streaming_messages,
            run_roundtrips_and_updates_through_store,
            a_dismissed_run_keeps_its_first_dismissal,
            a_dismissed_call_keeps_its_first_dismissal,
            artifact_roundtrips_through_store,
            duplicate_content_dedups_to_the_original_artifact,
            same_content_in_another_workspace_is_a_new_artifact,
            participants_resolve_channel_agents,
            dm_lookups_resolve_user_and_agent_dms,
            participants_list_for_channel_returns_all_kinds,
            agent_messages_elsewhere_page_newest_first_and_exclude_the_dm_and_progress,
            request_roundtrips_through_store,
            a_form_decision_stores_its_values,
            list_by_state_narrows_to_one_kind,
            a_decision_lands_once_and_conflicts_after,
            expire_pending_for_run_leaves_decided_rows_alone,
            grant_roundtrips_through_store,
            set_scope_replaces_the_live_scope_only,
            live_grants_can_be_read_for_one_agent_and_resource,
            capability_snapshots_are_immutable_and_content_addressed,
            revoke_lands_once_and_hides_the_grant_from_live_reads,
            one_live_grant_per_agent_and_resource,
            events_list_by_types_pages_newest_first,
            artifacts_page_by_class_and_age_and_delete,
            retention_policies_default_to_keep_for_ever,
            a_credential_round_trips_and_lists_by_domain,
            archiving_an_agent_hands_its_credentials_to_the_user,
            the_schema_refuses_a_second_number_for_one_agent,
            a_released_number_keeps_its_record,
            one_number_is_held_once_in_the_installation,
            the_live_records_of_a_number_span_every_workspace,
            an_adopted_number_is_recorded_without_an_intent,
            a_settled_purchase_intent_leaves_the_pending_list,
            set_config_replaces_the_binding_and_nothing_else,
            a_workspace_wide_row_proposes_owner_for_every_agent,
            a_number_on_no_list_proposes_nothing,
            the_highest_matching_row_proposes_the_tier,
            one_agent_list_does_not_reach_another_agent,
            a_second_row_for_one_number_moves_the_tier,
            a_deleted_row_proposes_nothing,
            one_number_and_one_address_of_the_same_text_are_two_rows,
            software_package_and_versions_roundtrip,
            a_second_package_of_the_same_name_conflicts,
            plugin_and_bindings_roundtrip,
            a_second_plugin_of_the_same_name_conflicts,
            an_update_writes_the_commit_and_the_manifest_version,
            one_field_holds_one_binding,
            an_uninstall_revokes_the_grants_and_keeps_nothing_else,
            the_frozen_tool_catalog_belongs_to_one_manifest_version,
        );
    };
}
