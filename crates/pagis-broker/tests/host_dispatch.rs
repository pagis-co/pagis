//! A host action runs on a present client, never on the daemon.
//!
//! These tests drive the broker, which is where the machine of a host
//! action is settled: which machines the snapshot offers, which one the
//! card names, what an absent machine answers, and what the audit fact
//! says. The dispatch itself is the executor's, and the presence registry
//! has its own tests.

use pagis_core::{
    EventLog, Grant, GrantStore, Host, HostStore, RequestState, RequestStore, SHELL_CAPABILITY,
};
use pagis_storage_sqlite::{SqliteEventLog, SqliteGrantStore, SqliteRequestStore};
use sqlx::SqlitePool;

use crate::capability_broker::{Harness, harness};

const SHELL_CALL: &str = r#"{"command":"git status"}"#;

/// A command that runs a program of the attacker's choice through
/// `git status`: git runs the program that `core.fsmonitor` names.
const ENVIRONMENT_OVERRIDE: &str =
    "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=x git status";

/// Register one machine of the workspace. It is present only once a
/// client connects, which is what a Pagis client does over its own
/// socket; a test that wants presence holds the connection it returns.
pub(crate) async fn register(harness: &Harness, name: &str, capabilities: &[&str]) -> Host {
    let capabilities: Vec<String> = capabilities.iter().map(|held| held.to_string()).collect();
    harness
        .hosts()
        .register(
            &harness.workspace.id,
            name,
            "macos",
            &capabilities,
            pagis_core::now_ms(),
        )
        .await
        .unwrap()
}

pub(crate) async fn grant_host(harness: &Harness, host: &Host, allow: &[&str]) -> Grant {
    let allow: Vec<String> = allow.iter().map(|rule| rule.to_string()).collect();
    let grant = Grant {
        id: pagis_core::GrantId::generate(),
        workspace_id: harness.workspace.id.clone(),
        agent_id: harness.agent.id.clone(),
        resource_kind: Grant::HOST_KIND.to_string(),
        resource_id: Some(host.id.to_string()),
        scope: Grant::allow_scope(&allow),
        revision: 1,
        created_at: pagis_core::now_ms(),
        revoked_at: None,
    };
    SqliteGrantStore::new(harness.pool.clone())
        .create(&grant)
        .await
        .unwrap();
    grant
}

async fn call_host_shell(harness: &Harness) -> pagis_broker::InvokeOutcome {
    call_host_command(harness, SHELL_CALL).await
}

async fn call_host_command(harness: &Harness, arguments: &str) -> pagis_broker::InvokeOutcome {
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
            pagis_broker::ToolCall::new("host_shell", arguments),
        )
        .await
        .unwrap()
}

/// Every `tool.completed` fact of this workspace, newest first.
async fn audit(harness: &Harness) -> Vec<serde_json::Value> {
    SqliteEventLog::new(harness.pool.clone())
        .list_by_types(&harness.workspace.id, &["tool.completed"], None, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload)
        .collect()
}

/// A local installation with the client running: one machine, and the
/// person is asked only for the approval. The card is the approval card,
/// and it names the machine.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn one_present_host_asks_only_for_the_approval(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let _connected = harness.presence().connect(&air.id);

    let outcome = call_host_shell(&harness).await;

    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("a host command waits for one approval: {outcome:?}");
    };
    assert_eq!(pending.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(pending.title, "Run a command on your Air");
    assert_eq!(pending.request.payload["host_id"], air.id.as_str());
    assert_eq!(pending.request.payload["host_name"], "Air");
    assert_eq!(pending.body, "git status");
}

/// An approved command reaches the executor with the machine on it. The
/// daemon runs nothing itself: the machine is the call's own target.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_approved_command_reaches_the_executor_with_its_host(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let _connected = harness.presence().connect(&air.id);
    let grant = grant_host(&harness, &air, &[]).await;

    let pagis_broker::InvokeOutcome::Waiting(pending) = call_host_shell(&harness).await else {
        panic!("a host command waits for an approval");
    };
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &pending.request.id,
            RequestState::Approved,
            None,
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let outcome = harness
        .broker
        .resolve(&pending.request.id, RequestState::Approved)
        .await
        .unwrap();

    assert!(matches!(outcome, pagis_broker::InvokeOutcome::Completed(_)));
    let calls = harness.executor.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].host.as_ref().map(|host| host.id.clone()),
        Some(air.id.clone())
    );
    assert_eq!(calls[0].grant_id.as_ref(), Some(&grant.id));
    // The person approved it on the card, so the client runs it in their
    // own shell.
    assert!(!calls[0].approved_by_rule);

    let facts = audit(&harness).await;
    let fact = facts
        .iter()
        .find(|fact| fact["name"] == "host_shell")
        .expect("the host action is in the audit log");
    assert_eq!(fact["host_id"], air.id.as_str());
}

/// A command the person allowed on this machine runs with no card, and it
/// still names the machine in the audit log.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_allow_rule_of_one_machine_runs_without_a_card(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    let _connected = harness.presence().connect(&air.id);
    grant_host(&harness, &air, &["git status"]).await;
    // The same command on another machine is another question, so the
    // rule of the machine that is not connected does not apply here.
    grant_host(&harness, &studio, &[]).await;

    let outcome = call_host_shell(&harness).await;

    assert!(
        matches!(outcome, pagis_broker::InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    // A rule approved it, so the client runs it under `/bin/sh`, the
    // dialect that the rule check parsed.
    let calls = harness.executor.calls.lock().unwrap().clone();
    assert!(calls[0].approved_by_rule);
    let facts = audit(&harness).await;
    let fact = facts
        .iter()
        .find(|fact| fact["name"] == "host_shell")
        .expect("the host action is in the audit log");
    assert_eq!(fact["authorization"], "allow_rule");
    assert_eq!(fact["host_id"], air.id.as_str());
}

/// An environment assignment can make the approved program run another
/// program, so a live rule for `git status` does not cover it. The
/// person sees the card, and nothing reaches the machine before they
/// decide.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_environment_override_under_a_live_rule_asks_and_dispatches_nothing(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let _connected = harness.presence().connect(&air.id);
    grant_host(&harness, &air, &["git status"]).await;

    let arguments = serde_json::json!({ "command": ENVIRONMENT_OVERRIDE }).to_string();
    let outcome = call_host_command(&harness, &arguments).await;

    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("an environment override waits for a card: {outcome:?}");
    };
    assert_eq!(pending.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(pending.body, ENVIRONMENT_OVERRIDE);
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// An allow rule belongs to one machine. The same command on another
/// machine is another question, so it asks, and approval is on whatever
/// the installation is: the effect class of a host action is `host` and
/// only a live rule of that machine's own Grant runs one without a card.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_rule_of_one_machine_does_not_allow_another(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    // The person allowed this command on the Air, and on nothing else.
    grant_host(&harness, &air, &["git status"]).await;
    grant_host(&harness, &studio, &[]).await;
    let _connected = harness.presence().connect(&studio.id);

    let outcome = call_host_shell(&harness).await;

    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("the other machine asks for an approval");
    };
    assert_eq!(pending.title, "Run a command on your Studio");
    assert_eq!(pending.request.payload["host_id"], studio.id.as_str());
    assert_eq!(pending.request.payload["effect_class"], "host");
}

/// No client is connected. The answer names the machine and says what to
/// do about it, and it comes back at once rather than after a wait.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_absent_host_answers_that_it_is_not_connected(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    grant_host(&harness, &air, &[]).await;

    let started = std::time::Instant::now();
    let outcome = call_host_shell(&harness).await;

    let result = outcome.result().expect("an answer, not a card");
    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(
        result.content.contains("your Air is not connected"),
        "{result:?}"
    );
    assert!(
        result.content.contains("open the Pagis client"),
        "{result:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(harness.executor.calls.lock().unwrap().is_empty());
    let facts = audit(&harness).await;
    assert_eq!(facts[0]["host_id"], air.id.as_str());
}

/// A browser-only local person has no machine at all. The answer tells
/// them to open the client.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_person_with_no_host_is_told_to_open_the_client(pool: SqlitePool) {
    let harness = harness(pool).await;

    let outcome = call_host_shell(&harness).await;

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(
        result.content.contains("no computer of yours is connected"),
        "{result:?}"
    );
    assert!(
        result.content.contains("Open the Pagis client"),
        "{result:?}"
    );
}

/// Two present machines, so the person is asked which. The choice is the
/// person's: the broker never picks between two machines of one person.
/// The approval that follows names the machine they chose.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn two_present_hosts_ask_the_person_which_one(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    let _air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);
    grant_host(&harness, &air, &[]).await;
    grant_host(&harness, &studio, &[]).await;

    let pagis_broker::InvokeOutcome::Waiting(question) = call_host_shell(&harness).await else {
        panic!("two present machines ask which one");
    };
    assert_eq!(question.request.kind, pagis_core::Request::CHOICE_KIND);
    assert_eq!(question.title, "Which computer should this run on?");
    assert_eq!(
        question.request.payload["options"],
        serde_json::json!([
            {"value": air.id.as_str(), "label": "Air (macos)"},
            {"value": studio.id.as_str(), "label": "Studio (macos)"},
        ])
    );

    // The person taps the Studio, and the approval card names it.
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

    let pagis_broker::InvokeOutcome::Waiting(approval) = outcome else {
        panic!("the machine is named, and the action still asks for an approval");
    };
    assert_eq!(approval.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(approval.title, "Run a command on your Studio");
    assert_eq!(approval.request.payload["host_id"], studio.id.as_str());
}

/// A person who dismisses the question ran nothing.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_dismissed_question_runs_nothing(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    let _air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);

    let pagis_broker::InvokeOutcome::Waiting(question) = call_host_shell(&harness).await else {
        panic!("two present machines ask which one");
    };
    let outcome = harness
        .broker
        .resolve(&question.request.id, RequestState::Denied)
        .await
        .unwrap();

    let result = outcome.result().expect("a refusal");
    assert!(result.is_error);
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// A phone registers with no shell capability. It is not offered for a
/// shell command, so a person whose only connected client is a phone is
/// told that nothing is connected.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_phone_is_not_offered_for_a_shell_command(pool: SqlitePool) {
    let harness = harness(pool).await;
    let phone = register(&harness, "Phone", &[]).await;
    let _connected = harness.presence().connect(&phone.id);
    grant_host(&harness, &phone, &["git status"]).await;

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
            pagis_broker::ToolCall::new("host_shell", SHELL_CALL),
        )
        .await
        .unwrap();

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// A sprite cannot reach a machine of another person. The grant names one
/// and the host read names the Workspace, so the other person's machine is
/// absent however it is named.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_sprite_cannot_reach_another_persons_host(pool: SqlitePool) {
    let harness = harness(pool).await;
    let other = crate::capability_broker::second_workspace(&harness).await;
    let theirs = harness
        .hosts()
        .register(
            &other.id,
            "Their Air",
            "macos",
            &[SHELL_CAPABILITY.to_string()],
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    // The other person's machine is connected, and this sprite even holds
    // a grant that names it. It is still out of reach.
    let _connected = harness.presence().connect(&theirs.id);
    grant_host(&harness, &theirs, &["git status"]).await;

    let outcome = call_host_shell(&harness).await;

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(harness.executor.calls.lock().unwrap().is_empty());
    let facts = audit(&harness).await;
    assert_eq!(facts[0]["host_id"], serde_json::Value::Null);
}

/// The answer to "which machine?" is checked against the Workspace too: a
/// client that posts back another person's host id names nothing.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_named_host_of_another_person_names_nothing(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    let _air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);
    let other = crate::capability_broker::second_workspace(&harness).await;
    let theirs = harness
        .hosts()
        .register(
            &other.id,
            "Their Air",
            "macos",
            &[SHELL_CAPABILITY.to_string()],
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let _theirs_connected = harness.presence().connect(&theirs.id);

    let pagis_broker::InvokeOutcome::Waiting(question) = call_host_shell(&harness).await else {
        panic!("two present machines ask which one");
    };
    // The options the daemon offered hold neither the other person's
    // machine nor any machine it does not own.
    let options = question.request.payload["options"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(options.len(), 2);
    // A client that answers with an id outside the list is refused: the
    // stored values are validated against the options, so this is the
    // belt to that braces.
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &question.request.id,
            RequestState::Approved,
            Some(serde_json::json!({"value": theirs.id.as_str()})),
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let outcome = harness
        .broker
        .resolve(&question.request.id, RequestState::Approved)
        .await
        .unwrap();

    let result = outcome.result().expect("a refusal");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// A machine that went away between the card and the answer is an answer
/// the person can act on.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_that_goes_away_under_the_card_answers_absent(pool: SqlitePool) {
    let harness = harness(pool).await;
    let air = register(&harness, "Air", &[SHELL_CAPABILITY]).await;
    let studio = register(&harness, "Studio", &[SHELL_CAPABILITY]).await;
    let air_connected = harness.presence().connect(&air.id);
    let _studio_connected = harness.presence().connect(&studio.id);

    let pagis_broker::InvokeOutcome::Waiting(question) = call_host_shell(&harness).await else {
        panic!("two present machines ask which one");
    };
    drop(air_connected);
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &question.request.id,
            RequestState::Approved,
            Some(serde_json::json!({"value": air.id.as_str()})),
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    let outcome = harness
        .broker
        .resolve(&question.request.id, RequestState::Approved)
        .await
        .unwrap();

    let result = outcome.result().expect("an answer, not a card");
    assert_eq!(result.code.as_deref(), Some("host_not_connected"));
    assert!(
        result.content.contains("your Air is not connected"),
        "{result:?}"
    );
    let facts = audit(&harness).await;
    assert_eq!(facts[0]["host_id"], air.id.as_str());
}

/// The host tool waits for one thing: the command, on a machine the
/// person is at. Two minutes is the whole budget, and it is not the
/// Computer shell's, which pays for a container wake as well.
#[test]
fn the_host_dispatch_budget_is_two_minutes() {
    assert_eq!(
        pagis_broker::HOST_DISPATCH_TIMEOUT,
        std::time::Duration::from_secs(120)
    );
}
