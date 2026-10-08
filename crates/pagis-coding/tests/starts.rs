//! The checks of a Coding Session start (ADR-0033), against the SQLite
//! stores: each refusal, in its order, and what the card of an allowed
//! start shows.

use std::sync::Arc;

use pagis_broker::{SessionStartAction, SessionStarts, ToolResult};
use pagis_coding::CodingSessionStarts;
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, Channel, ChannelId, ChannelKind, ChannelStore,
    CodingSession, CodingSessionId, CodingSessionPlace, CodingSessionState, CodingSessionStore,
    CodingSessionUsage, Grant, GrantId, GrantStore, Host, HostStore, MessageId, Run, RunId,
    RunState, RunStore, SessionApprovalMode, TriggerKind, Workspace, WorkspaceId, WorkspaceStore,
    now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteChannelStore, SqliteCodingSessionStore, SqliteGrantStore,
    SqliteHostStore, SqliteRunStore, SqliteWorkspaceStore,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;

struct World {
    pool: SqlitePool,
    starts: CodingSessionStarts,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    channel_id: ChannelId,
    run_id: RunId,
    host: Host,
}

async fn world(pool: SqlitePool) -> World {
    let person = pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", now_ms())
        .await
        .unwrap();
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: person.id,
        name: "Workspace".to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace.id.clone(),
        name: "Robin".to_string(),
        job: "engineer".to_string(),
        description: String::new(),
        personality: "calm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteAgentStore::new(pool.clone())
        .create(&agent)
        .await
        .unwrap();
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: workspace.id.clone(),
        kind: ChannelKind::Dm,
        title: Some("general".to_string()),
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    SqliteChannelStore::new(pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let run = Run {
        title: "A message with an attachment".into(),
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        channel_id: Some(channel.id.clone()),
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Running,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        started_at: Some(now_ms()),
        ended_at: None,
        created_at: now_ms(),
    };
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .unwrap();
    let host = SqliteHostStore::new(pool.clone())
        .register(
            &workspace.id,
            "Air",
            "macos",
            &["harness:claude".to_string()],
            now_ms(),
        )
        .await
        .unwrap();
    World {
        starts: CodingSessionStarts::new(
            Arc::new(SqliteCodingSessionStore::new(pool.clone())),
            Arc::new(SqliteGrantStore::new(pool.clone())),
        ),
        pool,
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        run_id: run.id,
        host,
    }
}

impl World {
    async fn describe(&self, arguments: Value) -> Result<SessionStartAction, ToolResult> {
        self.starts
            .describe(&self.workspace_id, &self.agent_id, &self.host, &arguments)
            .await
    }

    /// A session of the Agent in `state`.
    async fn session(&self, state: CodingSessionState) {
        let at = now_ms();
        let terminal = state.is_terminal();
        SqliteCodingSessionStore::new(self.pool.clone())
            .insert(&CodingSession {
                id: CodingSessionId::generate(),
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                harness_id: "claude".to_string(),
                harness_version: "0.87.0".to_string(),
                place: CodingSessionPlace::Host,
                host_id: Some(self.host.id.clone()),
                directory: "/Users/bo/code/app".to_string(),
                working_directory: None,
                worktree_branch: None,
                approval_mode: SessionApprovalMode::Person,
                title: "Earlier work".to_string(),
                state,
                end_reason: terminal.then(|| "closed".to_string()),
                end_detail: None,
                acp_session_id: None,
                channel_id: self.channel_id.clone(),
                root_message_id: MessageId::generate(),
                message_id: MessageId::generate(),
                run_id: self.run_id.clone(),
                usage: CodingSessionUsage::default(),
                created_at: at,
                updated_at: at,
                ended_at: terminal.then_some(at),
            })
            .await
            .unwrap();
    }

    /// The host Grant of the Agent on the machine, with a widest mode.
    async fn grant(&self, mode: SessionApprovalMode) {
        let mut grant = Grant {
            id: GrantId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            resource_kind: Grant::HOST_KIND.to_string(),
            resource_id: Some(self.host.id.to_string()),
            scope: Grant::allow_scope(&[]),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        };
        grant.scope = grant.with_session_approval_mode(mode);
        SqliteGrantStore::new(self.pool.clone())
            .create(&grant)
            .await
            .unwrap();
    }
}

fn arguments() -> Value {
    json!({
        "harness": "claude",
        "directory": "/Users/bo/code/app",
        "title": "Fix the login: OAuth & cookies!",
        "prompt": "Fix the login bug.",
    })
}

fn with(field: &str, value: Value) -> Value {
    let mut arguments = arguments();
    arguments[field] = value;
    arguments
}

fn code(refused: Result<SessionStartAction, ToolResult>) -> String {
    refused
        .expect_err("a refusal")
        .code
        .expect("a refusal has a code")
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_allowed_start_names_the_harness_the_branch_and_the_mode(pool: SqlitePool) {
    let world = world(pool).await;

    let action = world.describe(arguments()).await.unwrap();

    assert_eq!(
        action,
        SessionStartAction {
            harness_id: "claude".to_string(),
            harness_name: "Claude Code".to_string(),
            directory: "/Users/bo/code/app".to_string(),
            branch: Some("pagis/fix-the-login-oauth-cookies".to_string()),
            mode: SessionApprovalMode::Person,
        }
    );
    let action = world
        .describe(with("worktree", json!(false)))
        .await
        .unwrap();
    assert_eq!(action.branch, None);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_harness_that_is_not_in_the_catalog_is_refused(pool: SqlitePool) {
    let world = world(pool).await;

    // The first check wins: the directory is not absolute either.
    let refused = world
        .describe(json!({
            "harness": "aider",
            "directory": "code/app",
            "title": "Fix",
            "prompt": "Fix.",
        }))
        .await;

    assert_eq!(code(refused), "unknown_harness");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_directory_that_is_not_absolute_is_refused(pool: SqlitePool) {
    let world = world(pool).await;

    for directory in ["code/app", "~/code/app", ""] {
        let refused = world.describe(with("directory", json!(directory))).await;

        assert_eq!(code(refused), "bad_directory", "{directory:?}");
    }
}

/// An Agent holds at most four open sessions. A `closed` or a `failed`
/// one does not count, and an `interrupted` one does.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_fifth_open_session_is_refused_and_an_ended_one_does_not_count(pool: SqlitePool) {
    let world = world(pool).await;
    for state in [
        CodingSessionState::Working,
        CodingSessionState::Idle,
        CodingSessionState::NeedsDecision,
        CodingSessionState::Closed,
        CodingSessionState::Failed,
    ] {
        world.session(state).await;
    }

    assert!(world.describe(arguments()).await.is_ok());

    world.session(CodingSessionState::Interrupted).await;
    let refused = world.describe(arguments()).await.expect_err("a refusal");

    assert_eq!(refused.code.as_deref(), Some("session_limit"));
    assert!(
        refused
            .content
            .contains("Close one of your coding sessions first."),
        "{refused:?}"
    );
}

/// The mode is at most the widest mode of the host Grant, and with no
/// Grant the widest mode is `person`.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_mode_wider_than_the_grant_allows_is_refused(pool: SqlitePool) {
    let world = world(pool).await;

    let refused = world
        .describe(with("mode", json!("agent")))
        .await
        .expect_err("a refusal");
    assert_eq!(refused.code.as_deref(), Some("mode_not_allowed"));
    assert!(refused.content.contains("person"), "{refused:?}");

    world.grant(SessionApprovalMode::Agent).await;

    let action = world.describe(with("mode", json!("agent"))).await.unwrap();
    assert_eq!(action.mode, SessionApprovalMode::Agent);
    let refused = world
        .describe(with("mode", json!("auto")))
        .await
        .expect_err("a refusal");
    assert_eq!(refused.code.as_deref(), Some("mode_not_allowed"));
    assert!(refused.content.contains("agent"), "{refused:?}");
}
