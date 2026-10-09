//! The Coding Sessions and their transcripts (ADR-0033), on both
//! backends.
//!
//! Name every new body in `store_suite_coding_sessions!` below; the
//! guard test of the parent module fails while one is missing.

use pagis_core::{
    AgentId, ChannelId, CodingSession, CodingSessionEventKind, CodingSessionId, CodingSessionState,
    CodingSessionUsage, HarnessChoice, HarnessModeInfo, HarnessSetting, HostId, ModelTokenOwner,
    NewCodingSessionEvent, RunId, SHELL_CAPABILITY, Workspace, WorkspaceId,
};
use serde_json::json;

use super::Backend;
use crate::fixture;

use CodingSessionEventKind as Kind;

/// The rows a Coding Session points at: its Agent, the Run that started
/// it, the Channel of its Thread and its Host.
struct Owner {
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    run_id: RunId,
    channel_id: ChannelId,
    host_id: HostId,
}

impl Owner {
    fn session(&self) -> CodingSession {
        fixture::coding_session(
            &self.workspace_id,
            &self.agent_id,
            &self.run_id,
            &self.channel_id,
            &self.host_id,
        )
    }
}

async fn seed_owner(backend: &Backend, workspace: &Workspace) -> Owner {
    let stores = backend.stores();
    let agent = fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.expect("write the Agent");
    let channel = fixture::channel(&workspace.id);
    stores
        .channels
        .create(&channel)
        .await
        .expect("write the Channel");
    let run = fixture::queued_run(&workspace.id, &agent.id, &channel.id);
    stores.runs.create(&run).await.expect("write the Run");
    let host = stores
        .hosts
        .register(
            &workspace.id,
            "Air",
            "macos",
            &[SHELL_CAPABILITY.to_string()],
            1_000,
        )
        .await
        .expect("register the Host");
    Owner {
        workspace_id: workspace.id.clone(),
        agent_id: agent.id,
        run_id: run.id,
        channel_id: channel.id,
        host_id: host.id,
    }
}

/// A second Workspace of the same person, for the reads that must not
/// cross one.
async fn second_workspace(backend: &Backend, first: &Workspace) -> Workspace {
    let second = Workspace {
        id: WorkspaceId::generate(),
        ..first.clone()
    };
    backend
        .stores()
        .workspaces
        .create(&second)
        .await
        .expect("write the second Workspace");
    second
}

fn ended(mut session: CodingSession, state: CodingSessionState) -> CodingSession {
    session.state = state;
    session.end_reason = Some("closed_by_agent".to_string());
    session.ended_at = Some(session.updated_at);
    session
}

fn chunk(at: i64, kind: Kind, text: &str, message_id: Option<&str>) -> NewCodingSessionEvent {
    NewCodingSessionEvent {
        at,
        kind,
        payload: json!({"text": text, "message_id": message_id}),
    }
}

fn tool_call(at: i64, id: &str) -> NewCodingSessionEvent {
    NewCodingSessionEvent {
        at,
        kind: Kind::ToolCall,
        payload: json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": "cargo test"}),
    }
}

fn ids(sessions: &[CodingSession]) -> Vec<&CodingSessionId> {
    sessions.iter().map(|session| &session.id).collect()
}

pub async fn a_session_reads_back_and_an_update_writes_its_state_usage_and_end(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();

    assert_eq!(
        sessions
            .get(&workspace.id, &session.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&session)
    );

    let failed = CodingSession {
        state: CodingSessionState::Failed,
        end_reason: Some("harness_exited".to_string()),
        end_detail: Some("exit code 1\nerror: no such file".to_string()),
        worktree_branch: Some("pagis/fix-test".to_string()),
        usage: CodingSessionUsage {
            context_used: Some(52_000),
            context_size: Some(200_000),
            cost_amount: Some(0.42),
            cost_currency: Some("USD".to_string()),
        },
        updated_at: session.updated_at + 5_000,
        ended_at: Some(session.updated_at + 5_000),
        ..session.clone()
    };
    assert!(sessions.update(&failed).await.unwrap());
    assert_eq!(
        sessions.get(&workspace.id, &session.id).await.unwrap(),
        Some(failed)
    );

    let unknown = CodingSession {
        id: CodingSessionId::generate(),
        ..session
    };
    assert!(!sessions.update(&unknown).await.unwrap());
}

pub async fn the_harness_mode_and_the_offered_modes_read_back(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let mode = |id: &str, name: &str, description: Option<&str>| HarnessModeInfo {
        id: id.to_string(),
        name: name.to_string(),
        description: description.map(str::to_string),
    };
    let session = CodingSession {
        harness_mode: Some("default".to_string()),
        harness_modes: vec![
            mode("default", "Manual", Some("Asks before each edit")),
            mode("plan", "Plan", None),
            mode("bypassPermissions", "Bypass permissions", None),
        ],
        ..owner.session()
    };
    sessions.insert(&session).await.unwrap();
    assert_eq!(
        sessions.get(&workspace.id, &session.id).await.unwrap(),
        Some(session.clone())
    );

    let changed = CodingSession {
        harness_mode: Some("plan".to_string()),
        updated_at: session.updated_at + 1_000,
        ..session.clone()
    };
    assert!(sessions.update(&changed).await.unwrap());
    assert_eq!(
        sessions.get(&workspace.id, &session.id).await.unwrap(),
        Some(changed)
    );

    let modeless = owner.session();
    assert_eq!(modeless.harness_mode, None);
    sessions.insert(&modeless).await.unwrap();
    let read = sessions
        .get(&workspace.id, &modeless.id)
        .await
        .unwrap()
        .expect("the session reads back");
    assert_eq!(read.harness_mode, None);
    assert_eq!(read.harness_modes, []);
}

pub async fn the_harness_model_and_the_thought_level_read_back(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let choice = |id: &str, name: &str, description: Option<&str>| HarnessChoice {
        id: id.to_string(),
        name: name.to_string(),
        description: description.map(str::to_string),
    };
    let session = CodingSession {
        model: Some(HarnessSetting {
            option_id: "model".to_string(),
            current: "default".to_string(),
            choices: vec![
                choice("default", "Default (recommended)", Some("Opus 4.7")),
                choice("sonnet", "Sonnet", None),
            ],
        }),
        thought_level: Some(HarnessSetting {
            option_id: "effort".to_string(),
            current: "high".to_string(),
            choices: vec![choice("low", "Low", None), choice("high", "High", None)],
        }),
        ..owner.session()
    };
    sessions.insert(&session).await.unwrap();
    assert_eq!(
        sessions.get(&workspace.id, &session.id).await.unwrap(),
        Some(session.clone())
    );

    let changed = CodingSession {
        model: session.model.clone().map(|model| HarnessSetting {
            current: "sonnet".to_string(),
            ..model
        }),
        thought_level: None,
        updated_at: session.updated_at + 1_000,
        ..session.clone()
    };
    assert!(sessions.update(&changed).await.unwrap());
    assert_eq!(
        sessions.get(&workspace.id, &session.id).await.unwrap(),
        Some(changed)
    );

    let plain = owner.session();
    sessions.insert(&plain).await.unwrap();
    let read = sessions
        .get(&workspace.id, &plain.id)
        .await
        .unwrap()
        .expect("the session reads back");
    assert_eq!(read.model, None);
    assert_eq!(read.thought_level, None);
}

pub async fn the_list_is_newest_first_and_filters_by_agent_state_and_before(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let first_owner = seed_owner(backend, &workspace).await;
    let second_owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let oldest = first_owner.session();
    let middle = CodingSession {
        state: CodingSessionState::Working,
        ..second_owner.session()
    };
    let newest = ended(first_owner.session(), CodingSessionState::Closed);
    for session in [&oldest, &middle, &newest] {
        sessions.insert(session).await.unwrap();
    }

    let all = sessions
        .list(&workspace.id, None, None, None, 10)
        .await
        .unwrap();
    assert_eq!(ids(&all), [&newest.id, &middle.id, &oldest.id]);

    let of_first = sessions
        .list(&workspace.id, Some(&first_owner.agent_id), None, None, 10)
        .await
        .unwrap();
    assert_eq!(ids(&of_first), [&newest.id, &oldest.id]);

    let idle = sessions
        .list(
            &workspace.id,
            None,
            Some(CodingSessionState::Idle),
            None,
            10,
        )
        .await
        .unwrap();
    assert_eq!(ids(&idle), [&oldest.id]);

    let older = sessions
        .list(&workspace.id, None, None, Some(&newest.id), 10)
        .await
        .unwrap();
    assert_eq!(ids(&older), [&middle.id, &oldest.id]);

    let page = sessions
        .list(&workspace.id, None, None, None, 1)
        .await
        .unwrap();
    assert_eq!(ids(&page), [&newest.id]);
}

pub async fn count_open_counts_the_sessions_of_one_agent_that_are_not_terminal(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let other = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    for session in [
        owner.session(),
        CodingSession {
            state: CodingSessionState::Interrupted,
            ..owner.session()
        },
        CodingSession {
            state: CodingSessionState::NeedsDecision,
            ..owner.session()
        },
        ended(owner.session(), CodingSessionState::Closed),
        ended(owner.session(), CodingSessionState::Failed),
        other.session(),
    ] {
        sessions.insert(&session).await.unwrap();
    }

    assert_eq!(
        sessions
            .count_open(&workspace.id, &owner.agent_id)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sessions
            .count_open(&workspace.id, &other.agent_id)
            .await
            .unwrap(),
        1
    );
}

pub async fn list_open_holds_the_open_sessions_of_every_workspace_oldest_first(backend: &Backend) {
    let first = backend.seeded_workspace().await;
    let second = second_workspace(backend, &first).await;
    let first_owner = seed_owner(backend, &first).await;
    let second_owner = seed_owner(backend, &second).await;
    let sessions = &backend.stores().coding_sessions;
    let open_first = first_owner.session();
    let closed = ended(first_owner.session(), CodingSessionState::Closed);
    let open_second = CodingSession {
        state: CodingSessionState::Interrupted,
        ..second_owner.session()
    };
    let failed = ended(second_owner.session(), CodingSessionState::Failed);
    for session in [&open_first, &closed, &open_second, &failed] {
        sessions.insert(session).await.unwrap();
    }

    let open = sessions.list_open().await.unwrap();

    assert_eq!(ids(&open), [&open_first.id, &open_second.id]);
}

pub async fn append_event_merges_the_chunks_of_one_message_and_answers_the_rewritten_row(
    backend: &Backend,
) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();

    let prompt = sessions
        .append_event(
            &workspace.id,
            &session.id,
            chunk(10, Kind::Prompt, "Fix the test.", None),
        )
        .await
        .unwrap();
    let first = sessions
        .append_event(
            &workspace.id,
            &session.id,
            chunk(20, Kind::AgentMessage, "I fix ", Some("m1")),
        )
        .await
        .unwrap();
    let second = sessions
        .append_event(
            &workspace.id,
            &session.id,
            chunk(30, Kind::AgentMessage, "it now.", Some("m1")),
        )
        .await
        .unwrap();

    assert_eq!(prompt.len(), 1);
    assert_eq!(prompt[0].seq, 1);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].seq, 2);
    assert_eq!(second.len(), 1, "a merge answers the one rewritten row");
    assert_eq!(second[0].seq, 2);
    assert_eq!(
        second[0].at, 20,
        "a merge keeps the time of the first chunk"
    );
    assert_eq!(second[0].kind, Kind::AgentMessage);
    assert_eq!(second[0].payload["text"], "I fix it now.");
    assert_eq!(second[0].workspace_id, workspace.id);
    assert_eq!(second[0].coding_session_id, session.id);

    let rows = sessions
        .list_events(&workspace.id, &session.id, None, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1], second[0]);
}

pub async fn list_events_pages_by_seq(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();
    for n in 1..=5 {
        sessions
            .append_event(
                &workspace.id,
                &session.id,
                tool_call(n * 10, &format!("t{n}")),
            )
            .await
            .unwrap();
    }
    let page = |after, limit| {
        let (workspace_id, session_id) = (workspace.id.clone(), session.id.clone());
        async move {
            sessions
                .list_events(&workspace_id, &session_id, after, limit)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        }
    };

    assert_eq!(page(None, 2).await, [1, 2]);
    assert_eq!(page(Some(2), 2).await, [3, 4]);
    assert_eq!(page(Some(4), 2).await, [5]);
    assert_eq!(page(Some(5), 2).await, Vec::<i64>::new());
}

pub async fn latest_event_answers_the_newest_row_of_the_named_kinds(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();
    for event in [
        chunk(10, Kind::Prompt, "Fix the test.", None),
        chunk(20, Kind::AgentMessage, "Looking.", Some("m1")),
        tool_call(30, "t1"),
        chunk(40, Kind::AgentMessage, "Fixed.", Some("m2")),
        NewCodingSessionEvent {
            at: 50,
            kind: Kind::TurnEnd,
            payload: json!({"stopReason": "end_turn"}),
        },
    ] {
        sessions
            .append_event(&workspace.id, &session.id, event)
            .await
            .unwrap();
    }
    let latest = |kinds: &'static [Kind]| {
        let (workspace_id, session_id) = (workspace.id.clone(), session.id.clone());
        async move {
            sessions
                .latest_event(&workspace_id, &session_id, kinds)
                .await
                .unwrap()
                .map(|row| row.seq)
        }
    };

    assert_eq!(latest(&[Kind::AgentMessage]).await, Some(4));
    assert_eq!(latest(&[Kind::ToolCall, Kind::Permission]).await, Some(3));
    assert_eq!(latest(&[Kind::Prompt]).await, Some(1));
    assert_eq!(latest(&[Kind::Question]).await, None);
    assert_eq!(latest(&[]).await, None);
}

pub async fn a_workspace_reads_and_writes_nothing_of_another(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();
    sessions
        .append_event(&workspace.id, &session.id, tool_call(10, "t1"))
        .await
        .unwrap();

    assert_eq!(sessions.get(&other.id, &session.id).await.unwrap(), None);
    assert!(
        sessions
            .list(&other.id, None, None, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sessions
            .count_open(&other.id, &owner.agent_id)
            .await
            .unwrap(),
        0
    );
    assert!(
        sessions
            .list_events(&other.id, &session.id, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sessions
            .latest_event(&other.id, &session.id, &[Kind::ToolCall])
            .await
            .unwrap(),
        None
    );
    let moved = CodingSession {
        workspace_id: other.id.clone(),
        state: CodingSessionState::Working,
        ..session.clone()
    };
    assert!(!sessions.update(&moved).await.unwrap());

    let written = sessions
        .append_event(&other.id, &session.id, tool_call(20, "t2"))
        .await
        .unwrap();

    assert!(written.is_empty());
    let rows = sessions
        .list_events(&workspace.id, &session.id, None, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        sessions
            .get(&workspace.id, &session.id)
            .await
            .unwrap()
            .map(|read| read.state),
        Some(CodingSessionState::Idle)
    );
}

pub async fn the_schema_refuses_a_closed_session_with_no_end_reason(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let no_reason = CodingSession {
        end_reason: None,
        ..ended(owner.session(), CodingSessionState::Closed)
    };

    assert!(sessions.insert(&no_reason).await.is_err());
    assert_eq!(
        sessions.get(&workspace.id, &no_reason.id).await.unwrap(),
        None
    );
}

pub async fn model_token_owner_finds_the_session_of_a_set_hash(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = CodingSession {
        state: CodingSessionState::Starting,
        ..owner.session()
    };
    sessions.insert(&session).await.unwrap();

    assert!(
        sessions
            .set_model_token(&workspace.id, &session.id, "hash-a")
            .await
            .unwrap()
    );

    let expected = ModelTokenOwner {
        workspace_id: workspace.id.clone(),
        agent_id: owner.agent_id.clone(),
        session_id: session.id.clone(),
        run_id: owner.run_id.clone(),
    };
    // Each state with a running harness keeps the token, and a write of
    // the whole record does not clear it.
    for state in [
        CodingSessionState::Starting,
        CodingSessionState::Working,
        CodingSessionState::NeedsDecision,
        CodingSessionState::Idle,
    ] {
        let moved = CodingSession {
            state,
            ..session.clone()
        };
        assert!(sessions.update(&moved).await.unwrap());
        assert_eq!(
            sessions.model_token_owner("hash-a").await.unwrap().as_ref(),
            Some(&expected),
            "{state:?}"
        );
    }
    assert_eq!(sessions.model_token_owner("hash-b").await.unwrap(), None);
}

pub async fn a_second_set_model_token_stops_the_first_hash(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();
    sessions
        .set_model_token(&workspace.id, &session.id, "hash-a")
        .await
        .unwrap();

    sessions
        .set_model_token(&workspace.id, &session.id, "hash-b")
        .await
        .unwrap();

    assert_eq!(sessions.model_token_owner("hash-a").await.unwrap(), None);
    assert_eq!(
        sessions
            .model_token_owner("hash-b")
            .await
            .unwrap()
            .map(|found| found.session_id),
        Some(session.id)
    );
}

pub async fn a_session_with_no_running_harness_has_no_model_token_owner(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let interrupted = CodingSession {
        state: CodingSessionState::Interrupted,
        ..owner.session()
    };
    let closed = ended(owner.session(), CodingSessionState::Closed);
    let failed = ended(owner.session(), CodingSessionState::Failed);
    for (session, hash) in [
        (&interrupted, "hash-interrupted"),
        (&closed, "hash-closed"),
        (&failed, "hash-failed"),
    ] {
        sessions.insert(session).await.unwrap();
        sessions
            .set_model_token(&workspace.id, &session.id, hash)
            .await
            .unwrap();

        assert_eq!(
            sessions.model_token_owner(hash).await.unwrap(),
            None,
            "{:?}",
            session.state
        );
    }
}

pub async fn set_model_token_writes_no_session_of_another_workspace(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let owner = seed_owner(backend, &workspace).await;
    let sessions = &backend.stores().coding_sessions;
    let session = owner.session();
    sessions.insert(&session).await.unwrap();

    let written = sessions
        .set_model_token(&other.id, &session.id, "hash-a")
        .await
        .unwrap();

    assert!(!written);
    assert_eq!(sessions.model_token_owner("hash-a").await.unwrap(), None);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_coding_sessions {
    ($emit:path) => {
        $emit!(
            coding_sessions,
            a_session_reads_back_and_an_update_writes_its_state_usage_and_end,
            the_harness_mode_and_the_offered_modes_read_back,
            the_harness_model_and_the_thought_level_read_back,
            the_list_is_newest_first_and_filters_by_agent_state_and_before,
            count_open_counts_the_sessions_of_one_agent_that_are_not_terminal,
            list_open_holds_the_open_sessions_of_every_workspace_oldest_first,
            append_event_merges_the_chunks_of_one_message_and_answers_the_rewritten_row,
            list_events_pages_by_seq,
            latest_event_answers_the_newest_row_of_the_named_kinds,
            a_workspace_reads_and_writes_nothing_of_another,
            the_schema_refuses_a_closed_session_with_no_end_reason,
            model_token_owner_finds_the_session_of_a_set_hash,
            a_second_set_model_token_stops_the_first_hash,
            a_session_with_no_running_harness_has_no_model_token_owner,
            set_model_token_writes_no_session_of_another_workspace,
        );
    };
}
