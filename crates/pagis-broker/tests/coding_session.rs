//! The start of a Coding Session is a host action (ADR-0033, ADR-0015).
//!
//! These tests drive the broker: which snapshot offers the start, which
//! machines are candidates for a harness, what the `machine` argument
//! keeps, what a refusal before the card writes, and what the card says.
//! The checks of the start are the `SessionStarts` of `pagis-coding`,
//! which has its own tests. Here a scripted one stands in.

use pagis_broker::{CODING_SESSION_START, InvokeOutcome, SessionStartAction, ToolCall, ToolResult};
use pagis_core::{RequestState, RequestStore, SHELL_CAPABILITY, SessionApprovalMode};
use pagis_storage_sqlite::SqliteRequestStore;
use sqlx::SqlitePool;

use crate::capability_broker::{Harness, harness};
use crate::host_dispatch::{grant_host, register};

const DIRECTORY: &str = "/Users/bo/code/app";

const PROMPT: &str = "Fix the login bug.";

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

/// An absent capability is declared, never emulated (ADR-0005): a
/// Workspace whose machines declare no harness offers no start.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_start_is_absent_when_no_host_declares_a_harness(pool: SqlitePool) {
    let harness = harness(pool).await;
    register(&harness, "Air", &[SHELL_CAPABILITY]).await;

    let tools = tool_names(&harness).await;

    assert!(!tools.contains(&CODING_SESSION_START.to_string()));
    assert!(tools.contains(&"host_shell".to_string()));
}

/// One machine that declares a harness offers the start, whether it is
/// connected or not.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_start_is_offered_when_a_host_declares_a_harness(pool: SqlitePool) {
    let harness = harness(pool).await;
    register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    register(&harness, "Studio", &["harness:codex"]).await;

    assert!(
        tool_names(&harness)
            .await
            .contains(&CODING_SESSION_START.to_string())
    );
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
        card.body,
        format!(
            "Harness: Claude Code\nMachine: Air\nDirectory: {DIRECTORY}\n\
             Worktree: pagis/fix-the-login\nMode: Let the sprite decide\nPrompt: {PROMPT}"
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
    assert!(card.body.contains("Mode: Ask me"), "{}", card.body);
    assert!(
        card.body
            .ends_with(&format!("Prompt: {}…", "é".repeat(280))),
        "{}",
        card.body
    );
}
