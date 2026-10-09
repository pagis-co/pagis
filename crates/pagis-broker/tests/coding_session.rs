//! The start of a Coding Session is a host action (ADR-0033, ADR-0015).
//!
//! These tests drive the broker: which snapshot offers the start, which
//! machines are candidates for a harness, what the `machine` argument
//! keeps, what a refusal before the card writes, and what the card says.
//! The checks of the start are the `SessionStarts` of `pagis-coding`,
//! which has its own tests. Here a scripted one stands in.

use pagis_broker::{
    CODING_SESSION_ANSWER, CODING_SESSION_CANCEL, CODING_SESSION_CLOSE, CODING_SESSION_DECIDE,
    CODING_SESSION_ESCALATE, CODING_SESSION_LIST, CODING_SESSION_READ, CODING_SESSION_RESUME,
    CODING_SESSION_SEND, CODING_SESSION_SET_MODE, CODING_SESSION_START,
    COMPUTER_CODING_SESSION_START, InvokeOutcome, SessionStartAction, ToolCall, ToolResult,
};
use pagis_core::{
    GrantStore, Host, RequestState, RequestStore, RunStore, SHELL_CAPABILITY, SessionAllowRule,
    SessionApprovalMode,
};
use pagis_storage_sqlite::SqliteGrantStore;
use pagis_storage_sqlite::{SqliteRequestStore, SqliteRunStore};
use sqlx::SqlitePool;

use crate::capability_broker::{Harness, harness, harness_with_computer};
use crate::host_dispatch::{grant_host, register};

const DIRECTORY: &str = "/Users/bo/code/app";

const PROMPT: &str = "Fix the login bug.";

/// The start on a Host and the tools of a started session.
const SESSION_TOOLS: [&str; 9] = [
    CODING_SESSION_START,
    CODING_SESSION_SEND,
    CODING_SESSION_READ,
    CODING_SESSION_CANCEL,
    CODING_SESSION_CLOSE,
    CODING_SESSION_LIST,
    CODING_SESSION_RESUME,
    CODING_SESSION_ANSWER,
    CODING_SESSION_SET_MODE,
];

/// The tools of the `agent` mode. The snapshot holds them only for an
/// Agent that may decide a Harness Permission on some machine, or that
/// has a Computer, where a session runs in the `agent` mode.
const DECISION_TOOLS: [&str; 2] = [CODING_SESSION_DECIDE, CODING_SESSION_ESCALATE];

fn arguments(harness: &str, machine: Option<&str>) -> String {
    let mut arguments = serde_json::json!({
        "harness": harness,
        "directory": DIRECTORY,
        "title": "Fix the login",
        "prompt": PROMPT,
    });
    if let Some(machine) = machine {
        arguments["machine"] = machine.into();
    }
    arguments.to_string()
}

/// What the scripted check answers for a start that it allows.
fn action(branch: Option<&str>, mode: SessionApprovalMode) -> SessionStartAction {
    SessionStartAction {
        harness_id: "claude".to_string(),
        harness_name: "Claude Code".to_string(),
        directory: DIRECTORY.to_string(),
        branch: branch.map(str::to_string),
        mode,
        asks_permission: true,
        harness_mode: None,
    }
}

async fn tool_names(harness: &Harness) -> Vec<String> {
    harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap()
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}

/// The tool names of the snapshot of a new Run of the Agent. A Run takes
/// one snapshot, so a second look needs a second Run.
async fn tool_names_of_a_new_run(harness: &Harness) -> Vec<String> {
    let run = pagis_core::Run {
        id: pagis_core::RunId::generate(),
        ..harness.run.clone()
    };
    SqliteRunStore::new(harness.pool.clone())
        .create(&run)
        .await
        .unwrap();
    harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &run.id)
        .await
        .unwrap()
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect()
}

async fn start(harness: &Harness, arguments: &str) -> InvokeOutcome {
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            ToolCall::new(CODING_SESSION_START, arguments),
        )
        .await
        .unwrap()
}

async fn pending_requests(harness: &Harness) -> Vec<pagis_core::Request> {
    SqliteRequestStore::new(harness.pool.clone())
        .list_by_state(&harness.workspace.id, RequestState::Pending, None)
        .await
        .unwrap()
}

/// An absent capability is declared, never emulated (ADR-0005): with no
/// Computer and no machine that declares a harness, no session can start,
/// so the snapshot holds no session tool.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_session_tools_are_absent_when_no_host_declares_a_harness_and_there_is_no_computer(
    pool: SqlitePool,
) {
    let harness = harness(pool).await;
    register(&harness, "Air", &[SHELL_CAPABILITY]).await;

    let tools = tool_names(&harness).await;

    for tool in [COMPUTER_CODING_SESSION_START]
        .into_iter()
        .chain(SESSION_TOOLS)
        .chain(DECISION_TOOLS)
    {
        assert!(!tools.contains(&tool.to_string()), "{tool}");
    }
    assert!(tools.contains(&"host_shell".to_string()));
}

/// A session starts in the Agent's own Computer with no Host, in the
/// `agent` mode and with no host Grant. So with a Computer and no machine
/// that declares a harness, the snapshot holds the start in the Computer,
/// the tools of a started session and the tools of the `agent` mode, and
/// no start on a Host.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn with_a_computer_the_snapshot_holds_the_start_there_and_the_tools_of_the_agent_mode(
    pool: SqlitePool,
) {
    let harness = harness_with_computer(pool).await;
    register(&harness, "Air", &[SHELL_CAPABILITY]).await;

    let tools = tool_names(&harness).await;

    assert!(!tools.contains(&CODING_SESSION_START.to_string()));
    for tool in [COMPUTER_CODING_SESSION_START]
        .into_iter()
        .chain(SESSION_TOOLS.into_iter().skip(1))
        .chain(DECISION_TOOLS)
    {
        assert!(tools.contains(&tool.to_string()), "{tool}");
    }
}

/// The Person delegates the decision of a Harness Permission with the
/// widest mode on a host Grant (ADR-0033). An Agent with no live host
/// Grant whose widest mode is `agent` cannot decide, so the decision
/// tools are absent from its snapshot (ADR-0005).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_decision_tools_are_offered_only_on_a_host_grant_whose_widest_mode_is_agent(
    pool: SqlitePool,
) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY, "harness:claude"]).await;
    let has_decision_tools = |tools: &[String]| {
        DECISION_TOOLS
            .iter()
            .map(|tool| tools.contains(&tool.to_string()))
            .collect::<Vec<_>>()
    };

    let tools = tool_names(&harness).await;
    assert_eq!(has_decision_tools(&tools), [false, false], "no host Grant");
    for tool in SESSION_TOOLS {
        assert!(tools.contains(&tool.to_string()), "{tool}");
    }

    let mut grant = grant_host(&harness, &air, &[]).await;
    let grants = SqliteGrantStore::new(harness.pool.clone());
    for (mode, offered) in [
        (SessionApprovalMode::Person, false),
        (SessionApprovalMode::Agent, true),
    ] {
        grant.scope = grant.with_session_approval_mode(mode);
        grants
            .set_scope(&harness.workspace.id, &grant.id, &grant.scope)
            .await
            .unwrap();

        let tools = tool_names_of_a_new_run(&harness).await;

        assert_eq!(has_decision_tools(&tools), [offered, offered], "{mode:?}");
    }
}

/// The start in the Agent's own Computer asks nobody: the container is
/// the sandbox, as for `computer_shell` (ADR-0014), so it runs with no
/// card and no Request.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_start_in_the_computer_runs_with_no_card(pool: SqlitePool) {
    let harness = harness_with_computer(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            ToolCall::new(
                COMPUTER_CODING_SESSION_START,
                serde_json::json!({
                    "harness": "claude",
                    "directory": "/data/agent/app",
                    "title": "Fix the login",
                    "prompt": PROMPT,
                })
                .to_string(),
            ),
        )
        .await
        .unwrap();

    assert!(
        matches!(outcome, InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    assert!(pending_requests(&harness).await.is_empty());
}

/// The start in the Agent's own Computer names only a harness that runs
/// there.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_start_in_the_computer_refuses_a_harness_that_does_not_run_there(pool: SqlitePool) {
    let harness = harness_with_computer(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            ToolCall::new(
                COMPUTER_CODING_SESSION_START,
                serde_json::json!({
                    "harness": "gemini",
                    "directory": "/data/agent/app",
                    "title": "Fix the login",
                    "prompt": PROMPT,
                })
                .to_string(),
            ),
        )
        .await
        .unwrap();

    let InvokeOutcome::Rejected(result) = outcome else {
        panic!("the start is not rejected: {outcome:?}");
    };
    assert_eq!(result.code.as_deref(), Some("invalid_request"));
}

/// The note of a decision is its reason in the audit fact, and the note
/// of an escalation is the question on the card, so each needs words, at
/// most 2,000 characters. A decision allows or denies, and nothing else.
/// The note of a decision is its reason in the audit fact, and the note
/// of an escalation is the question on the card, so each needs words, at
/// most 2,000 characters. A decision allows or denies, and nothing else.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_decision_needs_allow_or_deny_and_a_note_of_at_most_2000_characters(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let mut grant = grant_host(&harness, &air, &[]).await;
    grant.scope = grant.with_session_approval_mode(SessionApprovalMode::Agent);
    SqliteGrantStore::new(harness.pool.clone())
        .set_scope(&harness.workspace.id, &grant.id, &grant.scope)
        .await
        .unwrap();
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let long = "x".repeat(2_001);

    for (tool, arguments) in [
        (
            CODING_SESSION_DECIDE,
            serde_json::json!({"session": "cs-1", "decision": "allow", "note": "  "}),
        ),
        (
            CODING_SESSION_DECIDE,
            serde_json::json!({"session": "cs-1", "decision": "allow", "note": long}),
        ),
        (
            CODING_SESSION_DECIDE,
            serde_json::json!({"session": "cs-1", "decision": "allow_always", "note": "Safe."}),
        ),
        (
            CODING_SESSION_DECIDE,
            serde_json::json!({"session": "cs-1", "decision": "allow"}),
        ),
        (
            CODING_SESSION_ESCALATE,
            serde_json::json!({"session": "cs-1", "note": ""}),
        ),
        (
            CODING_SESSION_ESCALATE,
            serde_json::json!({"session": "cs-1", "note": long}),
        ),
    ] {
        let outcome = harness
            .broker
            .invoke(
                &harness.workspace.id,
                &harness.run.id,
                &snapshot.id,
                ToolCall::new(tool, arguments.to_string()),
            )
            .await
            .unwrap();

        let InvokeOutcome::Rejected(result) = outcome else {
            panic!("{tool} {arguments} is not rejected: {outcome:?}");
        };
        assert_eq!(
            result.code.as_deref(),
            Some("invalid_request"),
            "{arguments}"
        );
    }
}

/// One machine that declares a harness offers the start and the tools of
/// a started session, whether it is connected or not.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_session_tools_are_offered_when_a_host_declares_a_harness(pool: SqlitePool) {
    let harness = harness(pool).await;
    register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    register(&harness, "Studio", &["harness:codex"]).await;

    let tools = tool_names(&harness).await;

    for tool in SESSION_TOOLS {
        assert!(tools.contains(&tool.to_string()), "{tool}");
    }
}

/// A machine is a candidate for the harnesses that it declares and for
/// no other.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_that_declares_claude_is_no_candidate_for_codex(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY, "harness:claude"]).await;
    let _connected = harness.presence().connect(&air.id);
    harness
        .starts
        .answers(Ok(action(None, SessionApprovalMode::Person)));

    let outcome = start(&harness, &arguments("codex", None)).await;

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(pending_requests(&harness).await.is_empty());

    let outcome = start(&harness, &arguments("claude", None)).await;

    assert!(matches!(outcome, InvokeOutcome::Waiting(_)), "{outcome:?}");
}

/// `machine` keeps the candidate with that name, case ignored, and a
/// name of no candidate answers with the names of the candidates.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn machine_keeps_one_of_two_present_hosts(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let studio = register(&harness, "Studio", &["harness:claude"]).await;
    let _air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);
    harness
        .starts
        .answers(Ok(action(None, SessionApprovalMode::Person)));

    let outcome = start(&harness, &arguments("claude", Some("studio"))).await;

    let InvokeOutcome::Waiting(card) = outcome else {
        panic!("the named machine asks for the approval: {outcome:?}");
    };
    assert_eq!(card.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(card.request.payload["host_id"], studio.id.as_str());

    let outcome = start(&harness, &arguments("claude", Some("Mini"))).await;

    let result = outcome.result().expect("an answer, not a card");
    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("machine_not_found"));
    assert!(result.content.contains("Mini"), "{result:?}");
    assert!(
        result.content.contains("Air") && result.content.contains("Studio"),
        "{result:?}"
    );
    assert_eq!(*harness.starts.hosts.lock().unwrap(), vec![studio.id]);
}

/// Two present candidates get the choice card, whose body names the
/// harness and the directory. No present candidate is an answer.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn two_present_candidates_get_the_choice_card_and_none_is_not_connected(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let studio = register(&harness, "Studio", &["harness:claude"]).await;

    let outcome = start(&harness, &arguments("claude", None)).await;

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));

    let _air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);

    let InvokeOutcome::Waiting(question) = start(&harness, &arguments("claude", None)).await else {
        panic!("two present machines ask which one");
    };
    assert_eq!(question.request.kind, pagis_core::Request::CHOICE_KIND);
    assert_eq!(question.body, format!("Claude Code in {DIRECTORY}"));
    assert!(harness.starts.hosts.lock().unwrap().is_empty());

    // The answer names the Studio, and the check of the start comes next,
    // for that machine, before the approval card.
    harness
        .starts
        .answers(Ok(action(None, SessionApprovalMode::Person)));
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &question.request.id,
            RequestState::Approved,
            Some(serde_json::json!({"value": studio.id.as_str()})),
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let outcome = harness
        .broker
        .resolve(&question.request.id, RequestState::Approved)
        .await
        .unwrap();

    let InvokeOutcome::Waiting(card) = outcome else {
        panic!("the chosen machine asks for the approval: {outcome:?}");
    };
    assert_eq!(card.title, "Start Claude Code on your Studio");
    assert_eq!(*harness.starts.hosts.lock().unwrap(), vec![studio.id]);
}

/// A refusal of the checks is an answer before any card: the Person is
/// not asked about a start that would not happen.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_refusal_of_session_starts_writes_no_request(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let _connected = harness.presence().connect(&air.id);
    harness.starts.answers(Err(ToolResult::error(
        "session_limit",
        "Close one of your coding sessions first.",
    )));

    let outcome = start(&harness, &arguments("claude", None)).await;

    let InvokeOutcome::Rejected(result) = outcome else {
        panic!("a refusal, not a card: {outcome:?}");
    };
    assert_eq!(result.code.as_deref(), Some("session_limit"));
    assert!(pending_requests(&harness).await.is_empty());
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// The card names the harness and the machine, and its body holds the
/// directory, the branch, the mode in the words of the UI and the start
/// of the prompt. An approve runs the start on that machine.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_card_names_the_harness_and_the_machine(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY, "harness:claude"]).await;
    let _connected = harness.presence().connect(&air.id);
    harness.starts.answers(Ok(action(
        Some("pagis/fix-the-login"),
        SessionApprovalMode::Agent,
    )));

    let InvokeOutcome::Waiting(card) = start(&harness, &arguments("claude", None)).await else {
        panic!("a start waits for one approval");
    };

    assert_eq!(card.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(card.title, "Start Claude Code on your Air");
    assert_eq!(card.request.payload["action_title"], card.title);
    assert_eq!(card.request.payload["effect_class"], "host");
    assert_eq!(card.request.payload["host_id"], air.id.as_str());
    assert_eq!(card.request.payload["host_name"], "Air");
    assert_eq!(
        card.request.payload["proposed_rules"],
        serde_json::json!([])
    );
    assert_eq!(
        card.request.payload["proposed_session_allow_rule"],
        serde_json::json!({"harness": "claude", "directory": DIRECTORY})
    );
    assert_eq!(
        card.request.payload["always_label"],
        format!("Always allow Claude Code sessions in {DIRECTORY} on Air")
    );
    assert_eq!(
        card.body,
        format!(
            "Harness: Claude Code\nMachine: Air\nDirectory: {DIRECTORY}\n\
             Worktree: pagis/fix-the-login\nMode: Let the sprite decide\nHarness mode: \
             Manual\nPrompt: {PROMPT}"
        )
    );
    assert_eq!(card.request.payload["body"], card.body);

    grant_host(&harness, &air, &[]).await;
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &card.request.id,
            RequestState::Approved,
            None,
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let outcome = harness
        .broker
        .resolve(&card.request.id, RequestState::Approved)
        .await
        .unwrap();

    assert!(
        matches!(outcome, InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    let calls = harness.executor.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_name, CODING_SESSION_START);
    assert_eq!(
        calls[0].host.as_ref().map(|host| host.id.clone()),
        Some(air.id)
    );
}

/// A start with no worktree says so, and a long prompt shows its first
/// 280 characters.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_card_of_a_start_with_no_worktree_and_a_long_prompt(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let _connected = harness.presence().connect(&air.id);
    harness
        .starts
        .answers(Ok(action(None, SessionApprovalMode::Person)));
    let prompt = "é".repeat(300);
    let arguments = serde_json::json!({
        "harness": "claude",
        "directory": DIRECTORY,
        "worktree": false,
        "title": "Fix the login",
        "prompt": prompt,
    })
    .to_string();

    let InvokeOutcome::Waiting(card) = start(&harness, &arguments).await else {
        panic!("a start waits for one approval");
    };

    assert!(
        card.body
            .contains("\nNo worktree: it works in the directory\n"),
        "{}",
        card.body
    );
    assert!(
        card.body
            .contains("Mode: Ask me\nHarness mode: Manual\nPrompt: "),
        "{}",
        card.body
    );
    assert!(
        card.body
            .ends_with(&format!("Prompt: {}…", "é".repeat(280))),
        "{}",
        card.body
    );
}

/// Pagis policy sees no action of a harness that never asks, so the card
/// of its start says so before the Person approves it.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_card_of_a_harness_that_never_asks_says_so(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:pi"]).await;
    let _connected = harness.presence().connect(&air.id);
    harness.starts.answers(Ok(SessionStartAction {
        harness_id: "pi".to_string(),
        harness_name: "pi".to_string(),
        asks_permission: false,
        ..action(None, SessionApprovalMode::Person)
    }));

    let InvokeOutcome::Waiting(card) = start(&harness, &arguments("pi", None)).await else {
        panic!("a start waits for one approval");
    };

    assert!(
        card.body
            .contains("\nMode: Ask me\npi does not ask before it acts.\nPrompt: "),
        "{}",
        card.body
    );
}

/// A host Grant of the Agent on `host` that holds one session Allow Rule
/// for Claude Code in `directory`.
async fn grant_session_rule(harness: &Harness, host: &Host, directory: &str) {
    let mut grant = grant_host(harness, host, &[]).await;
    let rule = SessionAllowRule::new("claude", directory).unwrap();
    grant.scope = grant.with_session_allow_rules(&[rule]);
    SqliteGrantStore::new(harness.pool.clone())
        .set_scope(&harness.workspace.id, &grant.id, &grant.scope)
        .await
        .unwrap();
}

/// What the scripted check answers for a start of Claude Code in
/// `directory`.
fn action_in(directory: &str) -> SessionStartAction {
    SessionStartAction {
        directory: directory.to_string(),
        ..action(None, SessionApprovalMode::Person)
    }
}

/// A session Allow Rule of the machine's host Grant that covers the
/// harness and the directory runs the start with no card. The checks of
/// the start still run first.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_start_that_a_session_allow_rule_covers_runs_with_no_card(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let _connected = harness.presence().connect(&air.id);
    grant_session_rule(&harness, &air, "/Users/bo/code").await;
    harness
        .starts
        .answers(Ok(action_in("/Users/bo/code/app/web")));

    let outcome = start(&harness, &arguments("claude", None)).await;

    assert!(
        matches!(outcome, InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    assert!(pending_requests(&harness).await.is_empty());
    let calls = harness.executor.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].approved_by_rule);
    assert_eq!(*harness.starts.hosts.lock().unwrap(), vec![air.id]);
}

/// A session Allow Rule is the rule of one machine, one harness and one
/// directory tree. A start on another machine of the same Person, of
/// another harness, or in a sibling directory with a shared prefix asks.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_start_that_no_session_allow_rule_covers_asks(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &["harness:claude"]).await;
    let studio = register(&harness, "Studio", &["harness:claude"]).await;
    let _studio_connected = harness.presence().connect(&studio.id);
    grant_session_rule(&harness, &air, DIRECTORY).await;
    grant_host(&harness, &studio, &[]).await;
    harness.starts.answers(Ok(action_in(DIRECTORY)));

    let outcome = start(&harness, &arguments("claude", Some("Studio"))).await;

    let InvokeOutcome::Waiting(card) = outcome else {
        panic!("the other machine asks: {outcome:?}");
    };
    assert_eq!(card.request.payload["host_id"], studio.id.as_str());

    let _air_connected = harness.presence().connect(&air.id);
    for (harness_id, directory) in [
        ("codex", DIRECTORY.to_string()),
        ("claude", format!("{DIRECTORY}2")),
    ] {
        harness.starts.answers(Ok(SessionStartAction {
            harness_id: harness_id.to_string(),
            ..action_in(&directory)
        }));

        let outcome = start(&harness, &arguments("claude", Some("Air"))).await;

        assert!(
            matches!(outcome, InvokeOutcome::Waiting(_)),
            "{harness_id} in {directory}: {outcome:?}"
        );
    }
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}
